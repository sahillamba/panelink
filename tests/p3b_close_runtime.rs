#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! End-to-end proof of explicit pane close through the real binary: the
//! schema-9 runtime route, `panelink panes`, `panelink close N`, the
//! `control panes|close` actions, auto-collapse when one shell exits, the last
//! close ending the workspace, and Ctrl-B x y in the real attach client.
//!
//! Every scenario runs in its own private `PANELINK_HOME` under /private/tmp
//! (the pattern of `tests/p3a_default_open.rs`, whose fixture and outer-PTY
//! client helpers are copied here); the user's real workspace is never
//! touched.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, ScreenBuffer};
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::{tcgetattr, LocalModes, Winsize};
use rustix_openpty::openpty;

const STEP: Duration = Duration::from_secs(10);
const ROWS: u16 = 24;
const COLUMNS: u16 = 100;
const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-p3b-close-{}-{}",
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
            state: root.join("state"),
            home: root.join("home"),
            root,
        };
        make(&fixture.home);
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", "/bin/zsh")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn server_dir(&self) -> PathBuf {
        self.state.join("server")
    }

    fn socket(&self) -> PathBuf {
        self.server_dir().join("control.sock")
    }

    fn control(&self, context: &str, args: &[&str]) -> (i32, String) {
        let output = self
            .command()
            .arg("control")
            .arg("--directory")
            .arg(self.server_dir())
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

    /// Run `panelink ARGS`; returns (exit code, stdout, stderr).
    fn panelink(&self, args: &[&str]) -> (i32, String, String) {
        let output = self.command().args(args).output().expect("run panelink");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn stop(&self) -> (i32, String) {
        let output = self.command().arg("stop").output().expect("panelink stop");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn server_pid(&self) -> Option<i32> {
        fs::read_to_string(self.state.join("server.pid"))
            .ok()?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    fn server_alive(&self) -> bool {
        // SAFETY: signal 0 only checks existence of our own child server.
        self.server_pid()
            .is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0)
    }

    fn wait_server_gone(&self) {
        let deadline = Instant::now() + STEP;
        while self.socket().exists() || self.server_alive() {
            assert!(Instant::now() < deadline, "server did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop();
        if let Some(pid) = self.server_pid() {
            // SAFETY: best-effort cleanup of this test's own server.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
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

/// Field `name` of the `index`-th (0-based) pane object in a panes JSON.
fn pane_field<'a>(json: &'a str, index: usize, name: &str) -> Option<&'a str> {
    let start = json.find(&format!("{{\"index\":{}", index + 1))?;
    field(&json[start..], name)
}

fn target_ref(value: &str) -> TargetRef {
    let fields: Vec<u64> = value
        .split(':')
        .map(|field| field.parse().unwrap())
        .collect();
    assert_eq!(fields.len(), 5);
    TargetRef {
        generation: fields[0],
        workspace: fields[1],
        pane: fields[2],
        session: fields[3],
        occupant_revision: fields[4],
    }
}

fn captured_screen(fixture: &Fixture, target: &str, buffer: ScreenBuffer) -> Vec<u8> {
    let store = HistoryStore::open(&fixture.server_dir().join("history")).unwrap();
    let deadline = Instant::now() + STEP;
    loop {
        if let Some(bytes) = store.load(target_ref(target), buffer).unwrap() {
            return bytes;
        }
        assert!(Instant::now() < deadline, "checkpoint not saved");
        thread::sleep(Duration::from_millis(20));
    }
}

fn checkpoint_text(bytes: &[u8]) -> String {
    screen_checkpoint::decode(bytes)
        .unwrap()
        .cells()
        .map(|cell| cell.text().to_owned())
        .collect()
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

/// The attach client on a private outer PTY with a continuous drain thread
/// (copied from `tests/p3a_default_open.rs`).
struct Client {
    child: Child,
    status_path: PathBuf,
    release_path: PathBuf,
    writer: fs::File,
    slave: std::os::fd::OwnedFd,
    capture: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
}

impl Client {
    fn spawn(fixture: &Fixture) -> Self {
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
        let controlling = slave.as_fd().try_clone_to_owned().unwrap();
        let clone = || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
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
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &fixture.home)
            .env("ZDOTDIR", &fixture.home)
            .env("SHELL", "/bin/zsh")
            .env("PANELINK_HOME", &fixture.state)
            .env("PS1", "P3BC$ ")
            .current_dir(&fixture.home)
            .stdin(clone())
            .stdout(clone())
            .stderr(clone());
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
            capture,
            stop,
            drain: Some(drain),
        }
    }

    fn attach_exit(&mut self) -> Option<i32> {
        let text = fs::read_to_string(&self.status_path).ok()?;
        text.strip_suffix('\n')?.parse().ok()
    }

    fn wait_attach_exit(&mut self) -> i32 {
        let deadline = Instant::now() + STEP;
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

fn open_client(fixture: &Fixture) -> Client {
    let mut client = Client::spawn(fixture);
    client.wait_raw_mode();
    client.wait_screen("shell prompt", |screen| {
        screen.contains("%") || screen.contains("$")
    });
    client
}

fn detach(client: &mut Client) {
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0);
}

/// `control panes` JSON once it reports `count` panes.
fn wait_pane_count(fixture: &Fixture, context: &str, count: usize) -> String {
    wait_control(&format!("{count} pane(s)"), || {
        let (code, panes) = fixture.control(context, &["panes"]);
        (code == 0 && field(&panes, "pane_count") == Some(count.to_string().as_str()))
            .then_some(panes)
    })
}

/// Split pane 1 to the right through `control split`; returns the new target.
fn control_split(fixture: &Fixture, context: &str) -> String {
    let (code, panes) = fixture.control(context, &["panes"]);
    assert_eq!(code, 0, "panes: {panes}");
    let target = pane_field(&panes, 0, "target").unwrap().to_owned();
    let revision = field(&panes, "layout_revision").unwrap().to_owned();
    let (code, split) = fixture.control(context, &["split", &target, &revision, "right"]);
    assert_eq!(code, 0, "split: {split}");
    assert_eq!(field(&split, "outcome"), Some("applied"), "{split}");
    field(&split, "new_target").unwrap().to_owned()
}

/// Pin `context` to `target`, claim its input and type `text` plus Enter.
fn claim_and_type(fixture: &Fixture, context: &str, target: &str, text: &str) {
    let (code, init) = fixture.control(context, &["init", target]);
    assert_eq!(code, 0, "init {target}: {init}");
    let (_, described) = fixture.control(context, &["describe"]);
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    let (code, claimed) = fixture.control(context, &["claim", target, &revision]);
    assert_eq!(code, 0, "claim: {claimed}");
    let (_, described) = fixture.control(context, &["describe"]);
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    // Let zsh reach its prompt so the line is read by the shell.
    thread::sleep(Duration::from_millis(500));
    let (code, typed) = fixture.control(context, &["keys", target, &revision, text]);
    assert_eq!(code, 0, "keys: {typed}");
    assert_eq!(field(&typed, "outcome"), Some("applied"), "{typed}");
}

/// The single remaining pane fills the whole root.
fn assert_single_full_size(panes: &str) {
    assert_eq!(field(panes, "pane_count"), Some("1"), "{panes}");
    assert_eq!(pane_field(panes, 0, "x"), Some("0"), "{panes}");
    assert_eq!(pane_field(panes, 0, "y"), Some("0"), "{panes}");
    assert_eq!(
        pane_field(panes, 0, "rows"),
        field(panes, "rows"),
        "{panes}"
    );
    assert_eq!(
        pane_field(panes, 0, "columns"),
        field(panes, "columns"),
        "{panes}"
    );
    assert_eq!(pane_field(panes, 0, "live"), Some("true"), "{panes}");
}

#[test]
fn panes_and_close_commands_collapse_layout_and_end_workspace() {
    let fixture = Fixture::new();

    // Without a workspace both commands fail with a fixed message.
    for args in [&["panes"][..], &["close", "1"][..]] {
        let (code, stdout, stderr) = fixture.panelink(args);
        assert_eq!(code, 1, "{args:?}: {stdout}{stderr}");
        assert!(stderr.contains("no workspace is running"), "{stderr}");
    }
    // Usage errors exit 2 and never contact a server.
    for args in [
        &["close"][..],
        &["close", "0"][..],
        &["close", "x"][..],
        &["close", "1", "2"][..],
        &["panes", "extra"][..],
    ] {
        let (code, _, _) = fixture.panelink(args);
        assert_eq!(code, 2, "{args:?}");
    }

    // Start the default workspace, then detach so only the server remains.
    let mut client = open_client(&fixture);
    detach(&mut client);
    drop(client);
    assert!(fixture.server_alive(), "server survives detach");

    let (code, init) = fixture.control("probe.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    let second = control_split(&fixture, "probe.ctx");
    let second_screen = captured_screen(&fixture, &second, ScreenBuffer::Normal);
    assert_eq!(
        screen_checkpoint::decode(&second_screen)
            .unwrap()
            .metadata()
            .target(),
        target_ref(&second)
    );

    let (code, listed, stderr) = fixture.panelink(&["panes"]);
    assert_eq!(code, 0, "panes: {listed}{stderr}");
    assert!(listed.starts_with("2 panes (layout revision "), "{listed}");
    assert!(listed.contains("\n  1  running "), "{listed}");
    assert!(listed.contains("\n  2  running "), "{listed}");
    assert!(listed.contains(&format!("(target {second})")), "{listed}");
    let (code, _, stderr) = fixture.panelink(&["close", "3"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("no pane 3"), "{stderr}");

    // Explicit close of pane 2: HUP stops zsh, the sibling fills the root.
    let started = Instant::now();
    let (code, stdout, stderr) = fixture.panelink(&["close", "2"]);
    assert_eq!(code, 0, "close 2: {stdout}{stderr}");
    assert!(
        stdout.contains("closed pane 2 (exited)")
            || stdout.contains("closed pane 2 (escalated: SIGKILL)"),
        "{stdout}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let (code, panes) = fixture.control("probe.ctx", &["panes"]);
    assert_eq!(code, 0, "{panes}");
    assert_single_full_size(&panes);
    assert!(fixture.server_alive(), "server survives closing one pane");
    assert!(
        screen_checkpoint::decode(&captured_screen(&fixture, &second, ScreenBuffer::Normal))
            .is_ok(),
        "closed pane's history remains readable"
    );
    let (code, listed, _) = fixture.panelink(&["panes"]);
    assert_eq!(code, 0);
    assert!(listed.starts_with("1 pane (layout revision "), "{listed}");

    // A stale revision is rejected without stopping anything.
    let (code, panes) = fixture.control("probe.ctx", &["panes"]);
    assert_eq!(code, 0);
    let first = pane_field(&panes, 0, "target").unwrap().to_owned();
    let first_screen = captured_screen(&fixture, &first, ScreenBuffer::Normal);
    drop(HistoryStore::open(&fixture.server_dir().join("history")).unwrap());
    assert_eq!(
        HistoryStore::open(&fixture.server_dir().join("history"))
            .unwrap()
            .load(target_ref(&first), ScreenBuffer::Normal)
            .unwrap(),
        Some(first_screen)
    );
    let (code, rejected) = fixture.control("probe.ctx", &["close", &first, "1"]);
    assert_eq!(code, 1, "{rejected}");
    assert_eq!(field(&rejected, "outcome"), Some("rejected"), "{rejected}");
    assert_eq!(
        field(&rejected, "reason"),
        Some("stale_layout"),
        "{rejected}"
    );
    let sequence = field(&rejected, "sequence").unwrap().to_owned();
    let (code, status) = fixture.control("probe.ctx", &["close-status", &sequence]);
    assert_eq!(code, 1, "{status}");
    assert_eq!(field(&status, "reason"), Some("stale_layout"), "{status}");
    // The closed pane's identity is reported as already closed.
    let (_, panes) = fixture.control("probe.ctx", &["panes"]);
    let revision = field(&panes, "layout_revision").unwrap().to_owned();
    let (code, again) = fixture.control("probe.ctx", &["close", &second, &revision]);
    assert_eq!(code, 1, "{again}");
    assert_eq!(field(&again, "reason"), Some("already_closed"), "{again}");
    assert!(fixture.server_alive());

    // Escalation: a shell that ignores HUP is KILLed after the 1 s window,
    // inside the server tick, and the reply still arrives.
    let third = control_split(&fixture, "probe.ctx");
    assert_ne!(third, second, "identities are never reused");
    claim_and_type(&fixture, "third.ctx", &third, "trap '' HUP");
    thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let (code, stdout, stderr) = fixture.panelink(&["close", "2"]);
    let elapsed = started.elapsed();
    assert_eq!(code, 0, "close escalated: {stdout}{stderr}");
    assert!(
        stdout.contains("closed pane 2 (escalated: SIGKILL)"),
        "{stdout}"
    );
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
    assert_single_full_size(&wait_pane_count(&fixture, "probe.ctx", 1));
    assert!(fixture.server_alive());

    // Auto-collapse: re-split, then `exit` in pane 2 through typed keys.
    let fourth = control_split(&fixture, "probe.ctx");
    assert!(fourth != second && fourth != third, "fresh identity");
    claim_and_type(&fixture, "fourth.ctx", &fourth, "exit");
    let panes = wait_pane_count(&fixture, "probe.ctx", 1);
    assert_single_full_size(&panes);
    assert!(
        screen_checkpoint::decode(&captured_screen(&fixture, &fourth, ScreenBuffer::Normal))
            .is_ok(),
        "natural exit retains its checkpoint"
    );
    assert!(fixture.server_alive(), "server survives one shell exit");

    // Closing the last pane ends the workspace.
    let (code, stdout, stderr) = fixture.panelink(&["close", "1"]);
    assert_eq!(code, 0, "close 1: {stdout}{stderr}");
    assert!(
        stdout.contains("closed pane 1 (") && stdout.contains("the workspace has ended"),
        "{stdout}"
    );
    fixture.wait_server_gone();
    // A fresh reader after server teardown sees the committed checkpoint of
    // the last pane; reading does not start a child or interpret terminal IO.
    let committed = HistoryStore::open(&fixture.server_dir().join("history"))
        .unwrap()
        .load(target_ref(&first), ScreenBuffer::Normal)
        .unwrap()
        .unwrap();
    assert_eq!(
        screen_checkpoint::decode(&committed)
            .unwrap()
            .metadata()
            .target(),
        target_ref(&first)
    );
    let (code, _, stderr) = fixture.panelink(&["panes"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("no workspace is running"), "{stderr}");
}

#[test]
fn ctrl_b_x_y_in_the_attach_client_closes_the_focused_pane() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);

    // Ctrl-B r splits right; the new right pane is focused.
    client.write(b"\x02r");
    let (code, init) = fixture.control("ui.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    wait_pane_count(&fixture, "ui.ctx", 2);
    thread::sleep(Duration::from_millis(400));

    // Ctrl-B x asks; y confirms the close of pane 2.
    client.write(b"\x02x");
    client.wait_screen("close prompt", |screen| screen.contains("Close pane 2?"));
    client.write(b"y");
    let panes = wait_pane_count(&fixture, "ui.ctx", 1);
    assert_single_full_size(&panes);
    client.wait_screen("closed message", |screen| screen.contains("pane closed"));
    assert!(client.attach_exit().is_none(), "client stays attached");
    assert!(fixture.server_alive(), "workspace keeps running");

    // The survivor takes typing across the full width.
    client.type_slowly(b"echo LEFT$((2+3))\r");
    client.wait_screen("LEFT5", |screen| screen.contains("LEFT5"));
    let (code, listed, _) = fixture.panelink(&["panes"]);
    assert_eq!(code, 0);
    assert!(listed.starts_with("1 pane "), "{listed}");
    assert!(listed.contains("  1  running "), "{listed}");
    assert!(listed.contains(" at col 0, row 0 "), "{listed}");
    detach(&mut client);
}

#[test]
fn live_capture_and_storage_failure_leave_the_shell_running() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);
    detach(&mut client);
    drop(client);
    let (code, init) = fixture.control("capture.ctx", &["init"]);
    assert_eq!(code, 0, "{init}");
    let (code, panes) = fixture.control("capture.ctx", &["panes"]);
    assert_eq!(code, 0, "{panes}");
    let target = pane_field(&panes, 0, "target").unwrap().to_owned();
    claim_and_type(&fixture, "input.ctx", &target, "echo P5CAPTURE42");
    wait_control("visible screen saved", || {
        let bytes = captured_screen(&fixture, &target, ScreenBuffer::Normal);
        checkpoint_text(&bytes)
            .contains("P5CAPTURE42")
            .then_some("saved".into())
    });
    let (code, detached) = fixture.control("input.ctx", &["detach"]);
    assert_eq!(code, 0, "{detached}");
    let saved = captured_screen(&fixture, &target, ScreenBuffer::Normal);
    assert!(checkpoint_text(&saved).contains("P5CAPTURE42"));

    // An invalid existing destination is never replaced; subsequent output
    // still reaches the live terminal and the server reports capture failure.
    let history = fixture.server_dir().join("history");
    let entry = fs::read_dir(&history)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::OpenOptions::new()
        .append(true)
        .open(&entry)
        .unwrap()
        .write_all(b"partial")
        .unwrap();
    let (_, described) = fixture.control("input.ctx", &["describe"]);
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    let (code, typed) = fixture.control(
        "input.ctx",
        &["keys", &target, &revision, "echo SHELLSURVIVES88"],
    );
    assert_eq!(code, 0, "{typed}");
    assert_eq!(field(&typed, "outcome"), Some("applied"));
    wait_control("capture failure diagnostic", || {
        let log = fs::read_to_string(fixture.state.join("server.log")).ok()?;
        log.contains("screen history capture failed").then_some(log)
    });
    let (code, panes) = fixture.control("input.ctx", &["panes"]);
    assert_eq!(code, 0, "{panes}");
    assert_eq!(pane_field(&panes, 0, "live"), Some("true"));
    assert!(HistoryStore::open(&history)
        .unwrap()
        .load(target_ref(&target), ScreenBuffer::Normal)
        .is_err());
}

#[test]
fn alternate_screen_checkpoint_is_distinct_from_normal_screen() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);
    detach(&mut client);
    drop(client);
    let (code, init) = fixture.control("alternate.ctx", &["init"]);
    assert_eq!(code, 0, "{init}");
    let (code, panes) = fixture.control("alternate.ctx", &["panes"]);
    assert_eq!(code, 0, "{panes}");
    let target = pane_field(&panes, 0, "target").unwrap().to_owned();
    let normal = captured_screen(&fixture, &target, ScreenBuffer::Normal);
    claim_and_type(
        &fixture,
        "alternate-input.ctx",
        &target,
        "printf '\\033[?1049hALT-P5-READY'",
    );
    let alternate = captured_screen(&fixture, &target, ScreenBuffer::Alternate);
    assert!(checkpoint_text(&alternate).contains("ALT-P5-READY"));
    assert_eq!(
        screen_checkpoint::decode(&alternate).unwrap().buffer(),
        ScreenBuffer::Alternate
    );
    assert_eq!(
        screen_checkpoint::decode(&normal).unwrap().buffer(),
        ScreenBuffer::Normal
    );
    let (code, panes) = fixture.control("alternate.ctx", &["panes"]);
    assert_eq!(code, 0, "{panes}");
    assert_eq!(pane_field(&panes, 0, "live"), Some("true"));
}
