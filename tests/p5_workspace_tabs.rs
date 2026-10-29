#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real `workspace open` tab-navigation and keyboard-copy acceptance on a
//! private outer PTY.
//!
//! The registry and every route server live below a private PANELINK_HOME.
//! The attach client is a real controlling-terminal process and its rendered
//! screen is interpreted through vt100 rather than matched as an ANSI byte
//! transcript.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::{tcgetattr, LocalModes, Termios, Winsize};
use rustix_openpty::openpty;

const STEP: Duration = Duration::from_secs(12);
const COMMAND_STEP: Duration = Duration::from_secs(12);
const STOP_STEP: Duration = Duration::from_secs(9);
const ROWS: u16 = 24;
const COLUMNS: u16 = 100;
const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    owned_routes: Vec<(u64, u64)>,
}

impl Fixture {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-p5-tabs-{}-{serial}",
            std::process::id()
        ));
        make_private_dir(&root);
        let home = root.join("home");
        make_private_dir(&home);
        Self {
            root,
            home,
            owned_routes: Vec::new(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", "/bin/zsh")
            .env("PS1", "P5T$ ")
            .env("PANELINK_HOME", &self.root)
            .current_dir(&self.home);
        command
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("start private Panelink command");
        let deadline = Instant::now() + COMMAND_STEP;
        loop {
            if child.try_wait().expect("poll Panelink command").is_some() {
                return child.wait_with_output().expect("collect Panelink command");
            }
            if Instant::now() >= deadline {
                // SAFETY: this process group belongs only to this fixture command.
                unsafe { libc::kill(-i32::try_from(child.id()).unwrap(), libc::SIGKILL) };
                let output = child.wait_with_output().expect("reap timed-out command");
                panic!("Panelink command exceeded {COMMAND_STEP:?}: {args:?}; {output:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn cleanup_command(&self, args: &[&str]) {
        let mut child = match self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        };
        let deadline = Instant::now() + COMMAND_STEP;
        loop {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            if Instant::now() >= deadline {
                // SAFETY: this process group belongs only to this fixture command.
                if let Ok(pid) = i32::try_from(child.id()) {
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                } else {
                    let _ = child.kill();
                }
                let _ = child.wait();
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn create_project(&self, name: &str, initial_tab: &str) {
        let output = self.run(&["workspace", "project", "create", name, initial_tab]);
        assert!(output.status.success(), "create {name}: {output:?}");
    }

    fn create_tab(&self, project: u64, name: &str) {
        let project = project.to_string();
        let output = self.run(&["workspace", "tab", "create", &project, name]);
        assert!(output.status.success(), "create {name}: {output:?}");
    }

    fn project_id(&self, name: &str) -> u64 {
        let output = self.run(&["workspace", "list"]);
        assert!(output.status.success(), "list registry: {output:?}");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                let words = line.split_whitespace().collect::<Vec<_>>();
                if words.first() != Some(&"Project") || words.len() < 3 {
                    return None;
                }
                let label = words[2..].join(" ").trim_end_matches(" *").to_owned();
                (label == name).then(|| words[1].parse::<u64>().expect("project ID is numeric"))
            })
            .unwrap_or_else(|| panic!("project {name:?} missing from registry:\n{output:?}"))
    }

    fn tab_id(&self, project_name: &str, tab_name: &str) -> u64 {
        let output = self.run(&["workspace", "list"]);
        assert!(output.status.success(), "list registry: {output:?}");
        let mut in_project = false;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let words = line.split_whitespace().collect::<Vec<_>>();
            if words.first() == Some(&"Project") {
                in_project = words.get(2).copied() == Some(project_name)
                    || words
                        .get(2..)
                        .is_some_and(|tail| tail.join(" ").trim_end_matches(" *") == project_name);
            } else if in_project && words.first() == Some(&"Tab") && words.len() >= 3 {
                let label = words[2..].join(" ").trim_end_matches(" *").to_owned();
                if label == tab_name {
                    return words[1].parse().expect("tab ID is numeric");
                }
            }
        }
        panic!("tab {project_name}/{tab_name} missing from registry:\n{output:?}");
    }

    fn own_route(&mut self, project: u64, tab: u64) {
        if !self.owned_routes.contains(&(project, tab)) {
            self.owned_routes.push((project, tab));
        }
    }

    fn route_root(&self, project: u64, tab: u64) -> PathBuf {
        self.root
            .join("projects")
            .join(project.to_string())
            .join("workspaces")
            .join(tab.to_string())
    }

    fn route_context_count(&self, project: u64, tab: u64) -> usize {
        fs::read_dir(self.route_root(project, tab).join("contexts"))
            .expect("route-local contexts directory")
            .count()
    }

    fn stop_route(&self, project: u64, tab: u64) {
        let route_root = self.route_root(project, tab);
        let server = route_root.join("server");
        let project_arg = project.to_string();
        let tab_arg = tab.to_string();
        let mut child = match self
            .command()
            .args(["workspace", "stop", &project_arg, &tab_arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        };
        let deadline = Instant::now() + STOP_STEP;
        loop {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !server_lock_held(&server) {
            return;
        }

        // A fallback signal is limited to the exact server identity recorded
        // under this fixture-owned route, including its kernel start time.
        if let Some(pid) = verified_server_pid(&route_root) {
            // SAFETY: `pid` was revalidated against the private route record.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            if wait_server_exit(&server, Duration::from_secs(5)) {
                return;
            }
            if verified_server_pid(&route_root) == Some(pid) {
                // SAFETY: the same route-owned process identity was rechecked.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                let _ = wait_server_exit(&server, Duration::from_secs(2));
            }
        }
    }

    fn stop_default_route(&self) {
        self.cleanup_command(&["stop"]);
        let server = self.root.join("server");
        if server_lock_held(&server) {
            if let Some(pid) = verified_server_pid(&self.root) {
                // SAFETY: this PID/start-time pair belongs to the private
                // Default/Main route created by the plain-launch portion.
                unsafe { libc::kill(pid, libc::SIGTERM) };
                if !wait_server_exit(&server, Duration::from_secs(5))
                    && verified_server_pid(&self.root) == Some(pid)
                {
                    // SAFETY: revalidated the same private server identity.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    let _ = wait_server_exit(&server, Duration::from_secs(2));
                }
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_default_route();
        for &(project, tab) in self.owned_routes.iter().rev() {
            self.stop_route(project, tab);
        }
        let all_stopped = !server_lock_held(&self.root.join("server"))
            && self.owned_routes.iter().all(|&(project, tab)| {
                !server_lock_held(&self.route_root(project, tab).join("server"))
            });
        if all_stopped {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn make_private_dir(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|error| panic!("create private directory {}: {error}", path.display()));
}

fn server_lock_held(server: &Path) -> bool {
    use std::os::fd::AsRawFd;

    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(server.join("server.lock"))
    {
        Ok(lock) => lock,
        Err(_) => return false,
    };
    // SAFETY: this fixture owns the descriptor for the route's private lock file.
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        // SAFETY: release the lock just acquired above.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        false
    } else {
        true
    }
}

fn wait_server_exit(server: &Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while server_lock_held(server) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

fn verified_server_pid(route_root: &Path) -> Option<i32> {
    let record = fs::read_to_string(route_root.join("server.pid")).ok()?;
    let mut fields = record.split_whitespace();
    let pid = fields.next()?.parse::<i32>().ok()?;
    let start = fields.next()?.parse::<u64>().ok()?;
    if fields.next().is_some() || pid <= 0 {
        return None;
    }
    // SAFETY: proc_pidinfo writes the documented BSD process info record.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the output buffer and size describe `info` exactly.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).ok()?,
        )
    };
    (usize::try_from(written).ok()? == size
        && info.pbi_uid == rustix::process::getuid().as_raw()
        && info.pbi_status != libc::SZOMB
        && info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec == start)
        .then_some(pid)
}

struct Client {
    supervisor: Child,
    status_path: PathBuf,
    release_path: PathBuf,
    master: fs::File,
    slave: std::os::fd::OwnedFd,
    initial: Termios,
    capture: Arc<Mutex<Vec<u8>>>,
    stop_drain: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
}

impl Client {
    fn spawn_workspace_open(fixture: &Fixture, project: u64, tab: u64) -> Self {
        let project = project.to_string();
        let tab = tab.to_string();
        Self::spawn(
            fixture,
            &["workspace".to_owned(), "open".to_owned(), project, tab],
        )
    }

    fn spawn_raw_attach(fixture: &Fixture, project: u64, tab: u64) -> Self {
        let route_root = fixture.route_root(project, tab);
        Self::spawn(
            fixture,
            &[
                "attach".to_owned(),
                "--directory".to_owned(),
                route_root.join("server").to_string_lossy().into_owned(),
                "--context-dir".to_owned(),
                route_root.join("contexts").to_string_lossy().into_owned(),
            ],
        )
    }

    fn spawn_plain_panelink(fixture: &Fixture) -> Self {
        Self::spawn(fixture, &[])
    }

    fn spawn(fixture: &Fixture, args: &[String]) -> Self {
        let winsize = Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open private outer PTY");
        let master = fs::File::from(pty.controller);
        let flags = fcntl_getfl(&master).expect("read outer PTY flags");
        fcntl_setfl(&master, flags | OFlags::NONBLOCK).expect("set outer PTY nonblocking");
        let slave = pty.user;
        let initial = tcgetattr(&slave).expect("save outer terminal settings");
        let controlling = slave
            .as_fd()
            .try_clone_to_owned()
            .expect("clone outer controlling terminal");
        let stdio = || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let status_path = fixture.root.join(format!("client-{serial}.status"));
        let release_path = fixture.root.join(format!("client-{serial}.release"));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(SUPERVISOR)
            .arg("p5-workspace-open-supervisor")
            .arg(&status_path)
            .arg(&release_path)
            .arg(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", &fixture.home)
            .env("SHELL", "/bin/zsh")
            .env("PS1", "P5T$ ")
            .env("TERM", "xterm-256color")
            .env("PANELINK_HOME", &fixture.root)
            .current_dir(&fixture.home)
            .stdin(stdio())
            .stdout(stdio())
            .stderr(stdio());
        // SAFETY: reviewed Rustix wrappers create a fresh session and set this
        // fixture-owned PTY as the child process's controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let supervisor = command.spawn().expect("spawn workspace open on PTY");
        drop(command);

        let capture = Arc::new(Mutex::new(Vec::new()));
        let stop_drain = Arc::new(AtomicBool::new(false));
        let mut reader = master.try_clone().expect("clone outer PTY master");
        let drain = {
            let capture = Arc::clone(&capture);
            let stop = Arc::clone(&stop_drain);
            thread::spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(count) => {
                            let mut captured = capture.lock().unwrap();
                            if captured.len() + count <= MAX_CAPTURE {
                                captured.extend_from_slice(&buffer[..count]);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        Self {
            supervisor,
            status_path,
            release_path,
            master,
            slave,
            initial,
            capture,
            stop_drain,
            drain: Some(drain),
        }
    }

    fn exit(&self) -> Option<i32> {
        fs::read_to_string(&self.status_path)
            .ok()?
            .strip_suffix('\n')?
            .parse()
            .ok()
    }

    fn wait_raw_mode(&self) {
        let deadline = Instant::now() + STEP;
        loop {
            if !tcgetattr(&self.slave)
                .expect("query outer terminal settings")
                .local_modes
                .contains(LocalModes::ICANON)
            {
                return;
            }
            if let Some(code) = self.exit() {
                panic!("workspace open exited before attach (status {code})");
            }
            assert!(
                Instant::now() < deadline,
                "workspace open did not enter raw mode"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let deadline = Instant::now() + STEP;
        let mut cursor = 0;
        while cursor < bytes.len() {
            match self.master.write(&bytes[cursor..]) {
                Ok(0) => panic!("outer PTY write returned zero"),
                Ok(count) => cursor += count,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted =>
                {
                    assert!(Instant::now() < deadline, "outer PTY input stalled");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("write outer PTY: {error}"),
            }
        }
    }

    fn type_shell(&mut self, line: &str) {
        self.write(line.as_bytes());
        self.write(b"\r");
    }

    fn click_cell(&mut self, column: u16, row: u16) {
        let column = column + 1;
        let row = row + 1;
        let click = format!("\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m");
        self.write(click.as_bytes());
    }

    fn click_text(&mut self, label: &str) {
        let (row, column) = screen_label_position(&self.parser(), label)
            .unwrap_or_else(|| panic!("missing visible control {label:?}:\n{}", self.screen()));
        self.click_cell(column, row);
    }

    fn parser(&self) -> vt100::Parser {
        let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
        parser.process(&self.capture.lock().unwrap());
        parser
    }

    fn screen(&self) -> String {
        self.parser().screen().contents()
    }

    fn wait_screen(&self, what: &str, done: impl Fn(&vt100::Parser, &str) -> bool) -> String {
        let deadline = Instant::now() + STEP;
        loop {
            let parser = self.parser();
            let screen = parser.screen().contents();
            if done(&parser, &screen) {
                return screen;
            }
            if let Some(code) = self.exit() {
                panic!("workspace open exited ({code}) waiting for {what}:\n{screen}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{screen}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_normal_prompt(&self, what: &str) {
        self.wait_screen(what, |parser, screen| {
            !parser.screen().hide_cursor()
                && copy_mode_caret(parser).is_none()
                && screen.contains("P5T$ ")
        });
    }

    fn wait_header(&self, project: &str, tab: &str) -> String {
        self.wait_screen(&format!("{project}/{tab} header"), |parser, screen| {
            let header = screen.lines().next().unwrap_or_default();
            let selected = format!("[{tab}]");
            if !parser.screen().alternate_screen()
                || !header.contains(project)
                || !header.contains(&selected)
                || screen.contains("(Esc closes)")
            {
                return false;
            }
            let Some(start) = header.find(&selected) else {
                return false;
            };
            let start_column = header[..start].chars().count();
            selected.chars().enumerate().all(|(offset, _)| {
                u16::try_from(start_column + offset)
                    .ok()
                    .and_then(|column| parser.screen().cell(0, column))
                    .is_some_and(|cell| cell.inverse())
            })
        })
    }

    fn wait_marker(&self, marker: &str) -> String {
        self.wait_screen(marker, |parser, _| {
            // Match exact child-content rows; right padding now contains UI
            // chrome. Row one is live content for raw attach and harmless
            // blank pane padding below the header for workspace open.
            pane_marker_position(
                parser,
                PaneCells {
                    top: 1,
                    bottom: ROWS.saturating_sub(1) as usize,
                    left: 1,
                    right: COLUMNS.saturating_sub(1) as usize,
                },
                marker,
            )
            .is_some()
        })
    }

    fn ctrl_b(&mut self, key: u8) {
        self.write(&[0x02, key]);
    }

    fn begin_navigation(&mut self) {
        self.write(&[0x02]);
        self.wait_screen("Ctrl-B navigation prefix", |_, screen| {
            screen.contains(" NAV ")
        });
    }

    fn navigation_key(&mut self, key: &[u8]) {
        self.begin_navigation();
        self.write(key);
        self.write(&[0x1b]);
        self.wait_screen("navigation prefix exit", |_, screen| {
            !screen.contains(" NAV ")
        });
    }

    fn wait_exit(&self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(code) = self.exit() {
                return code;
            }
            assert!(Instant::now() < deadline, "workspace open did not detach");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_restored(&self) {
        assert_eq!(
            tcgetattr(&self.slave).unwrap().input_modes,
            self.initial.input_modes
        );
        assert_eq!(
            tcgetattr(&self.slave).unwrap().output_modes,
            self.initial.output_modes
        );
        assert_eq!(
            tcgetattr(&self.slave).unwrap().control_modes,
            self.initial.control_modes
        );
        assert_eq!(
            tcgetattr(&self.slave).unwrap().local_modes,
            self.initial.local_modes
        );
    }
}

fn copy_mode_caret(parser: &vt100::Parser) -> Option<(u16, u16)> {
    let screen = parser.screen();
    for row in 2..ROWS.saturating_sub(1) {
        for column in 1..COLUMNS.saturating_sub(1) {
            if screen.cell(row, column).is_some_and(|cell| cell.inverse()) {
                return Some((row, column));
            }
        }
    }
    None
}

fn inverse_cells_below_header(parser: &vt100::Parser) -> usize {
    (1..ROWS)
        .flat_map(|row| (0..COLUMNS).map(move |column| (row, column)))
        .filter(|&(row, column)| {
            parser
                .screen()
                .cell(row, column)
                .is_some_and(|cell| cell.inverse())
        })
        .count()
}

fn rendered_divider_column(parser: &vt100::Parser) -> Option<usize> {
    (1..COLUMNS.saturating_sub(1) as usize).find(|&column| {
        // A divider crosses the top pane border. Scrollbar tracks occupy
        // content-height padding only and must not be mistaken for a divider.
        pane_cell_text(parser, 1, column) == "|"
            && (1..ROWS as usize)
                .filter(|&row| pane_cell_text(parser, row, column) == "|")
                .count()
                >= usize::from(ROWS.saturating_sub(1)) / 2
    })
}

#[derive(Clone, Copy)]
struct PaneCells {
    top: usize,
    bottom: usize,
    left: usize,
    right: usize,
}

fn pane_cell_text(parser: &vt100::Parser, row: usize, column: usize) -> String {
    parser
        .screen()
        .cell(row as u16, column as u16)
        .map(|cell| cell.contents().to_owned())
        .unwrap_or_else(|| " ".to_owned())
}

fn screen_label_position(parser: &vt100::Parser, label: &str) -> Option<(u16, u16)> {
    parser
        .screen()
        .contents()
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            let column = line.find(label)?;
            Some((u16::try_from(row).ok()?, u16::try_from(column).ok()?))
        })
}

fn pane_content_rows() -> (usize, usize) {
    // The workspace header consumes row zero; pane borders consume one row
    // above and below the PTY's content area.
    (2, ROWS.saturating_sub(1) as usize)
}

fn whole_root_cells() -> PaneCells {
    let (top, bottom) = pane_content_rows();
    PaneCells {
        top,
        bottom,
        left: 1,
        right: COLUMNS.saturating_sub(1) as usize,
    }
}

fn split_pane_cells(parser: &vt100::Parser) -> Option<(PaneCells, PaneCells)> {
    let divider = rendered_divider_column(parser)?;
    let (top, bottom) = pane_content_rows();
    Some((
        PaneCells {
            top,
            bottom,
            left: 1,
            right: divider.checked_sub(1)?,
        },
        PaneCells {
            top,
            bottom,
            left: divider + 2,
            right: COLUMNS.saturating_sub(1) as usize,
        },
    ))
}

fn pane_cell_lines(parser: &vt100::Parser, pane: PaneCells) -> Vec<String> {
    (pane.top..pane.bottom)
        .map(|row| {
            (pane.left..pane.right)
                .map(|column| pane_cell_text(parser, row, column))
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn pane_marker_position(
    parser: &vt100::Parser,
    pane: PaneCells,
    marker: &str,
) -> Option<(usize, usize)> {
    pane_cell_lines(parser, pane)
        .into_iter()
        .enumerate()
        .find_map(|(row_offset, line)| {
            (line.trim() == marker).then_some((
                pane.top + row_offset,
                pane.left + line.find(marker).unwrap_or_default(),
            ))
        })
}

fn shell_prompt_is_visible(parser: &vt100::Parser, pane: PaneCells) -> bool {
    let (cursor_row, cursor_column) = parser.screen().cursor_position();
    let cursor_row = usize::from(cursor_row);
    let cursor_column = usize::from(cursor_column);
    if !(pane.top <= cursor_row && cursor_row < pane.bottom)
        || !(pane.left <= cursor_column && cursor_column < pane.right)
    {
        return false;
    }
    (pane.left..cursor_column)
        .map(|column| pane_cell_text(parser, cursor_row, column))
        .collect::<String>()
        .chars()
        .any(|character| !character.is_whitespace())
}

fn wait_pane_shell_prompt(client: &Client, left_side: bool) {
    client.wait_screen("focused pane shell prompt", |parser, screen| {
        if screen.contains(" NAV ") {
            return false;
        }
        let Some((left, right)) = split_pane_cells(parser) else {
            return false;
        };
        shell_prompt_is_visible(parser, if left_side { left } else { right })
    });
}

fn wait_root_shell_prompt(client: &Client) {
    client.wait_screen("focused zoomed shell prompt", |parser, screen| {
        !screen.contains(" NAV ")
            && inverse_cells_below_header(parser) == 0
            && shell_prompt_is_visible(parser, whole_root_cells())
    });
}

fn wait_focused_split_shell_prompt(client: &Client) -> bool {
    client.wait_screen("focused shell prompt after pane mode", |parser, screen| {
        if screen.contains(" NAV ") {
            return false;
        }
        let Some((left, right)) = split_pane_cells(parser) else {
            return false;
        };
        shell_prompt_is_visible(parser, left) || shell_prompt_is_visible(parser, right)
    });
    let parser = client.parser();
    let (left, _) = split_pane_cells(&parser).expect("two pane regions remain rendered");
    let cursor = parser.screen().cursor_position();
    let cursor_row = usize::from(cursor.0);
    let cursor_column = usize::from(cursor.1);
    cursor_row >= left.top
        && cursor_row < left.bottom
        && cursor_column >= left.left
        && cursor_column < left.right
}

fn wait_whole_root_marker(client: &Client, marker: &str) -> String {
    wait_pane_marker(client, whole_root_cells(), marker)
}

fn assert_pane_size(parser: &vt100::Parser, pane: PaneCells, marker: &str) {
    assert_eq!(
        pane_size_before_marker(parser, pane, marker),
        expected_pane_size(pane),
        "pane PTY size at {marker}"
    );
}

fn wait_pane_marker(client: &Client, pane: PaneCells, marker: &str) -> String {
    client.wait_screen(marker, |parser, _| {
        pane_marker_position(parser, pane, marker).is_some()
    })
}

fn pane_shell_pid(parser: &vt100::Parser, pane: PaneCells, prefix: &str) -> u32 {
    pane_cell_lines(parser, pane)
        .into_iter()
        .find_map(|line| {
            line.trim()
                .strip_prefix(prefix)
                .and_then(|value| value.parse::<u32>().ok())
        })
        .unwrap_or_else(|| panic!("missing pane-local numeric PID after {prefix:?}"))
}

fn pane_size_before_marker(parser: &vt100::Parser, pane: PaneCells, marker: &str) -> (u16, u16) {
    let lines = pane_cell_lines(parser, pane);
    let marker_line = lines
        .iter()
        .position(|line| line.trim() == marker)
        .unwrap_or_else(|| panic!("missing pane-local size marker {marker:?}"));
    let fields = lines
        .get(
            marker_line
                .checked_sub(1)
                .expect("marker follows stty output"),
        )
        .expect("stty size appears before marker")
        .split_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        2,
        "expected pane-local stty size before {marker}"
    );
    (
        fields[0].parse().expect("stty rows are numeric"),
        fields[1].parse().expect("stty columns are numeric"),
    )
}

fn expected_pane_size(pane: PaneCells) -> (u16, u16) {
    (
        u16::try_from(pane.bottom - pane.top).expect("pane rows fit u16"),
        u16::try_from(pane.right - pane.left).expect("pane columns fit u16"),
    )
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.exit().is_none() {
            let _ = self.master.write(&[0x18]);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.exit().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.exit().is_none() {
            // SAFETY: the supervisor was started in its own process group;
            // this signal reaches only the client and wrapper from this test.
            unsafe { libc::kill(-i32::try_from(self.supervisor.id()).unwrap(), libc::SIGKILL) };
        }
        let _ = fs::write(&self.release_path, b"release\n");
        let _ = self.supervisor.wait();
        self.stop_drain.store(true, Ordering::Relaxed);
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

const SUPERVISOR: &str = r#"
status_path=$1
release_path=$2
shift 2
trap ':' HUP INT TERM
"$@"
code=$?
printf '%s\n' "$code" > "$status_path.tmp" && /bin/mv "$status_path.tmp" "$status_path"
while [ ! -f "$release_path" ]; do /bin/sleep 0.05; done
exit 0
"#;

#[test]
fn workspace_open_switches_registered_routes_through_nav_and_picker() {
    let mut fixture = Fixture::new();
    fixture.create_project("Editors", "Main");
    let editors = fixture.project_id("Editors");
    fixture.create_tab(editors, "Review");
    fixture.create_project("Tools", "Main");

    let editors_main = fixture.tab_id("Editors", "Main");
    let editors_review = fixture.tab_id("Editors", "Review");
    let tools_main = fixture.tab_id("Tools", "Main");
    fixture.own_route(editors, editors_main);
    fixture.own_route(editors, editors_review);
    let tools = fixture.project_id("Tools");
    fixture.own_route(tools, tools_main);

    let mut client = Client::spawn_workspace_open(&fixture, editors, editors_main);
    client.wait_raw_mode();
    client.wait_header("Editors", "Main");
    assert!(server_lock_held(
        &fixture.route_root(editors, editors_main).join("server")
    ));
    assert!(fixture.route_context_count(editors, editors_main) > 0);

    client.type_shell("export P5_ROUTE_A=EDITORS_MAIN; printf 'P5_ROUTE_'A'_READY\\n'");
    client.wait_marker("P5_ROUTE_A_READY");

    // Ctrl-B n/p selects tabs in the active project and preserves the live
    // route process when returning to its earlier tab.
    client.ctrl_b(b'n');
    client.wait_header("Editors", "Review");
    assert!(server_lock_held(
        &fixture.route_root(editors, editors_review).join("server")
    ));
    assert!(fixture.route_context_count(editors, editors_review) > 0);
    client.type_shell("export P5_ROUTE_A2=EDITORS_REVIEW; printf 'P5_ROUTE_'A2'_READY\\n'");
    client.wait_marker("P5_ROUTE_A2_READY");
    client.ctrl_b(b'p');
    client.wait_header("Editors", "Main");
    client.type_shell(
        "printf 'P5_ROUTE_A_RETURNED_%s_REVIEW=%s\\n' \"$P5_ROUTE_A\" \"${P5_ROUTE_A2-unset}\"",
    );
    client.wait_marker("P5_ROUTE_A_RETURNED_EDITORS_MAIN_REVIEW=unset");
    client.ctrl_b(b'n');
    client.wait_header("Editors", "Review");
    client.type_shell("printf 'P5_REVIEW_RETURNED_%s\\n' \"$P5_ROUTE_A2\"");
    client.wait_marker("P5_REVIEW_RETURNED_EDITORS_REVIEW");
    client.ctrl_b(b'p');
    client.wait_header("Editors", "Main");

    // The distinct project has an independent server, contexts directory,
    // generation, and process set, even though both routes begin at pane 1.
    client.ctrl_b(b']');
    client.wait_header("Tools", "Main");
    let a_root = fixture.route_root(editors, editors_main);
    let b_root = fixture.route_root(tools, tools_main);
    assert!(server_lock_held(&a_root.join("server")));
    assert!(server_lock_held(&b_root.join("server")));
    assert!(fixture.route_context_count(tools, tools_main) > 0);
    assert_ne!(a_root.join("server"), b_root.join("server"));
    assert_ne!(a_root.join("contexts"), b_root.join("contexts"));
    assert_ne!(
        fs::read(a_root.join("server/generation")).expect("A generation"),
        fs::read(b_root.join("server/generation")).expect("B generation")
    );
    client.type_shell("export P5_ROUTE_B=TOOLS_MAIN; printf 'P5_ROUTE_'B'_READY\\n'");
    client.wait_marker("P5_ROUTE_B_READY");
    client.type_shell("printf 'P5_B_A_VALUE=%s\\n' \"${P5_ROUTE_A-unset}\"");
    client.wait_marker("P5_B_A_VALUE=unset");

    client.ctrl_b(b'[');
    client.wait_header("Editors", "Main");
    client
        .type_shell("printf 'P5_ROUTE_A_AGAIN_%s_B=%s\\n' \"$P5_ROUTE_A\" \"${P5_ROUTE_B-unset}\"");
    client.wait_marker("P5_ROUTE_A_AGAIN_EDITORS_MAIN_B=unset");
    assert!(server_lock_held(&a_root.join("server")));
    assert!(server_lock_held(&b_root.join("server")));

    client.ctrl_b(b']');
    client.wait_header("Tools", "Main");
    client.type_shell("printf 'P5_TOOLS_RETURNED_A=%s\\n' \"${P5_ROUTE_A-unset}\"");
    client.wait_marker("P5_TOOLS_RETURNED_A=unset");
    client.ctrl_b(b'[');
    client.wait_header("Editors", "Main");

    // The documented Ctrl-B w project/workspace picker is local UI. Confirm
    // both registered projects and the secondary tab are rendered, then close.
    client.ctrl_b(b'w');
    let picker = client.wait_screen("workspace picker entries", |_, screen| {
        screen.contains("(Esc closes)")
            && screen.contains("Editors")
            && screen.contains("Tools")
            && screen.contains("Review")
    });
    assert!(picker.contains("Editors") && picker.contains("Tools"));
    client.write(b"\x1b[C");
    client.wait_screen("Tools/Main picker selection", |_, screen| {
        screen.lines().any(|line| line.trim() == "> Tools / Main")
    });
    client.write(b"\r");
    client.wait_header("Tools", "Main");
    client.ctrl_b(b'[');
    client.wait_header("Editors", "Main");
    client.ctrl_b(b'w');
    client.write(&[0x1b]);
    client.wait_header("Editors", "Main");

    // Ctrl-X ends this attach client while all route-owned servers remain live.
    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0, "Ctrl-X detach should succeed");
    client.assert_restored();
    for &(project, tab) in &fixture.owned_routes {
        assert!(
            server_lock_held(&fixture.route_root(project, tab).join("server")),
            "route {project}/{tab} stopped when the client detached"
        );
        assert!(fixture.route_context_count(project, tab) > 0);
    }

    // Raw attach remains headerless; ordinary no-argument startup now presents
    // the saved catalog route with its workspace strip.
    let mut raw = Client::spawn_raw_attach(&fixture, editors, editors_main);
    raw.wait_raw_mode();
    raw.type_shell("printf 'P5_RAW_'READY'\\n'");
    raw.wait_marker("P5_RAW_READY");
    let raw_screen = raw.screen();
    let raw_header = raw_screen.lines().next().unwrap_or_default();
    assert!(!raw_header.contains("Editors") && !raw_header.contains("Tools"));
    raw.write(&[0x18]);
    assert_eq!(raw.wait_exit(STEP), 0, "raw attach Ctrl-X");
    raw.assert_restored();

    let mut plain = Client::spawn_plain_panelink(&fixture);
    plain.wait_raw_mode();
    plain.wait_header("Editors", "Main");
    plain.type_shell("printf 'P5_PLAIN_'READY'\\n'");
    plain.wait_marker("P5_PLAIN_READY");
    plain.write(&[0x18]);
    assert_eq!(plain.wait_exit(STEP), 0, "plain panelink Ctrl-X");
    plain.assert_restored();
}

#[test]
fn workspace_open_keyboard_copy_mode_consumes_child_input_and_restores_terminal() {
    let mut fixture = Fixture::new();
    fixture.create_project("Editors", "Main");
    let editors = fixture.project_id("Editors");
    let main = fixture.tab_id("Editors", "Main");
    fixture.own_route(editors, main);

    let mut client = Client::spawn_workspace_open(&fixture, editors, main);
    client.wait_raw_mode();
    client.wait_header("Editors", "Main");

    // A shell response gives the acceptance a concrete, parsed-screen barrier
    // before copy mode captures the visible pane and cursor.
    client.type_shell("PS1='P5T$ '; printf 'P5_COPY_MODE_READY\\n'");
    client.wait_marker("P5_COPY_MODE_READY");
    client.wait_normal_prompt("fixture shell prompt installed");

    client.ctrl_b(b'c');
    client.wait_screen("copy caret in pane content", |parser, _| {
        parser.screen().hide_cursor() && copy_mode_caret(parser).is_some()
    });
    let initial_caret =
        copy_mode_caret(&client.parser()).expect("copy caret is an inverse content cell");

    // Move locally and observe the inverse cell move while its previous cell
    // clears. The copy caret is rendered as cell styling; the hidden hardware
    // cursor position is not a copy-mode caret position.
    client.write(b"\x1b[C");
    client.wait_screen("copy caret movement", |parser, _| {
        let current = copy_mode_caret(parser);
        let old_cleared = parser
            .screen()
            .cell(initial_caret.0, initial_caret.1)
            .is_some_and(|cell| !cell.inverse());
        parser.screen().hide_cursor()
            && current.is_some_and(|position| position != initial_caret)
            && old_cleared
    });
    let moved_caret = copy_mode_caret(&client.parser()).expect("moved inverse copy caret");
    assert_ne!(moved_caret, initial_caret);
    assert!(client
        .parser()
        .screen()
        .cell(initial_caret.0, initial_caret.1)
        .is_some_and(|cell| !cell.inverse()));

    // This marker-only redirection has no whitespace or copy-mode anchor/copy
    // key. If it leaks after unanchored Enter, the fixture home gains P5_FLAG.
    const COPY_MODE_INPUT: &[u8] = b">P5_FLAG";
    assert!(!COPY_MODE_INPUT.contains(&b' '));
    assert!(!COPY_MODE_INPUT.contains(&b'y') && !COPY_MODE_INPUT.contains(&b'Y'));
    let marker = fixture.home.join("P5_FLAG");
    assert!(!marker.exists(), "fixture marker must start absent");
    client.write(COPY_MODE_INPUT);
    client.write(b"\r");

    // Wait for the visible shell cursor, cleared content caret and normal
    // fixture prompt before sending a barrier command to the child shell.
    client.wait_normal_prompt("unanchored Enter exits copy mode");
    client.type_shell("printf 'P5_COPY_MODE_ENTER_EXITED\\n'");
    client.wait_marker("P5_COPY_MODE_ENTER_EXITED");
    assert!(
        !marker.exists(),
        "copy-mode typing or Enter reached the child shell"
    );

    // Escape cancels locally. A subsequent shell barrier proves normal child
    // input has resumed, and Ctrl-X still restores the outer terminal.
    client.ctrl_b(b'c');
    client.wait_screen("second copy caret", |parser, _| {
        parser.screen().hide_cursor() && copy_mode_caret(parser).is_some()
    });
    client.write(&[0x1b]);
    client.wait_normal_prompt("Escape exits copy mode");
    client.type_shell("printf 'P5_COPY_MODE_ESC_EXITED\\n'");
    client.wait_marker("P5_COPY_MODE_ESC_EXITED");
    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0, "Ctrl-X detach should succeed");
    client.assert_restored();
    assert!(server_lock_held(
        &fixture.route_root(editors, main).join("server")
    ));
}

#[test]
fn workspace_open_pane_operations_acceptance_uses_attached_keyboard_and_live_ptys() {
    let mut fixture = Fixture::new();
    fixture.create_project("Editors", "Main");
    let project = fixture.project_id("Editors");
    let tab = fixture.tab_id("Editors", "Main");
    fixture.own_route(project, tab);

    let mut client = Client::spawn_workspace_open(&fixture, project, tab);
    client.wait_raw_mode();
    client.wait_header("Editors", "Main");
    // The first shell command installs a known prompt and serves as the
    // concrete readiness barrier; the outer header alone is not shell proof.
    client.type_shell("PS1='P5T$ '; printf 'P5A_INIT\\n'");
    wait_whole_root_marker(&client, "P5A_INIT");

    // Ctrl-B's prefix is observed in the rendered NAV state before the direct
    // split key is sent. The newly focused pane must answer through its shell.
    client.begin_navigation();
    client.write(b"r");
    wait_pane_shell_prompt(&client, false);
    client.type_shell("PS1='P5T$ '; printf 'P5B_INIT\\n'");
    client.wait_screen("pane-local P5B_INIT", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5B_INIT").is_some())
    });
    let initial = client.parser();
    let (left_initial, right_initial) =
        split_pane_cells(&initial).expect("vertical divider rendered");
    let a_initial = pane_marker_position(&initial, left_initial, "P5A_INIT")
        .expect("pane-local left shell marker");
    let b_initial = pane_marker_position(&initial, right_initial, "P5B_INIT")
        .expect("pane-local right shell marker");
    let initial_divider = rendered_divider_column(&initial).expect("vertical divider rendered");
    assert!(
        a_initial.1 < initial_divider,
        "A marker must be left of divider"
    );
    assert!(
        b_initial.1 > initial_divider,
        "B marker must be right of divider"
    );

    client.type_shell("printf 'P5PID_B_%d\\n' $$; printf 'P5B_ID\\n'");
    client.wait_screen("pane-local P5B_ID", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5B_ID").is_some())
    });
    let b_id_parser = client.parser();
    let (_, right_initial) = split_pane_cells(&b_id_parser).expect("split panes remain visible");
    let b_pid = pane_shell_pid(&b_id_parser, right_initial, "P5PID_B_");
    client.type_shell("stty size; printf 'P5SB\\n'");
    client.wait_screen("pane-local P5SB", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5SB").is_some())
    });
    let b_size_parser = client.parser();
    let (_, right_initial) = split_pane_cells(&b_size_parser).expect("split panes remain visible");
    let size_b_before = pane_size_before_marker(&b_size_parser, right_initial, "P5SB");
    assert_eq!(size_b_before, expected_pane_size(right_initial));

    client.navigation_key(b"\x1b[D");
    wait_pane_shell_prompt(&client, true);
    client.type_shell("printf 'P5PID_A_%d\\n' $$; printf 'P5A_ID\\n'");
    client.wait_screen("pane-local P5A_ID", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(left, _)| pane_marker_position(parser, left, "P5A_ID").is_some())
    });
    let a_id_parser = client.parser();
    let (left_initial, _) = split_pane_cells(&a_id_parser).expect("split panes remain visible");
    let a_pid = pane_shell_pid(&a_id_parser, left_initial, "P5PID_A_");
    client.type_shell("stty size; printf 'P5SA\\n'");
    client.wait_screen("pane-local P5SA", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(left, _)| pane_marker_position(parser, left, "P5SA").is_some())
    });
    let a_size_parser = client.parser();
    let (left_initial, _) = split_pane_cells(&a_size_parser).expect("split panes remain visible");
    let size_a_before = pane_size_before_marker(&a_size_parser, left_initial, "P5SA");
    assert_eq!(size_a_before, expected_pane_size(left_initial));
    assert_eq!(size_b_before.0, size_a_before.0);

    // Zoom is requested through Ctrl-B z. The header remains at row zero and
    // the focused A shell renders against the full root width and height.
    client.begin_navigation();
    client.write(b"z");
    client.wait_screen("zoomed pane without vertical divider", |parser, screen| {
        rendered_divider_column(parser).is_none()
            && inverse_cells_below_header(parser) == 0
            && screen
                .lines()
                .next()
                .is_some_and(|line| line.contains("Editors"))
    });
    wait_root_shell_prompt(&client);
    client.type_shell("stty size; printf 'P5SAZ\\n'");
    wait_whole_root_marker(&client, "P5SAZ");
    wait_root_shell_prompt(&client);
    let zoomed = client.parser();
    assert_eq!(
        pane_size_before_marker(&zoomed, whole_root_cells(), "P5SAZ"),
        expected_pane_size(whole_root_cells())
    );
    let zoomed_marker = pane_marker_position(&zoomed, whole_root_cells(), "P5SAZ")
        .expect("pane-local zoom marker visible");
    let (cursor_row, cursor_column) = zoomed.screen().cursor_position();
    assert!(
        zoomed_marker.0 > 0,
        "zoomed pane content starts below header"
    );
    assert!(
        usize::from(cursor_row) >= whole_root_cells().top
            && usize::from(cursor_row) < whole_root_cells().bottom,
        "cursor remains in pane below header"
    );
    assert!(
        usize::from(cursor_column) >= whole_root_cells().left
            && usize::from(cursor_column) < whole_root_cells().right,
        "zoomed cursor stays within root content: column={cursor_column}, bounds={}..{}, terminal_columns={COLUMNS}",
        whole_root_cells().left,
        whole_root_cells().right
    );

    // Unzoom, then open the visible swap chooser. The short comment is a
    // harmless leak sentinel: if chooser input reaches zsh, it comments out
    // the following shell barrier and the bounded wait fails.
    client.begin_navigation();
    client.write(b"z");
    client.wait_screen("unzoomed vertical divider", |parser, _| {
        rendered_divider_column(parser).is_some() && inverse_cells_below_header(parser) == 0
    });
    wait_pane_shell_prompt(&client, true);

    // Refresh short exact-line markers immediately before swap so subsequent
    // assertions do not depend on startup output surviving PTY reflow.
    wait_pane_shell_prompt(&client, true);
    client.type_shell("printf 'P5A_SWAP\\n'");
    client.wait_screen("pane-local P5A_SWAP", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(left, _)| pane_marker_position(parser, left, "P5A_SWAP").is_some())
    });
    client.navigation_key(b"\x1b[C");
    wait_pane_shell_prompt(&client, false);
    client.type_shell("printf 'P5B_SWAP\\n'");
    client.wait_screen("pane-local P5B_SWAP", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5B_SWAP").is_some())
    });
    client.navigation_key(b"\x1b[D");
    wait_pane_shell_prompt(&client, true);

    client.begin_navigation();
    client.write(b"m");
    client.wait_screen("swap chooser selection", |parser, _| {
        inverse_cells_below_header(parser) > 0
    });
    client.write(b"#P5SWAP");
    client.write(b"\r");
    client.wait_screen("swapped pane-local shell markers", |parser, _| {
        split_pane_cells(parser).is_some_and(|(left, right)| {
            inverse_cells_below_header(parser) == 0
                && pane_marker_position(parser, left, "P5B_SWAP").is_some()
                && pane_marker_position(parser, right, "P5A_SWAP").is_some()
        })
    });
    assert!(
        !wait_focused_split_shell_prompt(&client),
        "A remains focused on the right after swap"
    );
    client.type_shell("printf 'P5PID_A_%d\\n' $$; printf 'P5A_POST\\n'");
    client.wait_screen("pane-local P5A_POST", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5A_POST").is_some())
    });
    let a_post_swap = client.parser();
    let (_, right_swapped) = split_pane_cells(&a_post_swap).expect("two pane regions remain");
    assert_eq!(
        pane_shell_pid(&a_post_swap, right_swapped, "P5PID_A_"),
        a_pid
    );

    // Divider mode consumes its arrow locally. Preview must move the rendered
    // internal divider before Enter commits; actual PTY dimensions are checked
    // against the post-commit cell geometry from both live shells.
    client.begin_navigation();
    client.write(b"e");
    client.wait_screen("divider preview selection", |parser, _| {
        inverse_cells_below_header(parser) > 0
    });
    let divider_before_preview =
        rendered_divider_column(&client.parser()).expect("initial preview divider");
    client.write(b"\x1b[C");
    client.wait_screen(
        "local vertical divider preview moved right-arrow",
        |parser, _| {
            rendered_divider_column(parser).is_some_and(|column| column != divider_before_preview)
        },
    );
    let preview_divider = rendered_divider_column(&client.parser()).expect("moved divider preview");
    assert_ne!(preview_divider, divider_before_preview);
    client.write(b"\r");
    client.wait_screen("committed divider geometry", |parser, _| {
        inverse_cells_below_header(parser) == 0
            && rendered_divider_column(parser) == Some(preview_divider)
    });

    wait_pane_shell_prompt(&client, false);
    client.type_shell("printf 'P5PID_A_%d\\n' $$; stty size; printf 'P5SA2\\n'");
    client.wait_screen("pane-local P5SA2", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "P5SA2").is_some())
    });
    let a_after_divider = client.parser();
    let (_, right_after_divider) =
        split_pane_cells(&a_after_divider).expect("split panes remain visible");
    assert_eq!(
        pane_shell_pid(&a_after_divider, right_after_divider, "P5PID_A_"),
        a_pid
    );
    assert_pane_size(&a_after_divider, right_after_divider, "P5SA2");
    let (cursor_row, _) = client.parser().screen().cursor_position();
    assert!(
        usize::from(cursor_row) >= right_after_divider.top,
        "post-divider shell cursor remains below header"
    );

    // Focus the left/B shell through the rendered NAV prefix, and verify its
    // real PTY size plus process identity after swap and divider commit.
    client.navigation_key(b"\x1b[D");
    wait_pane_shell_prompt(&client, true);
    client.type_shell("printf 'P5PID_B_%d\\n' $$; stty size; printf 'P5SB2\\n'");
    client.wait_screen("pane-local P5SB2", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(left, _)| pane_marker_position(parser, left, "P5SB2").is_some())
    });
    let b_post_divider = client.parser();
    let (left_after_divider, _) =
        split_pane_cells(&b_post_divider).expect("split panes remain visible");
    assert_eq!(
        pane_shell_pid(&b_post_divider, left_after_divider, "P5PID_B_"),
        b_pid
    );
    assert_pane_size(&b_post_divider, left_after_divider, "P5SB2");

    // A second divider preview is canceled with Escape. The divider returns to
    // its committed column, and the shell barrier proves no mode bytes leaked.
    client.begin_navigation();
    client.write(b"e");
    client.wait_screen("second divider preview selection", |parser, _| {
        inverse_cells_below_header(parser) > 0
    });
    client.write(b"\x1b[C");
    client.wait_screen("second divider preview moves right", |parser, _| {
        rendered_divider_column(parser).is_some_and(|column| column != preview_divider)
    });
    client.write(b"#P5CANCEL");
    client.write(&[0x1b]);
    client.wait_screen("divider preview canceled", |parser, _| {
        rendered_divider_column(parser) == Some(preview_divider)
            && inverse_cells_below_header(parser) == 0
    });
    wait_pane_shell_prompt(&client, true);
    client.type_shell("printf 'P5CANCEL_OK\\n'");
    client.wait_screen("pane-local P5CANCEL_OK", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(left, _)| pane_marker_position(parser, left, "P5CANCEL_OK").is_some())
    });

    // Detach restores the outer PTY modes while the private route server and
    // its child shells remain live for Fixture-owned cleanup.
    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0, "Ctrl-X detach should succeed");
    client.assert_restored();
    assert!(server_lock_held(
        &fixture.route_root(project, tab).join("server")
    ));
}

