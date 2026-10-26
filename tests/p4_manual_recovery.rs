#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::managed_provider_store::{ManagedLaunchOutcome, ManagedProviderScope};
use panelink::protocol::OperationKey;
use panelink::provider_binding::VerificationSource;
use panelink::provider_bridge::{
    Frame, FrameScope, Selection, BRIDGE_PROTOCOL_VERSION, SUPPORTED_PROVIDER_VERSION,
};
use panelink::provider_bridge_protocol::{self, Message};
use panelink::provider_bridge_runtime::{ProviderBridgeRuntime, RuntimeStatus};
use panelink::provider_recovery_endpoint::{self, ManualReason, RecoveryCommand, RecoveryReply};
use panelink::provider_recovery_runtime::ManualRecovery;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"PLBRSN02";
const HEADER: usize = 24;
const PREPARED_STATE_OFFSET: usize = 144;
const REQUEST_DEADLINE: Duration = Duration::from_secs(8);

#[derive(Clone, Copy)]
enum Validation {
    Verified,
    Missing,
    ForeignProject,
    Child,
}

struct SourceStats {
    navigation_count: AtomicUsize,
    spent_before_navigation: AtomicBool,
    hold_navigation_ack: AtomicBool,
    release_navigation_ack: AtomicBool,
    selected_id: Mutex<Option<String>>,
    sequence: AtomicU64,
}

struct Fixture {
    root: PathBuf,
    project: PathBuf,
    binding_store: PathBuf,
    prepared_store: PathBuf,
    endpoint_socket: PathBuf,
    target: TargetRef,
    scope: ManagedProviderScope,
    stats: Arc<SourceStats>,
    source_thread: Option<JoinHandle<()>>,
    source_done: mpsc::Receiver<()>,
    runtime: Option<ProviderBridgeRuntime>,
    manual: Option<ManualRecovery>,
}

