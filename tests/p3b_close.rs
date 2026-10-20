#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-PTY proof of the trusted in-process close/stop transaction.
//!
//! Exercises `MultiSessionOwner::execute_close`, automatic collapse in
//! `poll`, and re-split with fresh identities, using the production launch
//! preparation, spawn and journal. In-process core evidence only: no socket
//! route, runtime driver, CLI or UI reaches this owner.

use panelink::close_protocol::{encode_close_request, CloseOutcome, CloseRequest, StopOutcome};
use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use panelink::pane_layout::{Rect, SplitDirection};
use panelink::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::server::multi_session::MultiSessionOwner;
use panelink::server::{Connection, SessionReply};
use panelink::server_pty::PtyEffects;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{SessionHello, SessionRequest};
use panelink::split_protocol::{encode_split_request, SplitRequest};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const NOW: u64 = 100;
const LAUNCH_LEASE: u64 = 9_201;
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 81;
const PANE_COLUMNS: u16 = 40;
/// Prints READY/PID, then answers each line: `SIZE` reports `stty size`;
/// anything else is acknowledged with the child's own PID.
/// READY also reports `$ZSH_VERSION` (empty under `/bin/sh`) to prove the shell.
const CHILD_SCRIPT: &[u8] = b"printf 'P3SP-SHELL-zsh%s-END\\n' \"${ZSH_VERSION:+-yes}\"; printf 'P3SP-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; while IFS= read -r token; do if [ \"$token\" = SIZE ]; then printf 'P3SP-SIZE-%s-PID-%s-END\\n' \"$(stty size | tr ' ' x)\" \"$$\"; else printf 'P3SP-ACK-%s-PID-%s-END\\n' \"$token\" \"$$\"; fi; done";
/// Ignores HUP (installed before READY is printed), so stop must escalate.
const HUP_IGNORING_SCRIPT: &[u8] = b"trap '' HUP; printf 'P3SP-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; while :; do /bin/sleep 10; done";
const SCENARIO_DEADLINE: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateFixture(PathBuf);

impl PrivateFixture {
    fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp").join(format!(
                "panelink-p3b-close-{}-{serial}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One explicit shell under test and its non-interactive, no-rc startup.
#[derive(Clone, Copy)]
struct Shell {
    executable: &'static [u8],
    /// Arguments before `-c`; zsh uses `-f` (NO_RCS) so no rc file runs.
    startup: &'static [&'static [u8]],
}

const ZSH: Shell = Shell {
    executable: b"/bin/zsh",
    startup: &[b"-f"],
};
const SH: Shell = Shell {
    executable: b"/bin/sh",
    startup: &[],
};

/// Explicit child launch context. HOME and ZDOTDIR name an empty private
/// directory, so even an accidental rc lookup finds nothing; nothing is
/// inherited from the test process or server environment.
/// The directory is separate from server storage, which must start empty.
struct LaunchContext {
    _directory: PrivateFixture,
    cwd: Vec<u8>,
    home: Vec<u8>,
}

impl LaunchContext {
    fn new() -> Self {
        let directory = PrivateFixture::new().expect("create private launch directory");
        let path = directory.path().as_os_str().as_bytes().to_vec();
        Self {
            cwd: path.clone(),
            home: path,
            _directory: directory,
        }
    }

    fn environment(&self) -> [EnvironmentEntry<'_>; 4] {
        [
            EnvironmentEntry {
                name: b"TERM",
                value: b"panelink-probe",
            },
            EnvironmentEntry {
                name: b"PATH",
                value: b"/usr/bin:/bin",
            },
            EnvironmentEntry {
                name: b"HOME",
                value: &self.home,
            },
            EnvironmentEntry {
                name: b"ZDOTDIR",
                value: &self.home,
            },
        ]
    }
}

fn shell_arguments<'a>(shell: Shell, script: &'a [u8], marker: &'a str) -> Vec<&'a [u8]> {
    let mut arguments: Vec<&[u8]> = shell.startup.to_vec();
    arguments.extend_from_slice(&[b"-c", script, b"panelink-split-test", marker.as_bytes()]);
    arguments
}