/// SGR coordinates are one-based; test assertions use parsed zero-based cells.
fn pointer_event(client: &mut Client, code: u8, column: usize, row: usize, release: bool) {
    let suffix = if release { 'm' } else { 'M' };
    client.write(format!("\x1b[<{code};{};{}{suffix}", column + 1, row + 1).as_bytes());
}

fn click_cell(client: &mut Client, column: usize, row: usize) {
    pointer_event(client, 0, column, row, false);
    pointer_event(client, 0, column, row, true);
}

fn visible_label_position(parser: &vt100::Parser, label: &str) -> Option<(usize, usize)> {
    (0..ROWS as usize).find_map(|row| {
        let line = (0..COLUMNS as usize)
            .map(|column| {
                let text = pane_cell_text(parser, row, column);
                if text.is_empty() {
                    " ".to_owned()
                } else {
                    text
                }
            })
            .collect::<String>();
        line.find(label)
            .map(|index| (row, line[..index].chars().count()))
    })
}

fn click_visible_label(client: &mut Client, label: &str) {
    client.wait_screen(label, |parser, _| {
        visible_label_position(parser, label).is_some()
    });
    let (row, column) = visible_label_position(&client.parser(), label).expect("visible UI label");
    click_cell(client, column + 1, row);
}

#[test]
fn workspace_pointer_controls_preserve_live_ptys_and_confirm_close() {
    let mut fixture = Fixture::new();
    fixture.create_project("Pointer", "Main");
    let project = fixture.project_id("Pointer");
    let tab = fixture.tab_id("Pointer", "Main");
    fixture.own_route(project, tab);
    let mut client = Client::spawn_workspace_open(&fixture, project, tab);
    client.wait_raw_mode();
    client.wait_header("Pointer", "Main");
    client.type_shell("PS1='P5T$ '; printf 'PTR_READY\\n'");
    wait_whole_root_marker(&client, "PTR_READY");
    wait_root_shell_prompt(&client);

    // These controls occupy the border below the workspace header. Both are
    // addressed through rendered labels rather than arbitrary content clicks.
    click_visible_label(&mut client, "[Menu]");
    client.wait_screen("discoverable pane actions", |_, screen| {
        screen.contains("Split") && screen.contains("Zoom") && screen.contains("More")
    });
    click_visible_label(&mut client, "[More");
    client.wait_screen("remaining pane actions", |_, screen| {
        screen.contains("Close") && screen.contains("Detach")
    });
    click_visible_label(&mut client, "[Cancel]");
    wait_root_shell_prompt(&client);
    click_visible_label(&mut client, "[x]");
    client.wait_screen("close confirmation", |_, screen| screen.contains("Cancel"));
    click_visible_label(&mut client, "Cancel");
    wait_root_shell_prompt(&client);
    client.type_shell("printf 'PTR_CANCEL_OK\\n'");
    wait_whole_root_marker(&client, "PTR_CANCEL_OK");

    client.begin_navigation();
    client.write(b"r");
    wait_pane_shell_prompt(&client, false);
    client.type_shell("PS1='P5T$ '; printf 'PTR_PID_%d\\n' $$; printf 'PTR_RIGHT\\n'");
    client.wait_screen("right pane readiness", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "PTR_RIGHT").is_some())
    });
    let before = client.parser();
    let (_, right) = split_pane_cells(&before).unwrap();
    let right_pid = pane_shell_pid(&before, right, "PTR_PID_");
    let divider = rendered_divider_column(&before).unwrap();
    let desired = divider + 3;
    pointer_event(&mut client, 0, divider, 8, false);
    pointer_event(&mut client, 32, desired, 8, false);
    client.wait_screen("pointer divider preview", |parser, _| {
        rendered_divider_column(parser) == Some(desired)
    });
    pointer_event(&mut client, 0, desired, 8, true);
    client.wait_screen("pointer divider commit", |parser, _| {
        rendered_divider_column(parser) == Some(desired)
            && !parser.screen().hide_cursor()
            && split_pane_cells(parser)
                .is_some_and(|(_, right)| shell_prompt_is_visible(parser, right))
    });
    client.type_shell("printf 'PTR_AFTER_PID_%d\\n' $$; stty size; printf 'PTR_SIZE\\n'");
    client.wait_screen("resized shell response", |parser, _| {
        split_pane_cells(parser)
            .is_some_and(|(_, right)| pane_marker_position(parser, right, "PTR_SIZE").is_some())
    });
    let after = client.parser();
    let (_, right) = split_pane_cells(&after).unwrap();
    assert_eq!(pane_shell_pid(&after, right, "PTR_AFTER_PID_"), right_pid);
    assert_pane_size(&after, right, "PTR_SIZE");

    client
        .type_shell("for i in {1..80}; do printf 'PTR_ROW_%03d\\n' $i; done; printf 'PTR_TAIL\\n'");
    client.wait_screen("scrollable right pane tail", |parser, _| {
        split_pane_cells(parser).is_some_and(|(_, right)| {
            pane_marker_position(parser, right, "PTR_TAIL").is_some()
                && shell_prompt_is_visible(parser, right)
        })
    });
    // A fast wheel supplies only a conservative extent. This client defers
    // a drag until extent discovery completes. Wait for actual oldest content
    // after the deferred release before starting the next drag.
    pointer_event(&mut client, 64, COLUMNS as usize - 5, 8, false);
    client.wait_screen("normal history thumb", |parser, _| {
        parser.screen().hide_cursor()
            && (2..ROWS as usize - 1)
                .any(|row| pane_cell_text(parser, row, COLUMNS as usize - 1) == "#")
    });
    let thumb_row = (2..ROWS as usize - 1)
        .find(|&row| pane_cell_text(&client.parser(), row, COLUMNS as usize - 1) == "#")
        .expect("visible right history thumb");
    pointer_event(&mut client, 0, COLUMNS as usize - 1, thumb_row, false);
    pointer_event(&mut client, 32, COLUMNS as usize - 1, 2, false);
    pointer_event(&mut client, 0, COLUMNS as usize - 1, 2, true);
    // The press began before the real extent was known. The view stays put
    // until discovery returns, then the deferred release seeks the oldest
    // rows; wait for that content rather than a conservative thumb frame.
    client.wait_screen("oldest history through scrollbar drag", |parser, _| {
        pane_cell_text(parser, 2, COLUMNS as usize - 1) == "#"
            && split_pane_cells(parser).is_some_and(|(_, right)| {
                pane_marker_position(parser, right, "PTR_TAIL").is_none()
                    && pane_marker_position(parser, right, "PTR_ROW_001").is_some()
            })
    });
    pointer_event(&mut client, 0, COLUMNS as usize - 1, 2, false);
    pointer_event(
        &mut client,
        32,
        COLUMNS as usize - 1,
        ROWS as usize - 2,
        false,
    );
    pointer_event(
        &mut client,
        0,
        COLUMNS as usize - 1,
        ROWS as usize - 2,
        true,
    );
    client.wait_screen("scrollbar drag returns to live tail", |parser, _| {
        split_pane_cells(parser).is_some_and(|(_, right)| {
            pane_marker_position(parser, right, "PTR_TAIL").is_some()
                && shell_prompt_is_visible(parser, right)
        })
    });

    // Click the right pane's own close control; a later click confirms. The
    // left pane survives and expands, proving the click did not close focus's
    // replacement or become shell input.
    click_cell(&mut client, COLUMNS as usize - 3, 1);
    client.wait_screen("right close confirmation", |_, screen| {
        screen.contains("Cancel")
    });
    click_visible_label(&mut client, "[Close]");
    client.wait_screen("surviving expanded pane", |parser, _| {
        rendered_divider_column(parser).is_none() && !parser.screen().hide_cursor()
    });
    wait_root_shell_prompt(&client);
    client.type_shell("printf 'PTR_SURVIVOR\\n'");
    wait_whole_root_marker(&client, "PTR_SURVIVOR");
    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0);
    client.assert_restored();
    assert!(server_lock_held(
        &fixture.route_root(project, tab).join("server")
    ));
}