impl Fixture {
    fn new(validations: Vec<(&'static str, Validation)>) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = Path::new("/private/tmp").join(format!("panelink-p4-manual-{nonce}"));
        let project = root.join("project");
        let binding_store = root.join("binding");
        let prepared_store = root.join("prepared");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("private root");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&project)
            .expect("private project");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&binding_store)
            .expect("private binding store");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&prepared_store)
            .expect("private prepared store");

        let target = TargetRef {
            generation: 301,
            workspace: 307,
            pane: 311,
            session: 313,
            occupant_revision: 317,
        };
        let scope = ManagedProviderScope::new(
            target,
            project.as_os_str().as_encoded_bytes(),
            SUPPORTED_PROVIDER_VERSION,
            OperationKey {
                generation: target.generation,
                lease: 331,
                sequence: 337,
            },
            [0x5a; 32],
            None,
        )
        .expect("managed scope");

        let source_socket = root.join("provider.sock");
        let endpoint_socket = root.join("recovery.sock");
        let listener = UnixListener::bind(&source_socket).expect("private source listener");
        let stats = Arc::new(SourceStats {
            navigation_count: AtomicUsize::new(0),
            spent_before_navigation: AtomicBool::new(false),
            hold_navigation_ack: AtomicBool::new(false),
            release_navigation_ack: AtomicBool::new(false),
            selected_id: Mutex::new(None),
            sequence: AtomicU64::new(0),
        });
        let source_stats = Arc::clone(&stats);
        let prepared_path = prepared_store.join("prepared-resume.record");
        let source_path = source_socket.clone();
        let source_validations = validations;
        let (source_done_tx, source_done) = mpsc::sync_channel(1);
        let source_thread = thread::spawn(move || {
            source_connect(source_path, source_stats, prepared_path, source_validations);
            let _ = source_done_tx.try_send(());
        });

        let runtime = ProviderBridgeRuntime::start_managed(
            &binding_store,
            scope.clone(),
            ManagedLaunchOutcome::Applied,
            listener,
        )
        .expect("bridge runtime");
        let manual = ManualRecovery::start(&prepared_store, &endpoint_socket, scope.clone())
            .expect("manual recovery owner");
        Self {
            root,
            project,
            binding_store,
            prepared_store,
            endpoint_socket,
            target,
            scope,
            stats,
            source_thread: Some(source_thread),
            source_done,
            runtime: Some(runtime),
            manual: Some(manual),
        }
    }

    fn warm(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(900);
        while Instant::now() < deadline {
            let runtime = self.runtime.as_mut().expect("runtime");
            let snapshot = runtime.poll(Instant::now());
            self.manual
                .as_mut()
                .expect("manual owner")
                .poll(runtime, Instant::now());
            if snapshot.bridge_instance().is_some()
                && matches!(
                    snapshot.status(),
                    RuntimeStatus::Unbound | RuntimeStatus::Stale
                )
                && self.stats.sequence.load(Ordering::Acquire) >= 1
            {
                thread::sleep(Duration::from_millis(20));
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("manual recovery fixture did not become ready");
    }

    fn exchange(&mut self, command: RecoveryCommand) -> RecoveryReply {
        let socket = self.endpoint_socket.clone();
        let target = self.target;
        let project = self.project.as_os_str().as_encoded_bytes().to_vec();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let client = thread::spawn(move || {
            let result = provider_recovery_endpoint::exchange(
                socket,
                target,
                &project,
                command,
                Instant::now() + REQUEST_DEADLINE,
            );
            let _ = reply_tx.send(result);
        });
        let deadline = Instant::now() + REQUEST_DEADLINE;
        let result = loop {
            let runtime = self.runtime.as_mut().expect("runtime");
            runtime.poll(Instant::now());
            self.manual
                .as_mut()
                .expect("manual owner")
                .poll(runtime, Instant::now());
            match reply_rx.try_recv() {
                Ok(result) => break result,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("recovery client exited without result")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            assert!(
                Instant::now() < deadline,
                "recovery request completed before deadline"
            );
            thread::sleep(Duration::from_millis(2));
        };
        client.join().expect("client thread");
        result.expect("recovery endpoint exchange")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.finish(Instant::now() + Duration::from_millis(250));
        }
        self.manual.take();
        if let Some(source_thread) = self.source_thread.take() {
            if self
                .source_done
                .recv_timeout(Duration::from_secs(1))
                .is_ok()
            {
                let _ = source_thread.join();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn source_connect(
    path: PathBuf,
    stats: Arc<SourceStats>,
    prepared_path: PathBuf,
    validations: Vec<(&'static str, Validation)>,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        if Instant::now() >= deadline {
            return;
        }
        match UnixStream::connect(&path) {
            Ok(stream) => break stream,
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => return,
        }
    };
    let _ = stream.set_nonblocking(true);
    let provision = read_packet(&mut stream, Instant::now() + Duration::from_secs(2))
        .expect("kind-5 provision");
    assert_eq!(provision.0, 5);
    assert_eq!(provision.1, 0);
    let hello = match provider_bridge_protocol::decode(&provision.2).expect("Hello codec") {
        Message::Hello(hello) => hello,
        _ => panic!("provision carried non-Hello"),
    };
    write_packet(
        &mut stream,
        1,
        0,
        &provider_bridge_protocol::encode(&Message::Hello(hello.clone())).expect("Hello encode"),
        Instant::now() + Duration::from_secs(2),
    )
    .expect("source Hello");
    let welcome =
        read_packet(&mut stream, Instant::now() + Duration::from_secs(2)).expect("Welcome");
    assert_eq!(welcome.0, 2);
    assert_eq!(welcome.1, 0);

    while Instant::now() < deadline {
        let (kind, challenge, payload) = match read_packet(&mut stream, deadline) {
            Ok(packet) => packet,
            Err(_) => return,
        };
        match kind {
            3 if payload.is_empty() => {
                let sequence = stats.sequence.fetch_add(1, Ordering::AcqRel) + 1;
                let selected = stats.selected_id.lock().expect("selected id").clone();
                let selection = match selected {
                    Some(id) => Selection::session(&id, VerificationSource::OrderedSelection)
                        .expect("selected route"),
                    None => Selection::Home,
                };
                let frame = Frame::new(
                    BRIDGE_PROTOCOL_VERSION,
                    hello.target(),
                    hello.project_identity(),
                    hello.provider_version(),
                    hello.bridge_instance(),
                    sequence,
                    FrameScope::TopLevel,
                    selection,
                )
                .expect("snapshot frame");
                let response = provider_bridge_protocol::encode(&Message::Observation(frame))
                    .expect("snapshot codec");
                write_packet(
                    &mut stream,
                    4,
                    challenge,
                    &response,
                    Instant::now() + Duration::from_secs(1),
                )
                .expect("snapshot response");
            }
            6 => {
                let id = decode_validate_id(&payload).expect("validation request");
                let status = validations
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
                    .map(|(_, validation)| *validation)
                    .unwrap_or(Validation::Verified);
                let response = encode_validation_response(id, hello.project_identity(), status);
                write_packet(
                    &mut stream,
                    7,
                    challenge,
                    &response,
                    Instant::now() + Duration::from_secs(1),
                )
                .expect("validation response");
            }
            8 => {
                let (id, revision, _ticket) = decode_navigation(&payload).expect("navigation");
                let bytes = read_bounded(&prepared_path).expect("prepared record before action");
                stats.spent_before_navigation.store(
                    bytes.get(PREPARED_STATE_OFFSET) == Some(&2),
                    Ordering::Release,
                );
                stats.navigation_count.fetch_add(1, Ordering::AcqRel);
                *stats.selected_id.lock().expect("selected id") = Some(id.to_owned());
                if stats.hold_navigation_ack.load(Ordering::Acquire) {
                    let gate_deadline = Instant::now() + Duration::from_secs(1);
                    while !stats.release_navigation_ack.load(Ordering::Acquire)
                        && Instant::now() < gate_deadline
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                let response = encode_navigation_response(id, revision, &payload[8..24]);
                write_packet(
                    &mut stream,
                    9,
                    challenge,
                    &response,
                    Instant::now() + Duration::from_secs(1),
                )
                .expect("navigation response");
            }
            _ => panic!("unexpected v2 message kind {kind}"),
        }
    }
}

fn read_packet(stream: &mut UnixStream, deadline: Instant) -> io::Result<(u16, u64, Vec<u8>)> {
    let mut header = [0u8; HEADER];
    read_exact_until(stream, &mut header, deadline)?;
    if &header[..8] != MAGIC || u16::from_be_bytes([header[8], header[9]]) != 2 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad v2 header"));
    }
    let kind = u16::from_be_bytes([header[10], header[11]]);
    let size = u32::from_be_bytes(header[12..16].try_into().expect("length")) as usize;
    let challenge = u64::from_be_bytes(header[16..24].try_into().expect("challenge"));
    if size > 5_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "v2 frame too large",
        ));
    }
    let mut payload = vec![0; size];
    read_exact_until(stream, &mut payload, deadline)?;
    Ok((kind, challenge, payload))
}

fn write_packet(
    stream: &mut UnixStream,
    kind: u16,
    challenge: u64,
    payload: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    if payload.len() > 5_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v2 frame too large",
        ));
    }
    let mut header = [0u8; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&2u16.to_be_bytes());
    header[10..12].copy_from_slice(&kind.to_be_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    header[16..24].copy_from_slice(&challenge.to_be_bytes());
    write_all_until(stream, &header, deadline)?;
    write_all_until(stream, payload, deadline)
}

