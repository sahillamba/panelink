#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Four-pane attach acceptance through the real client on a private outer PTY.
//! Server state is observed with target-scoped `panelink control` contexts.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panelink::pane_layout::Rect;
use panelink::terminal::compose::pane_controls;
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::{tcgetattr, LocalModes, Termios, Winsize};
use rustix_openpty::openpty;

const STEP: Duration = Duration::from_secs(10);
const ROWS: u16 = 24;
const COLUMNS: u16 = 100;
const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    terminfo: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-p3-attach-four-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time is after the epoch")
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        make_private_dir(&root);
        let fixture = Self {
            runtime: root.join("runtime"),
            terminfo: root.join("terminfo"),
            home: root.join("home"),
            root,
        };
        make_private_dir(&fixture.runtime);
        make_private_dir(&fixture.terminfo);
        make_private_dir(&fixture.home);
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("PS1", "ATTACH4$ ")
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn control(&self, context: &str, args: &[&str]) -> (i32, String) {
        let output = self
            .command()
            .arg("control")
            .arg("--directory")
            .arg(&self.runtime)
            .arg("--context")
            .arg(self.root.join(context))
            .args(args)
            .output()
            .expect("run control CLI");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn layout(&self, context: &str) -> String {
        let (code, output) = self.control(context, &["layout"]);
        assert_eq!(code, 0, "control layout: {output}");
        output
    }

    fn wait_layout(&self, context: &str, count: usize) -> String {
        wait_control(&format!("{count} panes"), || {
            let layout = self.layout(context);
            (field(&layout, "pane_count").and_then(|value| value.parse().ok()) == Some(count))
                .then_some(layout)
        })
    }

    fn init_target(&self, context: &str, target: &str) {
        let (code, output) = self.control(context, &["init", target]);
        assert_eq!(code, 0, "control init for {target}: {output}");
    }

    fn read_pane(&self, context: &str, target: &str) -> String {
        let (code, output) = self.control(context, &["read", target]);
        assert_eq!(code, 0, "control read for {target}: {output}");
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_private_dir(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("create private fixture directory");
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("\"{name}\":");
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn targets(layout: &str) -> Vec<String> {
    layout
        .match_indices("\"target\":\"")
        .map(|(index, pattern)| {
            let rest = &layout[index + pattern.len()..];
            rest[..rest.find('"').expect("target string is closed")].to_owned()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PaneRect {
    x: u16,
    y: u16,
    rows: u16,
    columns: u16,
}

impl PaneRect {
    fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.columns)
            && y < self.y.saturating_add(self.rows)
    }

    fn center(self) -> (u16, u16) {
        (self.x + self.columns / 2, self.y + self.rows / 2)
    }
}

fn rect_for(layout: &str, target: &str) -> PaneRect {
    let marker = format!("\"target\":\"{target}\"");
    let start = layout
        .find(&marker)
        .unwrap_or_else(|| panic!("target {target} missing from layout: {layout}"));
    let end = start + layout[start..].find('}').expect("pane object is closed");
    let pane = &layout[start..=end];
    let number = |name| {
        field(pane, name)
            .unwrap_or_else(|| panic!("pane field {name} missing from {pane}"))
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("pane field {name} is not a u16 in {pane}"))
    };
    PaneRect {
        x: number("x"),
        y: number("y"),
        rows: number("rows"),
        columns: number("columns"),
    }
}

struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(fixture: &Fixture) -> Option<(Self, String)> {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let tic = Command::new("/usr/bin/tic")
            .args([OsStr::new("-x"), OsStr::new("-o")])
            .arg(&fixture.terminfo)
            .arg(&source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("run tic");
        assert!(tic.success(), "compile private terminfo");

        let mut child = fixture
            .command()
            .arg("serve")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--terminfo")
            .arg(&fixture.terminfo)
            .args([
                "--initialize",
                "--allow-input",
                "--",
                "/bin/zsh",
                "-f",
                "-c",
                "printf 'PANE1-READY\\n'; exec /bin/zsh -f -i",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn panelink serve");
        let lines = line_reader(child.stdout.take().expect("server stdout"));
        let server = Self { child, lines };
        let startup = server.lines.recv_timeout(STEP).expect("serve startup line");
        if startup.contains("listener_unavailable") {
            println!("SKIPPED: pathname bind unavailable on this host: {startup}");
            return None;
        }
        assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
        let first = field(&startup, "target")
            .expect("initial target")
            .to_owned();
        Some((server, first))
    }

    fn sigterm_and_wait(&mut self) -> ExitStatus {
        if self.child.try_wait().ok().flatten().is_none() {
            // SAFETY: signal only the server process created by this fixture.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
        }
        wait_child(&mut self.child, STEP).expect("server stops after SIGTERM")
    }

    fn is_running(&mut self) -> bool {
        self.child
            .try_wait()
            .expect("poll server process")
            .is_none()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // SAFETY: signal only the server process created by this fixture.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            if wait_child(&mut self.child, Duration::from_secs(3)).is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

fn line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut stdout = stdout.take(64 * 1024);
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while let Ok(1) = stdout.read(&mut byte) {
            if byte[0] == b'\n' {
                if sender
                    .send(String::from_utf8_lossy(&buffer).into_owned())
                    .is_err()
                {
                    return;
                }
                buffer.clear();
            } else {
                buffer.push(byte[0]);
            }
        }
    });
    receiver
}

fn wait_child(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The real attach client on a private 100x24 outer PTY, drained continuously.
struct Client {
    supervisor: Child,
    status_path: PathBuf,
    release_path: PathBuf,
    attach_pid_path: PathBuf,
    master: fs::File,
    slave: std::os::fd::OwnedFd,
    initial_termios: Termios,
    initial_flags: OFlags,
    capture: Arc<Mutex<Vec<u8>>>,
    stop_drain: Arc<std::sync::atomic::AtomicBool>,
    drain: Option<JoinHandle<()>>,
}

impl Client {
    fn spawn(fixture: &Fixture) -> Self {
        use std::sync::atomic::AtomicBool;

        let winsize = Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open private outer PTY");
        let master = fs::File::from(pty.controller);
        let master_flags = fcntl_getfl(&master).expect("read outer master flags");
        fcntl_setfl(&master, master_flags | OFlags::NONBLOCK)
            .expect("set outer master nonblocking");
        let slave = pty.user;
        let initial_termios = tcgetattr(&slave).expect("save outer termios");
        let initial_flags = fcntl_getfl(&slave).expect("save outer slave flags");
        let controlling = slave
            .as_fd()
            .try_clone_to_owned()
            .expect("clone controlling terminal");
        let stdio = || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let status_path = fixture.root.join(format!("attach-{serial}.status"));
        let release_path = fixture.root.join(format!("release-{serial}"));
        let attach_pid_path = fixture.root.join(format!("attach-{serial}.pid"));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(ATTACH_SUPERVISOR)
            .arg("panelink-attach-supervisor")
            .arg(&status_path)
            .arg(&release_path)
            .arg(&attach_pid_path)
            .arg(env!("CARGO_BIN_EXE_panelink"))
            .arg("attach")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--context-dir")
            .arg(fixture.root.join("attach-contexts"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", &fixture.home)
            .env("PS1", "ATTACH4$ ")
            .current_dir(&fixture.home)
            .stdin(stdio())
            .stdout(stdio())
            .stderr(stdio());
        // SAFETY: reviewed Rustix wrappers create a fresh session and set its
        // inherited private PTY as the controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let supervisor = command.spawn().expect("spawn attach on outer PTY");
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
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        Self {
            supervisor,
            status_path,
            release_path,
            attach_pid_path,
            master,
            slave,
            initial_termios,
            initial_flags,
            capture,
            stop_drain,
            drain: Some(drain),
        }
    }

    fn attach_exit(&self) -> Option<i32> {
        let text = fs::read_to_string(&self.status_path).ok()?;
        text.strip_suffix('\n')?.parse().ok()
    }

    fn wait_attach_exit(&self) -> i32 {
        self.wait_attach_exit_within(STEP)
    }

    fn wait_attach_exit_within(&self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(code) = self.attach_exit() {
                return code;
            }
            assert!(Instant::now() < deadline, "attach did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn attach_pid(&self) -> Option<u32> {
        let pid = fs::read_to_string(&self.attach_pid_path)
            .ok()?
            .trim()
            .parse()
            .ok()?;
        (pid > 1 && pid <= i32::MAX as u32 && pid != self.supervisor.id()).then_some(pid)
    }

    fn wait_attach_pid(&self) -> u32 {
        let deadline = Instant::now() + STEP;
        loop {
            if let Some(pid) = self.attach_pid() {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "attach PID marker was not created"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill_attach_and_wait(&mut self) -> (u32, i32) {
        let pid = self.wait_attach_pid();
        // SAFETY: `pid` came from this fixture's private marker, is positive,
        // and is distinct from the supervisor process retained for cleanup.
        let result = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        assert_eq!(result, 0, "SIGKILL only the owned attach process {pid}");
        let status = self.wait_attach_exit_within(STEP);
        assert_ne!(status, 0, "SIGKILL must produce a nonzero attach status");
        // The supervisor deliberately remains alive to retain the outer PTY
        // until this fixture explicitly releases it. A killed attach cannot
        // restore termios, so this scenario can release it immediately.
        fs::write(&self.release_path, b"release\n").expect("release killed-client supervisor");
        let supervisor_status = wait_child(&mut self.supervisor, STEP)
            .expect("supervisor exits after recording killed attach status");
        assert!(
            supervisor_status.success(),
            "supervisor status: {supervisor_status}"
        );
        (pid, status)
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

    fn type_slowly(&mut self, text: &[u8]) {
        for byte in text {
            self.write(&[*byte]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn click(&mut self, rect: PaneRect) {
        let (x, y) = rect.center();
        assert!(rect.contains(x, y), "click lies in pane rectangle {rect:?}");
        let column = x + 1;
        let row = y + 1;
        let click = format!("\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m");
        self.write(click.as_bytes());
    }

    fn capture(&self) -> Vec<u8> {
        self.capture.lock().unwrap().clone()
    }

    fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
        parser.process(&self.capture());
        parser.screen().contents()
    }

    fn wait_screen(&self, what: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + STEP;
        loop {
            let screen = self.screen();
            if done(&screen) {
                return screen;
            }
            if let Some(status) = self.attach_exit() {
                panic!("attach exited ({status}) waiting for {what}: {screen}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{screen}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_raw_mode(&self) {
        let deadline = Instant::now() + STEP;
        loop {
            let termios = tcgetattr(&self.slave).expect("query outer termios");
            if !termios.local_modes.contains(LocalModes::ICANON) {
                return;
            }
            if let Some(status) = self.attach_exit() {
                panic!(
                    "attach exited before raw mode ({status}): {:?}",
                    self.screen()
                );
            }
            assert!(Instant::now() < deadline, "attach never entered raw mode");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn termios_restored(&self) {
        let actual = tcgetattr(&self.slave).expect("query restored outer terminal");
        assert_eq!(actual.input_modes, self.initial_termios.input_modes);
        assert_eq!(actual.output_modes, self.initial_termios.output_modes);
        assert_eq!(actual.control_modes, self.initial_termios.control_modes);
        assert_eq!(actual.local_modes, self.initial_termios.local_modes);
        assert_eq!(actual.input_speed(), self.initial_termios.input_speed());
        assert_eq!(actual.output_speed(), self.initial_termios.output_speed());
        assert_eq!(
            format!("{:?}", actual.special_codes),
            format!("{:?}", self.initial_termios.special_codes)
        );
        assert_eq!(fcntl_getfl(&self.slave).unwrap(), self.initial_flags);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.attach_exit().is_none() {
            let _ = self.master.write_all(b"\x18");
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.attach_exit().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::write(&self.release_path, b"release\n");
        if wait_child(&mut self.supervisor, Duration::from_secs(2)).is_none() {
            let _ = self.supervisor.kill();
            let _ = self.supervisor.wait();
        }
        self.stop_drain.store(true, Ordering::Relaxed);
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

const ATTACH_SUPERVISOR: &str = r#"
status_path=$1
release_path=$2
attach_pid_path=$3
shift 3
trap ':' HUP INT TERM
/bin/sh -c 'printf "%s\n" "$$" > "$1.tmp" && /bin/mv "$1.tmp" "$1"; shift; exec "$@"' panelink-attach-wrapper "$attach_pid_path" "$@"
code=$?
printf '%s\n' "$code" > "$status_path.tmp" && /bin/mv "$status_path.tmp" "$status_path"
while [ ! -f "$release_path" ]; do /bin/sleep 0.05; done
exit 0
"#;

fn split_layout_focused(screen: &str, panes: &[(PaneRect, bool)]) -> bool {
    panes.iter().all(|(rect, focused)| {
        let Some(controls) = pane_controls(Rect {
            x: rect.x,
            y: rect.y,
            rows: rect.rows,
            columns: rect.columns,
        }) else {
            return false;
        };
        let Some(header) = screen.lines().nth(usize::from(rect.y)) else {
            return false;
        };
        let text_at = |x, length| {
            header
                .chars()
                .skip(usize::from(x))
                .take(length)
                .collect::<String>()
        };
        text_at(controls.menu.x, 6) == " Menu "
            && text_at(controls.close.x, 3) == " x "
            && text_at(controls.focus_marker.x, 1) == if *focused { ">" } else { " " }
    })
}

/// Server topology alone precedes client completion. Require that exact layout
/// to be rendered with the uniquely added target focused before sending keys.
fn wait_split_settled(client: &Client, previous: &str, current: &str, before: &str) {
    let old_targets = targets(previous);
    let new_targets = targets(current);
    let added: Vec<_> = new_targets
        .iter()
        .filter(|target| !old_targets.contains(target))
        .collect();
    assert_eq!(added.len(), 1, "one target added by this split: {current}");
    let panes: Vec<_> = new_targets
        .iter()
        .map(|target| (rect_for(current, target), target == added[0]))
        .collect();
    assert!(
        !split_layout_focused(before, &panes),
        "the pre-split screen must not acknowledge the new layout"
    );
    client.wait_screen(
        "new split layout rendered and new target focused",
        |screen| split_layout_focused(screen, &panes),
    );
}

fn wait_control(what: &str, mut check: impl FnMut() -> Option<String>) -> String {
    let deadline = Instant::now() + STEP;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn read_until_contains(fixture: &Fixture, context: &str, target: &str, marker: &str) -> String {
    wait_control(&format!("{marker} in target {target}"), || {
        let text = fixture.read_pane(context, target);
        text.contains(marker).then_some(text)
    })
}

fn wait_file_pid(path: &Path, what: &str) -> u32 {
    wait_control(what, || {
        let text = fs::read_to_string(path).ok()?;
        let pid = text.trim().parse::<u32>().ok()?;
        (pid > 1 && pid <= i32::MAX as u32).then_some(pid.to_string())
    })
    .parse()
    .expect("validated PID")
}

fn wait_pid_absent(pid: u32) {
    let deadline = Instant::now() + STEP;
    loop {
        // SAFETY: signal zero only probes the exact child PID captured from
        // the managed pane's own shell.
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "managed child PID {pid} still exists"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn rendered_pane_contains(parser: &vt100::Parser, rect: PaneRect, needle: &str) -> bool {
    (rect.y..rect.y + rect.rows).any(|row| {
        parser
            .screen()
            .contents_between(row, rect.x, row, rect.x + rect.columns - 1)
            .contains(needle)
    })
}

fn assert_rendered_divider_partition(
    parser: &vt100::Parser,
    rows: u16,
    columns: u16,
    panes: &[PaneRect],
) {
    let mut dividers = BTreeSet::new();
    for y in 0..rows {
        for x in 0..columns {
            if !panes.iter().any(|rect| rect.contains(x, y)) {
                dividers.insert((x, y));
            }
        }
    }
    assert!(!dividers.is_empty(), "rendered layout has divider cells");
    for &(x, y) in &dividers {
        let contents = parser.screen().cell(y, x).unwrap().contents();
        assert!(
            contents == "|" || contents == "-",
            "divider cell ({x},{y}) rendered as {contents:?}; outer screen:\n{}",
            parser.screen().contents()
        );
    }
    let junction = dividers.iter().any(|&(x, y)| {
        [
            (x.checked_sub(1), Some(y)),
            (x.checked_add(1).filter(|next| *next < columns), Some(y)),
            (Some(x), y.checked_sub(1)),
            (Some(x), y.checked_add(1).filter(|next| *next < rows)),
        ]
        .into_iter()
        .filter_map(|(x, y)| x.zip(y))
        .filter(|neighbor| dividers.contains(neighbor))
        .count()
            >= 3
    });
    assert!(
        junction,
        "rendered divider grid includes its T/cross junction"
    );
}

#[test]
fn marker_wait_requires_shell_execution_instead_of_command_echo() {
    let fixture = Fixture::new();
    let Some((_server, target)) = Server::start(&fixture) else {
        return;
    };
    fixture.init_target("pane-0.ctx", &target);
    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("shell ready", |screen| screen.contains("PANE1-READY"));

    // Demonstrate the old barrier's false positive without executing a command.
    client.type_slowly(b"echo ATTACH4_OLD");
    read_until_contains(&fixture, "pane-0.ctx", &target, "ATTACH4_OLD");
    client.write(b"\x15"); // discard the unexecuted draft
    client.type_slowly(b"printf 'ATTACH4_%s\\n' EXECUTED");
    let draft = read_until_contains(&fixture, "pane-0.ctx", &target, "EXECUTED");
    assert!(!draft.contains("ATTACH4_EXECUTED"), "{draft}");
    client.write(b"\r");
    read_until_contains(&fixture, "pane-0.ctx", &target, "ATTACH4_EXECUTED");
}

#[test]
fn real_attach_routes_renders_closes_and_detaches_four_panes() {
    let fixture = Fixture::new();
    let Some((mut server, first)) = Server::start(&fixture) else {
        return;
    };
    fixture.init_target("pane-0.ctx", &first);

    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("initial pane readiness", |screen| {
        screen.contains("PANE1-READY")
    });
    let one = wait_control("100-column outer layout", || {
        let layout = fixture.layout("pane-0.ctx");
        (field(&layout, "columns") == Some("100") && field(&layout, "pane_count") == Some("1"))
            .then_some(layout)
    });

    // Ctrl-B r creates the right pane. Focus left with Ctrl-B + Left, then
    // split Down; focus the original right pane and split it Down as well.
    // Splits complete asynchronously in the client worker; the client refuses
    // a second split while one is pending ("split already pending"), so wait
    // for the client to observe each completion before the next request.
    let before = client.screen();
    client.write(b"\x02r");
    let two = fixture.wait_layout("pane-0.ctx", 2);
    assert_eq!(targets(&two).len(), 2, "{two}");
    wait_split_settled(&client, &one, &two, &before);
    let before = client.screen();
    client.write(b"\x02\x1b[Dd");
    let three = fixture.wait_layout("pane-0.ctx", 3);
    assert_eq!(targets(&three).len(), 3, "{three}");
    wait_split_settled(&client, &two, &three, &before);
    let before = client.screen();
    client.write(b"\x02\x1b[Cd");
    let four = fixture.wait_layout("pane-0.ctx", 4);
    wait_split_settled(&client, &three, &four, &before);
    let pane_targets = targets(&four);
    assert_eq!(pane_targets.len(), 4, "{four}");
    assert!(
        pane_targets.contains(&first),
        "original target retained: {four}"
    );
    let target_set: BTreeSet<_> = pane_targets.iter().collect();
    assert_eq!(
        target_set.len(),
        4,
        "four distinct complete targets: {four}"
    );

    let mut contexts = Vec::with_capacity(4);
    for (index, target) in pane_targets.iter().enumerate() {
        let context = if target == &first {
            "pane-0.ctx".to_owned()
        } else {
            format!("pane-{index}.ctx")
        };
        if target != &first {
            fixture.init_target(&context, target);
        }
        contexts.push(context);
    }

    // Exercise actual mouse focus and key routing to each pane, then use the
    // target-scoped control reads to prove every marker stayed isolated.
    // Build each marker in the shell: echoed command text must not satisfy the
    // wait before Enter executes it and drains the previous pane's input.
    for (index, target) in pane_targets.iter().enumerate() {
        let layout = fixture.layout("pane-0.ctx");
        let rect = rect_for(&layout, target);
        client.click(rect);
        let marker = format!("ATTACH4_MARKER_{index}");
        client.type_slowly(format!("printf 'ATTACH4_%s\\n' MARKER_{index}\r").as_bytes());
        let own = read_until_contains(&fixture, &contexts[index], target, &marker);
        for (other_index, other) in pane_targets.iter().enumerate() {
            if index != other_index {
                let other_screen = fixture.read_pane(&contexts[other_index], other);
                assert!(
                    !other_screen.contains(&marker),
                    "marker {marker} leaked to target {other}: {other_screen}"
                );
            }
        }
        assert!(own.contains(&marker), "target {target}: {own}");
    }
    client.wait_screen("fourth marker rendered", |screen| {
        screen.contains("ATTACH4_MARKER_3")
    });
    // Split feedback temporarily overlays the bottom row, potentially covering
    // divider cells there. Let the client status TTL elapse before asserting
    // the complete pane/divider drawing.
    thread::sleep(Duration::from_secs(4));

    let final_layout = fixture.layout("pane-0.ctx");
    let root_rows = field(&final_layout, "rows")
        .expect("root rows")
        .parse::<u16>()
        .unwrap();
    let root_columns = field(&final_layout, "columns")
        .expect("root columns")
        .parse::<u16>()
        .unwrap();
    let pane_rects: Vec<_> = pane_targets
        .iter()
        .map(|target| rect_for(&final_layout, target))
        .collect();
    let capture = client.capture();
    let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
    parser.process(&capture);
    for (index, (target, rect)) in pane_targets.iter().zip(&pane_rects).enumerate() {
        let marker = format!("ATTACH4_MARKER_{index}");
        assert!(
            rendered_pane_contains(&parser, *rect, &marker),
            "{marker} was not rendered inside target {target} rect {rect:?}; screen:\n{}",
            parser.screen().contents()
        );
    }
    assert_rendered_divider_partition(&parser, root_rows, root_columns, &pane_rects);

    // The menu exposes the pane-limit reason and consumes an attempted Right
    // split without changing the four-pane layout.
    let menu_rect = pane_rects[0];
    client.click(menu_rect);
    client.write(b"\x02s");
    client.wait_screen("pane actions menu", |screen| {
        screen.contains("Layout: Split / Resize")
    });
    client.write(b"\r");
    client.wait_screen("pane limit menu reason", |screen| {
        screen.contains("Pane limit reached (4)")
    });
    client.write(b"r");
    client.wait_screen("pane limit reason on Right selection", |screen| {
        screen.contains("Pane limit reached (4)")
    });
    client.write(b"\x1b");
    client.wait_screen("pane menu closed", |screen| {
        !screen.contains("Pane limit reached")
    });
    let after_fifth_attempt = fixture.layout("pane-0.ctx");
    assert_eq!(field(&after_fifth_attempt, "pane_count"), Some("4"));

    // Close a lower-left leaf with Ctrl-B x/y. Its sibling expands to the
    // entire left subtree, and the remaining three panes stay independently usable.
    let closing = pane_targets
        .iter()
        .map(|target| (target, rect_for(&final_layout, target)))
        .find(|(_, rect)| rect.x == 0 && rect.y > 0)
        .expect("lower-left pane exists");
    let sibling = pane_targets
        .iter()
        .map(|target| (target, rect_for(&final_layout, target)))
        .find(|(target, rect)| *target != closing.0 && rect.x == closing.1.x && rect.y == 0)
        .expect("upper-left sibling exists");
    client.click(closing.1);
    client.write(b"\x02x");
    client.wait_screen("close confirmation", |screen| screen.contains("Close pane"));
    client.write(b"y");
    let after_close = fixture.wait_layout("pane-0.ctx", 3);
    let surviving_targets = targets(&after_close);
    assert_eq!(surviving_targets.len(), 3, "{after_close}");
    assert!(
        !surviving_targets.contains(closing.0),
        "closed target: {after_close}"
    );
    let expanded = rect_for(&after_close, sibling.0);
    assert_eq!(
        expanded.y, 0,
        "expanded sibling starts at root top: {after_close}"
    );
    assert_eq!(
        expanded.rows,
        field(&after_close, "rows").unwrap().parse().unwrap()
    );
    assert!(
        expanded.rows > sibling.1.rows,
        "sibling expanded: {after_close}"
    );

    // The server layout query can observe the close before the attach client
    // has received and rendered the new topology. Wait for the old left
    // divider row to become pane content before clicking using new geometry.
    let old_left_divider_row = closing
        .1
        .y
        .checked_sub(1)
        .expect("lower pane has a divider");
    client.wait_screen("closed left divider removed from client render", |screen| {
        screen
            .lines()
            .nth(usize::from(old_left_divider_row))
            .and_then(|line| line.chars().next())
            .is_some_and(|cell| cell != '-')
    });
    thread::sleep(Duration::from_millis(300));

    let mut after_close_reads = Vec::new();
    for (index, target) in surviving_targets.iter().enumerate() {
        let original_index = pane_targets
            .iter()
            .position(|original| original == target)
            .expect("every survivor was in the original layout");
        let context = &contexts[original_index];
        let rect = rect_for(&after_close, target);
        client.click(rect);
        let marker = format!("ATTACH4_AFTER_CLOSE_{index}");
        client.type_slowly(format!("printf 'ATTACH4_%s\\n' AFTER_CLOSE_{index}\r").as_bytes());
        let text = read_until_contains(&fixture, context, target, &marker);
        after_close_reads.push((target, marker, text));
    }
    for (index, (target, marker, own_screen)) in after_close_reads.iter().enumerate() {
        assert!(own_screen.contains(marker), "target {target}: {own_screen}");
        for (other_index, (other_target, other_marker, other_screen)) in
            after_close_reads.iter().enumerate()
        {
            if index != other_index {
                assert!(
                    !other_screen.contains(marker),
                    "marker {marker} from {target} leaked to {other_target}: {other_screen}"
                );
                assert!(
                    !own_screen.contains(other_marker),
                    "target {target} saw {other_marker} from {other_target}: {own_screen}"
                );
            }
        }
    }

    // Ctrl-X detaches the client only. Verify restored terminal state and that
    // every surviving server-owned child is still running afterward.
    client.write(b"\x18");
    assert_eq!(
        client.wait_attach_exit(),
        0,
        "attach exits cleanly on Ctrl-X"
    );
    client.termios_restored();
    let capture = client.capture();
    for mode in [
        &b"\x1b[?1006l"[..],
        b"\x1b[?1002l",
        b"\x1b[?1000l",
        b"\x1b[?1049l",
        b"\x1b[?2004l",
    ] {
        assert!(
            capture.windows(mode.len()).any(|window| window == mode),
            "{mode:?}"
        );
    }
    for target in &surviving_targets {
        let original_index = pane_targets
            .iter()
            .position(|original| original == target)
            .expect("every survivor was in the original layout");
        let context = &contexts[original_index];
        let (code, described) = fixture.control(context, &["describe"]);
        assert_eq!(code, 0, "control describe: {described}");
        assert_eq!(
            field(&described, "child_exited"),
            Some("false"),
            "{described}"
        );
    }
    drop(client);
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
}

#[test]
fn killed_attach_client_reattaches_to_same_live_child_without_replaying_input() {
    let fixture = Fixture::new();
    let (mut server, target) = Server::start(&fixture)
        .expect("pathname server fixture must be available for abrupt-client acceptance");
    fixture.init_target("crash-reconnect.ctx", &target);

    let ready_marker = format!("ABRUPT_READY_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let child_pid_path = fixture.root.join("managed-child.pid");
    let mut first = Client::spawn(&fixture);
    first.wait_raw_mode();
    let attach_pid = first.wait_attach_pid();
    first.wait_screen("initial child readiness", |screen| {
        screen.contains("PANE1-READY")
    });

    // The ready marker is split across printf arguments so terminal command
    // echo cannot satisfy the assertion before the shell executes it.
    let child_pid_path_text = child_pid_path.to_string_lossy();
    let initial_command = format!(
        "printf 'ABRUPT_' ; printf 'READY_{}\\n'; printf '%s\\n' \"$$\" > '{}'\r",
        ready_marker.trim_start_matches("ABRUPT_READY_"),
        child_pid_path_text
    );
    first.type_slowly(initial_command.as_bytes());
    first.wait_screen("unique ready marker from managed child", |screen| {
        screen.contains(&ready_marker)
    });
    let initial_screen =
        read_until_contains(&fixture, "crash-reconnect.ctx", &target, &ready_marker);
    assert_eq!(
        initial_screen.matches(&ready_marker).count(),
        1,
        "{initial_screen}"
    );
    let child_pid = wait_file_pid(&child_pid_path, "managed child PID marker");
    // SAFETY: signal zero probes only the attach PID recorded by this client.
    assert_eq!(
        unsafe { libc::kill(attach_pid as i32, 0) },
        0,
        "owned attach process is live"
    );
    assert!(server.is_running(), "server is alive before attach SIGKILL");

    let (killed_pid, attach_status) = first.kill_attach_and_wait();
    assert_eq!(
        killed_pid, attach_pid,
        "killed the verified live attach PID"
    );
    assert_ne!(attach_status, 0);
    drop(first); // also close the dead client's outer PTY before reattaching
    assert!(
        server.is_running(),
        "server remains alive after attach SIGKILL"
    );
    let (describe_code, description) = fixture.control("crash-reconnect.ctx", &["describe"]);
    assert_eq!(
        describe_code, 0,
        "describe after client SIGKILL: {description}"
    );
    assert_eq!(
        field(&description, "child_exited"),
        Some("false"),
        "{description}"
    );
    let after_kill_layout = fixture.layout("crash-reconnect.ctx");
    assert_eq!(
        targets(&after_kill_layout),
        vec![target.clone()],
        "{after_kill_layout}"
    );

    // The same fixture root keeps attach context metadata across independent
    // outer PTYs. No input is sent while client ownership is uncertain.
    let mut second = Client::spawn(&fixture);
    second.wait_raw_mode();
    second.wait_screen("previous child output after reattach", |screen| {
        screen.contains(&ready_marker)
    });
    let reattached_layout = fixture.layout("crash-reconnect.ctx");
    assert_eq!(
        targets(&reattached_layout),
        vec![target.clone()],
        "{reattached_layout}"
    );
    let retained = fixture.read_pane("crash-reconnect.ctx", &target);
    assert!(
        retained.contains(&ready_marker),
        "prior child output missing: {retained}"
    );

    let observed_pid_path = fixture.root.join("reattached-child.pid");
    let observed_pid_path_text = observed_pid_path.to_string_lossy();
    second
        .type_slowly(format!("printf '%s\\n' \"$$\" > '{}'\r", observed_pid_path_text).as_bytes());
    let observed_pid = wait_file_pid(&observed_pid_path, "reattached child PID marker");
    assert_eq!(
        observed_pid, child_pid,
        "reattach reached the same managed child"
    );

    // This marker also cannot appear in the echoed command because its two
    // halves are separate printf arguments. It is sent once, after reattach.
    let post_marker = format!("AFTER_KILL_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let post_command = format!(
        "printf 'AFTER_' ; printf 'KILL_{}\\n'\r",
        post_marker.trim_start_matches("AFTER_KILL_"),
    );
    second.type_slowly(post_command.as_bytes());
    let after_input = read_until_contains(&fixture, "crash-reconnect.ctx", &target, &post_marker);
    assert_eq!(
        after_input.matches(&post_marker).count(),
        1,
        "{after_input}"
    );
    assert_eq!(
        after_input.matches(&ready_marker).count(),
        1,
        "old input must not replay after reattachment: {after_input}"
    );

    second.write(b"\x18");
    assert_eq!(
        second.wait_attach_exit(),
        0,
        "reattached client exits cleanly on Ctrl-X"
    );
    second.termios_restored();
    let (describe_code, description) = fixture.control("crash-reconnect.ctx", &["describe"]);
    assert_eq!(
        describe_code, 0,
        "describe before server stop: {description}"
    );
    assert_eq!(
        field(&description, "child_exited"),
        Some("false"),
        "{description}"
    );

    assert_eq!(server.sigterm_and_wait().code(), Some(0));
    wait_pid_absent(child_pid);
}
