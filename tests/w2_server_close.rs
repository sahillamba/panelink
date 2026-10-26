#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Two real PTYs prove that an HUP-ignoring close can progress while its
//! sibling continues to receive input and produce screen output.

use panelink::close_protocol::{encode_close_request, CloseRequest, StopOutcome};
use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::managed_launch::LaunchJournalPort;
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::server::multi_session::{CloseStatus, MultiSessionOwner};
use panelink::server::{Connection, SessionReply};
use panelink::server_pty::PtyEffects;
use panelink::session_protocol::{SessionHello, SessionRequest};
use panelink::split_protocol::{encode_split_request, SplitRequest};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const NOW: u64 = 10;
const LAUNCH_LEASE: u64 = 1;
const ROWS: u16 = 24;
const COLUMNS: u16 = 81;
const SPLIT_CONTENT_ROWS: u16 = ROWS - 2;
const SPLIT_CONTENT_COLUMNS: u16 = (COLUMNS - 1) / 2 - 2;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = Path::new("/private/tmp").join(format!(
            "panelink-w2-close-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create private fixture");
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Journal(HashSet<u64>);

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _: OperationKey, session: u64) -> io::Result<()> {
        if self.0.contains(&session) {
            Err(io::Error::other("duplicate session"))
        } else {
            Ok(())
        }
    }

    fn record(&mut self, record: panelink::server_storage::LaunchRecord) -> io::Result<()> {
        if record.state == panelink::server_storage::LaunchState::Intent
            && !self.0.insert(record.session)
        {
            return Err(io::Error::other("duplicate session"));
        }
        Ok(())
    }
}

fn env(home: &[u8]) -> [EnvironmentEntry<'_>; 4] {
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
            value: home,
        },
        EnvironmentEntry {
            name: b"ZDOTDIR",
            value: home,
        },
    ]
}

