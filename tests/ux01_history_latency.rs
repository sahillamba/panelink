#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Ignored synthetic history-scroll measurements through a private outer PTY.
//! These numbers are debug-build observations, not CI timing assertions.

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

const STEP: Duration = Duration::from_secs(15);
const COMMAND_STEP: Duration = Duration::from_secs(12);
const QUIET_WINDOW: Duration = Duration::from_millis(65);
const MAX_CAPTURE: usize = 16 * 1024 * 1024;
const WHEEL_SAMPLES: usize = 30;
const BURST_EVENTS: usize = 20;
const OUTPUT_LINES: usize = 900;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-ux01-{}-{serial}",
            std::process::id()
        ));
        make_private_dir(&root);
        let home = root.join("home");
        make_private_dir(&home);
        Self { root, home }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", "/bin/zsh")
            .env("PS1", "UX01$ ")
            .env("PANELINK_HOME", &self.root)
            .current_dir(&self.home);
        command
    }

    fn stop(&self) -> bool {
        let mut child = match self
            .command()
            .args(["stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return !server_lock_held(&self.root.join("server")),
        };
        let deadline = Instant::now() + COMMAND_STEP;
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if child.try_wait().ok().flatten().is_none() {
            // SAFETY: this command runs in a fixture-owned process group.
            unsafe { libc::kill(-i32::try_from(child.id()).unwrap(), libc::SIGKILL) };
            let _ = child.wait();
        }

        let server = self.root.join("server");
        if wait_server_exit(&server, Duration::from_secs(2)) {
            return true;
        }
        if let Some(pid) = verified_server_pid(&self.root) {
            // SAFETY: PID and kernel start time were checked against this private fixture.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            if wait_server_exit(&server, Duration::from_secs(5)) {
                return true;
            }
            if verified_server_pid(&self.root) == Some(pid) {
                // SAFETY: the same private server process identity was revalidated.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                return wait_server_exit(&server, Duration::from_secs(2));
            }
        }
        false
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.stop() {
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
    // SAFETY: the descriptor names this fixture's private server lock file.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
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

fn verified_server_pid(root: &Path) -> Option<i32> {
    let record = fs::read_to_string(root.join("server.pid")).ok()?;
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
    screen: Arc<Mutex<String>>,
    stop_drain: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
    rows: u16,
    columns: u16,
}

impl Client {
    fn spawn(fixture: &Fixture, rows: u16, columns: u16) -> Self {
        let winsize = Winsize {
            ws_row: rows,
            ws_col: columns,
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
            .arg("ux01-private-pty-supervisor")
            .arg(&status_path)
            .arg(&release_path)
            .arg(env!("CARGO_BIN_EXE_panelink"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", &fixture.home)
            .env("SHELL", "/bin/zsh")
            .env("PS1", "UX01$ ")
            .env("TERM", "xterm-256color")
            .env("PANELINK_HOME", &fixture.root)
            .current_dir(&fixture.home)
            .stdin(stdio())
            .stdout(stdio())
            .stderr(stdio());
        // SAFETY: create a fresh session and use the fixture-owned PTY as its controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let supervisor = command
            .spawn()
            .expect("spawn Panelink on private outer PTY");
        drop(command);

        let screen = Arc::new(Mutex::new(String::new()));
        let stop_drain = Arc::new(AtomicBool::new(false));
        let mut reader = master.try_clone().expect("clone outer PTY master");
        let drain = {
            let screen = Arc::clone(&screen);
            let stop = Arc::clone(&stop_drain);
            thread::spawn(move || {
                let mut parser = vt100::Parser::new(rows, columns, 0);
                let mut buffer = [0_u8; 16 * 1024];
                let mut byte_count = 0_usize;
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(1)),
                        Ok(count) => {
                            byte_count = byte_count.saturating_add(count);
                            if byte_count <= MAX_CAPTURE {
                                parser.process(&buffer[..count]);
                                *screen.lock().unwrap() = parser.screen().contents();
                            }
                        }
                        Err(error)
                            if error.kind() == io::ErrorKind::WouldBlock
                                || error.kind() == io::ErrorKind::Interrupted =>
                        {
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(1)),
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
            screen,
            stop_drain,
            drain: Some(drain),
            rows,
            columns,
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
                panic!("Panelink exited before attach (status {code})");
            }
            assert!(Instant::now() < deadline, "Panelink did not enter raw mode");
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

    fn type_shell(&mut self, command: &str) {
        // If the pane is currently displaying history, this first Enter returns
        // it to live output before the first command byte is sent.
        self.write(b"\r");
        self.write(command.as_bytes());
        self.write(b"\r");
    }

    fn snapshot(&self) -> String {
        self.screen.lock().unwrap().clone()
    }

    fn pane_screen(&self) -> String {
        let screen = self.snapshot();
        screen
            .lines()
            .skip(1)
            .take(usize::from(self.rows.saturating_sub(2)))
            .map(|line| {
                line.chars()
                    .skip(1)
                    .take(usize::from(self.columns.saturating_sub(2)))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wait_screen(&self, description: &str, marker: &str) {
        let deadline = Instant::now() + STEP;
        loop {
            let screen = self.snapshot();
            if screen.contains(marker) {
                return;
            }
            if let Some(code) = self.exit() {
                panic!("Panelink exited ({code}) waiting for {description}:\n{screen}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}:\n{screen}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn try_wait_screen_change(
        &self,
        before: &str,
        started: Instant,
        limit: Duration,
        _description: &str,
    ) -> Option<Duration> {
        let deadline = started + limit;
        loop {
            let current = self.pane_screen();
            if current != before {
                return Some(Instant::now().duration_since(started));
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_screen_change(&self, before: &str, started: Instant, description: &str) -> Duration {
        self.try_wait_screen_change(before, started, Duration::from_secs(5), description)
            .unwrap_or_else(|| panic!("no pane change after {description}"))
    }

    fn wait_pane_stable(&self, start: Instant, description: &str) -> Duration {
        let deadline = start + Duration::from_secs(5);
        let mut previous = self.pane_screen();
        let mut unchanged_since = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(2));
            let current = self.pane_screen();
            if current != previous {
                previous = current;
                unchanged_since = Instant::now();
            } else if Instant::now().duration_since(unchanged_since) >= QUIET_WINDOW {
                return Instant::now().duration_since(start);
            }
            assert!(
                Instant::now() < deadline,
                "pane did not settle after {description}:\n{}",
                self.pane_screen()
            );
        }
    }

    fn wait_pane_quiet(&self, quiet: Duration, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut previous = self.pane_screen();
        let mut unchanged_since = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(2));
            let current = self.pane_screen();
            if current != previous {
                previous = current;
                unchanged_since = Instant::now();
            } else if Instant::now().duration_since(unchanged_since) >= quiet {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane did not remain quiet for {quiet:?} before {description}"
            );
        }
    }

    fn observe_after_release(
        &self,
        before: &str,
        start: Instant,
        limit: Duration,
    ) -> (Option<Duration>, Option<Duration>) {
        let deadline = start + limit;
        let mut previous = before.to_owned();
        let mut first_change = None;
        let mut unchanged_since = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(2));
            let current = self.pane_screen();
            if current != previous {
                first_change.get_or_insert_with(|| Instant::now().duration_since(start));
                previous = current;
                unchanged_since = Instant::now();
            } else if Instant::now().duration_since(unchanged_since) >= QUIET_WINDOW {
                return (first_change, Some(Instant::now().duration_since(start)));
            }
            if Instant::now() >= deadline {
                return (first_change, None);
            }
        }
    }

    fn wait_exit(&self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(code) = self.exit() {
                return code;
            }
            assert!(Instant::now() < deadline, "Panelink did not detach");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_restored(&self) {
        let restored = tcgetattr(&self.slave).expect("query restored terminal settings");
        assert_eq!(restored.input_modes, self.initial.input_modes);
        assert_eq!(restored.output_modes, self.initial.output_modes);
        assert_eq!(restored.control_modes, self.initial.control_modes);
        assert_eq!(restored.local_modes, self.initial.local_modes);
    }
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
            // SAFETY: this supervisor process group belongs only to this fixture client.
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

#[derive(Default)]
struct Timings {
    first_change: Vec<Duration>,
    settled: Vec<Duration>,
}

fn pointer_event(client: &mut Client, code: u8, column: usize, row: usize, release: bool) {
    let suffix = if release { 'm' } else { 'M' };
    client.write(format!("\x1b[<{code};{};{}{suffix}", column + 1, row + 1).as_bytes());
}

fn wheel_up(client: &mut Client, column: usize, row: usize) {
    pointer_event(client, 64, column, row, false);
}

fn wheel_samples(client: &mut Client, count: usize, column: usize, row: usize) -> Timings {
    let mut timings = Timings::default();
    for _ in 0..count {
        let before = client.pane_screen();
        let started = Instant::now();
        wheel_up(client, column, row);
        let first = client.wait_screen_change(&before, started, "wheel-up");
        let settled = client.wait_pane_stable(started, "wheel-up");
        timings.first_change.push(first);
        timings.settled.push(settled);
    }
    timings
}

fn burst(client: &mut Client, count: usize, column: usize, row: usize) -> Timings {
    let before = client.pane_screen();
    let started = Instant::now();
    let mut events = String::new();
    for _ in 0..count {
        events.push_str(&format!("\x1b[<64;{};{}M", column + 1, row + 1));
    }
    client.write(events.as_bytes());
    let first = client.wait_screen_change(&before, started, "wheel burst");
    let settled = client.wait_pane_stable(started, "wheel burst");
    Timings {
        first_change: vec![first],
        settled: vec![settled],
    }
}

fn thumb_row(client: &Client) -> Option<usize> {
    let screen = client.snapshot();
    screen.lines().enumerate().find_map(|(row, line)| {
        (row > 0
            && row + 1 < usize::from(client.rows)
            && line.chars().nth(usize::from(client.columns - 1)) == Some('#'))
        .then_some(row)
    })
}

fn prime_scrollbar_extent(client: &mut Client) {
    let column = usize::from(client.columns - 1);
    let row = thumb_row(client).expect("scrollbar thumb visible before extent preflight");
    let before = client.pane_screen();
    let started = Instant::now();
    pointer_event(client, 0, column, row, false);
    pointer_event(client, 0, column, row, true);
    let _ = client.try_wait_screen_change(
        &before,
        started,
        Duration::from_secs(1),
        "scrollbar extent preflight",
    );
    let _ = client.wait_pane_stable(started, "scrollbar extent preflight");
    client.wait_pane_quiet(Duration::from_millis(250), "scrollbar extent discovery");
}

fn seek_live(client: &mut Client) {
    client.write(&[0x02]);
    client.wait_screen("NAV before returning live", " NAV ");
    let before = client.pane_screen();
    let started = Instant::now();
    client.write(b"\x1b[F");
    client.write(&[0x1b]);
    let _ = client.try_wait_screen_change(
        &before,
        started,
        Duration::from_millis(500),
        "Ctrl-B End live seek",
    );
    let _ = client.wait_pane_stable(started, "live-history return");
    client.wait_pane_quiet(Duration::from_millis(250), "live shell input");
}

fn drag_thumb(client: &mut Client, prime_extent: bool) -> (bool, bool, Timings) {
    // The preceding burst can have a coalesced follow-up read in flight. Give
    // it a longer quiescence window so this comparison describes the gesture.
    client.wait_pane_quiet(Duration::from_millis(250), "scrollbar press");
    if prime_extent {
        // A first thumb press may perform the bounded extent read rather than
        // start a drag; complete that read before measuring an exact-range drag.
        prime_scrollbar_extent(client);
    }
    let column = usize::from(client.columns - 1);
    let start_row = thumb_row(client).expect("scrollbar thumb visible in parsed outer-PTY cells");
    let before = client.pane_screen();
    pointer_event(client, 0, column, start_row, false);
    let middle_row = usize::from(client.rows / 2);
    let end_row = usize::from(client.rows - 2);
    pointer_event(client, 32, column, middle_row, false);
    pointer_event(client, 32, column, end_row.saturating_sub(2), false);
    pointer_event(client, 32, column, end_row, false);
    thread::sleep(Duration::from_millis(100));
    let before_release = client.pane_screen();
    let changed_before_release = before_release != before;

    let started = Instant::now();
    pointer_event(client, 0, column, end_row, true);
    let (first, stable) =
        client.observe_after_release(&before_release, started, Duration::from_secs(2));
    let first_change = first.into_iter().collect();
    let settled = vec![stable.unwrap_or(Duration::from_secs(2))];
    (
        changed_before_release,
        stable.is_some(),
        Timings {
            first_change,
            settled,
        },
    )
}

fn milliseconds(durations: &[Duration]) -> Vec<f64> {
    durations
        .iter()
        .map(|duration| duration.as_secs_f64() * 1_000.0)
        .collect()
}

fn percentile(sorted: &[f64], percentile: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (sorted.len() - 1) as f64 * percentile as f64 / 100.0;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let fraction = rank - lower as f64;
    sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
}

fn stats(durations: &[Duration]) -> (f64, f64, f64) {
    let mut values = milliseconds(durations);
    values.sort_by(f64::total_cmp);
    (
        percentile(&values, 50),
        percentile(&values, 95),
        values.last().copied().unwrap_or_default(),
    )
}

fn emit_summary(scenario: &str, rows: u16, columns: u16, timings: &Timings, extra: &str) {
    let (first_p50, first_p95, first_max) = stats(&timings.first_change);
    let (settled_p50, settled_p95, settled_max) = stats(&timings.settled);
    let first_values = milliseconds(&timings.first_change);
    let settled_values = milliseconds(&timings.settled);
    println!(
        "UX01_JSON {{\"scenario\":\"{scenario}\",\"host\":\"{rows}x{columns}\",\"samples\":{},\"first_change_samples\":{},\"first_change_ms\":{{\"p50\":{first_p50:.3},\"p95\":{first_p95:.3},\"max\":{first_max:.3},\"raw\":{first_values:?}}},\"settled_ms\":{{\"p50\":{settled_p50:.3},\"p95\":{settled_p95:.3},\"max\":{settled_max:.3},\"raw\":{settled_values:?}}},\"binary\":\"{}\"{extra}}}",
        timings.settled.len(),
        timings.first_change.len(),
        env!("CARGO_BIN_EXE_panelink")
    );
}

fn wait_shell_prompt(client: &Client) {
    let deadline = Instant::now() + STEP;
    loop {
        let screen = client.snapshot();
        if screen
            .lines()
            .any(|line| line.contains("UX01$ ") && !line.contains("OUT_"))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "shell prompt not visible:\n{screen}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_file(path: &Path, description: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_geometry(rows: u16, columns: u16) {
    let fixture = Fixture::new();
    let mut client = Client::spawn(&fixture, rows, columns);
    client.wait_raw_mode();
    client.type_shell("PS1='UX01$ '; printf 'UX01_READY\\n'");
    client.wait_screen("initial shell readiness", "UX01_READY");
    wait_shell_prompt(&client);
    client.type_shell(
        "i=1; while [ $i -le 4000 ]; do printf 'HIST_%04d\\n' $i; i=$((i + 1)); done; printf 'HIST_DONE\\n'",
    );
    client.wait_screen("synthetic 4000-line history", "HIST_DONE");
    wait_shell_prompt(&client);

    let column = usize::from(columns / 2);
    let row = usize::from(rows / 2);
    let idle_wheel = wheel_samples(&mut client, WHEEL_SAMPLES, column, row);
    emit_summary("wheel_idle", rows, columns, &idle_wheel, "");

    let idle_burst = burst(&mut client, BURST_EVENTS, column, row);
    emit_summary("wheel_burst_idle", rows, columns, &idle_burst, "");

    let (idle_drag_changed, idle_drag_stable, idle_drag) = drag_thumb(&mut client, true);
    emit_summary(
        "thumb_drag_idle",
        rows,
        columns,
        &idle_drag,
        &format!(
            ",\"content_changed_before_release\":{idle_drag_changed},\"stable_after_release\":{idle_drag_stable}"
        ),
    );

    seek_live(&mut client);
    let output_done = fixture.root.join("synthetic-output.done");
    client.type_shell(&format!(
        "i=0; while [ $i -lt {OUTPUT_LINES} ]; do printf 'OUT_%04d\\n' $i; sleep 0.01; i=$((i + 1)); done; : > '{}'; printf 'OUT_DONE\\n'",
        output_done.display()
    ));
    client.wait_screen("sustained output started", "OUT_0000");
    let output_wheel = wheel_samples(&mut client, WHEEL_SAMPLES, column, row);
    emit_summary("wheel_sustained_output", rows, columns, &output_wheel, "");

    let output_burst = burst(&mut client, BURST_EVENTS, column, row);
    emit_summary(
        "wheel_burst_sustained_output",
        rows,
        columns,
        &output_burst,
        "",
    );

    let (output_drag_changed, output_drag_stable, output_drag) = drag_thumb(&mut client, false);
    emit_summary(
        "thumb_drag_sustained_output",
        rows,
        columns,
        &output_drag,
        &format!(
            ",\"content_changed_before_release\":{output_drag_changed},\"stable_after_release\":{output_drag_stable}"
        ),
    );
    let reading_position = client.pane_screen();
    thread::sleep(Duration::from_millis(250));
    let reading_position_drifted = client.pane_screen() != reading_position;
    println!(
        "UX01_JSON {{\"scenario\":\"reading_position_drift\",\"host\":\"{rows}x{columns}\",\"samples\":1,\"interval_ms\":250,\"drifted\":{reading_position_drifted},\"binary\":\"{}\"}}",
        env!("CARGO_BIN_EXE_panelink")
    );
    wait_file(&output_done, "synthetic output completion marker");
    client.type_shell("printf 'OUT_BARRIER\\n'");
    client.wait_screen("output shell barrier", "OUT_BARRIER");

    // Create a second pane using the supported prefix+r split. Put history in
    // the right pane, then focus left before scrolling the unfocused right pane.
    client.write(&[0x02, b'r']);
    wait_vertical_split(&client);
    client.type_shell(
        "i=1; while [ $i -le 1500 ]; do printf 'FOCUS_HIST_%04d\\n' $i; i=$((i + 1)); done; printf 'FOCUS_HIST_DONE\\n'",
    );
    client.wait_screen("second pane history", "FOCUS_HIST_DONE");
    client.type_shell("printf 'FOCUS_HIST_BARRIER\\n'");
    client.wait_screen("second pane prompt barrier", "FOCUS_HIST_BARRIER");
    focus_left(&mut client, "FOCUS_LEFT_PRE_WHEEL");
    assert!(
        marker_in_left_pane(&client.snapshot(), rows, columns, "FOCUS_LEFT_PRE_WHEEL"),
        "focus setup marker should be in the left pane"
    );
    let focus_before = client.pane_screen();
    let right_column = usize::from(columns * 3 / 4);
    wheel_up(&mut client, right_column, row);
    let focus_wheel_changed =
        client.wait_screen_change(&focus_before, Instant::now(), "unfocused-pane wheel");
    let _ = focus_wheel_changed;
    client.type_shell("printf 'FOCUS_LEFT_OK\\n'");
    client.wait_screen(
        "focus remains on left after right-pane wheel",
        "FOCUS_LEFT_OK",
    );
    let wheel_focus_left = marker_in_left_pane(&client.snapshot(), rows, columns, "FOCUS_LEFT_OK");

    focus_left(&mut client, "FOCUS_LEFT_PRE_DRAG_START");
    let before_drag_wheel = client.pane_screen();
    let drag_wheel_started = Instant::now();
    wheel_up(&mut client, right_column, row);
    let _ = client.wait_screen_change(
        &before_drag_wheel,
        drag_wheel_started,
        "right pane history preparation before unfocused thumb drag",
    );
    focus_left(&mut client, "FOCUS_LEFT_PRE_DRAG");
    assert!(
        marker_in_left_pane(&client.snapshot(), rows, columns, "FOCUS_LEFT_PRE_DRAG"),
        "focus setup marker should be in the left pane before scrollbar drag"
    );
    let current_thumb = thumb_row(&client).expect("unfocused right pane thumb remains rendered");
    pointer_event(
        &mut client,
        0,
        usize::from(columns - 1),
        current_thumb,
        false,
    );
    pointer_event(
        &mut client,
        32,
        usize::from(columns - 1),
        usize::from(rows / 2),
        false,
    );
    pointer_event(
        &mut client,
        32,
        usize::from(columns - 1),
        usize::from(rows - 2),
        false,
    );
    pointer_event(
        &mut client,
        0,
        usize::from(columns - 1),
        usize::from(rows - 2),
        true,
    );
    client.type_shell("printf 'FOCUS_LEFT_DRAG_OK\\n'");
    client.wait_screen(
        "focus remains on left after right-pane thumb drag",
        "FOCUS_LEFT_DRAG_OK",
    );
    let drag_focus_left =
        marker_in_left_pane(&client.snapshot(), rows, columns, "FOCUS_LEFT_DRAG_OK");
    println!(
        "UX01_JSON {{\"scenario\":\"unfocused_pane_focus\",\"host\":\"{rows}x{columns}\",\"samples\":2,\"wheel_kept_focus\":{wheel_focus_left},\"thumb_kept_focus\":{drag_focus_left},\"binary\":\"{}\"}}",
        env!("CARGO_BIN_EXE_panelink")
    );

    client.write(&[0x18]);
    assert_eq!(client.wait_exit(STEP), 0, "Ctrl-X detach should succeed");
    client.assert_restored();
    drop(client);
    assert!(
        fixture.stop(),
        "private workspace server must terminate before cleanup"
    );
    assert!(
        !server_lock_held(&fixture.root.join("server")),
        "private workspace lock still held after stop"
    );
    println!("UX01_CLEANUP {{\"host\":\"{rows}x{columns}\",\"client_detached\":true,\"server_lock_released\":true}}");
}

fn wait_vertical_split(client: &Client) {
    let deadline = Instant::now() + STEP;
    loop {
        let screen = client.snapshot();
        let lines = screen.lines().collect::<Vec<_>>();
        if divider_column(&lines, client.rows, client.columns).is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "vertical split not visible:\n{screen}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn divider_column(lines: &[&str], rows: u16, columns: u16) -> Option<usize> {
    (1..usize::from(columns.saturating_sub(1)))
        .map(|column| {
            let count = (1..usize::from(rows.saturating_sub(1)))
                .filter(|&row| {
                    lines.get(row).and_then(|line| line.chars().nth(column)) == Some('|')
                })
                .count();
            (column, count)
        })
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count >= usize::from(rows.saturating_sub(1)) / 2)
        .map(|(column, _)| column)
}

fn marker_in_left_pane(screen: &str, rows: u16, columns: u16, marker: &str) -> bool {
    let lines = screen.lines().collect::<Vec<_>>();
    let Some(divider) = divider_column(&lines, rows, columns) else {
        return false;
    };
    screen
        .lines()
        .skip(1)
        .take(usize::from(rows.saturating_sub(2)))
        .any(|line| {
            line.chars()
                .skip(1)
                .take(divider.saturating_sub(2))
                .collect::<String>()
                .contains(marker)
        })
}

fn focus_left(client: &mut Client, marker: &str) {
    client.write(&[0x02]);
    client.wait_screen("NAV before focusing left pane", " NAV ");
    client.write(b"\x1b[D");
    client.write(&[0x1b]);
    client.type_shell(&format!("printf '{marker}\\n'"));
    client.wait_screen("left-pane focus marker", marker);
}

#[test]
#[ignore = "synthetic private-PTY latency baseline; run explicitly with --ignored --nocapture"]
fn ux01_private_history_latency_baseline() {
    for (rows, columns) in [(24, 100), (40, 120)] {
        run_geometry(rows, columns);
    }
}