fn launch_payload(
    key: OperationKey,
    shell: Shell,
    (rows, columns): (u16, u16),
    context: &LaunchContext,
    script: &[u8],
    marker: &str,
) -> Vec<u8> {
    let arguments = shell_arguments(shell, script, marker);
    let environment = context.environment();
    let request = LaunchRequest {
        key,
        rows,
        columns,
        requested_executable: shell.executable,
        requested_working_directory: &context.cwd,
        client_working_directory: &context.cwd,
        arguments: &arguments,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_launch_request(&request, &mut payload).expect("encode explicit launch");
    payload
}

fn key(generation: u64, sequence: u64) -> OperationKey {
    OperationKey {
        generation,
        lease: LAUNCH_LEASE,
        sequence,
    }
}

fn split_payload(
    split_key: OperationKey,
    target: TargetRef,
    revision: u64,
    shell: Shell,
    context: &LaunchContext,
    script: &[u8],
    marker: &str,
) -> Vec<u8> {
    let arguments = shell_arguments(shell, script, marker);
    let environment = context.environment();
    let launch = LaunchRequest {
        key: split_key,
        rows: ROOT_ROWS,
        columns: PANE_COLUMNS,
        requested_executable: shell.executable,
        requested_working_directory: &context.cwd,
        client_working_directory: &context.cwd,
        arguments: &arguments,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: split_key,
            target,
            expected_layout_revision: revision,
            direction: SplitDirection::Right,
            launch: &launch,
        },
        &mut payload,
    )
    .expect("encode schema-7 split request");
    payload
}

fn prepare(
    request: panelink::managed_launch_protocol::LaunchRequestView<'_>,
    target: TargetRef,
    terminfo: &Path,
) -> Result<PreparedLaunch, ()> {
    prepare_launch(request, target, terminfo).map_err(|_| ())
}

fn spawn(prepared: PreparedLaunch) -> Result<PtyEffects, ()> {
    prepared.spawn().map(PtyEffects::new).map_err(|_| ())
}

fn resize(effects: &mut PtyEffects, rows: u16, columns: u16) -> Result<(), Outcome> {
    effects.resize(rows, columns)
}

fn input_hello(nonce: u64) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    }
}

/// One connected target with its own operation sequence.
struct Client {
    connection: Connection,
    target: TargetRef,
    sequence: u64,
}

impl Client {
    fn next_key(&mut self) -> OperationKey {
        self.sequence += 1;
        OperationKey {
            generation: self.target.generation,
            lease: self.connection.lease(),
            sequence: self.sequence,
        }
    }
}

fn describe(
    owner: &mut MultiSessionOwner<PtyEffects>,
    client: &Client,
) -> panelink::session_protocol::SessionInfo {
    let payload = panelink::session_protocol::encode_session_request(&SessionRequest::Describe)
        .expect("encode Describe");
    match owner
        .handle_session(&client.connection, &payload, NOW)
        .expect("dispatch Describe")
    {
        SessionReply::Info(info) => info,
        other => panic!("expected Describe info, got {other:?}"),
    }
}

fn connect_and_claim(
    owner: &mut MultiSessionOwner<PtyEffects>,
    target: TargetRef,
    nonce: u64,
) -> Client {
    let (connection, welcome) = owner
        .connect(target, &input_hello(nonce), NOW)
        .expect("connect read/input lease pinned to target");
    assert_eq!(
        welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_INPUT
    );
    let mut client = Client {
        connection,
        target,
        sequence: 0,
    };
    let info = describe(owner, &client);
    assert_eq!(info.target, target);
    assert_eq!(info.owner_lease, 0, "split never claims input ownership");
    let claim_key = client.next_key();
    let payload = panelink::session_protocol::encode_session_request(&SessionRequest::Claim {
        key: claim_key,
        target,
        expected_owner_revision: info.owner_revision,
        ttl_seconds: 30,
    })
    .expect("encode Claim");
    let SessionReply::Payload(result) = owner
        .handle_session(&client.connection, &payload, NOW)
        .expect("claim input")
    else {
        panic!("Claim returns an encoded operation result");
    };
    assert_eq!(
        decode_response_payload(&result).expect("decode Claim").body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    client
}

fn read_screen(owner: &mut MultiSessionOwner<PtyEffects>, client: &mut Client) -> String {
    let key = client.next_key();
    let request = ControlRequest {
        key,
        target: client.target,
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 3_000,
            max_lines: panelink::pane_layout::MAX_PANE_ROWS,
        },
    };
    let mut payload = Vec::new();
    encode_request_payload(&request, &mut payload).expect("encode read");
    let response = owner
        .handle_control(&client.connection, &payload, NOW)
        .expect("dispatch read");
    match decode_response_payload(&response)
        .expect("decode read")
        .body
    {
        ResponseBody::Read { target, text, .. } => {
            assert_eq!(target, client.target);
            text.to_owned()
        }
        other => panic!("expected screen response, got {other:?}"),
    }
}

