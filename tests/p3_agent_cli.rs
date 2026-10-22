#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Default-workspace agent CLI integration coverage. Every process is isolated
//! under a private PANELINK_HOME and uses the real `serve` workspace runtime.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
    terminfo: PathBuf,
    server: Option<Child>,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-agent-cli-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            terminfo: root.join("terminfo"),
            root,
            server: None,
        };
        for path in [&fixture.state, &fixture.home, &fixture.terminfo] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.state.join("server"))
            .unwrap();
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
            .env("PS1", "AGENT$ ")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn command_output(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .output()
            .expect("run panelink CLI")
    }

    fn start(&mut self) -> String {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("compile private terminfo");
        assert!(status.success());
        let mut child = self
            .command()
            .args(["serve", "--directory"])
            .arg(self.state.join("server"))
            .arg("--terminfo")
            .arg(&self.terminfo)
            .args([
                "--initialize",
                "--allow-input",
                "--",
                "/bin/zsh",
                "-f",
                "-i",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start default workspace server");
        let mut stdout = child.stdout.take().unwrap();
        use std::io::Read;
        let mut startup = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut byte = [0_u8; 1];
            match stdout.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => startup.push(byte[0]),
                _ => {
                    assert!(Instant::now() < deadline, "serve did not start");
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        self.server = Some(child);
        String::from_utf8(startup).expect("serve JSON line")
    }

    fn control(&self, context: &str, args: &[&str]) -> Output {
        let path = self.root.join(context);
        self.command()
            .arg("control")
            .arg("--directory")
            .arg(self.state.join("server"))
            .arg("--context")
            .arg(path)
            .args(args)
            .output()
            .expect("run control CLI")
    }

    fn agent(&self, args: &[&str]) -> Output {
        self.command_output(args)
    }

    fn assert_ok(output: &Output, what: &str) -> String {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{what}: stdout={stdout:?} stderr={stderr:?}"
        );
        stdout
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            if server.try_wait().ok().flatten().is_none() {
                let _ = server.kill();
                let _ = server.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("\"{name}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

#[test]
fn agent_cli_list_read_claim_send_wait_events_close_and_stale_indices() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    assert_eq!(field(&startup, "action"), Some("serve"), "{startup}");
    assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
    let target = field(&startup, "target").unwrap().to_owned();

    let initialized = fixture.control("split.ctx", &["init", &target]);
    Fixture::assert_ok(&initialized, "control init");
    let split = fixture.control("split.ctx", &["split", &target, "1", "right"]);
    let split = Fixture::assert_ok(&split, "control split");
    assert_eq!(field(&split, "outcome"), Some("applied"), "{split}");
    let second_target = field(&split, "new_target").unwrap().to_owned();
    let initialized_second = fixture.control("second.ctx", &["init", &second_target]);
    Fixture::assert_ok(&initialized_second, "initialize second pane context");

    let listing = Fixture::assert_ok(&fixture.agent(&["pane", "list", "--json"]), "pane list");
    assert!(listing.contains("\"index\":1"), "{listing}");
    assert!(listing.contains("\"index\":2"), "{listing}");
    assert!(listing.contains("\"target\":"), "{listing}");
    assert!(listing.contains("\"alive\":true"), "{listing}");
    assert!(listing.contains("\"size\":"), "{listing}");
    assert_eq!(field(&listing, "action"), Some("pane_list"));

    let read = Fixture::assert_ok(
        &fixture.agent(&["pane", "read", "1", "--lines", "24", "--json"]),
        "pane read",
    );
    assert_eq!(field(&read, "action"), Some("pane_read"), "{read}");
    assert_eq!(field(&read, "source"), Some("screen"), "{read}");
    assert!(read.contains("\"text\":"), "{read}");

    let refused = fixture.agent(&[
        "pane",
        "send",
        "2",
        "echo MUST_NOT_SEND",
        "--enter",
        "--json",
    ]);
    assert!(!refused.status.success());
    let refused = String::from_utf8_lossy(&refused.stdout);
    assert_eq!(
        field(&refused, "error"),
        Some("input_not_claimed"),
        "{refused}"
    );

    let claimed = Fixture::assert_ok(
        &fixture.agent(&["pane", "claim", "1", "--json"]),
        "pane claim",
    );
    assert_eq!(field(&claimed, "action"), Some("pane_claim"), "{claimed}");
    let sent = Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "send",
            "1",
            "printf 'AGENT_ECHO\\n'",
            "--enter",
            "--json",
        ]),
        "pane send --enter",
    );
    assert_eq!(field(&sent, "action"), Some("pane_send"), "{sent}");
    assert_eq!(field(&sent, "status"), Some("applied"), "{sent}");

    let matched = Fixture::assert_ok(
        &fixture.agent(&[
            "wait",
            "1",
            "--text",
            "AGENT_ECHO",
            "--timeout",
            "5",
            "--json",
        ]),
        "wait text",
    );
    assert_eq!(field(&matched, "matched"), Some("true"), "{matched}");
    assert_eq!(field(&matched, "condition"), Some("text"), "{matched}");
    let timeout = fixture.agent(&[
        "wait",
        "1",
        "--text",
        "NEVER_PRESENT",
        "--timeout",
        "1",
        "--json",
    ]);
    assert_eq!(timeout.status.code(), Some(1));
    let timeout = String::from_utf8_lossy(&timeout.stdout);
    assert_eq!(field(&timeout, "status"), Some("timed_out"), "{timeout}");

    let quiet = Fixture::assert_ok(
        &fixture.agent(&["wait", "1", "--quiet", "1", "--timeout", "4", "--json"]),
        "wait quiet",
    );
    assert_eq!(field(&quiet, "matched"), Some("true"), "{quiet}");

    let waiter = fixture
        .command()
        .args([
            "wait",
            "1",
            "--change",
            "--timeout",
            "5",
            "--json",
            "--as",
            "change-waiter",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn screen-change wait");
    thread::sleep(Duration::from_millis(300));
    let changed = Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "send",
            "1",
            "printf 'CHANGE_EVENT\\n'",
            "--enter",
            "--json",
        ]),
        "send change marker",
    );
    assert_eq!(field(&changed, "status"), Some("applied"), "{changed}");
    let output = waiter.wait_with_output().expect("wait for screen change");
    assert!(output.status.success(), "screen change wait: {output:?}");
    let changed_wait = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        field(&changed_wait, "condition"),
        Some("change"),
        "{changed_wait}"
    );

    let exit_waiter = fixture
        .command()
        .args([
            "wait",
            "2",
            "--exit",
            "--timeout",
            "5",
            "--json",
            "--as",
            "exit-waiter",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn exit wait");
    thread::sleep(Duration::from_millis(300));
    let exited = Fixture::assert_ok(
        &fixture.agent(&[
            "pane", "send", "2", "exit 3", "--enter", "--claim", "--json",
        ]),
        "send exit",
    );
    assert_eq!(field(&exited, "status"), Some("applied"), "{exited}");
    let output = exit_waiter.wait_with_output().expect("wait for child exit");
    assert!(output.status.success(), "exit wait: {output:?}");
    let exit_result = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        field(&exit_result, "condition"),
        Some("exit"),
        "{exit_result}"
    );
    if exit_result.contains("\"exit_status\":{\"code\":3}") {
        assert!(exit_result.contains("exited (code 3)"), "{exit_result}");
        eprintln!("L18 exit-status acceptance exercised observed code 3 path");
    } else {
        assert!(
            exit_result.contains("\"exit_status\":null"),
            "{exit_result}"
        );
        assert!(
            exit_result.contains("exited (status unavailable)"),
            "{exit_result}"
        );
        eprintln!("L18 exit-status acceptance exercised documented pane-collapse fallback");
    }

    let released = Fixture::assert_ok(
        &fixture.agent(&["pane", "release", "1", "--json"]),
        "pane release",
    );
    assert_eq!(
        field(&released, "action"),
        Some("pane_release"),
        "{released}"
    );

    let events = fixture
        .command()
        .args([
            "events",
            "--timeout",
            "5",
            "--json",
            "--as",
            "event-observer",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn event stream");
    thread::sleep(Duration::from_millis(300));
    let closed = Fixture::assert_ok(
        &fixture.agent(&["pane", "close", "1", "--json", "--as", "closer"]),
        "pane close",
    );
    assert_eq!(field(&closed, "action"), Some("pane_close"), "{closed}");
    let event_output = events.wait_with_output().expect("wait for event timeout");
    assert!(event_output.status.success(), "events: {event_output:?}");
    let events = String::from_utf8_lossy(&event_output.stdout);
    assert!(
        events
            .lines()
            .any(|line| line.contains("\"event\":\"pane_closed\"")),
        "{events}"
    );

    let stale = fixture.agent(&["pane", "read", "2", "--json"]);
    assert!(!stale.status.success());
    let stale = String::from_utf8_lossy(&stale.stdout);
    assert_eq!(field(&stale, "error"), Some("no_workspace"), "{stale}");
}

#[test]
fn close_event_and_stale_index_remain_scoped_to_current_layout() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    let first_target = field(&startup, "target").unwrap().to_owned();
    Fixture::assert_ok(
        &fixture.control("split.ctx", &["init", &first_target]),
        "initialize split context",
    );
    let split = Fixture::assert_ok(
        &fixture.control("split.ctx", &["split", &first_target, "1", "right"]),
        "create second pane",
    );
    assert_eq!(field(&split, "outcome"), Some("applied"), "{split}");

    let events = fixture
        .command()
        .args([
            "events",
            "--timeout",
            "5",
            "--json",
            "--as",
            "layout-observer",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn layout event stream");
    thread::sleep(Duration::from_millis(300));
    let closed = Fixture::assert_ok(
        &fixture.agent(&["pane", "close", "2", "--json", "--as", "layout-closer"]),
        "close second pane",
    );
    assert_eq!(field(&closed, "action"), Some("pane_close"), "{closed}");
    let event_output = events.wait_with_output().expect("wait for event timeout");
    assert!(event_output.status.success(), "events: {event_output:?}");
    let lines = String::from_utf8_lossy(&event_output.stdout);
    assert!(
        lines
            .lines()
            .any(|line| line.contains("\"event\":\"pane_closed\"")),
        "{lines}"
    );
    let stale = fixture.agent(&["pane", "read", "2", "--json"]);
    assert!(!stale.status.success());
    let stale = String::from_utf8_lossy(&stale.stdout);
    assert_eq!(field(&stale, "error"), Some("pane_not_found"), "{stale}");
}
