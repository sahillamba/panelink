#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Default-workspace agent CLI integration coverage. Every process is isolated
//! under a private PANELINK_HOME and uses the real `serve` workspace runtime.

use std::fs;
use std::fs::OpenOptions;
use std::os::unix::fs::symlink;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panelink::client_context::ContextFile;
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::protocol::OperationKey;
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::session_protocol::{encode_session_hello, SessionHello};
use panelink::transport::AuthenticatedStream;
use panelink::ui_protocol::{
    decode_layout_reply, decode_resize_reply, encode_layout_query, encode_root_resize,
    ResizeStatus, RootResize,
};

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

    fn agent_with_context_dir(&self, context_dir: &Path, args: &[&str]) -> Output {
        let mut command = self.command();
        command
            .env("PANELINK_AGENT_CONTEXT_DIR", context_dir)
            .args(args);
        run_bounded(command, "agent CLI with explicit context directory")
    }

    fn wait_for_server_stop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let exited = self
                .server
                .as_mut()
                .and_then(|server| server.try_wait().ok().flatten())
                .is_some();
            let socket_removed = !self.state.join("server/control.sock").exists();
            if exited && socket_removed {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "server did not stop and remove its control socket"
            );
            thread::sleep(Duration::from_millis(10));
        }
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

fn deadline_after(duration: Duration) -> Instant {
    Instant::now() + duration
}

