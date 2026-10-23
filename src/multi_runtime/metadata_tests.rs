use super::*;
use crate::control_protocol::{encode_request_payload, Command, ControlRequest, TargetRef};
use crate::handshake_protocol::{
    Hello, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_PANE_METADATA, CAPABILITY_READ,
    CAPABILITY_WORKSPACE_EVENTS,
};
use crate::multi_startup::{StartedMultiServer, StartupLaunch};
use crate::pane_metadata_protocol::{
    decode_observation, encode_query, Observation, PathObservation, PathStatus,
};
use crate::process_metadata::MetadataWorker;
use crate::server::SessionReply;
use crate::session_protocol::{encode_session_request, SessionHello, SessionRequest};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        })
    }

    fn block(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).unwrap();
        }
    }

    fn wait_entered(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = self.state.lock().unwrap();
        while !state.0 {
            let wait = deadline.saturating_duration_since(Instant::now());
            assert!(!wait.is_zero(), "metadata sampler did not enter gate");
            let (next, timed) = self.changed.wait_timeout(state, wait).unwrap();
            state = next;
            assert!(
                !timed.timed_out() || state.0,
                "metadata sampler gate timed out"
            );
        }
    }

    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}

fn fixture() -> (Fixture, MultiServerDriver, TargetRef) {
    let root = PathBuf::from("/private/tmp").join(format!(
        "panelink-metadata-runtime-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let fixture = Fixture(root.clone());
    let server_dir = root.join("server");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&server_dir)
        .unwrap();
    let terminfo = root.join("terminfo");
    fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();
    let storage = crate::server_storage::ServerStorage::initialize(&server_dir).unwrap();
    let context = crate::launch::LaunchContext {
        working_directory: root.clone(),
        environment: [
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), root.as_os_str().to_owned()),
        ]
        .into_iter()
        .collect(),
    };
    let arguments = [
        OsString::from("-c"),
        OsString::from("printf 'METADATA-READY\\r\\n'; while :; do /bin/sleep 1; done"),
    ];
    let executable = OsString::from("/bin/sh");
    let started = StartedMultiServer::start(
        storage,
        &terminfo,
        &StartupLaunch {
            executable: &executable,
            arguments: &arguments,
            context: &context,
            rows: 8,
            columns: 60,
        },
        true,
    )
    .unwrap();
    let target = started.startup().target.unwrap();
    (fixture, started.into_driver(), target)
}

fn request_envelope(
    connection: Connection,
    target: TargetRef,
) -> (MultiEnvelope, Receiver<MultiReply>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let envelope = MultiEnvelope {
        request: MultiRequest::Request {
            connection,
            payload: encode_query(target).unwrap(),
        },
        reply: sender,
    };
    (envelope, receiver)
}

fn observation(reply: MultiReply) -> Observation {
    let MultiReply::Response(payload) = reply else {
        panic!("expected metadata response, got {reply:?}");
    };
    decode_observation(&payload).unwrap()
}