fn read_exact_until(
    stream: &mut UnixStream,
    mut output: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !output.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixture read timed out",
            ));
        }
        match stream.read(output) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "fixture peer closed",
                ))
            }
            Ok(count) => output = &mut output[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_all_until(stream: &mut UnixStream, mut input: &[u8], deadline: Instant) -> io::Result<()> {
    while !input.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixture write timed out",
            ));
        }
        match stream.write(input) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "fixture write empty",
                ))
            }
            Ok(count) => input = &input[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn decode_validate_id(payload: &[u8]) -> io::Result<&str> {
    if payload.len() < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short validation",
        ));
    }
    let len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if len == 0 || payload.len() != len + 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad validation length",
        ));
    }
    std::str::from_utf8(&payload[2..])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad validation id"))
}

fn encode_validation_response(id: &str, project: &[u8], validation: Validation) -> Vec<u8> {
    let (status, response_id, response_project) = match validation {
        Validation::Verified => (1u8, id.as_bytes(), project),
        Validation::Missing => (2, &[][..], &[][..]),
        Validation::ForeignProject => (3, &[][..], &[][..]),
        Validation::Child => (4, &[][..], &[][..]),
    };
    let mut payload = vec![0; 8];
    payload[0] = status;
    payload[2..4].copy_from_slice(&(response_id.len() as u16).to_be_bytes());
    payload[4..6].copy_from_slice(&(response_project.len() as u16).to_be_bytes());
    payload.extend_from_slice(response_id);
    payload.extend_from_slice(response_project);
    payload
}

