#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::client_context::{ContextFile, ReconcileResult};
use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::event_journal::EventCursor;
use panelink::event_protocol::{decode_event, Event, EventBody};
use panelink::handshake_protocol::{
    decode_welcome, Hello, Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ,
};
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::server::{EffectSink, StateOwner};
use panelink::server_pty::PtyEffects;
use panelink::server_runtime::ServerDriver;
use panelink::session_protocol::{
    decode_session_info, encode_session_hello, encode_session_request, LeaseRef, SessionHello,
    SessionInfo, SessionRequest,
};
use panelink::terminal::{TerminalSession, TerminalSize};
use panelink::transport::{AuthenticatedStream, OwnedFrame};
use panelink::watch_protocol::{
    decode_reconciliation, decode_watch_end, encode_watch_request, WatchEnd, WatchRequest,
    WatchStatus,
};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const GENERATION: u64 = 8_173;
const CLIENT_IO_DEADLINE: Duration = Duration::from_secs(2);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(12);
const TOKEN_PREFIX: &str = "socket-submit-p3-";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p3-socket-detach-{}-{serial}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create private socket/reconnect fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn context_path(&self) -> PathBuf {
        self.0.join("client.context")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct InitialObservation {
    welcome: Welcome,
    target: TargetRef,
    owner_revision: u64,
    ready_pid: String,
    check_token: String,
    submit_key: OperationKey,
    submit_payload: Vec<u8>,
}

struct ReconnectObservation {
    welcome: Welcome,
    status: OperationStatus,
    reconciliation: ReconcileResult,
    duplicate_status: OperationStatus,
    screen: String,
    detach_ack: Vec<u8>,
}

struct FinalObservation {
    welcome: Welcome,
    info: SessionInfo,
    screen: String,
}

fn target() -> TargetRef {
    TargetRef {
        generation: GENERATION,
        workspace: 29,
        pane: 31,
        session: 37,
        occupant_revision: 41,
    }
}

fn make_shell(script: &str, cwd: &Path) -> TerminalSession {
    let launch = ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: cwd.to_path_buf(),
    };
    let environment: Environment = [
        (OsString::from("TERM"), OsString::from("panelink-probe")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ]
    .into_iter()
    .collect();
    TerminalSession::spawn(
        &launch,
        &[OsString::from("-c"), OsString::from(script)],
        &environment,
        TerminalSize::new(8, 80).expect("valid small test terminal"),
    )
    .expect("spawn one explicit shell on a private PTY")
}

fn now_secs(started: Instant) -> u64 {
    100 + started.elapsed().as_secs()
}

fn hello(nonce: u64, resume: Option<LeaseRef>) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume,
    }
}

fn attach_socketpair(driver: &mut ServerDriver<PtyEffects>) -> AuthenticatedStream {
    let (server_socket, client_socket) = UnixStream::pair().expect("create actual Unix socketpair");
    let server_stream = AuthenticatedStream::new(server_socket)
        .expect("authenticate the server side against the same-user peer");
    driver
        .attach(server_stream)
        .expect("start one bounded worker for the authenticated socketpair");
    AuthenticatedStream::new(client_socket)
        .expect("authenticate the client side against the same-user peer")
}

fn drive_client<T: Send>(
    driver: &mut ServerDriver<PtyEffects>,
    started: Instant,
    operation: impl FnOnce() -> T + Send,
) -> T {
    thread::scope(|scope| {
        let client = scope.spawn(operation);
        let deadline = Instant::now() + SCENARIO_DEADLINE;
        while !client.is_finished() {
            driver
                .tick(|| now_secs(started))
                .expect("advance server owner and process effects while client exchanges frames");
            assert!(
                Instant::now() < deadline,
                "bounded socket exchange exceeded its scenario deadline"
            );
            thread::sleep(Duration::from_millis(1));
        }
        client
            .join()
            .expect("bounded socket exchange thread completes")
    })
}