fn encode_launch(
    key: OperationKey,
    dir: &[u8],
    script: &[u8],
    marker: &[u8],
    rows: u16,
    cols: u16,
) -> Vec<u8> {
    let args: [&[u8]; 4] = [b"-c", script, b"panelink-w2", marker];
    let environment = env(dir);
    let request = LaunchRequest {
        key,
        rows,
        columns: cols,
        requested_executable: b"/bin/sh",
        requested_working_directory: dir,
        client_working_directory: dir,
        arguments: &args,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_launch_request(&request, &mut payload).unwrap();
    payload
}

fn key(generation: u64, sequence: u64) -> OperationKey {
    OperationKey {
        generation,
        lease: LAUNCH_LEASE,
        sequence,
    }
}

struct PaneClient {
    connection: Connection,
    target: TargetRef,
    sequence: u64,
}

impl PaneClient {
    fn next_key(&mut self) -> OperationKey {
        self.sequence += 1;
        OperationKey {
            generation: self.target.generation,
            lease: self.connection.lease(),
            sequence: self.sequence,
        }
    }
}

fn input_connection(owner: &mut MultiSessionOwner<PtyEffects>, target: TargetRef) -> PaneClient {
    let hello = SessionHello {
        hello: Hello {
            nonce: target.session,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let (connection, _) = owner.connect(target, &hello, NOW).unwrap();
    let SessionReply::Info(info) = owner
        .handle_session(
            &connection,
            &panelink::session_protocol::encode_session_request(&SessionRequest::Describe).unwrap(),
            NOW,
        )
        .unwrap()
    else {
        panic!("Describe")
    };
    let claim = SessionRequest::Claim {
        key: OperationKey {
            generation: target.generation,
            lease: connection.lease(),
            sequence: 1,
        },
        target,
        expected_owner_revision: info.owner_revision,
        ttl_seconds: 30,
    };
    let SessionReply::Payload(reply) = owner
        .handle_session(
            &connection,
            &panelink::session_protocol::encode_session_request(&claim).unwrap(),
            NOW,
        )
        .unwrap()
    else {
        panic!("Claim")
    };
    assert_eq!(
        decode_response_payload(&reply).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    PaneClient {
        connection,
        target,
        sequence: 1,
    }
}

fn send_and_read(
    owner: &mut MultiSessionOwner<PtyEffects>,
    client: &mut PaneClient,
    text: &str,
) -> String {
    let mut payload = Vec::new();
    let SessionReply::Info(info) = owner
        .handle_session(
            &client.connection,
            &panelink::session_protocol::encode_session_request(&SessionRequest::Describe).unwrap(),
            NOW,
        )
        .unwrap()
    else {
        panic!("Describe")
    };
    encode_request_payload(
        &ControlRequest {
            key: client.next_key(),
            target: client.target,
            command: Command::Submit {
                owner_revision: info.owner_revision,
                text,
            },
        },
        &mut payload,
    )
    .unwrap();
    let reply = owner
        .handle_control(&client.connection, &payload, NOW)
        .unwrap();
    assert!(matches!(
        decode_response_payload(&reply).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    ));
    let marker = format!("W2-SIBLING-ACK-{text}");
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        owner.poll(NOW).unwrap();
        let mut payload = Vec::new();
        encode_request_payload(
            &ControlRequest {
                key: client.next_key(),
                target: client.target,
                command: Command::Read {
                    source: ReadSource::Screen,
                    max_bytes: 8_000,
                    max_lines: 200,
                },
            },
            &mut payload,
        )
        .unwrap();
        let screen = match decode_response_payload(
            &owner
                .handle_control(&client.connection, &payload, NOW)
                .unwrap(),
        )
        .unwrap()
        .body
        {
            ResponseBody::Read { text, .. } => text.to_owned(),
            other => panic!("expected read: {other:?}"),
        };
        if screen.contains(&marker) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "sibling did not produce {marker}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn read_target_screen(
    owner: &mut MultiSessionOwner<PtyEffects>,
    connection: &Connection,
    target: TargetRef,
    sequence: &mut u64,
) -> String {
    *sequence += 1;
    let mut payload = Vec::new();
    encode_request_payload(
        &ControlRequest {
            key: OperationKey {
                generation: target.generation,
                lease: connection.lease(),
                sequence: *sequence,
            },
            target,
            command: Command::Read {
                source: ReadSource::Screen,
                max_bytes: 8_000,
                max_lines: 200,
            },
        },
        &mut payload,
    )
    .unwrap();
    match decode_response_payload(&owner.handle_control(connection, &payload, NOW).unwrap())
        .unwrap()
        .body
    {
        ResponseBody::Read { text, .. } => text.to_owned(),
        other => panic!("expected read: {other:?}"),
    }
}

#[test]
fn hup_ignoring_close_keeps_real_sibling_pty_readable_and_writable() {
    let fixture = Fixture::new();
    let dir = fixture.0.as_os_str().as_encoded_bytes().to_vec();
    let generation = 4401;
    let mut ledger = Ledger::new(generation).unwrap();
    ledger.open_lease(LAUNCH_LEASE, NOW).unwrap();
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true).unwrap();
    let mut journal = Journal::default();
    let terminfo = fixture.0.clone();
    let first_script = b"printf 'W2-SIBLING-READY\\n'; while IFS= read -r line; do printf 'W2-SIBLING-ACK-%s\\n' \"$line\"; done";
    let first_payload = encode_launch(
        key(generation, 1),
        &dir,
        first_script,
        b"first",
        ROWS,
        COLUMNS,
    );
    let first = owner
        .execute_launch(
            &first_payload,
            NOW,
            &mut journal,
            |request, target| prepare_launch(request, target, &terminfo).map_err(|_| ()),
            |prepared: PreparedLaunch| prepared.spawn().map(PtyEffects::new).map_err(|_| ()),
        )
        .unwrap()
        .target
        .unwrap();
    let second_script = b"trap '' HUP; printf 'W2-CLOSE-READY\\n'; while :; do /bin/sleep 10; done";
    let launch = LaunchRequest {
        key: key(generation, 2),
        rows: SPLIT_CONTENT_ROWS,
        columns: SPLIT_CONTENT_COLUMNS,
        requested_executable: b"/bin/sh",
        requested_working_directory: &dir,
        client_working_directory: &dir,
        arguments: &[b"-c", second_script, b"panelink-w2", b"second"],
        environment: &env(&dir),
    };
    let mut split = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: key(generation, 2),
            target: first,
            expected_layout_revision: 1,
            direction: SplitDirection::Right,
            launch: &launch,
        },
        &mut split,
    )
    .unwrap();
    let second = owner
        .execute_split(
            &split,
            NOW,
            &mut journal,
            |request, target| prepare_launch(request, target, &terminfo).map_err(|_| ()),
            |prepared: PreparedLaunch| prepared.spawn().map(PtyEffects::new).map_err(|_| ()),
            |effects: &mut PtyEffects, rows, columns| effects.resize(rows, columns),
        )
        .unwrap()
        .new_target
        .unwrap();
    let (closing_connection, _) = owner
        .connect(
            second,
            &SessionHello {
                hello: Hello {
                    nonce: 889,
                    required_capabilities: CAPABILITY_READ,
                    optional_capabilities: 0,
                },
                resume: None,
            },
            NOW,
        )
        .unwrap();
    let mut close_read_sequence = 0;
    let ready_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < ready_deadline {
        owner.poll(NOW).unwrap();
        if read_target_screen(
            &mut owner,
            &closing_connection,
            second,
            &mut close_read_sequence,
        )
        .contains("W2-CLOSE-READY")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        read_target_screen(
            &mut owner,
            &closing_connection,
            second,
            &mut close_read_sequence
        )
        .contains("W2-CLOSE-READY"),
        "the HUP trap must be installed before close admission"
    );
    let mut sibling = input_connection(&mut owner, first);
    let start = Instant::now();
    let mut close_bytes = Vec::new();
    let close_key = sibling.next_key();
    encode_close_request(
        &CloseRequest {
            key: close_key,
            target: second,
            expected_layout_revision: 2,
        },
        &mut close_bytes,
    )
    .unwrap();
    assert!(matches!(
        owner.begin_close(&close_bytes, NOW, start).unwrap(),
        CloseStatus::InProgress
    ));

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ack = false;
    while Instant::now() < deadline && !ack {
        owner.poll(NOW).unwrap();
        let screen = send_and_read(&mut owner, &mut sibling, "during-hup");
        ack = screen.contains("W2-SIBLING-ACK-during-hup");
        if !ack {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(
        ack,
        "sibling PTY made no input/output progress during HUP wait"
    );

    owner
        .advance_closes_at(start + Duration::from_secs(1))
        .unwrap();
    assert!(send_and_read(&mut owner, &mut sibling, "after-escalation")
        .contains("W2-SIBLING-ACK-after-escalation"));
    let close_deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < close_deadline && owner.layout_snapshot().unwrap().pane_count() == 2 {
        owner.poll(NOW).unwrap();
        owner
            .advance_closes_at(start + Duration::from_secs(2))
            .unwrap();
        if owner.layout_snapshot().unwrap().pane_count() == 2 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(owner.layout_snapshot().unwrap().pane_count(), 1);
    let request = panelink::close_protocol::decode_close_request(&close_bytes).unwrap();
    assert_eq!(request.key, close_key);
    assert_eq!(owner.pane_statuses()[0].unwrap().geometry.target, first);
    let result = owner
        .close_status_state(&sibling.connection, request.key, NOW)
        .unwrap();
    let CloseStatus::Terminal(result) = result else {
        panic!("close not terminal")
    };
    assert_eq!(
        result.outcome,
        panelink::close_protocol::CloseOutcome::Stopped(StopOutcome::Escalated)
    );
}
