#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Private local IPC evidence; this does not launch or attest a provider.

use panelink::control_protocol::TargetRef;
use panelink::provider_binding::{BindingState, VerificationSource};
use panelink::provider_binding_store::ProviderBindingStore;
use panelink::provider_bridge::{
    Admission, BridgeReceiver, Frame, FrameScope, LeasePolicy, Selection, BRIDGE_PROTOCOL_VERSION,
};
use panelink::provider_bridge_protocol::{self as wire, Hello, Message, HEADER_BYTES};
use panelink::provider_bridge_transport::{BridgeReader, BridgeSender, Grant};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);
const PROJECT: &[u8] = b"private-bridge-directory-identity";
const VERSION: &str = "2.0.32";
const SECRET: [u8; 32] = [19; 32]; // Deliberately public fixture capability.

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        for _ in 0..32 {
            let path = Path::new("/private/tmp").join(format!(
                "pl-p4-wire-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => panic!("private fixture creation failed"),
            }
        }
        panic!("private fixture allocation exhausted");
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 2,
        pane: 3,
        session: 4,
        occupant_revision: 5,
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn hello() -> Hello {
    Hello::new(target(), PROJECT, VERSION, 7, SECRET).unwrap()
}
fn grant() -> Grant {
    Grant::new(target(), PROJECT, VERSION, 7, SECRET).unwrap()
}
fn frame(sequence: u64, selection: Selection) -> Frame {
    Frame::new(
        BRIDGE_PROTOCOL_VERSION,
        target(),
        PROJECT,
        VERSION,
        7,
        sequence,
        FrameScope::TopLevel,
        selection,
    )
    .unwrap()
}
fn selected(sequence: u64, session: &str) -> Frame {
    frame(
        sequence,
        Selection::session(session, VerificationSource::TuiRouteSnapshot).unwrap(),
    )
}
fn receiver() -> BridgeReceiver {
    let mut receiver =
        BridgeReceiver::new(target(), PROJECT, 7, LeasePolicy::new(5000).unwrap()).unwrap();
    receiver.request_reconciliation();
    receiver
}
fn accept_before(listener: &UnixListener, stop: Instant) -> UnixStream {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < stop => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => panic!("private accept failed or expired"),
        }
    }
}

#[test]
fn private_pathname_channel_admits_persists_switches_and_revokes_home() {
    let fixture = Fixture::new();
    let socket = fixture.path().join("s");
    let listener = UnixListener::bind(&socket).expect("private pathname listener");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = thread::spawn(move || {
        let stream = UnixStream::connect(socket).expect("fixture connect");
        let mut sender = BridgeSender::connect(stream, hello(), deadline()).unwrap();
        sender
            .send_observation(&selected(1, "conversation_A"), deadline())
            .unwrap();
        sender
            .send_observation(&selected(2, "conversation_B"), deadline())
            .unwrap();
        sender
            .send_observation(&frame(3, Selection::Home), deadline())
            .unwrap();
    });
    let stream = accept_before(&listener, deadline());
    let mut reader = BridgeReader::accept(stream, grant(), deadline()).unwrap();
    let mut receiver = receiver();
    let mut store = ProviderBindingStore::open(fixture.path(), target(), PROJECT, VERSION).unwrap();
    let start = Instant::now();
    for revision in 1..=2 {
        let frame = reader.read_observation(deadline()).unwrap();
        let now = start.elapsed().as_millis() as u64;
        assert_eq!(
            receiver.receive(frame, now, 1000 + revision).unwrap(),
            Admission::Pending { revision }
        );
        receiver
            .confirm_pending(revision, now, |binding| store.save(binding).map(|_| ()))
            .unwrap();
        receiver.tick(start.elapsed().as_millis() as u64).unwrap();
        assert_eq!(receiver.state(), BindingState::Verified);
    }
    let home = reader.read_observation(deadline()).unwrap();
    assert_eq!(
        receiver
            .receive(home, start.elapsed().as_millis() as u64, 1003)
            .unwrap(),
        Admission::Revoked
    );
    assert_eq!(receiver.state(), BindingState::Stale);
    assert_eq!(
        receiver.last_confirmed().unwrap().session_id(),
        "conversation_B"
    );
    client.join().expect("fixture sender finished");
    drop(store);
    let store = ProviderBindingStore::open(fixture.path(), target(), PROJECT, VERSION).unwrap();
    let record = store.load().unwrap().unwrap();
    let restored = BridgeReceiver::restore(
        target(),
        PROJECT,
        8,
        LeasePolicy::new(5000).unwrap(),
        record,
        2,
    )
    .unwrap();
    assert_eq!(restored.state(), BindingState::Stale);
    assert!(restored.pending().is_none());
    assert_eq!(restored.last_confirmed().unwrap().revision(), 2);
}

fn raw_handshake(stream: &mut UnixStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    for part in wire::encode(&Message::Hello(hello())).unwrap().chunks(5) {
        stream.write_all(part).unwrap();
        thread::sleep(Duration::from_millis(1));
    }
    let mut header = [0; HEADER_BYTES];
    stream.read_exact(&mut header).unwrap();
    let decoded = wire::decode_header(&header).unwrap();
    let mut complete = header.to_vec();
    complete.resize(HEADER_BYTES + decoded.body_len(), 0);
    stream.read_exact(&mut complete[HEADER_BYTES..]).unwrap();
    assert!(matches!(
        wire::decode(&complete).unwrap(),
        Message::Welcome { bridge_instance: 7 }
    ));
}

#[test]
fn malformed_transport_cannot_replace_a_durable_binding_and_failure_revokes_owner() {
    let fixture = Fixture::new();
    let (server, mut client) = UnixStream::pair().unwrap();
    let sender = thread::spawn(move || {
        raw_handshake(&mut client);
        for part in wire::encode(&Message::Observation(selected(1, "conversation_A")))
            .unwrap()
            .chunks(7)
        {
            client.write_all(part).unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        let mut malicious =
            wire::encode(&Message::Observation(selected(2, "conversation_B"))).unwrap();
        malicious[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        client.write_all(&malicious[..HEADER_BYTES]).unwrap();
    });
    let mut reader = BridgeReader::accept(server, grant(), deadline()).unwrap();
    let mut receiver = receiver();
    let mut store = ProviderBindingStore::open(fixture.path(), target(), PROJECT, VERSION).unwrap();
    receiver
        .receive(reader.read_observation(deadline()).unwrap(), 1, 1001)
        .unwrap();
    receiver
        .confirm_pending(1, 2, |binding| store.save(binding).map(|_| ()))
        .unwrap();
    assert!(reader.read_observation(deadline()).is_err());
    assert!(reader.is_poisoned());
    assert!(reader.read_observation(deadline()).is_err());
    receiver.mark_unavailable(3).unwrap();
    assert_eq!(receiver.state(), BindingState::Unavailable);
    assert!(receiver.pending().is_none());
    assert_eq!(
        receiver.last_confirmed().unwrap().session_id(),
        "conversation_A"
    );
    assert_eq!(store.load().unwrap().unwrap().binding().revision(), 1);
    sender.join().expect("raw fixture sender finished");
}