#[test]
fn workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe() {
    let mut fixture = Fixture::new();
    fixture.create_project("Editors", "Main");
    let project = fixture.project_id("Editors");
    let main = fixture.tab_id("Editors", "Main");
    fixture.own_route(project, main);

    let mut client = Client::spawn_workspace_open(&fixture, project, main);
    client.wait_raw_mode();
    client.wait_header("Editors", "Main");
    client.type_shell("PS1='P5T$ '; printf 'UX06_MAIN_READY\\n'");
    client.wait_marker("UX06_MAIN_READY");

    // The visible plus is the only create gesture. Ready must select its exact
    // newly registered Shell route without replacing the existing Main server.
    client.wait_screen("visible tab create control", |parser, _| {
        parser
            .screen()
            .contents()
            .lines()
            .next()
            .is_some_and(|header| header.trim_end().ends_with("[+ New]"))
    });
    client.click_text("[+ New]");
    client.wait_header("Editors", "Shell 1");
    let shell = fixture.tab_id("Editors", "Shell 1");
    fixture.own_route(project, shell);
    assert!(server_lock_held(
        &fixture.route_root(project, main).join("server")
    ));
    assert!(server_lock_held(
        &fixture.route_root(project, shell).join("server")
    ));
    assert!(fixture.route_context_count(project, shell) > 0);

    client
        .type_shell("PS1='P5T$ '; printf 'UX06_PID_BEFORE_%d\\n' $$; printf 'UX06_SHELL_READY\\n'");
    client.wait_marker("UX06_SHELL_READY");
    let before = client.parser();
    let shell_pid = pane_shell_pid(&before, whole_root_cells(), "UX06_PID_BEFORE_");
    let shell_generation = fs::read(fixture.route_root(project, shell).join("server/generation"))
        .expect("created Shell generation");

    // Rename replaces the bounded draft and commits through the current
    // workspace revision; the ID, generation, server and child stay in place.
    client.ctrl_b(b't');
    client.write(b"r");
    client.wait_screen("inline tab rename", |_, screen| {
        screen.contains("Rename tab") && screen.contains("[Save]") && screen.contains("[Cancel]")
    });
    client.write(&[0x7f; 7]);
    client.write(b"Shell Renamed");
    client.write(&[13]);
    client.wait_header("Editors", "Shell Renamed");
    assert_eq!(fixture.tab_id("Editors", "Shell Renamed"), shell);
    assert_eq!(
        fs::read(fixture.route_root(project, shell).join("server/generation"),)
            .expect("same Shell generation after rename"),
        shell_generation
    );
    client.type_shell("printf 'UX06_PID_AFTER_%d\\n' $$; printf 'UX06_RENAME_READY\\n'");
    client.wait_marker("UX06_RENAME_READY");
    assert_eq!(
        pane_shell_pid(&client.parser(), whole_root_cells(), "UX06_PID_AFTER_"),
        shell_pid
    );

    // Cancel is a real mouse target and changes no registry or process state.
    client.ctrl_b(b't');
    client.write(b"x");
    client.wait_screen("close tab confirmation", |_, screen| {
        screen.contains("Ends all tab processes") && screen.contains("[Close tab] [Cancel]")
    });
    client.click_text("[Cancel]");
    assert!(server_lock_held(
        &fixture.route_root(project, shell).join("server")
    ));
    assert_eq!(fixture.tab_id("Editors", "Shell Renamed"), shell);

    // A second, explicit confirmation stops exactly Shell 1. The deterministic
    // adjacent route is Main; no replacement shell is created.
    client.ctrl_b(b't');
    client.write(b"x");
    client.wait_screen("second close confirmation", |_, screen| {
        screen.contains("[Close tab] [Cancel]")
    });
    client.click_text("[Close tab]");
    assert!(
        server_lock_held(&fixture.route_root(project, main).join("server")),
        "closing Shell 1 must leave Main's server lock held"
    );
    client.wait_header("Editors", "Main");
    let shell_server = fixture.route_root(project, shell).join("server");
    let registry_after_close = fixture.run(&["workspace", "list"]);
    assert!(
        !server_lock_held(&shell_server),
        "Shell server remains held (pid {:?}) after close; registry:\n{}\nClient screen:\n{}",
        verified_server_pid(&fixture.route_root(project, shell)),
        String::from_utf8_lossy(&registry_after_close.stdout),
        client.screen()
    );
    assert!(server_lock_held(
        &fixture.route_root(project, main).join("server")
    ));
    assert!(
        registry_after_close.status.success(),
        "list after close: {registry_after_close:?}"
    );
    assert!(!String::from_utf8_lossy(&registry_after_close.stdout).contains("Shell Renamed"));

    // Closing Editors/Main deterministically selects the reserved compatibility
    // route. Closing that final visible route must leave a client-only empty
    // view, not launch a replacement process.
    client.ctrl_b(b't');
    client.write(b"x");
    client.wait_screen("Editors/Main close confirmation", |_, screen| {
        screen.contains("[Close tab] [Cancel]")
    });
    client.click_text("[Close tab]");
    client.wait_header("Default", "Main");
    let default_server = fixture.root.join("server");
    let default_generation_path = default_server.join("generation");
    let stopped_generation = fs::read(&default_generation_path)
        .expect("the deterministic reserved fallback has one generation");
    assert!(server_lock_held(&default_server));

    client.ctrl_b(b't');
    client.write(b"x");
    client.wait_screen("last visible tab close confirmation", |_, screen| {
        screen.contains("[Close tab] [Cancel]")
    });
    client.click_text("[Close tab]");
    client.wait_screen("empty workspace after last close", |_, screen| {
        screen.contains("No open tabs.") && screen.contains("New tab") && screen.contains("Detach")
    });
    assert!(!server_lock_held(&default_server));
    assert_eq!(
        fs::read(&default_generation_path).expect("closed route keeps its generation record"),
        stopped_generation,
        "empty-state installation did not spawn a replacement"
    );
    assert!(!server_lock_held(
        &fixture.route_root(project, main).join("server")
    ));

    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0, "Ctrl-X detach should succeed");
    client.assert_restored();
}
