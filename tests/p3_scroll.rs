#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! In-process scroll protocol proof: read-only history snapshots, alternate
//! screen wheel admission, authorization and operation-key deduplication.

use panelink::control_protocol::TargetRef;
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use panelink::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::server::multi_session::MultiSessionOwner;
use panelink::server::{Connection, SessionReply};
use panelink::server_pty::PtyEffects;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{SessionHello, SessionRequest};
use panelink::ui_protocol::{
    decode_history_reply, encode_history_query, encode_mouse_wheel_input, prefix_history_reply,
    MouseWheelInput,
};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const NOW: u64 = 100;
const LAUNCH_LEASE: u64 = 9_801;
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 80;
const WAIT: Duration = Duration::from_secs(8);
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> io::Result<Self> {
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p3-scroll-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Client {
    connection: Connection,
    sequence: u64,
    target: TargetRef,
}

impl Client {
    fn key(&mut self) -> OperationKey {
        self.sequence += 1;
        OperationKey {
            generation: self.target.generation,
            lease: self.connection.lease(),
            sequence: self.sequence,
        }
    }
}

fn launch_payload(generation: u64, path: &Path, script: &[u8]) -> Vec<u8> {
    let arguments: [&[u8]; 4] = [b"-c", script, b"panelink-scroll-test", b"scroll-child"];
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
            sequence: 1,
        },
        rows: ROOT_ROWS,
        columns: ROOT_COLUMNS,
        requested_executable: b"/bin/sh",
        requested_working_directory: path.as_os_str().as_bytes(),
        client_working_directory: path.as_os_str().as_bytes(),
        arguments: &arguments,
        environment: &environment,
    };
    let mut bytes = Vec::new();
    encode_launch_request(&request, &mut bytes).expect("encode explicit child launch");
    bytes
}

fn prepare(
    request: panelink::managed_launch_protocol::LaunchRequestView<'_>,
    target: TargetRef,
    root: &Path,
) -> Result<PreparedLaunch, ()> {
    prepare_launch(request, target, root).map_err(|_| ())
}

fn spawn(prepared: PreparedLaunch) -> Result<PtyEffects, ()> {
    prepared.spawn().map(PtyEffects::new).map_err(|_| ())
}

fn launch_owner(
    fixture: &Fixture,
    script: &[u8],
) -> (ServerStorage, MultiSessionOwner<PtyEffects>, TargetRef) {
    let mut storage = ServerStorage::initialize(&fixture.0).expect("initialize storage");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).expect("create operation ledger");
    ledger
        .open_lease(LAUNCH_LEASE, NOW)
        .expect("open launch lease");
    let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, true)
        .expect("assemble input-enabled owner");
    let payload = launch_payload(generation, &fixture.0, script);
    let result = {
        let mut journal = storage.journal().expect("open launch journal");
        owner
            .execute_launch(
                &payload,
                NOW,
                &mut journal,
                |request, target| prepare(request, target, &fixture.0),
                spawn,
            )
            .expect("launch scroll test child")
    };
    assert_eq!(result.outcome, Outcome::Applied);
    (storage, owner, result.target.unwrap())
}