fn wait_for_workers_to_close(driver: &mut ServerDriver<PtyEffects>, started: Instant) {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    while driver.active_connections() != 0 {
        driver
            .tick(|| now_secs(started))
            .expect("continue server-side process and queued request progress");
        assert!(
            Instant::now() < deadline,
            "connection worker did not close within its bounded I/O deadlines"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn write_frame(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
) {
    let frame = Frame::new(kind, request_id, payload).expect("construct bounded protocol frame");
    stream
        .write_frame(&frame, Instant::now() + CLIENT_IO_DEADLINE)
        .expect("write one socket frame before its absolute deadline");
}

fn read_frame(stream: &mut AuthenticatedStream) -> OwnedFrame {
    stream
        .read_frame(Instant::now() + CLIENT_IO_DEADLINE)
        .expect("read one socket frame before its absolute deadline")
}

fn establish(stream: &mut AuthenticatedStream, request: &SessionHello) -> Welcome {
    let payload = encode_session_hello(request).expect("encode fresh or resume Hello");
    write_frame(stream, MessageKind::Hello, 0, &payload);
    let frame = read_frame(stream);
    assert_eq!(frame.kind(), MessageKind::Welcome, "server returns Welcome");
    assert_eq!(frame.request_id(), 0, "Welcome uses request ID zero");
    decode_welcome(frame.payload()).expect("decode server-issued generation and lease")
}

fn request(stream: &mut AuthenticatedStream, next_request_id: &mut u64, payload: &[u8]) -> Vec<u8> {
    let request_id = *next_request_id;
    assert_ne!(request_id, 0, "client request IDs are nonzero");
    *next_request_id = request_id
        .checked_add(1)
        .expect("test request ID does not overflow");
    write_frame(stream, MessageKind::Request, request_id, payload);
    let frame = read_frame(stream);
    assert_eq!(
        frame.kind(),
        MessageKind::Response,
        "request gets a response"
    );
    assert_eq!(
        frame.request_id(),
        request_id,
        "response matches the one in-flight request"
    );
    frame.payload().to_vec()
}

fn request_without_reading_reply(
    stream: &mut AuthenticatedStream,
    next_request_id: &mut u64,
    payload: &[u8],
) {
    let request_id = *next_request_id;
    assert_ne!(request_id, 0);
    *next_request_id = request_id
        .checked_add(1)
        .expect("test request ID does not overflow");
    write_frame(stream, MessageKind::Request, request_id, payload);
}

fn encode_control(request: &ControlRequest<'_>) -> Vec<u8> {
    let mut payload = Vec::new();
    encode_request_payload(request, &mut payload).expect("encode typed control request");
    payload
}

fn response_status(payload: &[u8], expected_key: OperationKey) -> OperationStatus {
    let response = decode_response_payload(payload).expect("decode schema-1 operation response");
    assert_eq!(
        response.key, expected_key,
        "response identifies the exact operation"
    );
    match response.body {
        ResponseBody::Status(status) => status,
        _ => panic!("expected operation status response"),
    }
}

fn establish_events(
    stream: &mut AuthenticatedStream,
    nonce: u64,
    resume: Option<LeaseRef>,
) -> Welcome {
    let welcome = establish(
        stream,
        &SessionHello {
            hello: Hello {
                nonce,
                required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT | CAPABILITY_EVENTS,
                optional_capabilities: 0,
            },
            resume,
        },
    );
    assert_ne!(welcome.selected_capabilities & CAPABILITY_EVENTS, 0);
    stream.enable_metadata_events().unwrap();
    welcome
}

fn reconcile_live(
    stream: &mut AuthenticatedStream,
    next: &mut u64,
) -> panelink::server::ReconcileState {
    let payload = encode_watch_request(&WatchRequest::Reconcile {
        target: target(),
        max_bytes: 4096,
        max_lines: 8,
    })
    .unwrap();
    let bytes = request(stream, next, &payload);
    let state = decode_reconciliation(&bytes).unwrap();
    assert_eq!(state.info.target, target());
    assert_eq!(state.cursor.generation, GENERATION);
    assert!(bytes.len() <= 4096);
    panelink::server::ReconcileState {
        info: state.info,
        cursor: state.cursor,
        screen_revision: state.screen_revision,
        truncated: state.truncated,
        text: state.text.to_owned(),
    }
}

fn reconcile_until(
    stream: &mut AuthenticatedStream,
    next: &mut u64,
    marker: &str,
) -> panelink::server::ReconcileState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = reconcile_live(stream, next);
        if state.text.contains(marker) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "missing live snapshot marker: {marker}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn live_submit(
    stream: &mut AuthenticatedStream,
    next: &mut u64,
    context: &mut ContextFile,
    revision: u64,
    text: &str,
) {
    let key = context.reserve().unwrap();
    let payload = encode_control(&ControlRequest {
        key,
        target: target(),
        command: Command::Submit {
            owner_revision: revision,
            text,
        },
    });
    let result = request(stream, next, &payload);
    assert_eq!(
        response_status(&result, key),
        OperationStatus::Completed(Outcome::Applied)
    );
    context.ack(key).unwrap();
}

fn live_watch(
    stream: &mut AuthenticatedStream,
    next: &mut u64,
    cursor: EventCursor,
    wait_ms: u16,
) -> (Vec<Event>, WatchEnd) {
    let payload = encode_watch_request(&WatchRequest::Watch {
        target: target(),
        cursor,
        limit: 64,
        wait_ms,
    })
    .unwrap();
    let request_id = *next;
    *next += 1;
    write_frame(stream, MessageKind::Request, request_id, &payload);
    let deadline = Instant::now() + CLIENT_IO_DEADLINE;
    let mut events = Vec::new();
    for _ in 0..=64 {
        let frame = stream.read_frame(deadline).unwrap();
        match frame.kind() {
            MessageKind::Event => {
                assert_eq!(frame.request_id(), 0);
                let event = decode_event(frame.payload()).unwrap();
                assert_eq!(event.target, target());
                assert_eq!(event.cursor, cursor.sequence + events.len() as u64 + 1);
                events.push(event);
            }
            MessageKind::Response => {
                assert_eq!(frame.request_id(), request_id);
                let end = decode_watch_end(frame.payload()).unwrap();
                assert_eq!(usize::from(end.event_count), events.len());
                return (events, end);
            }
            _ => panic!("unexpected finite watch frame: {frame:?}"),
        }
    }
    panic!("watch lacked its bounded terminal response");
}

#[test]
fn pending_live_watch_observes_real_output_and_reconciles_the_same_child() {
    let directory = TempDir::new();
    let script = "/bin/stty -echo; printf 'LIVE-READY-%s\\n' \"$$\"; count=0; while IFS= read -r line; do case \"$line\" in TOKEN) count=$((count + 1)); printf 'LIVE-GOT:%s\\n' \"$count\";; CHECK) printf 'LIVE-CHECK:%s\\n' \"$count\";; EXIT) printf 'LIVE-EXIT\\n'; exit 0;; esac; done";
    let effects = PtyEffects::new(make_shell(script, directory.path()));
    let state = StateOwner::new(GENERATION, target(), effects, true).unwrap();
    let mut driver = ServerDriver::new(state);
    let started = Instant::now();
    let mut client = attach_socketpair(&mut driver);
    let context_path = directory.context_path();
    let init_path = context_path.clone();
    let (welcome, snapshot, owner_revision) = drive_client(&mut driver, started, move || {
        let welcome = establish_events(&mut client, 101, None);
        let mut next = 1;
        let before = reconcile_until(&mut client, &mut next, "LIVE-READY-");
        let mut context = ContextFile::create(
            &init_path,
            welcome.generation,
            welcome.lease,
            Instant::now() + CLIENT_IO_DEADLINE,
        )
        .unwrap();
        let key = context.reserve().unwrap();
        let claim = encode_session_request(&SessionRequest::Claim {
            key,
            target: target(),
            expected_owner_revision: before.info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        let result = request(&mut client, &mut next, &claim);
        assert_eq!(
            response_status(&result, key),
            OperationStatus::Completed(Outcome::Applied)
        );
        context.ack(key).unwrap();
        let metadata_before = fs::read(&init_path).unwrap();
        let snapshot = reconcile_live(&mut client, &mut next);
        assert_eq!(snapshot.info.owner_lease, welcome.lease);
        assert_eq!(
            fs::read(&init_path).unwrap(),
            metadata_before,
            "reconciliation must not reserve or acknowledge client input"
        );
        let revision = snapshot.info.owner_revision;
        (welcome, snapshot, revision)
    });
    wait_for_workers_to_close(&mut driver, started);
    let ready_line = snapshot
        .text
        .lines()
        .find(|line| line.starts_with("LIVE-READY-"))
        .unwrap()
        .trim()
        .to_owned();
    let mut watcher = attach_socketpair(&mut driver);
    let mut producer = attach_socketpair(&mut driver);
    let (permit_sender, permit_receiver) = mpsc::sync_channel(1);
    let mut permit_sender = Some(permit_sender);
    let final_state = thread::scope(|scope| {
        let operation = scope.spawn(move || {
            let lease = Some(LeaseRef {
                generation: welcome.generation,
                lease: welcome.lease,
            });
            establish_events(&mut producer, 102, lease);
            let mut next = 1;
            let mut context =
                ContextFile::open_existing(&context_path, Instant::now() + CLIENT_IO_DEADLINE)
                    .unwrap();
            let watch_cursor = snapshot.cursor;
            let waiting = scope.spawn(move || {
                establish_events(&mut watcher, 103, lease);
                let mut watch_id = 1;
                live_watch(&mut watcher, &mut watch_id, watch_cursor, 1000)
            });
            // The owner thread sends this only after it actually retains the
            // watch. No fixed sleep is accepted as proof of a waiting watch.
            permit_receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            live_submit(
                &mut producer,
                &mut next,
                &mut context,
                owner_revision,
                "TOKEN",
            );
            let (events, end) = waiting.join().unwrap();
            assert_eq!(end.status, WatchStatus::Changed);
            assert!(!events.is_empty());
            assert!(events
                .iter()
                .any(|event| matches!(event.body, EventBody::ScreenChanged { .. })));
            assert_eq!(end.cursor.sequence, events.last().unwrap().cursor);
            let after = reconcile_until(&mut producer, &mut next, "LIVE-GOT:1");
            assert!(after.cursor.sequence >= end.cursor.sequence);
            for event in events {
                if let EventBody::ScreenChanged { revision } = event.body {
                    assert!(after.screen_revision >= revision);
                }
            }
            assert!(after.text.contains(&ready_line));
            assert_eq!(after.info.owner_lease, welcome.lease);
            // Ordered application evidence: a watch/reconcile never adds a
            // second TOKEN to the shell's input stream.
            live_submit(
                &mut producer,
                &mut next,
                &mut context,
                owner_revision,
                "CHECK",
            );
            let checked = reconcile_until(&mut producer, &mut next, "LIVE-CHECK:1");
            assert!(checked.text.contains(&ready_line));
            let mut cursor = checked.cursor;
            live_submit(
                &mut producer,
                &mut next,
                &mut context,
                owner_revision,
                "EXIT",
            );
            let exit_deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let (events, end) = live_watch(&mut producer, &mut next, cursor, 200);
                assert_ne!(end.status, WatchStatus::ResyncRequired);
                cursor = end.cursor;
                if events
                    .iter()
                    .any(|event| event.body == EventBody::ChildExited)
                {
                    break;
                }
                assert!(
                    Instant::now() < exit_deadline,
                    "collected child exit event missing"
                );
            }
            let exited = reconcile_live(&mut producer, &mut next);
            assert!(exited.info.child_exited);
            assert!(exited.text.contains("LIVE-EXIT"));
            exited
        });
        let deadline = Instant::now() + SCENARIO_DEADLINE;
        while !operation.is_finished() {
            driver.tick(|| now_secs(started)).unwrap();
            if driver.pending_watches() != 0 {
                if let Some(sender) = permit_sender.take() {
                    sender.try_send(()).unwrap();
                }
            }
            assert!(
                Instant::now() < deadline,
                "live watch scenario exceeded deadline"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert!(permit_sender.is_none(), "watch was never observed pending");
        operation.join().unwrap()
    });
    assert!(final_state.info.child_exited);
    wait_for_workers_to_close(&mut driver, started);
    assert!(driver.state().effects().child_exited());
}

fn next_read(
    stream: &mut AuthenticatedStream,
    next_request_id: &mut u64,
    context: &mut ContextFile,
    target: TargetRef,
) -> String {
    let key = context.reserve().expect("durably reserve bounded read key");
    let payload = encode_control(&ControlRequest {
        key,
        target,
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 4_000,
            max_lines: 8,
        },
    });
    let response_payload = request(stream, next_request_id, &payload);
    let response = decode_response_payload(&response_payload).expect("decode bounded screen read");
    assert_eq!(response.key, key, "read response carries the reserved key");
    let text = match response.body {
        ResponseBody::Read {
            target: observed_target,
            text,
            ..
        } => {
            assert_eq!(observed_target, target, "read identifies the full target");
            text.to_owned()
        }
        _ => panic!("expected current screen response"),
    };
    context
        .ack(key)
        .expect("durably acknowledge exact read response");
    text
}

fn read_until(
    stream: &mut AuthenticatedStream,
    next_request_id: &mut u64,
    context: &mut ContextFile,
    target: TargetRef,
    marker: &str,
) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        let screen = next_read(stream, next_request_id, context, target);
        if screen.contains(marker) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "bounded screen read did not find marker"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn describe(stream: &mut AuthenticatedStream, next_request_id: &mut u64) -> SessionInfo {
    let payload = encode_session_request(&SessionRequest::Describe).expect("encode Describe");
    let response = request(stream, next_request_id, &payload);
    decode_session_info(&response).expect("decode current target and ownership state")
}

fn ready_pid(screen: &str) -> String {
    screen
        .lines()
        .find_map(|line| {
            let marker = "READY-P3-";
            let offset = line.find(marker)?;
            let value = line[offset + marker.len()..].trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
        .expect("bounded screen contains the shell's ready PID marker")
}

fn assert_one_application_marker(screen: &str, pid: &str) {
    assert_eq!(
        screen.matches(&format!("GOT-P3-{pid}")).count(),
        1,
        "the shell handled the submitted line exactly once"
    );
}

#[test]
fn socket_loss_status_reconciliation_and_detach_preserve_one_server_owned_pty() {
    let directory = TempDir::new();
    let started = Instant::now();
    let token = format!("{TOKEN_PREFIX}{}", std::process::id());
    let check_token = format!("socket-check-p3-{}", std::process::id());
    let script = format!(
        "printf 'READY-P3-%s\\n' \"$$\"; \\
         handled=0; \\
         while IFS= read -r received; do \\
           if [ \"$received\" = '{token}' ]; then \\
             handled=$((handled + 1)); printf 'GOT-P3-%s\\n' \"$$\"; \\
           elif [ \"$received\" = '{check_token}' ]; then \\
             printf 'CHECK-P3-%s-%s\\n' \"$$\" \"$handled\"; \\
           fi; \\
         done"
    );
    let effects = PtyEffects::new(make_shell(&script, directory.path()));
    let state = StateOwner::new(GENERATION, target(), effects, true)
        .expect("construct one volatile single-session owner");
    let mut driver = ServerDriver::new(state);

    let first_client = attach_socketpair(&mut driver);
    let context_path = directory.context_path();
    let first_token = token.clone();
    let first_check_token = check_token.clone();
    let initial = drive_client(&mut driver, started, move || {
        let mut client = first_client;
        let welcome = establish(&mut client, &hello(1, None));
        assert_eq!(welcome.generation, GENERATION);
        let mut request_id = 1;
        let first_info = describe(&mut client, &mut request_id);
        assert_eq!(first_info.target.generation, welcome.generation);
        assert_eq!(first_info.owner_revision, 1);
        assert_eq!(first_info.owner_lease, 0);
        assert!(!first_info.child_exited);

        let mut context = ContextFile::create(
            &context_path,
            welcome.generation,
            welcome.lease,
            Instant::now() + CLIENT_IO_DEADLINE,
        )
        .expect("create private persistent context for the server-issued lease");
        let screen = read_until(
            &mut client,
            &mut request_id,
            &mut context,
            first_info.target,
            "READY-P3-",
        );
        let pid = ready_pid(&screen);

        let claim_key = context.reserve().expect("durably reserve Claim key");
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target: first_info.target,
            expected_owner_revision: first_info.owner_revision,
            ttl_seconds: 30,
        })
        .expect("encode Claim with exact Describe target and owner revision");
        let claim_reply = request(&mut client, &mut request_id, &claim);
        assert_eq!(
            response_status(&claim_reply, claim_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        context.ack(claim_key).expect("durably acknowledge Claim");

        let claimed_info = describe(&mut client, &mut request_id);
        assert_eq!(claimed_info.target, first_info.target);
        assert_eq!(claimed_info.owner_lease, welcome.lease);
        assert_eq!(claimed_info.owner_revision, first_info.owner_revision + 1);

        let submit_key = context
            .reserve()
            .expect("durably reserve uncertain Submit key");
        let submit_payload = encode_control(&ControlRequest {
            key: submit_key,
            target: claimed_info.target,
            command: Command::Submit {
                owner_revision: claimed_info.owner_revision,
                text: &first_token,
            },
        });
        // Deliberately discard the acknowledgment. The ContextFile remains marked
        // uncertain; this client does not retransmit the nonreplayable input.
        request_without_reading_reply(&mut client, &mut request_id, &submit_payload);
        drop(client);
        drop(context);
        InitialObservation {
            welcome,
            target: claimed_info.target,
            owner_revision: claimed_info.owner_revision,
            ready_pid: pid,
            check_token: first_check_token,
            submit_key,
            submit_payload,
        }
    });

    // The worker exits only after the owner replied to the submitted frame. The
    // driver continues ticking, so that accepted work finishes after socket loss.
    wait_for_workers_to_close(&mut driver, started);
    assert!(
        !driver.state().effects().child_exited(),
        "abrupt client socket loss leaves the server-owned child alive"
    );

    let resumed_client = attach_socketpair(&mut driver);
    let resumed_context_path = directory.context_path();
    let resumed_submit_payload = initial.submit_payload.clone();
    let resumed_ready_pid = initial.ready_pid.clone();
    let resumed = drive_client(&mut driver, started, move || {
        let mut client = resumed_client;
        let lease = LeaseRef {
            generation: initial.welcome.generation,
            lease: initial.welcome.lease,
        };
        let welcome = establish(&mut client, &hello(2, Some(lease)));
        assert_eq!(welcome.generation, initial.welcome.generation);
        assert_eq!(
            welcome.lease, initial.welcome.lease,
            "resume reuses the lease"
        );

        let mut request_id = 1;
        let status_payload = encode_session_request(&SessionRequest::OperationStatus {
            key: initial.submit_key,
        })
        .expect("encode status observation for the exact uncertain key");
        let status_deadline = Instant::now() + SCENARIO_DEADLINE;
        let status = loop {
            let response = request(&mut client, &mut request_id, &status_payload);
            let status = response_status(&response, initial.submit_key);
            if status != OperationStatus::InProgress {
                break status;
            }
            assert!(
                Instant::now() < status_deadline,
                "operation status remained in progress past its bounded deadline"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status, OperationStatus::Completed(Outcome::Applied));

        let mut context = ContextFile::open(
            &resumed_context_path,
            welcome.generation,
            welcome.lease,
            Instant::now() + CLIENT_IO_DEADLINE,
        )
        .expect("reopen the exact durable generation/lease context");
        assert_eq!(context.context().uncertain(), Some(initial.submit_key));
        let reconciliation = context
            .reconcile(initial.submit_key, status)
            .expect("reconcile status for the exact uncertain operation");
        assert_eq!(reconciliation, ReconcileResult::Cleared);
        assert_eq!(context.context().uncertain(), None);

        // This explicit same-key duplicate is a deliberate server dedup regression
        // probe after status reconciliation; it is not the client retry behavior.
        let duplicate = request(&mut client, &mut request_id, &resumed_submit_payload);
        let duplicate_status = response_status(&duplicate, initial.submit_key);
        assert_eq!(
            duplicate_status,
            OperationStatus::Completed(Outcome::Applied)
        );

        // Send a separate ordered application marker after the duplicate. The
        // shell reports how many token lines it handled only after it reaches
        // this later input, which checks for a delayed second GOT without using
        // a sleep or tty echo as success evidence.
        let check_key = context
            .reserve()
            .expect("durably reserve ordered application-check key");
        let check_payload = encode_control(&ControlRequest {
            key: check_key,
            target: initial.target,
            command: Command::Submit {
                owner_revision: initial.owner_revision,
                text: &initial.check_token,
            },
        });
        let check_reply = request(&mut client, &mut request_id, &check_payload);
        assert_eq!(
            response_status(&check_reply, check_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        context
            .ack(check_key)
            .expect("durably acknowledge ordered check");

        let screen = read_until(
            &mut client,
            &mut request_id,
            &mut context,
            initial.target,
            &format!("CHECK-P3-{resumed_ready_pid}-1"),
        );
        assert_one_application_marker(&screen, &resumed_ready_pid);
        assert!(screen.contains(&format!("READY-P3-{resumed_ready_pid}")));

        let detach = encode_session_request(&SessionRequest::Detach).expect("encode Detach");
        let detach_ack = request(&mut client, &mut request_id, &detach);
        assert_eq!(
            detach_ack,
            [0, 2, 0, 2],
            "Detach is acknowledged before close"
        );
        drop(context);
        drop(client);
        ReconnectObservation {
            welcome,
            status,
            reconciliation,
            duplicate_status,
            screen,
            detach_ack,
        }
    });

    assert_eq!(resumed.welcome.generation, initial.welcome.generation);
    assert_eq!(resumed.welcome.lease, initial.welcome.lease);
    assert_eq!(resumed.status, OperationStatus::Completed(Outcome::Applied));
    assert_eq!(resumed.reconciliation, ReconcileResult::Cleared);
    assert_eq!(
        resumed.duplicate_status,
        OperationStatus::Completed(Outcome::Applied)
    );
    assert_eq!(resumed.detach_ack, [0, 2, 0, 2]);
    assert_one_application_marker(&resumed.screen, &initial.ready_pid);
    wait_for_workers_to_close(&mut driver, started);
    assert!(
        !driver.state().effects().child_exited(),
        "explicit Detach closes only the attachment, not the child"
    );

    let final_client = attach_socketpair(&mut driver);
    let final_context_path = directory.context_path();
    let final_ready_pid = initial.ready_pid.clone();
    let final_observation = drive_client(&mut driver, started, move || {
        let mut client = final_client;
        let lease = LeaseRef {
            generation: resumed.welcome.generation,
            lease: resumed.welcome.lease,
        };
        let welcome = establish(&mut client, &hello(3, Some(lease)));
        assert_eq!(welcome.generation, resumed.welcome.generation);
        assert_eq!(
            welcome.lease, resumed.welcome.lease,
            "second resume reuses lease"
        );

        let mut request_id = 1;
        let info = describe(&mut client, &mut request_id);
        assert_eq!(info.target, initial.target);
        assert_eq!(info.owner_lease, welcome.lease);
        assert!(!info.child_exited);
        let mut context = ContextFile::open(
            &final_context_path,
            welcome.generation,
            welcome.lease,
            Instant::now() + CLIENT_IO_DEADLINE,
        )
        .expect("reuse the same persistent context after explicit Detach");
        let screen = read_until(
            &mut client,
            &mut request_id,
            &mut context,
            info.target,
            &format!("GOT-P3-{final_ready_pid}"),
        );
        assert_one_application_marker(&screen, &final_ready_pid);
        assert!(screen.contains(&format!("READY-P3-{final_ready_pid}")));
        assert!(screen.contains(&format!("CHECK-P3-{final_ready_pid}-1")));
        FinalObservation {
            welcome,
            info,
            screen,
        }
    });

    assert_eq!(
        final_observation.welcome.generation,
        initial.welcome.generation
    );
    assert_eq!(final_observation.welcome.lease, initial.welcome.lease);
    assert_eq!(final_observation.info.target, initial.target);
    assert_eq!(final_observation.info.owner_lease, initial.welcome.lease);
    assert!(!final_observation.info.child_exited);
    assert_one_application_marker(&final_observation.screen, &initial.ready_pid);
    wait_for_workers_to_close(&mut driver, started);
    assert!(
        !driver.state().effects().child_exited(),
        "the same child remains alive after the final socketpair closes"
    );
}