fn run_bounded(mut command: Command, description: &str) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bounded agent CLI command");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child
            .try_wait()
            .expect("poll bounded agent CLI command")
            .is_some()
        {
            return child
                .wait_with_output()
                .expect("collect bounded agent CLI output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .expect("collect timed-out agent CLI output");
            panic!(
                "{description} exceeded its 10-second deadline: stdout={:?} stderr={:?}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn context_directory_failure_can_be_overridden_for_read_only_pane_discovery() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    let first_target = field(&startup, "target")
        .unwrap_or_else(|| panic!("startup lacked target: {startup}"))
        .to_owned();
    Fixture::assert_ok(
        &fixture.control("split.ctx", &["init", &first_target]),
        "initialize split context",
    );
    let split = Fixture::assert_ok(
        &fixture.control("split.ctx", &["split", &first_target, "1", "right"]),
        "create second pane",
    );
    assert_eq!(field(&split, "outcome"), Some("applied"), "{split}");

    // A non-directory at the default location reproduces a context-root
    // failure without relying on host permission bits or sandbox behavior.
    fs::write(fixture.state.join("agent-contexts"), "blocked").unwrap();
    let default_failure = fixture.agent(&["pane", "list", "--json"]);
    assert!(!default_failure.status.success());
    let default_failure = String::from_utf8_lossy(&default_failure.stdout);
    assert_eq!(
        field(&default_failure, "error"),
        Some("context_directory_unsafe"),
        "{default_failure}"
    );

    // The explicit root must itself be creatable and private. A missing
    // parent is reported without printing its path or silently changing roots.
    let unavailable_root = fixture.root.join("missing-parent").join("agent-contexts");
    let unavailable =
        fixture.agent_with_context_dir(&unavailable_root, &["pane", "list", "--json"]);
    assert!(!unavailable.status.success());
    let unavailable = String::from_utf8_lossy(&unavailable.stdout);
    assert_eq!(
        field(&unavailable, "error"),
        Some("context_directory_unavailable"),
        "{unavailable}"
    );
    assert!(unavailable.contains("PANELINK_AGENT_CONTEXT_DIR"));
    assert!(!unavailable.contains(unavailable_root.to_str().unwrap()));

    let relative_root = Path::new("relative-agent-contexts");
    let relative = fixture.agent_with_context_dir(relative_root, &["pane", "list", "--json"]);
    assert!(!relative.status.success());
    assert_eq!(
        field(&String::from_utf8_lossy(&relative.stdout), "error"),
        Some("context_directory_unsafe")
    );

    let target_root = fixture.root.join("private-context-target");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&target_root)
        .unwrap();
    let symlink_root = fixture.root.join("context-symlink");
    symlink(&target_root, &symlink_root).unwrap();
    let linked = fixture.agent_with_context_dir(&symlink_root, &["pane", "list", "--json"]);
    assert!(!linked.status.success());
    assert_eq!(
        field(&String::from_utf8_lossy(&linked.stdout), "error"),
        Some("context_directory_unsafe")
    );

    let public_root = fixture.root.join("nonprivate-contexts");
    fs::DirBuilder::new()
        .mode(0o755)
        .create(&public_root)
        .unwrap();
    let public = fixture.agent_with_context_dir(&public_root, &["pane", "list", "--json"]);
    assert!(!public.status.success());
    assert_eq!(
        field(&String::from_utf8_lossy(&public.stdout), "error"),
        Some("context_directory_unsafe")
    );

    let context_root = fixture.root.join("agent-contexts-override");
    let listing = Fixture::assert_ok(
        &fixture.agent_with_context_dir(&context_root, &["pane", "list", "--json"]),
        "pane list with explicit context directory",
    );
    assert_eq!(field(&listing, "action"), Some("pane_list"), "{listing}");
    assert!(listing.contains("\"index\":1"), "{listing}");
    assert!(listing.contains("\"index\":2"), "{listing}");

    let read = Fixture::assert_ok(
        &fixture.agent_with_context_dir(
            &context_root,
            &["pane", "read", "2", "--lines", "24", "--json"],
        ),
        "pane read with explicit context directory",
    );
    assert_eq!(field(&read, "action"), Some("pane_read"), "{read}");
    assert_eq!(field(&read, "source"), Some("screen"), "{read}");

    // Seed a durable uncertain query reservation in the exact context used
    // above. A rejected override must leave it intact; reopening the same
    // stable root then reconciles NotAdmitted before issuing the read.
    let target = field(&read, "target").expect("pane read target");
    let target_name = target.replace(':', "-");
    let context_path = context_root
        .join("agent")
        .join(format!("{target_name}.ctx"));
    let mut context =
        ContextFile::open_existing(&context_path, deadline_after(Duration::from_secs(2)))
            .expect("open pane caller context");
    let uncertain_key = context.reserve().expect("reserve uncertain query key");
    drop(context);

    let rejected_switch = fixture.agent_with_context_dir(
        &public_root,
        &["pane", "read", "2", "--lines", "24", "--json"],
    );
    assert!(!rejected_switch.status.success());
    assert_eq!(
        field(&String::from_utf8_lossy(&rejected_switch.stdout), "error"),
        Some("context_directory_unsafe")
    );
    let context = ContextFile::open_existing(&context_path, deadline_after(Duration::from_secs(2)))
        .expect("reopen context after rejected override");
    assert_eq!(context.context().uncertain(), Some(uncertain_key));
    drop(context);

    let reconciled = fixture.agent_with_context_dir(
        &context_root,
        &["pane", "read", "2", "--lines", "24", "--json"],
    );
    assert!(!reconciled.status.success());
    assert_eq!(
        field(&String::from_utf8_lossy(&reconciled.stdout), "error"),
        Some("previous_operation_settled")
    );
    let context = ContextFile::open_existing(&context_path, deadline_after(Duration::from_secs(2)))
        .expect("reopen reconciled caller context");
    assert_eq!(context.context().uncertain(), None);
    drop(context);
    Fixture::assert_ok(
        &fixture.agent_with_context_dir(
            &context_root,
            &["pane", "read", "2", "--lines", "24", "--json"],
        ),
        "separate pane read after exact context reconciliation",
    );
}

struct ObserverGuard(Option<Child>);

impl ObserverGuard {
    fn id(&self) -> u32 {
        self.0.as_ref().expect("observer child").id()
    }

    fn signal(&self, signal: &str) {
        let pid = self.id().to_string();
        let status = Command::new("/bin/kill")
            .args([format!("-{signal}"), pid])
            .status()
            .expect("signal test observer");
        assert!(status.success(), "could not send SIG{signal} to observer");
    }

    fn wait_until(&mut self, deadline: Instant) -> std::process::ExitStatus {
        loop {
            let child = self.0.as_mut().expect("observer child");
            if let Some(status) = child.try_wait().expect("poll event observer") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "event observer missed its quiet deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            // A stopped test child must not leak if any assertion unwinds.
            let pid = child.id().to_string();
            let _ = Command::new("/bin/kill").args(["-KILL", &pid]).status();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn raw_request(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    payload: &[u8],
) -> panelink::transport::OwnedFrame {
    let frame = Frame::new(MessageKind::Request, request_id, payload).expect("request frame");
    stream
        .write_frame(&frame, deadline_after(Duration::from_secs(2)))
        .expect("write raw request");
    let response = stream
        .read_frame(deadline_after(Duration::from_secs(2)))
        .expect("read raw response");
    assert_eq!(response.kind(), MessageKind::Response);
    assert_eq!(response.request_id(), request_id);
    response
}

fn raw_resize(
    stream: &mut AuthenticatedStream,
    request_id: &mut u64,
    generation: u64,
    lease: u64,
    sequence: &mut u64,
    revision: u64,
    dimensions: (u16, u16),
) -> u64 {
    let key = OperationKey {
        generation,
        lease,
        sequence: *sequence,
    };
    *sequence = sequence.checked_add(1).expect("operation sequence");
    let payload = encode_root_resize(&RootResize {
        key,
        expected_layout_revision: revision,
        rows: dimensions.0,
        columns: dimensions.1,
    })
    .expect("encode root resize");
    let response = raw_request(stream, *request_id, &payload);
    *request_id = request_id.checked_add(1).expect("request sequence");
    let reply = decode_resize_reply(response.payload()).expect("decode root resize reply");
    assert_eq!(reply.key, key);
    assert!(matches!(
        reply.status,
        ResizeStatus::Applied | ResizeStatus::ResizePending
    ));
    assert!(
        reply.revision > revision,
        "accepted resize must advance layout revision"
    );
    reply.revision
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

    let released = Fixture::assert_ok(
        &fixture.agent(&["pane", "release", "1", "--json"]),
        "pane release",
    );
    assert_eq!(
        field(&released, "action"),
        Some("pane_release"),
        "{released}"
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
    fixture.wait_for_server_stop();

    let stale = fixture.agent(&["pane", "read", "2", "--json"]);
    assert!(!stale.status.success());
    let stale = String::from_utf8_lossy(&stale.stdout);
    assert_eq!(field(&stale, "error"), Some("no_workspace"), "{stale}");
}

#[test]
fn close_event_and_stale_index_remain_scoped_to_current_layout() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    let first_target = field(&startup, "target")
        .unwrap_or_else(|| panic!("startup lacked target: {startup}"))
        .to_owned();
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

#[test]
fn workspace_events_pin_filtered_pane_across_same_generation_replacement() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    let first_target = field(&startup, "target")
        .unwrap_or_else(|| panic!("startup lacked target: {startup}"))
        .to_owned();
    Fixture::assert_ok(
        &fixture.control("split.ctx", &["init", &first_target]),
        "initialize split context",
    );
    let split = Fixture::assert_ok(
        &fixture.control("split.ctx", &["split", &first_target, "1", "right"]),
        "create replacement pane",
    );
    assert_eq!(field(&split, "outcome"), Some("applied"), "{split}");
    let observer = fixture
        .command()
        .args([
            "events",
            "--pane",
            "1",
            "--timeout",
            "2",
            "--json",
            "--as",
            "pinned-layout-observer",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pinned layout event stream");
    thread::sleep(Duration::from_millis(300));
    let output_event = Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "send",
            "1",
            "printf 'LAYOUT_KEEPALIVE\\n'",
            "--enter",
            "--claim",
            "--json",
        ]),
        "send output without changing topology",
    );
    assert_eq!(
        field(&output_event, "status"),
        Some("applied"),
        "{output_event}"
    );
    thread::sleep(Duration::from_millis(100));
    let closed = Fixture::assert_ok(
        &fixture.agent(&["pane", "close", "1", "--json", "--as", "close-pinned-pane"]),
        "close initially selected pane",
    );
    assert_eq!(field(&closed, "action"), Some("pane_close"), "{closed}");
    let output = observer
        .wait_with_output()
        .expect("wait for pinned event stream");
    assert!(output.status.success(), "events: {output:?}");
    let lines = String::from_utf8_lossy(&output.stdout);
    assert!(
        lines
            .lines()
            .any(|line| line.contains("\"event\":\"layout_changed\"")),
        "{lines}"
    );
    assert!(
        lines
            .lines()
            .any(|line| line.contains("\"event\":\"output\"")),
        "pane output event was lost while workspace watch timed out: {lines}"
    );
    assert!(
        lines
            .lines()
            .any(|line| line.contains("\"event\":\"pane_closed\"")),
        "{lines}"
    );
    assert!(
        !lines.contains("\"event\":\"pane_opened\""),
        "replacement entered pinned stream: {lines}"
    );
    assert!(
        lines
            .lines()
            .any(|line| line.contains("\"event\":\"quiet\"")),
        "{lines}"
    );
}

#[test]
fn real_cli_workspace_watch_gap_recovers_after_ring_overflow() {
    let mut fixture = Fixture::new();
    fixture.start();
    let server_directory = fixture.state.join("server");
    let mut stream =
        AuthenticatedStream::connect(&server_directory, deadline_after(Duration::from_secs(5)))
            .expect("connect raw resize client to private server");
    let hello = SessionHello {
        hello: Hello {
            nonce: 0x4750_434c_4945_4e54,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let payload = encode_session_hello(&hello).expect("encode read/input Hello");
    let frame = Frame::new(MessageKind::Hello, 0, &payload).expect("Hello frame");
    stream
        .write_frame(&frame, deadline_after(Duration::from_secs(2)))
        .expect("write raw Hello");
    let welcome_frame = stream
        .read_frame(deadline_after(Duration::from_secs(2)))
        .expect("read raw Welcome");
    assert_eq!(welcome_frame.kind(), MessageKind::Welcome);
    let welcome = panelink::handshake_protocol::decode_welcome(welcome_frame.payload())
        .expect("decode raw Welcome");
    assert_eq!(
        welcome.selected_capabilities & (CAPABILITY_READ | CAPABILITY_INPUT),
        CAPABILITY_READ | CAPABILITY_INPUT
    );

    let mut request_id = 1;
    let layout = raw_request(&mut stream, request_id, &encode_layout_query());
    request_id += 1;
    let mut revision = decode_layout_reply(layout.payload())
        .expect("decode initial layout")
        .revision;
    let mut operation_sequence = 1;
    let output_path = fixture.root.join("workspace-gap-events.jsonl");
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&output_path)
        .expect("create private event output file");
    let error_path = fixture.root.join("workspace-gap-events.stderr");
    let error_output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&error_path)
        .expect("create private event error file");
    let observer = fixture
        .command()
        .args([
            "events",
            "--timeout",
            "30",
            "--json",
            "--as",
            "gap-observer",
        ])
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(error_output))
        .spawn()
        .expect("spawn CLI workspace observer");
    let mut observer = ObserverGuard(Some(observer));

    let mut resize_number = 0_u16;
    let mut cursor_established = false;
    let readiness_deadline = deadline_after(Duration::from_secs(4));
    while Instant::now() < readiness_deadline {
        let (rows, columns) = if resize_number.is_multiple_of(2) {
            (36, 110)
        } else {
            (37, 111)
        };
        revision = raw_resize(
            &mut stream,
            &mut request_id,
            welcome.generation,
            welcome.lease,
            &mut operation_sequence,
            revision,
            (rows, columns),
        );
        resize_number += 1;
        if fs::read_to_string(&output_path)
            .expect("read observer output while it runs")
            .contains("\"event\":\"layout_changed\"")
        {
            cursor_established = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        cursor_established,
        "CLI did not report a layout change during bounded readiness resizes"
    );

    observer.signal("STOP");
    for _ in 0..70 {
        let (rows, columns) = if resize_number.is_multiple_of(2) {
            (36, 110)
        } else {
            (37, 111)
        };
        revision = raw_resize(
            &mut stream,
            &mut request_id,
            welcome.generation,
            welcome.lease,
            &mut operation_sequence,
            revision,
            (rows, columns),
        );
        resize_number += 1;
    }
    assert!(
        resize_number >= 70,
        "not enough successful layout invalidations"
    );
    observer.signal("CONT");
    let status = observer.wait_until(deadline_after(Duration::from_secs(32)));
    assert!(
        status.success(),
        "event observer exited with {status}: {}",
        fs::read_to_string(&error_path).expect("read event observer error output")
    );
    let events = fs::read_to_string(&output_path).expect("read complete event output");
    assert_eq!(
        events.matches("\"event\":\"layout_resync\"").count(),
        1,
        "expected one real workspace journal-gap recovery: {events}"
    );
    assert!(
        events
            .lines()
            .any(|line| line.contains("\"event\":\"quiet\"")),
        "observer did not finish at its quiet deadline: {events}"
    );
    assert!(
        events.find("\"event\":\"layout_changed\"") < events.find("\"event\":\"layout_resync\""),
        "readiness notification must precede the recovered gap: {events}"
    );
}

#[test]
fn pane_metadata_tracks_managed_child_cd_and_exec_without_arguments_or_environment() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
    let listing = Fixture::assert_ok(
        &fixture.agent(&["pane", "list", "--json"]),
        "initial metadata",
    );
    assert_eq!(field(&listing, "cwd"), fixture.home.to_str(), "{listing}");
    assert_eq!(field(&listing, "command"), Some("/bin/zsh"), "{listing}");
    assert_eq!(field(&listing, "metadata_source"), Some("managed_child"));
    assert_eq!(field(&listing, "cwd_status"), Some("available"));
    assert!(field(&listing, "sampled_at_mono_ms")
        .unwrap()
        .parse::<u64>()
        .is_ok());
    Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "send",
            "1",
            "/bin/sleep 2",
            "--enter",
            "--claim",
            "--json",
        ]),
        "foreground descendant",
    );
    let listing = Fixture::assert_ok(
        &fixture.agent(&["pane", "list", "--json"]),
        "descendant metadata",
    );
    assert_eq!(field(&listing, "command"), Some("/bin/zsh"), "{listing}");
    let changed = fixture.home.join("changed");
    fs::create_dir(&changed).unwrap();
    let command = format!(
        "cd {}; export PANELINK_METADATA_SECRET=PRIVATE_METADATA_SENTINEL",
        changed.display()
    );
    Fixture::assert_ok(
        &fixture.agent(&["pane", "send", "1", &command, "--enter", "--json"]),
        "child cd",
    );
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let listing =
            Fixture::assert_ok(&fixture.agent(&["pane", "list", "--json"]), "cwd refresh");
        assert!(!listing.contains("PRIVATE_METADATA_SENTINEL"), "{listing}");
        assert!(!listing.contains("PANELINK_METADATA_SECRET"), "{listing}");
        if field(&listing, "cwd") == changed.to_str() {
            break;
        }
        assert!(Instant::now() < deadline, "cwd failed to update: {listing}");
        thread::sleep(Duration::from_millis(20));
    }
    Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "send",
            "1",
            "exec /bin/sleep 30",
            "--enter",
            "--json",
        ]),
        "direct exec",
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let listing =
            Fixture::assert_ok(&fixture.agent(&["pane", "list", "--json"]), "exec refresh");
        if field(&listing, "command") == Some("/bin/sleep") {
            assert_eq!(field(&listing, "cwd"), changed.to_str(), "{listing}");
            assert!(!listing.contains("sleep 30"), "argv leaked: {listing}");
            assert!(!listing.contains("PRIVATE_METADATA_SENTINEL"), "{listing}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "executable failed to update: {listing}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    Fixture::assert_ok(
        &fixture.agent(&["pane", "close", "1", "--json"]),
        "close sampled child",
    );
    fixture.wait_for_server_stop();
}

#[test]
fn pane_read_scrollback_returns_live_history_separately_from_screen() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
    Fixture::assert_ok(
        &fixture.agent(&[
            "pane", "send", "1",
            "i=0; while [ $i -lt 200 ]; do printf 'HIST%03d\\n' $i; i=$((i+1)); done; printf 'SCREEN-END\\n'",
            "--enter", "--claim", "--json",
        ]),
        "produce live scrollback",
    );
    // Wait for output completion, rather than the echoed command: the final
    // output contains the final numbered row, which is absent from the command.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let screen = Fixture::assert_ok(
            &fixture.agent(&["pane", "read", "1", "--json"]),
            "read current screen",
        );
        if screen.contains("HIST199") && screen.contains("SCREEN-END") {
            break;
        }
        assert!(Instant::now() < deadline, "output did not finish: {screen}");
        thread::sleep(Duration::from_millis(10));
    }
    let history = Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "read",
            "1",
            "--scrollback",
            "--lines",
            "4",
            "--json",
        ]),
        "read live scrollback tail",
    );
    assert_eq!(
        field(&history, "source"),
        Some("live_scrollback"),
        "{history}"
    );
    assert_eq!(field(&history, "truncated"), Some("true"), "{history}");
    assert!(history.contains("HIST"), "{history}");
    assert!(
        !history.contains("SCREEN-END"),
        "visible rows leaked into history: {history}"
    );
    assert!(!history.contains("\\u001b"), "{history}");
    assert!(history.matches("\\n").count() == 3, "{history}");
    let one = Fixture::assert_ok(
        &fixture.agent(&[
            "pane",
            "read",
            "1",
            "--scrollback",
            "--lines",
            "1",
            "--json",
        ]),
        "read one live history row",
    );
    assert_eq!(field(&one, "source"), Some("live_scrollback"), "{one}");
    assert!(one.contains("HIST"), "{one}");
    assert!(one.matches("\\n").count() == 0, "{one}");
}
