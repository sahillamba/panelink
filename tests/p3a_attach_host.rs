#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Outer-PTY end-to-end proof of `panelink attach` against a real
//! `panelink serve --allow-input` over the pathname socket.
//!
//! The client runs as a session leader on a private outer PTY (like
//! `tests/p2_host.rs`), so its `/dev/tty` is that PTY. The test injects real
//! host bytes (keys, an SGR right-click, Ctrl-B s, Esc, Ctrl-X) and checks
//! both the rendered client output (through a vt100 parser) and the server
//! state (through the separate `panelink control` CLI).

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
            "/private/tmp/pl-p3a-attach-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let make = |path: &Path| {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .expect("create private fixture directory")
        };
        make(&root);
        let fixture = Self {
            runtime: root.join("runtime"),
            terminfo: root.join("terminfo"),
            home: root.join("home"),
            root,
        };
        make(&fixture.runtime);
        make(&fixture.terminfo);
        make(&fixture.home);
        fixture
    }

    /// Explicit minimal environment; the split child inherits exactly the
    /// attach client's environment, so it is minimal too.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("PS1", "P3AC$ ")
            .current_dir(&self.home);
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
            .stdin(Stdio::null())
            .output()
            .expect("run control CLI");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect()
}

struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(fixture: &Fixture) -> Option<(Self, String)> {
        Self::start_mode(fixture, true)
    }

    /// `initialize`: false reopens an existing server directory (a new
    /// generation; every earlier lease is unknown to it).
    fn start_mode(fixture: &Fixture, initialize: bool) -> Option<(Self, String)> {
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
        assert!(tic.success());
        let mut child = fixture
            .command()
            .arg("serve")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--terminfo")
            .arg(&fixture.terminfo)
            .args(initialize.then_some("--initialize"))
            .args([
                "--allow-input",
                "--",
                "/bin/zsh",
                "-f",
                "-c",
                "printf 'PANE1-READY\\n'; exec /bin/zsh -f -i",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn panelink serve");
        let lines = line_reader(child.stdout.take().unwrap());
        let server = Self { child, lines };
        let startup = server.lines.recv_timeout(STEP).expect("serve startup line");
        if startup.contains("listener_unavailable") {
            println!("SKIPPED: pathname bind unavailable on this host: {startup}");
            return None;
        }
        assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
        let first = field(&startup, "target").unwrap().to_owned();
        Some((server, first))
    }

    fn sigterm_and_wait(&mut self) -> ExitStatus {
        // SAFETY: signals this test's own unreaped child PID.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        wait_child(&mut self.child, STEP).expect("server stops after SIGTERM")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
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

/// The attach client on a private outer PTY with a continuous drain thread.
struct Client {
    child: Child,
    status_path: PathBuf,
    release_path: PathBuf,
    writer: fs::File,
    slave: std::os::fd::OwnedFd,
    initial: Termios,
    capture: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
}

impl Client {
    fn spawn(fixture: &Fixture) -> Self {
        Self::spawn_with(fixture, None)
    }

    /// `stderr`: optional replacement for the client's stderr (for example
    /// the write end of a full pipe); defaults to the outer PTY.
    fn spawn_with(fixture: &Fixture, stderr: Option<std::os::fd::OwnedFd>) -> Self {
        let winsize = Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open outer PTY");
        let master = fs::File::from(pty.controller);
        let flags = fcntl_getfl(&master).unwrap();
        fcntl_setfl(&master, flags | OFlags::NONBLOCK).unwrap();
        let slave = pty.user;
        let initial = tcgetattr(&slave).expect("save outer termios");
        let controlling = slave.as_fd().try_clone_to_owned().unwrap();
        let clone = || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
        // A /bin/sh session leader keeps the outer PTY's session alive after
        // attach exits (macOS revokes the slave when the leader exits), so the
        // restored termios stays queryable; it writes attach's exit status to
        // a file and waits for `release`. Same pattern as tests/p2_host.rs.
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let status_path = fixture.root.join(format!("attach-{serial}.status"));
        let release_path = fixture.root.join(format!("release-{serial}"));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(SUPERVISOR)
            .arg("panelink-attach-supervisor")
            .arg(&status_path)
            .arg(&release_path)
            .arg(env!("CARGO_BIN_EXE_panelink"))
            .arg("attach")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--context-dir")
            .arg(fixture.root.join("client-contexts"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", &fixture.home)
            .env("PS1", "P3AC$ ")
            .current_dir(&fixture.home)
            .stdin(clone())
            .stdout(clone())
            .stderr(match stderr {
                Some(fd) => Stdio::from(fs::File::from(fd)),
                None => clone(),
            });
        // SAFETY: only reviewed rustix wrappers that start a new session and
        // set the inherited outer PTY as its controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn panelink attach");
        drop(command);
        let capture = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = master.try_clone().unwrap();
        let drain = {
            let capture = Arc::clone(&capture);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(count) => {
                            let mut capture = capture.lock().unwrap();
                            if capture.len() + count <= MAX_CAPTURE {
                                capture.extend_from_slice(&buffer[..count]);
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        Self {
            child,
            status_path,
            release_path,
            writer: master,
            slave,
            initial,
            capture,
            stop,
            drain: Some(drain),
        }
    }

    /// Exit code of `panelink attach` once the supervisor published it.
    fn attach_exit(&mut self) -> Option<i32> {
        let text = fs::read_to_string(&self.status_path).ok()?;
        text.strip_suffix('\n')?.parse().ok()
    }

    fn wait_attach_exit(&mut self) -> i32 {
        self.wait_attach_exit_within(STEP)
    }

    fn wait_attach_exit_within(&mut self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(code) = self.attach_exit() {
                return code;
            }
            assert!(Instant::now() < deadline, "attach did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let deadline = Instant::now() + STEP;
        let mut cursor = 0;
        while cursor < bytes.len() {
            match self.writer.write(&bytes[cursor..]) {
                Ok(count) => cursor += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
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

    fn capture(&self) -> Vec<u8> {
        self.capture.lock().unwrap().clone()
    }

    fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
        parser.process(&self.capture());
        parser.screen().contents()
    }

    fn wait_screen(&mut self, what: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + STEP;
        loop {
            let screen = self.screen();
            if done(&screen) {
                return screen;
            }
            if let Some(status) = self.attach_exit() {
                panic!(
                    "attach exited ({status}) waiting for {what}: {:?}",
                    String::from_utf8_lossy(&self.capture())
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; screen:\n{screen}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_raw_mode(&mut self) {
        let deadline = Instant::now() + STEP;
        loop {
            let termios = tcgetattr(&self.slave).unwrap();
            if !termios.local_modes.contains(LocalModes::ICANON) {
                return;
            }
            if let Some(status) = self.attach_exit() {
                panic!(
                    "attach exited before raw mode ({status}): {:?}",
                    String::from_utf8_lossy(&self.capture())
                );
            }
            assert!(Instant::now() < deadline, "attach never entered raw mode");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn termios_restored(&self) {
        let actual = tcgetattr(&self.slave).unwrap();
        assert_eq!(actual.input_modes, self.initial.input_modes);
        assert_eq!(actual.output_modes, self.initial.output_modes);
        assert_eq!(actual.control_modes, self.initial.control_modes);
        assert_eq!(actual.local_modes, self.initial.local_modes);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = fs::write(&self.release_path, b"release\n");
        if wait_child(&mut self.child, Duration::from_secs(2)).is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
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

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
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

fn read_pane(fixture: &Fixture, context: &str, target: &str) -> String {
    let (code, output) = fixture.control(context, &["read", target]);
    assert_eq!(code, 0, "read {target}: {output}");
    output
}

#[test]
fn attach_types_right_click_splits_keyboard_menu_and_detaches_leaving_children() {
    let fixture = Fixture::new();
    let Some((mut server, first)) = Server::start(&fixture) else {
        return;
    };
    let (code, init) = fixture.control("one.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");

    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("pane 1 READY", |screen| screen.contains("PANE1-READY"));
    let capture = client.capture();
    for mode in [
        &b"\x1b[?1000h"[..],
        b"\x1b[?1006h",
        b"\x1b[?1049h",
        b"\x1b[?2004h",
    ] {
        assert!(
            capture.windows(mode.len()).any(|w| w == mode),
            "{mode:?} enabled"
        );
    }

    // The client resizes the server root to the host and claims pane 1.
    wait_control("root resize and pane-1 claim", || {
        let (_, layout) = fixture.control("one.ctx", &["layout"]);
        let (_, described) = fixture.control("one.ctx", &["describe"]);
        (field(&layout, "columns") == Some("100")
            && field(&described, "owner_lease").is_some_and(|lease| lease != "0"))
        .then_some(layout)
    });

    client.type_slowly(b"echo ACK$((40+1))\r");
    client.wait_screen("ACK41 output", |screen| screen.contains("\nACK41"));
    assert!(read_pane(&fixture, "one.ctx", &first).contains("ACK41"));

    // Right-click inside pane 1 opens the menu; nothing reaches the child.
    client.write(b"\x1b[<2;10;5M\x1b[<2;10;5m");
    client.wait_screen("split menu", |screen| {
        screen.contains("Split Right") && screen.contains("Split Down")
    });
    client.write(b"r");
    let layout = wait_control("two panes", || {
        let (_, layout) = fixture.control("one.ctx", &["layout"]);
        (field(&layout, "pane_count") == Some("2")).then_some(layout)
    });
    let panes = targets(&layout);
    assert_eq!(panes.len(), 2, "{layout}");
    assert_eq!(panes[0], first, "Split Right keeps pane 1 on the left");
    let second = panes[1].clone();
    let (code, init) = fixture.control("two.ctx", &["init", &second]);
    assert_eq!(code, 0, "init pane 2: {init}");
    // The new pane is claimed by the client and its prompt is rendered.
    wait_control("pane-2 claim", || {
        let (_, described) = fixture.control("two.ctx", &["describe"]);
        field(&described, "owner_lease")
            .is_some_and(|lease| lease != "0")
            .then_some(described)
    });
    wait_control("pane-2 prompt", || {
        let text = read_pane(&fixture, "two.ctx", &second);
        (text.contains('%') || text.contains("P3AC$")).then_some(text)
    });
    // Menu letters never reached pane 1.
    let pane_one = read_pane(&fixture, "one.ctx", &first);
    assert!(!pane_one.contains("P3AC$ r"), "{pane_one}");

    // Typing now goes to the newly focused pane 2 only.
    client.type_slowly(b"echo ACK$((40+2))\r");
    wait_control("ACK42 in pane 2", || {
        let text = read_pane(&fixture, "two.ctx", &second);
        (count(&text, "ACK42") >= 1).then_some(text)
    });
    assert!(!read_pane(&fixture, "one.ctx", &first).contains("ACK42"));
    client.wait_screen("ACK42 rendered", |screen| screen.contains("ACK42"));

    // Keyboard fallback: Ctrl-B s opens the menu for the focused pane; Esc
    // closes it and no byte (neither 's' nor ESC) reaches the child.
    thread::sleep(Duration::from_millis(300));
    let before = read_pane(&fixture, "two.ctx", &second);
    client.write(b"\x02s");
    client.wait_screen("keyboard menu", |screen| screen.contains("Pane limit"));
    client.write(b"\x1b");
    client.wait_screen("menu closed", |screen| !screen.contains("Pane limit"));
    thread::sleep(Duration::from_millis(300));
    let after = read_pane(&fixture, "two.ctx", &second);
    assert_eq!(
        field(&before, "text"),
        field(&after, "text"),
        "menu input leaked into pane 2"
    );

    // Ctrl-X detaches: exit 0, host modes restored, children still alive.
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0, "attach exits 0 on Ctrl-X");
    thread::sleep(Duration::from_millis(100));
    let capture = client.capture();
    for mode in [
        &b"\x1b[?1006l"[..],
        b"\x1b[?1000l",
        b"\x1b[?1049l",
        b"\x1b[?2004l",
    ] {
        assert!(
            capture.windows(mode.len()).any(|w| w == mode),
            "{mode:?} disabled"
        );
    }
    client.termios_restored();
    let (code, layout) = fixture.control("one.ctx", &["layout"]);
    assert_eq!(code, 0, "{layout}");
    assert_eq!(field(&layout, "pane_count"), Some("2"));
    // Both shells still answer input from another client after detach.
    let (_, described) = fixture.control("two.ctx", &["describe"]);
    assert_eq!(field(&described, "child_exited"), Some("false"));
    let (_, described) = fixture.control("one.ctx", &["describe"]);
    assert_eq!(field(&described, "child_exited"), Some("false"));
    drop(client);

    // Re-attach resumes both durable contexts, re-claims and shows both panes.
    let mut again = Client::spawn(&fixture);
    again.wait_raw_mode();
    again.wait_screen("both panes after re-attach", |screen| {
        screen.contains("ACK41") && screen.contains("ACK42")
    });
    again.write(b"\x18");
    assert_eq!(again.wait_attach_exit(), 0);
    drop(again);

    let status = server.sigterm_and_wait();
    assert_eq!(status.code(), Some(0));
    assert!(!fixture.runtime.join("control.sock").exists());
}

fn pipe() -> (std::os::fd::OwnedFd, std::os::fd::OwnedFd) {
    use std::os::fd::FromRawFd;
    let mut fds = [0_i32; 2];
    // SAFETY: pipe writes two new descriptors into the valid array.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: both descriptors were just created and are owned here.
    unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    }
}

fn signal(child: &Child, signal: i32) {
    // SAFETY: signals this test's own unreaped child PID.
    unsafe {
        libc::kill(child.id() as i32, signal);
    }
}

/// Time from the first byte of `pattern` appearing after `from` bytes of
/// capture, polled every 5 ms up to `limit`.
fn wait_bytes(client: &Client, from: usize, pattern: &[u8], limit: Duration) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        let capture = client.capture();
        if capture[from.min(capture.len())..]
            .windows(pattern.len())
            .any(|window| window == pattern)
        {
            return Some(started.elapsed());
        }
        thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
fn ctrl_x_restores_the_terminal_promptly_when_the_server_is_stalled() {
    let fixture = Fixture::new();
    let Some((mut server, _)) = Server::start(&fixture) else {
        return;
    };
    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("pane 1 READY", |screen| screen.contains("PANE1-READY"));
    // Freeze the server: every later exchange stalls (no reply, no EOF).
    signal(&server.child, libc::SIGSTOP);
    thread::sleep(Duration::from_millis(200));
    let from = client.capture().len();
    let pressed = Instant::now();
    client.write(b"\x18");
    let restored = wait_bytes(&client, from, b"\x1b[?1000l", Duration::from_secs(3))
        .expect("restore sequence written");
    assert!(
        restored <= Duration::from_secs(1),
        "terminal restored after {restored:?}"
    );
    // Termios is restored together with the restore bytes.
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let termios = tcgetattr(&client.slave).unwrap();
        if termios.local_modes.contains(LocalModes::ICANON) {
            break;
        }
        assert!(Instant::now() < deadline, "termios not restored");
        thread::sleep(Duration::from_millis(5));
    }
    client.termios_restored();
    // Bounded detach work against the stalled server (≤ 1.5 s budget, each
    // exchange ≤ 0.5 s), then exit.
    assert_eq!(client.wait_attach_exit_within(Duration::from_secs(4)), 0);
    assert!(
        pressed.elapsed() < Duration::from_secs(4),
        "exit took {:?}",
        pressed.elapsed()
    );
    drop(client);
    signal(&server.child, libc::SIGCONT);
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
}

#[test]
fn a_full_stderr_pipe_never_blocks_errors_or_exit() {
    let fixture = Fixture::new();
    // No server is running: attach fails before taking over the terminal and
    // must report without writing to (blocked) stderr.
    let (read_end, write_end) = pipe();
    let flags = fcntl_getfl(&write_end).unwrap();
    fcntl_setfl(&write_end, flags | OFlags::NONBLOCK).unwrap();
    let mut filler = fs::File::from(write_end.try_clone().unwrap());
    let chunk = [b'x'; 4096];
    while filler.write(&chunk).is_ok() {}
    // Restore blocking mode so a blocking stderr write would hang forever.
    fcntl_setfl(&write_end, flags).unwrap();
    drop(filler);
    let started = Instant::now();
    let mut client = Client::spawn_with(&fixture, Some(write_end));
    assert_eq!(client.wait_attach_exit_within(Duration::from_secs(5)), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
    // The diagnostic went to the terminal, not to stderr.
    assert!(wait_bytes(
        &client,
        0,
        b"error: server unavailable",
        Duration::from_secs(1)
    )
    .is_some());
    drop(read_end);

    // Same full pipe while attached to a live server: a normal session and
    // detach are unaffected.
    let Some((mut server, _)) = Server::start(&fixture) else {
        return;
    };
    let (read_end, write_end) = pipe();
    let flags = fcntl_getfl(&write_end).unwrap();
    fcntl_setfl(&write_end, flags | OFlags::NONBLOCK).unwrap();
    let mut filler = fs::File::from(write_end.try_clone().unwrap());
    while filler.write(&chunk).is_ok() {}
    fcntl_setfl(&write_end, flags).unwrap();
    drop(filler);
    let mut client = Client::spawn_with(&fixture, Some(write_end));
    client.wait_raw_mode();
    client.wait_screen("pane 1 READY", |screen| screen.contains("PANE1-READY"));
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit_within(Duration::from_secs(5)), 0);
    client.termios_restored();
    drop(client);
    drop(read_end);
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
}

#[test]
fn seventy_sequential_attach_detach_cycles_do_not_exhaust_leases() {
    let fixture = Fixture::new();
    let Some((mut server, first)) = Server::start(&fixture) else {
        return;
    };
    for cycle in 0..70 {
        let mut client = Client::spawn(&fixture);
        client.wait_raw_mode();
        client.wait_screen("pane 1 READY", |screen| screen.contains("PANE1-READY"));
        client.write(b"\x18");
        assert_eq!(client.wait_attach_exit(), 0, "cycle {cycle}");
        drop(client);
    }
    // Exactly one context file: every cycle resumed the same lease.
    let contexts: Vec<PathBuf> = fs::read_dir(fixture.root.join("client-contexts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(contexts.len(), 1);
    // Detach receipts were confirmed: no completed result stays retained.
    assert_all_results_retired(&fixture, &contexts[0]);
    // A fresh control client can still obtain a lease and read pane 1.
    let (code, init) = fixture.control("after.ctx", &["init"]);
    assert_eq!(code, 0, "lease capacity remains after 70 cycles: {init}");
    let text = read_pane(&fixture, "after.ctx", &first);
    assert!(text.contains("PANE1-READY"));
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
}

/// Every sequence the attach client used on `context` must be retired on
/// the server (status refused), proving explicit receipts were confirmed.
fn assert_all_results_retired(fixture: &Fixture, context: &Path) {
    let output = fixture
        .command()
        .arg("control")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--context")
        .arg(context)
        .arg("show")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(field(&shown, "uncertain"), Some("null"), "{shown}");
    let next: u64 = field(&shown, "next_sequence").unwrap().parse().unwrap();
    assert!(next > 1, "the client used keyed operations: {shown}");
    for sequence in 1..next {
        let output = fixture
            .command()
            .arg("control")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--context")
            .arg(context)
            .args(["status", &sequence.to_string()])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            !text.contains("\"status\":\"completed\""),
            "sequence {sequence} still retained: {text}"
        );
    }
}

#[test]
fn reattach_after_server_restart_replaces_dead_contexts() {
    let fixture = Fixture::new();
    let Some((mut server, _)) = Server::start(&fixture) else {
        return;
    };
    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("pane 1 READY", |screen| screen.contains("PANE1-READY"));
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0);
    drop(client);
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
    drop(server);

    // New generation: the saved lease is now unknown to the server.
    let Some((mut server, first)) = Server::start_mode(&fixture, false) else {
        return;
    };
    let mut client = Client::spawn(&fixture);
    client.wait_raw_mode();
    client.wait_screen("pane 1 READY after restart", |screen| {
        screen.contains("PANE1-READY")
    });
    client.type_slowly(b"echo RE$((40+3))\r");
    client.wait_screen("typing after restart", |screen| screen.contains("\nRE43"));
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0);
    drop(client);
    // The dead context was removed; only the new generation's context stays.
    let names: Vec<String> = fs::read_dir(fixture.root.join("client-contexts"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    let generation = first.split(':').next().unwrap();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with(&format!("pane-{generation}-")),
        "{names:?}"
    );
    assert_eq!(server.sigterm_and_wait().code(), Some(0));
}
