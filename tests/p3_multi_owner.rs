#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::handshake_protocol::{Hello, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::managed_launch::ManagedLaunchError;
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use panelink::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::scoped_watch_protocol::{
    decode_reconciliation as decode_scoped_reconciliation,
    encode_reconciliation as encode_scoped_reconciliation, Reconciliation as ScopedReconciliation,
    SessionCursor, RECONCILIATION_PREFIX_BYTES,
};
use panelink::server::multi_session::MultiSessionOwner;
use panelink::server::{ServerError, SessionReply};
use panelink::server_pty::PtyEffects;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{SessionHello, SessionRequest};
use panelink::session_registry::RegistryError;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const NOW: u64 = 100;
const LAUNCH_LEASE: u64 = 9_001;
const CHILD_SCRIPT: &[u8] =
    b"printf 'P3MO-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; exec /bin/sleep 60";
const INPUT_CHILD_SCRIPT: &[u8] = b"printf 'P3MI-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; count=0; while [ \"$count\" -lt 2 ]; do IFS= read -r token || exit 1; count=$((count + 1)); printf 'P3MI-ACK-%s-PID-%s-COUNT-%s-END\\n' \"$token\" \"$$\" \"$count\"; done; exec /bin/sleep 60";
const SCENARIO_DEADLINE: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateFixture(PathBuf);

impl PrivateFixture {
    fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp").join(format!(
                "panelink-p3-multi-owner-{}-{serial}",
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

fn launch_payload(generation: u64, sequence: u64, cwd: &Path, marker: &str) -> Vec<u8> {
    launch_payload_with_script(generation, sequence, cwd, marker, CHILD_SCRIPT)
}

fn launch_payload_with_script(
    generation: u64,
    sequence: u64,
    cwd: &Path,
    marker: &str,
    script: &[u8],
) -> Vec<u8> {
    let arguments: [&[u8]; 4] = [
        b"-c",
        script,
        b"panelink-multi-owner-test",
        marker.as_bytes(),
    ];
    let environment = [
        EnvironmentEntry {
            name: b"TERM",
            value: b"panelink-probe",
        },
        EnvironmentEntry {
            name: b"PATH",
            value: b"/usr/bin:/bin",
        },
    ];
    let request = LaunchRequest {
        key: OperationKey {
            generation,
            lease: LAUNCH_LEASE,
            sequence,
        },
        rows: 10,
        columns: 100,
        requested_executable: b"/bin/sh",
        requested_working_directory: cwd.as_os_str().as_bytes(),
        client_working_directory: cwd.as_os_str().as_bytes(),
        arguments: &arguments,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_launch_request(&request, &mut payload).expect("encode explicit launch fixture");
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

fn fresh_read_hello(nonce: u64) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: 0,
        },
        resume: None,
    }
}

fn fresh_input_hello(nonce: u64) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    }
}

fn fresh_events_hello(nonce: u64) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
            optional_capabilities: 0,
        },
        resume: None,
    }
}

fn encoded_read(target: TargetRef, key: OperationKey) -> Vec<u8> {
    let request = ControlRequest {
        key,
        target,
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 3_000,
            max_lines: 10,
        },
    };
    let mut payload = Vec::new();
    encode_request_payload(&request, &mut payload).expect("encode bounded read control request");
    payload
}

fn encoded_submit(
    target: TargetRef,
    key: OperationKey,
    owner_revision: u64,
    text: &str,
) -> Vec<u8> {
    let request = ControlRequest {
        key,
        target,
        command: Command::Submit {
            owner_revision,
            text,
        },
    };
    let mut payload = Vec::new();
    encode_request_payload(&request, &mut payload).expect("encode bounded submit request");
    payload
}

fn describe(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
) -> panelink::session_protocol::SessionInfo {
    let payload = panelink::session_protocol::encode_session_request(&SessionRequest::Describe)
        .expect("encode target-pinned Describe");
    match owner
        .handle_session(connection, &payload, NOW)
        .expect("dispatch target-pinned Describe")
    {
        SessionReply::Info(info) => info,
        other => panic!("expected Describe info, got {other:?}"),
    }
}