fn connect(
    owner: &mut MultiSessionOwner<PtyEffects>,
    target: TargetRef,
    nonce: u64,
    input: bool,
) -> Client {
    let capabilities = CAPABILITY_READ | if input { CAPABILITY_INPUT } else { 0 };
    let (connection, welcome) = owner
        .connect(
            target,
            &SessionHello {
                hello: Hello {
                    nonce,
                    required_capabilities: capabilities,
                    optional_capabilities: 0,
                },
                resume: None,
            },
            NOW,
        )
        .expect("connect target-pinned client");
    assert_eq!(welcome.selected_capabilities, capabilities);
    Client {
        connection,
        sequence: 0,
        target,
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
        .expect("read owner state")
    {
        SessionReply::Info(info) => info,
        _ => panic!("Describe returns session info"),
    }
}

fn claim(owner: &mut MultiSessionOwner<PtyEffects>, client: &mut Client) -> u64 {
    let info = describe(owner, client);
    let key = client.key();
    let payload = panelink::session_protocol::encode_session_request(&SessionRequest::Claim {
        key,
        target: client.target,
        expected_owner_revision: info.owner_revision,
        ttl_seconds: 30,
    })
    .expect("encode Claim");
    let SessionReply::Payload(result) = owner
        .handle_session(&client.connection, &payload, NOW)
        .expect("claim input")
    else {
        panic!("Claim returns an operation response");
    };
    assert_eq!(
        decode_response_payload(&result).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    describe(owner, client).owner_revision
}

fn history_payload(
    owner: &mut MultiSessionOwner<PtyEffects>,
    client: &Client,
    offset_rows: u16,
) -> Vec<u8> {
    let query = encode_history_query(client.target, offset_rows).unwrap();
    let captured = owner
        .observe_target(&client.connection, client.target, NOW, |effects| {
            let (snapshot, effective) = effects.history_snapshot(offset_rows).unwrap();
            let revision = effects.revision();
            let metadata =
                panelink::screen_checkpoint::CheckpointMetadata::new(client.target, 0, 1, revision)
                    .unwrap();
            let checkpoint = panelink::screen_checkpoint::encode(&snapshot, &metadata).unwrap();
            prefix_history_reply(
                client.target,
                revision,
                effective,
                effects.alternate_screen_active(),
                effects.mouse_reporting_enabled(),
                checkpoint,
            )
            .unwrap()
        })
        .expect("READ capability authorizes history observation");
    assert_eq!(query.len(), panelink::ui_protocol::HISTORY_QUERY_BYTES);
    captured
}

#[test]
fn normal_buffer_history_scrolls_back_for_a_read_only_connection() {
    let fixture = Fixture::new().expect("private fixture");
    let script = b"i=0; while [ $i -lt 50 ]; do printf 'P3SCROLL-LINE-%s\\n' $i; i=$((i+1)); done; exec /bin/sleep 60";
    let (_storage, mut owner, target) = launch_owner(&fixture, script);
    let client = connect(&mut owner, target, 4400, false);
    let deadline = Instant::now() + WAIT;
    let reply = loop {
        owner.poll(NOW).expect("poll retained PTY output");
        let reply = history_payload(&mut owner, &client, 3);
        let view = decode_history_reply(&reply).unwrap();
        if view.offset_rows() > 0 {
            break reply;
        }
        assert!(
            Instant::now() < deadline,
            "child output reaches normal history"
        );
        thread::sleep(Duration::from_millis(5));
    };
    let view = decode_history_reply(&reply).unwrap();
    assert!(!view.alternate_screen());
    assert!(!view.mouse_reporting());
    assert!(view.offset_rows() > 0);
    assert!(!view.checkpoint().is_empty());
}

#[test]
fn normal_history_is_read_only_and_alternate_mouse_wheel_is_authorized_and_deduplicated() {
    let fixture = Fixture::new().expect("private fixture");
    let script = b"i=0; while [ $i -lt 50 ]; do printf 'P3SCROLL-LINE-%s\\n' $i; i=$((i+1)); done; printf '\\033[?1049h\\033[?1000h\\033[?1006h'; exec /bin/sleep 60";
    let (_storage, mut owner, target) = launch_owner(&fixture, script);
    let read_client = connect(&mut owner, target, 4401, false);
    let deadline = Instant::now() + WAIT;
    loop {
        owner.poll(NOW).expect("poll retained PTY output");
        let ready = owner
            .observe_target(&read_client.connection, target, NOW, |effects| {
                effects.mouse_reporting_enabled() && effects.alternate_screen_active()
            })
            .expect("READ observes terminal mode");
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child enters alternate mouse mode"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let history = history_payload(&mut owner, &read_client, 3);
    let history_view = decode_history_reply(&history).unwrap();
    assert!(history_view.alternate_screen());
    assert!(history_view.mouse_reporting());
    // The alternate screen is never replaced with the hidden shell history.
    assert_eq!(history_view.offset_rows(), 0);

    let mut input_client = connect(&mut owner, target, 4402, true);
    let owner_revision = claim(&mut owner, &mut input_client);
    let input_key = input_client.key();
    let input = MouseWheelInput {
        key: input_key,
        target,
        expected_owner_revision: owner_revision,
        column: 2,
        row: 2,
        up: true,
    };
    let payload = encode_mouse_wheel_input(&input).unwrap();
    let no_input = owner
        .handle_mouse_wheel(
            &read_client.connection,
            &input,
            &payload,
            NOW,
            |effects| effects.validate_mouse_wheel(input.column, input.row, input.up),
            |effects| effects.mouse_wheel(input.column, input.row, input.up),
        )
        .expect("unauthorized input is a typed rejection");
    assert_eq!(
        decode_response_payload(&no_input).unwrap().body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );

    let first = owner
        .handle_mouse_wheel(
            &input_client.connection,
            &input,
            &payload,
            NOW,
            |effects| effects.validate_mouse_wheel(input.column, input.row, input.up),
            |effects| effects.mouse_wheel(input.column, input.row, input.up),
        )
        .expect("authorized wheel reaches terminal mouse encoder");
    let duplicate = owner
        .handle_mouse_wheel(
            &input_client.connection,
            &input,
            &payload,
            NOW,
            |effects| effects.validate_mouse_wheel(input.column, input.row, input.up),
            |effects| effects.mouse_wheel(input.column, input.row, input.up),
        )
        .expect("exact duplicate returns the retained result");
    let expected = ResponseBody::Status(OperationStatus::Completed(Outcome::Applied));
    assert_eq!(decode_response_payload(&first).unwrap().body, expected);
    assert_eq!(decode_response_payload(&duplicate).unwrap().body, expected);
}
