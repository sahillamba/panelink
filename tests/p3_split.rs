#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-PTY proof of the trusted in-process split transaction.
//!
//! This exercises `MultiSessionOwner::execute_split` with the production
//! launch preparation, spawn and journal. It is in-process core evidence only:
//! no socket route, runtime driver, CLI or UI reaches this owner.

use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::managed_launch::ManagedLaunchError;
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
use panelink::session_registry::RegistryError;
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
const LAUNCH_LEASE: u64 = 9_101;
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 81;
const PANE_COLUMNS: u16 = 40;
/// Prints READY/PID, then answers each line: `SIZE` reports `stty size`;
/// anything else is acknowledged with the child's own PID.
/// READY also reports `$ZSH_VERSION` (empty under `/bin/sh`) to prove the shell.
const CHILD_SCRIPT: &[u8] = b"printf 'P3SP-SHELL-zsh%s-END\\n' \"${ZSH_VERSION:+-yes}\"; printf 'P3SP-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; while IFS= read -r token; do if [ \"$token\" = SIZE ]; then printf 'P3SP-SIZE-%s-PID-%s-END\\n' \"$(stty size | tr ' ' x)\" \"$$\"; else printf 'P3SP-ACK-%s-PID-%s-END\\n' \"$token\" \"$$\"; fi; done";
const SCENARIO_DEADLINE: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateFixture(PathBuf);