fn receive_while_ticking(
    driver: &mut MultiServerDriver,
    receiver: &Receiver<MultiReply>,
) -> MultiReply {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match receiver.try_recv() {
            Ok(reply) => return reply,
            Err(mpsc::TryRecvError::Disconnected) => panic!("metadata reply disconnected"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        driver.tick(|| 10).unwrap();
        assert!(Instant::now() < deadline, "metadata reply timed out");
        std::thread::yield_now();
    }
}

#[test]
fn gated_sampler_times_out_stays_busy_and_discards_late_paths_while_pty_keeps_working() {
    let (_fixture, mut driver, target) = fixture();
    let hello = SessionHello {
        hello: Hello {
            nonce: 1,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT
                | CAPABILITY_EVENTS
                | CAPABILITY_WORKSPACE_EVENTS
                | CAPABILITY_PANE_METADATA,
        },
        resume: None,
    };
    let (connection, welcome) = driver.owner.connect(target, &hello, 10).unwrap();
    assert_ne!(welcome.selected_capabilities & CAPABILITY_PANE_METADATA, 0);

    let mut ready = None;
    for _ in 0..100 {
        driver.tick(|| 10).unwrap();
        if let Ok(state) = driver.owner.reconcile(&connection, target, 4096, 40, 10) {
            if state.text.contains("METADATA-READY") {
                ready = Some(state);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ready.is_some(),
        "managed PTY did not produce its ready marker"
    );

    let gate = Gate::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let origin = driver.metadata_origin;
    driver.metadata_worker = Some(
        MetadataWorker::start_with_sampler(origin, {
            let gate = Arc::clone(&gate);
            let calls = Arc::clone(&calls);
            move |target, _, origin| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    gate.block();
                    Observation {
                        target,
                        sampled_at_mono_ms: origin.elapsed().as_millis() as u64,
                        cwd: PathObservation::available("/tmp/late-secret-path".to_owned())
                            .unwrap(),
                        executable: PathObservation::available("/bin/sh".to_owned()).unwrap(),
                    }
                } else {
                    Observation {
                        target,
                        sampled_at_mono_ms: origin.elapsed().as_millis() as u64,
                        cwd: PathObservation::available("/tmp/current-path".to_owned()).unwrap(),
                        executable: PathObservation::available("/bin/sh".to_owned()).unwrap(),
                    }
                }
            }
        })
        .unwrap(),
    );

    let (first_envelope, first_reply) = request_envelope(connection, target);
    driver.begin_metadata(
        first_envelope,
        connection,
        &encode_query(target).unwrap(),
        10,
    );
    gate.wait_entered();
    assert!(driver.metadata_busy);

    // The PTY owner remains live while the native metadata call is blocked.
    let read = driver
        .owner
        .reconcile(&connection, target, 4096, 40, 10)
        .unwrap();
    assert!(read.text.contains("METADATA-READY"));
    let workspace = driver
        .owner
        .reconcile_workspace(&connection, target.generation, target.workspace, 10)
        .unwrap();
    assert!(workspace.layout.is_some());
    assert!(driver
        .owner
        .read_workspace_events(&connection, workspace.cursor, 8, 10)
        .is_ok());
    let describe = encode_session_request(&SessionRequest::Describe).unwrap();
    let SessionReply::Info(info) = driver
        .owner
        .handle_session(&connection, &describe, 10)
        .unwrap()
    else {
        panic!("expected session information");
    };
    let claim_key = crate::protocol::OperationKey {
        generation: target.generation,
        lease: connection.lease(),
        sequence: 1,
    };
    let claim = encode_session_request(&SessionRequest::Claim {
        key: claim_key,
        target,
        expected_owner_revision: info.owner_revision,
        ttl_seconds: 30,
    })
    .unwrap();
    let SessionReply::Payload(claim_reply) = driver
        .owner
        .handle_session(&connection, &claim, 10)
        .unwrap()
    else {
        panic!("expected claim result");
    };
    let applied = crate::response_protocol::ResponseBody::Status(
        crate::protocol::OperationStatus::Completed(crate::protocol::Outcome::Applied),
    );
    assert_eq!(
        crate::response_protocol::decode_response_payload(&claim_reply)
            .unwrap()
            .body,
        applied
    );
    let input = ControlRequest {
        key: crate::protocol::OperationKey {
            generation: target.generation,
            lease: connection.lease(),
            sequence: 2,
        },
        target,
        command: Command::Text {
            owner_revision: info.owner_revision + 1,
            text: "x",
        },
    };
    let mut input_payload = Vec::new();
    encode_request_payload(&input, &mut input_payload).unwrap();
    let input_reply = driver
        .owner
        .handle_control(&connection, &input_payload, 10)
        .unwrap();
    assert_eq!(
        crate::response_protocol::decode_response_payload(&input_reply)
            .unwrap()
            .body,
        applied
    );

    driver.metadata_pending.as_mut().unwrap().deadline = Instant::now() - Duration::from_millis(1);
    driver.tick(|| 10).unwrap();
    let timed_out = observation(first_reply.recv_timeout(Duration::from_secs(1)).unwrap());
    assert_eq!(timed_out.cwd.status, PathStatus::TimedOut);
    assert!(
        driver.metadata_busy,
        "native slot stays busy after owner timeout"
    );

    let (busy_envelope, busy_reply) = request_envelope(connection, target);
    driver.begin_metadata(
        busy_envelope,
        connection,
        &encode_query(target).unwrap(),
        10,
    );
    let busy = observation(busy_reply.recv_timeout(Duration::from_secs(1)).unwrap());
    assert_eq!(busy.cwd.status, PathStatus::Busy);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "busy query was not enqueued"
    );

    gate.release();
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    while driver.metadata_busy {
        driver.tick(|| 10).unwrap();
        assert!(
            Instant::now() < drain_deadline,
            "late metadata completion was not drained"
        );
        std::thread::yield_now();
    }
    assert!(
        first_reply.try_recv().is_err(),
        "late completion was not published"
    );

    let (next_envelope, next_reply) = request_envelope(connection, target);
    driver.begin_metadata(
        next_envelope,
        connection,
        &encode_query(target).unwrap(),
        10,
    );
    let current = observation(receive_while_ticking(&mut driver, &next_reply));
    assert_eq!(current.cwd.value.as_deref(), Some("/tmp/current-path"));
}