fn submit(owner: &mut MultiSessionOwner<PtyEffects>, client: &mut Client, text: &str) {
    let owner_revision = describe(owner, client).owner_revision;
    let key = client.next_key();
    let request = ControlRequest {
        key,
        target: client.target,
        command: Command::Submit {
            owner_revision,
            text,
        },
    };
    let mut payload = Vec::new();
    encode_request_payload(&request, &mut payload).expect("encode submit");
    let response = owner
        .handle_control(&client.connection, &payload, NOW)
        .expect("dispatch submit");
    assert_eq!(
        decode_response_payload(&response)
            .expect("decode submit")
            .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
}

fn wait_for(
    owner: &mut MultiSessionOwner<PtyEffects>,
    client: &mut Client,
    required: &str,
    forbidden: &[&str],
) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        owner.poll(NOW).expect("poll both retained PTYs");
        let screen = read_screen(owner, client);
        for text in forbidden {
            assert!(
                !screen.contains(text),
                "a target's screen must not contain its sibling's output"
            );
        }
        if screen.contains(required) {
            return screen;
        }
        assert!(Instant::now() < deadline, "child sentinel missed deadline");
        thread::sleep(POLL_INTERVAL);
    }
}

fn extract_pid(text: &str, marker: &str) -> Option<i32> {
    for (index, _) in text.match_indices(marker) {
        let suffix = &text[index + marker.len()..];
        let Some(pid) = suffix.strip_prefix("-PID-") else {
            continue;
        };
        let Some((digits, _)) = pid.split_once("-END") else {
            continue;
        };
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            return digits.parse().ok();
        }
    }
    None
}

fn assert_process_alive(pid: i32) {
    // SAFETY: signal zero only checks whether this PID exists and is visible.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(result, 0, "child remains owned and live");
}

fn assert_process_reaped(pid: i32) {
    // SAFETY: signal zero only checks whether this PID exists and is visible.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(result, -1, "a stopped or dropped child must be reaped");
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "signal-zero must report ESRCH after owner cleanup"
    );
}

fn pane_rect(owner: &MultiSessionOwner<PtyEffects>, target: TargetRef) -> Rect {
    owner
        .layout_snapshot()
        .expect("layout is established")
        .panes
        .into_iter()
        .flatten()
        .find(|pane| pane.target == target)
        .expect("target is a layout pane")
        .rect
}

struct Harness {
    fixture: PrivateFixture,
    context: LaunchContext,
    storage: ServerStorage,
    owner: MultiSessionOwner<PtyEffects>,
    generation: u64,
    sequence: u64,
    shell: Shell,
}

impl Harness {
    fn new(shell: Shell) -> Self {
        let fixture = PrivateFixture::new().expect("create private storage fixture");
        let context = LaunchContext::new();
        let storage = ServerStorage::initialize(fixture.path()).expect("initialize journal");
        let generation = storage.generation();
        let mut ledger = Ledger::new(generation).expect("create owner ledger");
        ledger
            .open_lease(LAUNCH_LEASE, NOW)
            .expect("open internal launch lease");
        let owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true)
            .expect("assemble input-enabled owner");
        Self {
            fixture,
            context,
            storage,
            owner,
            generation,
            sequence: 0,
            shell,
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    fn launch_first(&mut self, marker: &str) -> TargetRef {
        let sequence = self.next_sequence();
        let payload = launch_payload(
            key(self.generation, sequence),
            self.shell,
            (ROOT_ROWS, ROOT_COLUMNS),
            &self.context,
            CHILD_SCRIPT,
            marker,
        );
        let terminfo = self.fixture.path().to_path_buf();
        let mut journal = self.storage.journal().expect("open launch journal");
        let result = self
            .owner
            .execute_launch(
                &payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, &terminfo),
                spawn,
            )
            .expect("launch first child");
        assert_eq!(result.outcome, Outcome::Applied);
        result.target.expect("first launch has a target")
    }

    fn split_right(&mut self, target: TargetRef, script: &[u8], marker: &str) -> TargetRef {
        let sequence = self.next_sequence();
        let revision = self.owner.layout_snapshot().expect("layout").revision;
        let payload = split_payload(
            key(self.generation, sequence),
            target,
            revision,
            self.shell,
            &self.context,
            script,
            marker,
        );
        let terminfo = self.fixture.path().to_path_buf();
        let mut journal = self.storage.journal().expect("open launch journal");
        let result = self
            .owner
            .execute_split(
                &payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, &terminfo),
                spawn,
                resize,
            )
            .expect("split through the production launch path");
        assert_eq!(result.outcome, Outcome::Applied);
        result.new_target.expect("split assigns a new target")
    }

    fn close(&mut self, target: TargetRef) -> panelink::close_protocol::CloseResult {
        let sequence = self.next_sequence();
        let revision = self.owner.layout_snapshot().expect("layout").revision;
        let mut payload = Vec::new();
        encode_close_request(
            &CloseRequest {
                key: key(self.generation, sequence),
                target,
                expected_layout_revision: revision,
            },
            &mut payload,
        )
        .expect("encode schema-9 close");
        self.owner
            .execute_close(&payload, NOW, resize)
            .expect("execute close")
    }

    fn ready_pid(&mut self, client: &mut Client, marker: &str) -> i32 {
        let ready = format!("P3SP-READY-{marker}");
        let screen = wait_for(&mut self.owner, client, &ready, &[]);
        extract_pid(&screen, &ready).expect("READY carries PID")
    }

    fn assert_size(&mut self, client: &mut Client, pid: i32, rows: u16, columns: u16) {
        let expected = format!("P3SP-SIZE-{rows}x{columns}");
        submit(&mut self.owner, client, "SIZE");
        let screen = wait_for(&mut self.owner, client, &expected, &[]);
        assert_eq!(extract_pid(&screen, &expected), Some(pid));
    }
}