fn decode_navigation(payload: &[u8]) -> io::Result<(&str, u64, [u8; 16])> {
    if payload.len() < 29 || payload[26..28] != [0, 0] {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad navigation"));
    }
    let revision = u64::from_be_bytes(payload[..8].try_into().expect("revision"));
    let ticket = payload[8..24].try_into().expect("ticket");
    let id_len = u16::from_be_bytes([payload[24], payload[25]]) as usize;
    if payload.len() != 28 + id_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad navigation size",
        ));
    }
    let id = std::str::from_utf8(&payload[28..])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad navigation id"))?;
    Ok((id, revision, ticket))
}

fn encode_navigation_response(id: &str, revision: u64, ticket: &[u8]) -> Vec<u8> {
    let mut payload = vec![0; 28];
    payload[0] = 1;
    payload[2..4].copy_from_slice(&(id.len() as u16).to_be_bytes());
    payload[4..12].copy_from_slice(&revision.to_be_bytes());
    payload[12..28].copy_from_slice(ticket);
    payload.extend_from_slice(id.as_bytes());
    payload
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(16_000).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[test]
fn explicit_prepare_then_spend_navigate_and_fresh_durable_confirmation() {
    let mut fixture = Fixture::new(vec![
        ("session_deleted", Validation::Missing),
        ("session_foreign", Validation::ForeignProject),
        ("session_child", Validation::Child),
    ]);
    fixture.warm();
    for (id, reason) in [
        ("session_deleted", ManualReason::MissingSession),
        ("session_foreign", ManualReason::ForeignProject),
        ("session_child", ManualReason::ChildSession),
    ] {
        assert_eq!(
            fixture.exchange(RecoveryCommand::Prepare {
                session_id: id.to_owned(),
            }),
            RecoveryReply::ManualRequired { reason }
        );
        assert!(!fixture
            .prepared_store
            .join("prepared-resume.record")
            .exists());
        assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 0);
    }

    let prepared = fixture.exchange(RecoveryCommand::Prepare {
        session_id: "session_selected_later".to_owned(),
    });
    let (ticket, revision) = match prepared {
        RecoveryReply::Prepared { ticket, revision } => (ticket, revision),
        other => panic!("expected prepared receipt, got {other:?}"),
    };
    let prepared_path = fixture.prepared_store.join("prepared-resume.record");
    let bytes = read_bounded(&prepared_path).expect("prepared row after response");
    assert_eq!(
        bytes.get(PREPARED_STATE_OFFSET),
        Some(&1),
        "prepare is not spent"
    );
    assert!(!fixture
        .binding_store
        .join("managed-provider.record")
        .exists());
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 0);

    // A client that disconnects before the owner polls its queued command
    // cancels the request. It must not spend the prepared choice or navigate.
    send_raw_resume_and_disconnect(
        &fixture.endpoint_socket,
        fixture.target,
        fixture.project.as_os_str().as_encoded_bytes(),
        ticket,
        revision,
    );
    // A second request must get Busy because the disconnected resume is still
    // occupying the capacity-one owner channel. Receiving Busy proves the
    // endpoint worker observed the first peer's EOF and set its cancellation
    // flag before the owner polls that queued request.
    let socket = fixture.endpoint_socket.clone();
    let target = fixture.target;
    let project = fixture.project.as_os_str().as_encoded_bytes().to_vec();
    let (busy_tx, busy_rx) = mpsc::sync_channel(1);
    let busy_client = thread::spawn(move || {
        let result = provider_recovery_endpoint::exchange(
            socket,
            target,
            &project,
            RecoveryCommand::Status,
            Instant::now() + Duration::from_secs(1),
        );
        let _ = busy_tx.send(result);
    });
    assert_eq!(
        busy_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("endpoint processed cancellation before busy probe")
            .expect("fixed busy reply"),
        RecoveryReply::Busy
    );
    busy_client.join().expect("busy probe client thread");
    let cancel_deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < cancel_deadline {
        let runtime = fixture.runtime.as_mut().expect("runtime");
        runtime.poll(Instant::now());
        fixture
            .manual
            .as_mut()
            .expect("manual")
            .poll(runtime, Instant::now());
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 0);
    let bytes = read_bounded(&prepared_path).expect("prepared row remains");
    assert_eq!(bytes.get(PREPARED_STATE_OFFSET), Some(&1));

    let confirmed = fixture.exchange(RecoveryCommand::Resume { ticket, revision });
    let confirmed_revision = match confirmed {
        RecoveryReply::Confirmed { revision } => revision,
        other => panic!("expected confirmed result, got {other:?}"),
    };
    assert!(confirmed_revision >= revision);
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 1);
    assert!(fixture
        .stats
        .spent_before_navigation
        .load(Ordering::Acquire));
    let bytes = read_bounded(&prepared_path).expect("spent row after action");
    assert_eq!(bytes.get(PREPARED_STATE_OFFSET), Some(&2));
    assert!(
        fixture
            .stats
            .selected_id
            .lock()
            .expect("selected")
            .as_deref()
            == Some("session_selected_later")
    );

    let managed_path = fixture.binding_store.join("managed-provider.record");
    let managed_bytes = read_bounded(&managed_path).expect("managed record after confirmed reply");
    assert!(managed_bytes.starts_with(b"PLMPRV01"));
    assert_eq!(
        fixture
            .runtime
            .as_ref()
            .expect("runtime")
            .snapshot()
            .status(),
        RuntimeStatus::LastConfirmed
    );

    assert_eq!(
        fixture.exchange(RecoveryCommand::Resume { ticket, revision }),
        RecoveryReply::ManualRequired {
            reason: ManualReason::InterruptedOperation
        }
    );
    fixture.manual.take();
    fixture.manual = Some(
        ManualRecovery::start(
            &fixture.prepared_store,
            &fixture.endpoint_socket,
            fixture.scope.clone(),
        )
        .expect("reopened manual recovery"),
    );
    let warm_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < warm_deadline {
        let runtime = fixture.runtime.as_mut().expect("runtime");
        runtime.poll(Instant::now());
        fixture
            .manual
            .as_mut()
            .expect("manual")
            .poll(runtime, Instant::now());
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fixture.exchange(RecoveryCommand::Resume { ticket, revision }),
        RecoveryReply::ManualRequired {
            reason: ManualReason::InterruptedOperation
        }
    );
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 1);
}