impl PrivateFixture {
    fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp")
                .join(format!("panelink-p3-split-{}-{serial}", std::process::id()));
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
    generation: u64,
    sequence: u64,
    (target, revision): (TargetRef, u64),
    (direction, rows, columns): (SplitDirection, u16, u16),
    shell: Shell,
    context: &LaunchContext,
    marker: &str,
) -> Vec<u8> {
    let arguments = shell_arguments(shell, CHILD_SCRIPT, marker);
    let environment = context.environment();
    let split_key = key(generation, sequence);
    let launch = LaunchRequest {
        key: split_key,
        rows,
        columns,
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
            direction,
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
    assert_eq!(result, -1, "owner drop should reap its retained child");
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

#[test]
fn zsh_split_right_creates_one_independent_real_child_and_resizes_the_original() {
    split_scenario(ZSH);
}

#[test]
fn sh_split_right_creates_one_independent_real_child_and_resizes_the_original() {
    split_scenario(SH);
}

fn split_scenario(shell: Shell) {
    let fixture = PrivateFixture::new().expect("create atomic mode-0700 private fixture");
    let context = LaunchContext::new();
    assert!(
        fs::read_dir(context._directory.path())
            .expect("list private HOME")
            .next()
            .is_none(),
        "HOME/ZDOTDIR is empty, so no rc file can be sourced"
    );
    // The storage lock stays alive for the owner's whole lifetime.
    let mut storage = ServerStorage::initialize(fixture.path()).expect("initialize launch journal");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).expect("create owner ledger");
    ledger
        .open_lease(LAUNCH_LEASE, NOW)
        .expect("open reserved internal launch lease");
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true)
        .expect("assemble input-enabled owner");

    let first_name = "split-first";
    let first = {
        let mut journal = storage.journal().expect("open launch journal");
        owner
            .execute_launch(
                &launch_payload(
                    key(generation, 1),
                    shell,
                    (ROOT_ROWS, ROOT_COLUMNS),
                    &context,
                    CHILD_SCRIPT,
                    first_name,
                ),
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch first child")
    };
    assert_eq!(first.outcome, Outcome::Applied);
    let first_target = first.target.expect("first launch has a full target");
    let layout = owner
        .layout_snapshot()
        .expect("first launch establishes layout");
    assert_eq!(
        (
            layout.revision,
            layout.rows,
            layout.columns,
            layout.pane_count()
        ),
        (1, ROOT_ROWS, ROOT_COLUMNS, 1)
    );

    let mut first_client = connect_and_claim(&mut owner, first_target, 301);
    let first_ready = format!("P3SP-READY-{first_name}");
    let second_name = "split-second";
    let second_ready = format!("P3SP-READY-{second_name}");
    let screen = wait_for(&mut owner, &mut first_client, &first_ready, &[]);
    let first_pid = extract_pid(&screen, &first_ready).expect("first READY carries PID");
    assert_eq!(
        screen.contains("P3SP-SHELL-zsh-yes-END"),
        shell.executable == ZSH.executable,
        "the child is the requested shell"
    );

    let second_payload = split_payload(
        generation,
        2,
        (first_target, layout.revision),
        (SplitDirection::Right, ROOT_ROWS - 2, PANE_COLUMNS - 2),
        shell,
        &context,
        second_name,
    );
    let split = {
        let mut journal = storage.journal().expect("open launch journal");
        owner
            .execute_split(
                &second_payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
                resize,
            )
            .expect("split right through the production launch path")
    };
    assert_eq!(split.outcome, Outcome::Applied);
    assert_eq!(split.committed_layout_revision, 2);
    assert_eq!(split.existing_target, first_target);
    let second_target = split.new_target.expect("split assigns the new target");
    assert_ne!(second_target, first_target);
    assert_eq!(
        pane_rect(&owner, first_target),
        Rect {
            x: 0,
            y: 0,
            rows: ROOT_ROWS,
            columns: PANE_COLUMNS
        }
    );
    assert_eq!(
        pane_rect(&owner, second_target),
        Rect {
            x: PANE_COLUMNS + 1,
            y: 0,
            rows: ROOT_ROWS,
            columns: PANE_COLUMNS
        }
    );

    let mut second_client = connect_and_claim(&mut owner, second_target, 302);
    let screen = wait_for(
        &mut owner,
        &mut second_client,
        &second_ready,
        &[&first_ready],
    );
    let second_pid = extract_pid(&screen, &second_ready).expect("second READY carries PID");
    assert_eq!(
        screen.contains("P3SP-SHELL-zsh-yes-END"),
        shell.executable == ZSH.executable,
        "the split child is the requested shell"
    );
    assert_ne!(first_pid, second_pid);
    assert_process_alive(first_pid);

    // The original is still the same process and still answers, only on itself.
    let first_ack = format!("P3SP-ACK-T1-PID-{first_pid}-END");
    let second_ack = format!("P3SP-ACK-T2-PID-{second_pid}-END");
    submit(&mut owner, &mut first_client, "T1");
    wait_for(
        &mut owner,
        &mut first_client,
        &first_ack,
        &[&second_ready, "P3SP-ACK-T2"],
    );
    let second_screen = read_screen(&mut owner, &mut second_client);
    assert!(!second_screen.contains("P3SP-ACK-T1"));
    submit(&mut owner, &mut second_client, "T2");
    wait_for(
        &mut owner,
        &mut second_client,
        &second_ack,
        &[&first_ready, "P3SP-ACK-T1"],
    );
    let first_screen = read_screen(&mut owner, &mut first_client);
    assert!(!first_screen.contains("P3SP-ACK-T2"));

    // Each child's own PTY reports its inset content rectangle.
    let expected_size = format!("P3SP-SIZE-{}x{}", ROOT_ROWS - 2, PANE_COLUMNS - 2);
    submit(&mut owner, &mut first_client, "SIZE");
    let screen = wait_for(&mut owner, &mut first_client, &expected_size, &[]);
    assert_eq!(extract_pid(&screen, &expected_size), Some(first_pid));
    submit(&mut owner, &mut second_client, "SIZE");
    let screen = wait_for(&mut owner, &mut second_client, &expected_size, &[]);
    assert_eq!(extract_pid(&screen, &expected_size), Some(second_pid));

    // A lost acknowledgment replays the exact payload: same result, no effects.
    let retry = {
        let mut journal = storage.journal().expect("open launch journal");
        owner
            .execute_split(
                &second_payload,
                NOW,
                &mut journal,
                |_, _| -> Result<PreparedLaunch, ()> { panic!("exact retry must not prepare") },
                |_| -> Result<PtyEffects, ()> { panic!("exact retry must not spawn") },
                |_: &mut PtyEffects, _, _| -> Result<(), ()> {
                    panic!("exact retry must not resize")
                },
            )
            .expect("exact retry returns the retained split result")
    };
    assert_eq!(retry, split);
    assert_eq!(
        owner.layout_snapshot().map(|layout| layout.revision),
        Some(2)
    );
    // Neither ACK was replayed by the retry.
    owner.poll(NOW).expect("poll after retry");
    let first_screen = read_screen(&mut owner, &mut first_client);
    let second_screen = read_screen(&mut owner, &mut second_client);
    assert_eq!(first_screen.match_indices(&first_ack).count(), 1);
    assert_eq!(second_screen.match_indices(&second_ack).count(), 1);

    // A layout-owned root resize reaches both children at their new rectangles.
    let grown = owner
        .resize_layout(2, 30, 101, resize)
        .expect("resize the layout root and both PTYs");
    assert_eq!(grown.revision, 3);
    let expected_size = "P3SP-SIZE-28x48";
    submit(&mut owner, &mut first_client, "SIZE");
    let screen = wait_for(&mut owner, &mut first_client, expected_size, &[]);
    assert_eq!(extract_pid(&screen, expected_size), Some(first_pid));
    submit(&mut owner, &mut second_client, "SIZE");
    let screen = wait_for(&mut owner, &mut second_client, expected_size, &[]);
    assert_eq!(extract_pid(&screen, expected_size), Some(second_pid));

    // Too small for both panes: rejected, layout and children unchanged.
    assert!(owner.resize_layout(3, 30, 40, resize).is_err());
    assert_eq!(
        owner.layout_snapshot().map(|layout| layout.revision),
        Some(3)
    );

    // Fill the remaining capacity (SESSION_CAPACITY = MAX_PANES) with real
    // children by splitting each 30x50 half down (feasible: 29 usable rows
    // give a 14-row original and a 15-row new pane), then prove the next new
    // identity is refused before prepare or spawn.
    let capacity = panelink::session_registry::SESSION_CAPACITY as u64;
    let split_sources = [first_target, second_target];
    assert!(
        capacity as usize <= 2 + split_sources.len(),
        "extend the split sources if capacity grows"
    );
    let mut layout_revision = grown.revision;
    let mut extra_pids = Vec::new();
    for sequence in 3..=capacity {
        let source = split_sources[(sequence - 3) as usize];
        let name = format!("split-extra-{sequence}");
        let ready = format!("P3SP-READY-{name}");
        let payload = split_payload(
            generation,
            sequence,
            (source, layout_revision),
            (SplitDirection::Down, 13, 48),
            shell,
            &context,
            &name,
        );
        let extra = {
            let mut journal = storage.journal().expect("open launch journal");
            owner
                .execute_split(
                    &payload,
                    NOW,
                    &mut journal,
                    |request, target| prepare(request, target, fixture.path()),
                    spawn,
                    resize,
                )
                .expect("split down within capacity through the production launch path")
        };
        assert_eq!(extra.outcome, Outcome::Applied);
        assert_eq!(extra.committed_layout_revision, layout_revision + 1);
        assert_eq!(extra.existing_target, source);
        layout_revision = extra.committed_layout_revision;
        let extra_target = extra.new_target.expect("split assigns the new target");
        let mut extra_client = connect_and_claim(&mut owner, extra_target, 303 + sequence);
        let screen = wait_for(&mut owner, &mut extra_client, &ready, &[&first_ready]);
        extra_pids.push(extract_pid(&screen, &ready).expect("extra READY carries PID"));
    }
    assert_eq!(
        owner.layout_snapshot().map(|layout| layout.pane_count()),
        Some(capacity as usize)
    );

    let refused = {
        let mut journal = storage.journal().expect("open launch journal");
        owner.execute_launch(
            &launch_payload(
                key(generation, capacity + 1),
                shell,
                (ROOT_ROWS, PANE_COLUMNS),
                &context,
                b"exit 0",
                "over-capacity",
            ),
            NOW,
            &mut journal,
            |_, _| -> Result<PreparedLaunch, ()> { panic!("capacity precedes prepare") },
            |_| -> Result<PtyEffects, ()> { panic!("capacity precedes spawn") },
        )
    };
    assert!(matches!(
        refused,
        Err(ManagedLaunchError::Registry(RegistryError::Capacity))
    ));

    assert_process_alive(first_pid);
    assert_process_alive(second_pid);
    for pid in &extra_pids {
        assert_process_alive(*pid);
    }
    drop(owner);
    assert_process_reaped(first_pid);
    assert_process_reaped(second_pid);
    for pid in &extra_pids {
        assert_process_reaped(*pid);
    }
    drop(storage);
}