fn root_rect() -> Rect {
    Rect {
        x: 0,
        y: 0,
        rows: ROOT_ROWS,
        columns: ROOT_COLUMNS,
    }
}

#[test]
fn zsh_close_stop_collapse_and_resplit_on_real_ptys() {
    close_scenario(ZSH);
}

#[test]
fn sh_close_stop_collapse_and_resplit_on_real_ptys() {
    close_scenario(SH);
}

/// Close live sibling -> Exited; survivor fills the root and its PTY reports
/// the root size; re-split gets a fresh session; a HUP-ignoring child closes
/// as Escalated; a shell exit auto-collapses; closing the last pane ends the
/// workspace.
fn close_scenario(shell: Shell) {
    let mut h = Harness::new(shell);
    let first = h.launch_first("close-first");
    let mut first_client = connect_and_claim(&mut h.owner, first, 401);
    let first_pid = h.ready_pid(&mut first_client, "close-first");

    // 1. Close a live sibling: HUP is enough, the survivor expands.
    let second = h.split_right(first, CHILD_SCRIPT, "close-second");
    let mut second_client = connect_and_claim(&mut h.owner, second, 402);
    let second_pid = h.ready_pid(&mut second_client, "close-second");
    h.assert_size(&mut first_client, first_pid, ROOT_ROWS, PANE_COLUMNS);
    let started = Instant::now();
    let closed = h.close(second);
    assert_eq!(closed.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "a HUP-honouring child must not wait for the KILL window"
    );
    assert!(!closed.survivor_resize_pending);
    assert_process_reaped(second_pid);
    assert_eq!(pane_rect(&h.owner, first), root_rect());
    assert_eq!(h.owner.layout_snapshot().unwrap().pane_count(), 1);
    h.assert_size(&mut first_client, first_pid, ROOT_ROWS, ROOT_COLUMNS);
    assert_process_alive(first_pid);

    // 2. Re-split after close gets a fresh, never-reused session identity,
    // and the journal accepts it.
    let third = h.split_right(first, HUP_IGNORING_SCRIPT, "close-third");
    assert!(third.session > second.session);
    assert_ne!(third.pane, second.pane);
    let mut third_client = connect_and_claim(&mut h.owner, third, 403);
    let third_pid = h.ready_pid(&mut third_client, "close-third");

    // 3. A HUP-ignoring child requires KILL escalation.
    let started = Instant::now();
    let escalated = h.close(third);
    assert_eq!(
        escalated.outcome,
        CloseOutcome::Stopped(StopOutcome::Escalated)
    );
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_millis(2_500));
    assert_process_reaped(third_pid);
    assert_eq!(pane_rect(&h.owner, first), root_rect());

    // 4. A shell exiting on its own while its sibling lives collapses in poll.
    let fourth = h.split_right(first, CHILD_SCRIPT, "close-fourth");
    assert!(fourth.session > third.session);
    let mut fourth_client = connect_and_claim(&mut h.owner, fourth, 404);
    let fourth_pid = h.ready_pid(&mut fourth_client, "close-fourth");
    assert_eq!(pane_rect(&h.owner, first).columns, PANE_COLUMNS);
    // SAFETY: TERM to a PID this test spawned and still owns.
    assert_eq!(unsafe { libc::kill(fourth_pid, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    while h.owner.layout_snapshot().unwrap().pane_count() != 1 {
        let _ = h.owner.poll(NOW);
        assert!(
            Instant::now() < deadline,
            "auto-collapse missed its deadline"
        );
        thread::sleep(POLL_INTERVAL);
    }
    assert_process_reaped(fourth_pid);
    assert_eq!(pane_rect(&h.owner, first), root_rect());
    assert!(!h.owner.all_sessions_ended());
    h.assert_size(&mut first_client, first_pid, ROOT_ROWS, ROOT_COLUMNS);

    // 5. Closing the last pane stops it and ends the workspace.
    let last = h.close(first);
    assert_eq!(last.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
    assert_process_reaped(first_pid);
    assert!(h.owner.all_sessions_ended());
    assert_eq!(
        [second, third, fourth]
            .iter()
            .map(|target| target.session)
            .collect::<Vec<_>>(),
        [2, 3, 4]
    );
}