fn send_raw_resume_and_disconnect(
    path: &Path,
    target: TargetRef,
    project: &[u8],
    ticket: [u8; 16],
    revision: u64,
) {
    let stream = raw_resume_client(path, target, project, ticket, revision);
    thread::sleep(Duration::from_millis(20));
    drop(stream);
}

fn raw_resume_client(
    path: &Path,
    target: TargetRef,
    project: &[u8],
    ticket: [u8; 16],
    revision: u64,
) -> UnixStream {
    let mut stream = UnixStream::connect(path).expect("recovery endpoint");
    let mut body = Vec::with_capacity(66 + project.len());
    for value in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ] {
        body.extend_from_slice(&value.to_be_bytes());
    }
    body.extend_from_slice(&(project.len() as u16).to_be_bytes());
    body.extend_from_slice(project);
    body.extend_from_slice(&ticket);
    body.extend_from_slice(&revision.to_be_bytes());
    let mut header = [0u8; 16];
    header[..8].copy_from_slice(b"PLRECV01");
    header[8..10].copy_from_slice(&1u16.to_be_bytes());
    header[10..12].copy_from_slice(&2u16.to_be_bytes());
    header[12..16].copy_from_slice(&(body.len() as u32).to_be_bytes());
    stream.write_all(&header).expect("recovery request header");
    stream.write_all(&body).expect("recovery request body");
    stream
}