fn claim_input(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
    target: TargetRef,
    sequence: &mut u64,
) {
    let info = describe(owner, connection);
    assert_eq!(info.target, target);
    assert_eq!(
        info.owner_lease, 0,
        "new child begins without an input owner"
    );
    *sequence += 1;
    let payload = panelink::session_protocol::encode_session_request(&SessionRequest::Claim {
        key: OperationKey {
            generation: target.generation,
            lease: connection.lease(),
            sequence: *sequence,
        },
        target,
        expected_owner_revision: info.owner_revision,
        ttl_seconds: 30,
    })
    .expect("encode target-scoped Claim");
    let reply = owner
        .handle_session(connection, &payload, NOW)
        .expect("claim input through public session route");
    let SessionReply::Payload(result) = reply else {
        panic!("Claim returns an encoded operation result");
    };
    assert_eq!(
        decode_response_payload(&result)
            .expect("decode Claim result")
            .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let claimed = describe(owner, connection);
    assert_eq!(claimed.target, target);
    assert_eq!(claimed.owner_lease, connection.lease());
    assert!(claimed.owner_revision > info.owner_revision);
}

fn submit_once(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
    target: TargetRef,
    owner_revision: u64,
    sequence: &mut u64,
    token: &str,
) -> (Vec<u8>, Vec<u8>) {
    *sequence += 1;
    let payload = encoded_submit(
        target,
        OperationKey {
            generation: target.generation,
            lease: connection.lease(),
            sequence: *sequence,
        },
        owner_revision,
        token,
    );
    let response = owner
        .handle_control(connection, &payload, NOW)
        .expect("dispatch encoded Submit on the selected target");
    assert_eq!(
        decode_response_payload(&response)
            .expect("decode Submit result")
            .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    (payload, response)
}

fn read_screen(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
    target: TargetRef,
    sequence: &mut u64,
) -> String {
    *sequence += 1;
    let key = OperationKey {
        generation: target.generation,
        lease: connection.lease(),
        sequence: *sequence,
    };
    let payload = encoded_read(target, key);
    let response = owner
        .handle_control(connection, &payload, NOW)
        .expect("dispatch encoded control read through owner");
    match decode_response_payload(&response)
        .expect("decode bounded owner control response")
        .body
    {
        ResponseBody::Read {
            target: actual,
            text,
            ..
        } => {
            assert_eq!(actual, target);
            text.to_owned()
        }
        other => panic!("expected screen response, got {other:?}"),
    }
}

fn wait_for_marker(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
    target: TargetRef,
    marker: &str,
    forbidden: &str,
    sequence: &mut u64,
) -> (String, i32) {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        owner
            .poll(NOW)
            .expect("poll each retained PTY once per owner tick");
        let screen = read_screen(owner, connection, target, sequence);
        assert!(
            !screen.contains(forbidden),
            "a target's screen must not contain its sibling's marker"
        );
        if let Some(pid) = extract_pid(&screen, marker) {
            return (screen, pid);
        }
        assert!(
            Instant::now() < deadline,
            "child READY sentinel missed deadline"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_text(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &panelink::server::Connection,
    target: TargetRef,
    required: &str,
    forbidden: &str,
    sequence: &mut u64,
) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        owner.poll(NOW).expect("poll both retained child effects");
        let screen = read_screen(owner, connection, target, sequence);
        assert!(
            !screen.contains(forbidden),
            "input acknowledgment must stay on its selected target"
        );
        if screen.contains(required) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "child acknowledgment missed deadline"
        );
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

fn assert_process_alive(pid: i32) {
    // SAFETY: signal zero only checks whether this PID exists and is visible.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(
        result, 0,
        "both children remain owned and live before owner drop"
    );
}

#[test]
fn trusted_multi_owner_routes_reads_and_drops_both_real_children() {
    let fixture = PrivateFixture::new().expect("create atomic mode-0700 private fixture");
    let mut storage = ServerStorage::initialize(fixture.path()).expect("initialize launch journal");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).expect("create owner ledger");
    ledger
        .open_lease(LAUNCH_LEASE, NOW)
        .expect("open reserved internal launch lease");
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, false)
        .expect("assemble one owner with the launch lease");

    let first_name = "owner-first";
    let first_payload = launch_payload(generation, 1, fixture.path(), first_name);
    let first = {
        let mut journal = storage.journal().expect("open protected launch journal");
        owner
            .execute_launch(
                &first_payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch first child through prepared spawn")
    };
    assert_eq!(first.outcome, Outcome::Applied);
    let first_target = first.target.expect("first launch has a full target");

    let retry = {
        let mut journal = storage.journal().expect("reopen protected launch journal");
        owner
            .execute_launch(
                &first_payload,
                NOW,
                &mut journal,
                |_, _| -> Result<PreparedLaunch, ()> { panic!("exact retry must not prepare") },
                |_| -> Result<PtyEffects, ()> { panic!("exact retry must not spawn") },
            )
            .expect("exact lost-result retry returns retained launch result")
    };
    assert_eq!(retry, first);

    let second_name = "owner-second";
    let second_payload = launch_payload(generation, 2, fixture.path(), second_name);
    let second = {
        let mut journal = storage.journal().expect("open protected launch journal");
        owner
            .execute_launch(
                &second_payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch second child through prepared spawn")
    };
    assert_eq!(second.outcome, Outcome::Applied);
    let second_target = second.target.expect("second launch has a full target");
    assert_ne!(first_target, second_target);

    let (first_connection, first_welcome) = owner
        .connect(first_target, &fresh_read_hello(101), NOW)
        .expect("connect a fresh read lease pinned to target one");
    let (second_connection, second_welcome) = owner
        .connect(second_target, &fresh_read_hello(102), NOW)
        .expect("connect a fresh read lease pinned to target two");
    assert_ne!(first_welcome.lease, second_welcome.lease);

    let mut first_sequence = 0;
    let mut second_sequence = 0;
    let first_marker = format!("P3MO-READY-{first_name}");
    let second_marker = format!("P3MO-READY-{second_name}");
    let (first_screen, first_pid) = wait_for_marker(
        &mut owner,
        &first_connection,
        first_target,
        &first_marker,
        &second_marker,
        &mut first_sequence,
    );
    let (second_screen, second_pid) = wait_for_marker(
        &mut owner,
        &second_connection,
        second_target,
        &second_marker,
        &first_marker,
        &mut second_sequence,
    );
    assert!(first_screen.contains(&format!("{first_marker}-PID-{first_pid}-END")));
    assert!(second_screen.contains(&format!("{second_marker}-PID-{second_pid}-END")));
    assert_ne!(first_pid, second_pid);

    owner
        .poll(NOW)
        .expect("poll both live children after READY");
    let first_again = read_screen(
        &mut owner,
        &first_connection,
        first_target,
        &mut first_sequence,
    );
    assert!(first_again.contains(&first_marker));
    assert!(!first_again.contains(&second_marker));
    let second_again = read_screen(
        &mut owner,
        &second_connection,
        second_target,
        &mut second_sequence,
    );
    assert!(second_again.contains(&second_marker));
    assert!(!second_again.contains(&first_marker));

    assert_eq!(first_welcome.selected_capabilities, CAPABILITY_READ);
    assert_eq!(second_welcome.selected_capabilities, CAPABILITY_READ);
    assert!(
        matches!(
            owner.reconcile(&first_connection, first_target, 3_000, 10, NOW),
            Err(ServerError::Unauthorized)
        ),
        "existing READ-only grants do not gain event scope"
    );

    let (first_events_connection, first_events_welcome) = owner
        .connect(first_target, &fresh_events_hello(103), NOW)
        .expect("create new READ|EVENTS lease for the first target");
    let (second_events_connection, second_events_welcome) = owner
        .connect(second_target, &fresh_events_hello(104), NOW)
        .expect("create new READ|EVENTS lease for the second target");
    assert_eq!(
        first_events_welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_EVENTS
    );
    assert_eq!(
        second_events_welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_EVENTS
    );
    assert_ne!(first_events_connection.lease(), first_connection.lease());
    assert_ne!(second_events_connection.lease(), second_connection.lease());

    const SNAPSHOT_BUDGET: u32 = 3_000;
    const SNAPSHOT_LINES: u16 = 10;
    let first_scoped = owner
        .reconcile(
            &first_events_connection,
            first_target,
            SNAPSHOT_BUDGET,
            SNAPSHOT_LINES,
            NOW,
        )
        .expect("reconcile the first target through scoped owner API");
    let second_scoped = owner
        .reconcile(
            &second_events_connection,
            second_target,
            SNAPSHOT_BUDGET,
            SNAPSHOT_LINES,
            NOW,
        )
        .expect("reconcile the second target through scoped owner API");
    for (state, target, marker, sibling, pid) in [
        (
            &first_scoped,
            first_target,
            first_marker.as_str(),
            second_marker.as_str(),
            first_pid,
        ),
        (
            &second_scoped,
            second_target,
            second_marker.as_str(),
            first_marker.as_str(),
            second_pid,
        ),
    ] {
        assert_eq!(state.info.target, target);
        assert_eq!(state.cursor.generation, generation);
        assert_eq!(state.cursor.session, target.session);
        assert!(state.text.contains(&format!("{marker}-PID-{pid}-END")));
        assert!(!state.text.contains(sibling));
        assert_eq!(RECONCILIATION_PREFIX_BYTES, 128);
        let encoded = encode_scoped_reconciliation(&ScopedReconciliation {
            info: state.info,
            cursor: state.cursor,
            screen_revision: state.screen_revision,
            truncated: state.truncated,
            text: &state.text,
        })
        .expect("encode returned typed state using the actual schema-5 codec");
        assert!(encoded.len() <= SNAPSHOT_BUDGET as usize);
        assert_eq!(
            encoded.len(),
            RECONCILIATION_PREFIX_BYTES + state.text.len(),
            "schema-5 text starts after the existing 128-byte prefix"
        );
        let decoded =
            decode_scoped_reconciliation(&encoded).expect("schema-5 reconciliation round-trips");
        assert_eq!(decoded.info, state.info);
        assert_eq!(decoded.cursor, state.cursor);
        assert_eq!(decoded.screen_revision, state.screen_revision);
        assert_eq!(decoded.truncated, state.truncated);
        assert_eq!(decoded.text, state.text);
    }

    for (connection, target, state) in [
        (&first_events_connection, first_target, &first_scoped),
        (&second_events_connection, second_target, &second_scoped),
    ] {
        let current = owner
            .read_events(connection, target, state.cursor, 64, NOW)
            .expect("read target's scoped event page at its current cursor");
        assert_eq!(current.cursor, state.cursor);
        assert!(!current.resync_required);
        assert!(current.events.iter().all(|event| event.target == target));

        let initial_cursor = SessionCursor {
            generation,
            session: target.session,
            sequence: 0,
        };
        let history = owner
            .read_events(connection, target, initial_cursor, 64, NOW)
            .expect("read retained metadata from this target's initial cursor");
        assert_eq!(history.cursor.generation, generation);
        assert_eq!(history.cursor.session, target.session);
        assert!(history.events.iter().all(|event| event.target == target));
        assert!(
            !history.events.is_empty(),
            "READY output must have produced target-local screen metadata"
        );
    }
    assert!(
        matches!(
            owner.read_events(
                &second_events_connection,
                second_target,
                first_scoped.cursor,
                64,
                NOW,
            ),
            Err(ServerError::TargetStale)
        ),
        "a cursor names its session even when both event journals have progress"
    );
    assert!(
        matches!(
            owner.reconcile(
                &first_events_connection,
                second_target,
                SNAPSHOT_BUDGET,
                SNAPSHOT_LINES,
                NOW,
            ),
            Err(ServerError::Unauthorized)
        ),
        "an Events-capable lease stays pinned to its original target"
    );

    let cross_key = OperationKey {
        generation,
        lease: first_connection.lease(),
        sequence: first_sequence + 1,
    };
    let cross_payload = encoded_read(second_target, cross_key);
    let cross_response = owner
        .handle_control(&first_connection, &cross_payload, NOW)
        .expect("cross-target read is returned as a fixed rejection");
    assert_eq!(
        decode_response_payload(&cross_response)
            .expect("decode cross-target rejection")
            .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    let status = owner
        .operation_status(&first_connection, cross_key, NOW)
        .expect("inspect denied operation through global status");
    assert_eq!(
        decode_response_payload(&status)
            .expect("decode operation status")
            .body,
        ResponseBody::Status(OperationStatus::NotAdmitted)
    );

    // Fill the remaining capacity (SESSION_CAPACITY = MAX_PANES) with real
    // children; the next new identity must then be refused before prepare.
    let capacity = panelink::session_registry::SESSION_CAPACITY as u64;
    let mut extra_pids = Vec::new();
    for sequence in 3..=capacity {
        let name = format!("owner-extra-{sequence}");
        let payload = launch_payload(generation, sequence, fixture.path(), &name);
        let launched = {
            let mut journal = storage.journal().expect("open protected launch journal");
            owner
                .execute_launch(
                    &payload,
                    NOW,
                    &mut journal,
                    |request, target| prepare(request, target, fixture.path()),
                    spawn,
                )
                .expect("launch extra child within capacity")
        };
        assert_eq!(launched.outcome, Outcome::Applied);
        let extra_target = launched.target.expect("extra launch has a full target");
        let (extra_connection, _) = owner
            .connect(extra_target, &fresh_read_hello(100 + sequence), NOW)
            .expect("connect a fresh read lease pinned to the extra target");
        let extra_marker = format!("P3MO-READY-{name}");
        let mut extra_sequence = 0;
        let (_, extra_pid) = wait_for_marker(
            &mut owner,
            &extra_connection,
            extra_target,
            &extra_marker,
            &first_marker,
            &mut extra_sequence,
        );
        extra_pids.push(extra_pid);
    }

    let third_payload = launch_payload(generation, capacity + 1, fixture.path(), "owner-over");
    let third_result = {
        let mut journal = storage.journal().expect("open protected launch journal");
        owner.execute_launch(
            &third_payload,
            NOW,
            &mut journal,
            |_, _| -> Result<PreparedLaunch, ()> { panic!("capacity rejection precedes prepare") },
            |_| -> Result<PtyEffects, ()> { panic!("capacity rejection precedes spawn") },
        )
    };
    assert!(
        matches!(
            third_result,
            Err(ManagedLaunchError::Registry(RegistryError::Capacity))
        ),
        "launch beyond SESSION_CAPACITY must fail at the target cap"
    );

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

#[test]
fn public_claim_submit_routes_once_and_exact_retry_does_not_replay_input() {
    let fixture = PrivateFixture::new().expect("create atomic mode-0700 private fixture");
    let mut storage = ServerStorage::initialize(fixture.path()).expect("initialize launch journal");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).expect("create owner ledger");
    ledger
        .open_lease(LAUNCH_LEASE, NOW)
        .expect("open reserved internal launch lease");
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true)
        .expect("assemble input-enabled owner with launch lease");

    let first_name = "input-first";
    let first_payload = launch_payload_with_script(
        generation,
        1,
        fixture.path(),
        first_name,
        INPUT_CHILD_SCRIPT,
    );
    let first = {
        let mut journal = storage.journal().expect("open protected launch journal");
        owner
            .execute_launch(
                &first_payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch first input child through prepared spawn")
    };
    assert_eq!(first.outcome, Outcome::Applied);
    let first_target = first.target.expect("first launch has a full target");

    let second_name = "input-second";
    let second_payload = launch_payload_with_script(
        generation,
        2,
        fixture.path(),
        second_name,
        INPUT_CHILD_SCRIPT,
    );
    let second = {
        let mut journal = storage.journal().expect("open protected launch journal");
        owner
            .execute_launch(
                &second_payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch second input child through prepared spawn")
    };
    assert_eq!(second.outcome, Outcome::Applied);
    let second_target = second.target.expect("second launch has a full target");
    assert_ne!(first_target, second_target);

    let (first_connection, first_welcome) = owner
        .connect(first_target, &fresh_input_hello(201), NOW)
        .expect("connect read/input lease pinned to first target");
    let (second_connection, second_welcome) = owner
        .connect(second_target, &fresh_input_hello(202), NOW)
        .expect("connect read/input lease pinned to second target");
    assert_eq!(
        first_welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_INPUT
    );
    assert_eq!(
        second_welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_INPUT
    );

    let mut first_sequence = 0;
    let mut second_sequence = 0;
    let first_ready = format!("P3MI-READY-{first_name}");
    let second_ready = format!("P3MI-READY-{second_name}");
    let (first_screen, first_pid) = wait_for_marker(
        &mut owner,
        &first_connection,
        first_target,
        &first_ready,
        &second_ready,
        &mut first_sequence,
    );
    let (second_screen, second_pid) = wait_for_marker(
        &mut owner,
        &second_connection,
        second_target,
        &second_ready,
        &first_ready,
        &mut second_sequence,
    );
    assert!(first_screen.contains(&format!("{first_ready}-PID-{first_pid}-END")));
    assert!(second_screen.contains(&format!("{second_ready}-PID-{second_pid}-END")));
    assert_ne!(first_pid, second_pid);

    claim_input(
        &mut owner,
        &first_connection,
        first_target,
        &mut first_sequence,
    );
    claim_input(
        &mut owner,
        &second_connection,
        second_target,
        &mut second_sequence,
    );

    let first_token_one = "P3MI-TOKEN-FIRST-ONE";
    let first_ack_one = format!("P3MI-ACK-{first_token_one}-PID-{first_pid}-COUNT-1-END");
    let first_owner = describe(&mut owner, &first_connection).owner_revision;
    let (first_submit_one, first_submit_result) = submit_once(
        &mut owner,
        &first_connection,
        first_target,
        first_owner,
        &mut first_sequence,
        first_token_one,
    );
    let first_ack_screen = wait_for_text(
        &mut owner,
        &first_connection,
        first_target,
        &first_ack_one,
        "P3MI-ACK-P3MI-TOKEN-SECOND-ONE",
        &mut first_sequence,
    );
    assert_eq!(first_ack_screen.match_indices(&first_ack_one).count(), 1);
    let second_screen_without_input = read_screen(
        &mut owner,
        &second_connection,
        second_target,
        &mut second_sequence,
    );
    assert!(!second_screen_without_input.contains("P3MI-ACK-P3MI-TOKEN-FIRST-ONE"));

    let second_token_one = "P3MI-TOKEN-SECOND-ONE";
    let second_ack_one = format!("P3MI-ACK-{second_token_one}-PID-{second_pid}-COUNT-1-END");
    let second_owner = describe(&mut owner, &second_connection).owner_revision;
    let (second_submit_one, second_submit_result) = submit_once(
        &mut owner,
        &second_connection,
        second_target,
        second_owner,
        &mut second_sequence,
        second_token_one,
    );
    let second_ack_screen = wait_for_text(
        &mut owner,
        &second_connection,
        second_target,
        &second_ack_one,
        &first_ack_one,
        &mut second_sequence,
    );
    assert_eq!(second_ack_screen.match_indices(&second_ack_one).count(), 1);
    let first_screen_after_second_submit = read_screen(
        &mut owner,
        &first_connection,
        first_target,
        &mut first_sequence,
    );
    assert!(!first_screen_after_second_submit.contains(&second_ack_one));

    let first_retry = owner
        .handle_control(&first_connection, &first_submit_one, NOW)
        .expect("exact first Submit retry returns cached result");
    let second_retry = owner
        .handle_control(&second_connection, &second_submit_one, NOW)
        .expect("exact second Submit retry returns cached result");
    assert_eq!(first_retry, first_submit_result);
    assert_eq!(second_retry, second_submit_result);

    owner
        .poll(NOW)
        .expect("poll both children after cached retries");
    let first_after_retry = read_screen(
        &mut owner,
        &first_connection,
        first_target,
        &mut first_sequence,
    );
    let second_after_retry = read_screen(
        &mut owner,
        &second_connection,
        second_target,
        &mut second_sequence,
    );
    assert_eq!(first_after_retry.match_indices(&first_ack_one).count(), 1);
    assert_eq!(second_after_retry.match_indices(&second_ack_one).count(), 1);

    let first_token_two = "P3MI-TOKEN-FIRST-TWO";
    let first_ack_two = format!("P3MI-ACK-{first_token_two}-PID-{first_pid}-COUNT-2-END");
    let second_token_two = "P3MI-TOKEN-SECOND-TWO";
    let second_ack_two = format!("P3MI-ACK-{second_token_two}-PID-{second_pid}-COUNT-2-END");
    let first_owner_two = describe(&mut owner, &first_connection).owner_revision;
    let (_, _) = submit_once(
        &mut owner,
        &first_connection,
        first_target,
        first_owner_two,
        &mut first_sequence,
        first_token_two,
    );
    let first_count_two = wait_for_text(
        &mut owner,
        &first_connection,
        first_target,
        &first_ack_two,
        &second_ack_two,
        &mut first_sequence,
    );
    assert_eq!(first_count_two.match_indices(&first_ack_one).count(), 1);
    assert_eq!(first_count_two.match_indices(&first_ack_two).count(), 1);
    assert!(!first_count_two.contains("COUNT-3-END"));

    let second_owner_two = describe(&mut owner, &second_connection).owner_revision;
    let (_, _) = submit_once(
        &mut owner,
        &second_connection,
        second_target,
        second_owner_two,
        &mut second_sequence,
        second_token_two,
    );
    let second_count_two = wait_for_text(
        &mut owner,
        &second_connection,
        second_target,
        &second_ack_two,
        &first_ack_two,
        &mut second_sequence,
    );
    assert_eq!(second_count_two.match_indices(&second_ack_one).count(), 1);
    assert_eq!(second_count_two.match_indices(&second_ack_two).count(), 1);
    assert!(!second_count_two.contains("COUNT-3-END"));

    drop(owner);
    assert_process_reaped(first_pid);
    assert_process_reaped(second_pid);
    drop(storage);
}