#[test]
fn lost_resume_reply_stays_spent_after_reopen() {
    let mut fixture = Fixture::new(vec![]);
    fixture.warm();
    let (ticket, revision) = match fixture.exchange(RecoveryCommand::Prepare {
        session_id: "session_lost_ack".to_owned(),
    }) {
        RecoveryReply::Prepared { ticket, revision } => (ticket, revision),
        other => panic!("expected prepared receipt, got {other:?}"),
    };
    fixture
        .stats
        .hold_navigation_ack
        .store(true, Ordering::Release);
    let client = raw_resume_client(
        &fixture.endpoint_socket,
        fixture.target,
        fixture.project.as_os_str().as_encoded_bytes(),
        ticket,
        revision,
    );
    let action_deadline = Instant::now() + REQUEST_DEADLINE;
    while fixture.stats.navigation_count.load(Ordering::Acquire) == 0
        && Instant::now() < action_deadline
    {
        let runtime = fixture.runtime.as_mut().expect("runtime");
        runtime.poll(Instant::now());
        fixture
            .manual
            .as_mut()
            .expect("manual")
            .poll(runtime, Instant::now());
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 1);
    drop(client); // Lose the endpoint acknowledgment after the provider action.
    thread::sleep(Duration::from_millis(20));
    fixture
        .stats
        .release_navigation_ack
        .store(true, Ordering::Release);

    let confirm_deadline = Instant::now() + REQUEST_DEADLINE;
    while Instant::now() < confirm_deadline {
        let runtime = fixture.runtime.as_mut().expect("runtime");
        let snapshot = runtime.poll(Instant::now());
        fixture
            .manual
            .as_mut()
            .expect("manual")
            .poll(runtime, Instant::now());
        if snapshot.status() == RuntimeStatus::LastConfirmed {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fixture
            .runtime
            .as_ref()
            .expect("runtime")
            .snapshot()
            .status(),
        RuntimeStatus::LastConfirmed
    );
    assert!(fixture
        .binding_store
        .join("managed-provider.record")
        .exists());
    fixture.manual.take();
    fixture.manual = Some(
        ManualRecovery::start(
            &fixture.prepared_store,
            &fixture.endpoint_socket,
            fixture.scope.clone(),
        )
        .expect("reopened manual recovery"),
    );
    let warm_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < warm_deadline {
        let runtime = fixture.runtime.as_mut().expect("runtime");
        runtime.poll(Instant::now());
        fixture
            .manual
            .as_mut()
            .expect("manual")
            .poll(runtime, Instant::now());
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        fixture.exchange(RecoveryCommand::Resume { ticket, revision }),
        RecoveryReply::ManualRequired {
            reason: ManualReason::InterruptedOperation
        }
    );
    assert_eq!(fixture.stats.navigation_count.load(Ordering::Acquire), 1);
}
