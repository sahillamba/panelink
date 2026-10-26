#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Private-PTY/socketpair coverage for schema-11 workspace layout events.

use panelink::handshake_protocol::{
    decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ, CAPABILITY_WORKSPACE_EVENTS,
};
use panelink::launch::{Environment, LaunchContext};
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::protocol::OperationKey;
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{encode_session_hello, SessionHello};
use panelink::transport::{AuthenticatedStream, OwnedFrame};
use panelink::ui_protocol::{
    decode_layout_reply, decode_resize_reply, encode_layout_query, encode_root_resize,
    ResizeStatus, RootResize,
};
use panelink::watch_protocol::WatchStatus;
use panelink::workspace_watch_protocol::{
    decode_reconciliation, decode_watch_reply, encode_request, Request, WorkspaceCursor,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const IO_DEADLINE: Duration = Duration::from_secs(4);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(12);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new(label: &str) -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p3-workspace-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create private fixture directory");
        Self(path)
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compile_terminfo(directory: &Path) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
    let status = Command::new("/usr/bin/tic")
        .args([OsStr::new("-x"), OsStr::new("-o")])
        .arg(directory)
        .arg(source)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .status()
        .expect("run tic");
    assert!(status.success(), "compile private terminfo");
}

fn context(home: &Path) -> LaunchContext {
    LaunchContext {
        working_directory: home.to_path_buf(),
        environment: Environment::from([
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (OsString::from("ZDOTDIR"), home.as_os_str().to_owned()),
            (OsString::from("PS1"), OsString::from("P3WE$ ")),
        ]),
    }
}

fn shell_arguments() -> Vec<OsString> {
    vec![
        OsString::from("-f"),
        OsString::from("-c"),
        OsString::from("printf 'P3WE-READY\\n'; exec /bin/zsh -f -i"),
    ]
}

struct Client {
    stream: AuthenticatedStream,
    generation: u64,
    lease: u64,
    request_id: u64,
    sequence: u64,
}

impl Client {
    fn send(&mut self, payload: &[u8]) -> u64 {
        self.request_id += 1;
        let frame = Frame::new(MessageKind::Request, self.request_id, payload).unwrap();
        self.stream
            .write_frame(&frame, Instant::now() + IO_DEADLINE)
            .expect("write request");
        self.request_id
    }

    fn read(&mut self, request_id: u64) -> OwnedFrame {
        let frame = self
            .stream
            .read_frame(Instant::now() + IO_DEADLINE)
            .expect("read reply");
        assert_eq!(frame.request_id(), request_id);
        frame
    }

    fn request(&mut self, payload: &[u8]) -> OwnedFrame {
        let id = self.send(payload);
        self.read(id)
    }

    fn key(&mut self) -> OperationKey {
        self.sequence += 1;
        OperationKey {
            generation: self.generation,
            lease: self.lease,
            sequence: self.sequence,
        }
    }
}

fn connect(driver: &mut MultiServerDriver) -> UnixStream {
    let (server, client) = UnixStream::pair().unwrap();
    driver
        .attach(AuthenticatedStream::new(server).unwrap())
        .expect("admit socketpair worker");
    client
}

fn handshake(socket: UnixStream, nonce: u64, capabilities: u64) -> Client {
    let mut stream = AuthenticatedStream::new(socket).unwrap();
    let hello = encode_session_hello(&SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: capabilities,
        },
        resume: None,
    })
    .unwrap();
    stream
        .write_frame(
            &Frame::new(MessageKind::Hello, 0, &hello).unwrap(),
            Instant::now() + IO_DEADLINE,
        )
        .unwrap();
    let welcome = stream
        .read_frame(Instant::now() + IO_DEADLINE)
        .expect("Welcome");
    assert_eq!(welcome.kind(), MessageKind::Welcome);
    let welcome = decode_welcome(welcome.payload()).unwrap();
    stream.enable_snapshot_reads().unwrap();
    Client {
        stream,
        generation: welcome.generation,
        lease: welcome.lease,
        request_id: 0,
        sequence: 0,
    }
}

fn drive<T: Send>(driver: &mut MultiServerDriver, operation: impl FnOnce() -> T + Send) -> T {
    thread::scope(|scope| {
        let client = scope.spawn(operation);
        let deadline = Instant::now() + SCENARIO_DEADLINE;
        while !client.is_finished() {
            let _ = driver.tick(|| 100);
            assert!(Instant::now() < deadline, "socketpair exchange timed out");
            thread::sleep(Duration::from_millis(1));
        }
        client.join().expect("client operation")
    })
}

fn start() -> (
    PrivateDir,
    MultiServerDriver,
    panelink::control_protocol::TargetRef,
) {
    let storage_dir = PrivateDir::new("storage");
    let terminfo = PrivateDir::new("terminfo");
    let home = PrivateDir::new("home");
    compile_terminfo(&terminfo.0);
    let context = context(&home.0);
    let storage = ServerStorage::initialize(&storage_dir.0).unwrap();
    let arguments = shell_arguments();
    let started = StartedMultiServer::start(
        storage,
        &terminfo.0,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &arguments,
            context: &context,
            rows: 24,
            columns: 81,
        },
        true,
    )
    .expect("start private-PTY server");
    assert_eq!(
        started.startup().outcome,
        panelink::protocol::Outcome::Applied
    );
    let target = started.startup().target.unwrap();
    (storage_dir, started.into_driver(), target)
}

#[test]
fn workspace_watch_is_scoped_finite_read_only_and_does_not_block_layout_work() {
    let (_storage_dir, mut driver, target) = start();
    let event_socket = connect(&mut driver);
    let control_socket = connect(&mut driver);
    let denied_socket = connect(&mut driver);
    let (mut events, mut control, mut denied) = drive(&mut driver, move || {
        (
            handshake(
                event_socket,
                101,
                CAPABILITY_INPUT | CAPABILITY_WORKSPACE_EVENTS,
            ),
            handshake(control_socket, 102, CAPABILITY_INPUT),
            handshake(denied_socket, 103, 0),
        )
    });

    let reconcile_request = encode_request(&Request::Reconcile {
        generation: target.generation,
        workspace: target.workspace,
    })
    .unwrap();
    let reconcile_request_initial = reconcile_request.clone();
    let (mut events, initial) = drive(&mut driver, move || {
        let first = events.request(&reconcile_request_initial);
        assert_eq!(first.kind(), MessageKind::Response);
        let initial = decode_reconciliation(first.payload()).unwrap();
        let second = events.request(&reconcile_request_initial);
        let repeated = decode_reconciliation(second.payload()).unwrap();
        assert_eq!(repeated, initial, "reconcile is read-only and atomic");
        (events, initial)
    });
    assert_eq!(initial.cursor.generation, target.generation);
    assert_eq!(initial.cursor.workspace, target.workspace);
    assert_eq!(initial.revision, 1);
    assert!(initial.layout.is_some());

    let bad_scope = encode_request(&Request::Watch {
        cursor: WorkspaceCursor {
            workspace: target.workspace + 1,
            ..initial.cursor
        },
        limit: 8,
        wait_ms: 0,
    })
    .unwrap();
    let future = encode_request(&Request::Watch {
        cursor: WorkspaceCursor {
            sequence: initial.cursor.sequence + 1,
            ..initial.cursor
        },
        limit: 8,
        wait_ms: 0,
    })
    .unwrap();
    let no_capability = drive(&mut driver, move || {
        let unauthorized = events.request(&bad_scope);
        let ahead = events.request(&future);
        let denied_reply = denied.request(&reconcile_request);
        (events, denied_reply, unauthorized, ahead)
    });
    events = no_capability.0;
    assert_eq!(no_capability.1.kind(), MessageKind::Error);
    assert_eq!(no_capability.1.payload(), b"capability_denied");
    assert_eq!(no_capability.2.kind(), MessageKind::Error);
    assert_eq!(no_capability.2.payload(), b"unauthorized");
    assert_eq!(no_capability.3.kind(), MessageKind::Error);
    assert_eq!(no_capability.3.payload(), b"cursor_ahead");

    // A positive watch remains pending while the owner continues to answer a
    // separate layout query. The following root resize commits one revision
    // and wakes the watcher with the invalidation.
    let watch_payload = encode_request(&Request::Watch {
        cursor: initial.cursor,
        limit: 8,
        wait_ms: 1_000,
    })
    .unwrap();
    let watch_payload_repeat = watch_payload.clone();
    let watcher = thread::spawn(move || {
        let request_id = events.send(&watch_payload);
        let frame = events.read(request_id);
        (events, frame)
    });
    let pending_deadline = Instant::now() + Duration::from_secs(3);
    while driver.pending_watches() == 0 {
        let _ = driver.tick(|| 100);
        assert!(
            Instant::now() < pending_deadline,
            "watch entered pending set"
        );
        thread::sleep(Duration::from_millis(1));
    }
    let (mut control, observed_layout) = drive(&mut driver, move || {
        let frame = control.request(&encode_layout_query());
        assert_eq!(frame.kind(), MessageKind::Response);
        (control, decode_layout_reply(frame.payload()).unwrap())
    });
    assert!(
        driver.pending_watches() > 0,
        "layout query did not consume watch"
    );
    assert_eq!(observed_layout.revision, initial.revision);

    let resize_key = control.key();
    let resize_payload = encode_root_resize(&RootResize {
        key: resize_key,
        expected_layout_revision: observed_layout.revision,
        rows: 26,
        columns: 83,
    })
    .unwrap();
    let (mut control, resize) = drive(&mut driver, move || {
        let frame = control.request(&resize_payload);
        assert_eq!(frame.kind(), MessageKind::Response);
        (control, decode_resize_reply(frame.payload()).unwrap())
    });
    assert_eq!(resize.status, ResizeStatus::Applied);
    assert_eq!(resize.revision, initial.revision + 1);
    let (mut events, changed_frame) = watcher.join().expect("watcher thread");
    assert_eq!(changed_frame.kind(), MessageKind::Response);
    let changed = decode_watch_reply(changed_frame.payload()).unwrap();
    assert_eq!(changed.status, WatchStatus::Changed);
    assert_eq!(changed.events.len(), 1);
    assert_eq!(changed.events[0].revision, resize.revision);
    assert_eq!(changed.cursor.sequence, initial.cursor.sequence + 1);

    // Retrying this read returns the same retained event; it cannot replay a
    // mutation or advance the workspace journal.
    let repeated = drive(&mut driver, move || {
        let frame = events.request(&watch_payload_repeat);
        (events, decode_watch_reply(frame.payload()).unwrap())
    });
    events = repeated.0;
    assert_eq!(repeated.1, changed);

    // More than the 64 retained records expires the original cursor. Every
    // resize is a fresh, explicit operation with a new key and revision.
    let (mut control, final_revision) = drive(&mut driver, move || {
        let mut revision = resize.revision;
        for index in 0..65 {
            let key = control.key();
            let (rows, columns) = if index % 2 == 0 { (27, 84) } else { (26, 83) };
            let payload = encode_root_resize(&RootResize {
                key,
                expected_layout_revision: revision,
                rows,
                columns,
            })
            .unwrap();
            let frame = control.request(&payload);
            assert_eq!(frame.kind(), MessageKind::Response);
            let result = decode_resize_reply(frame.payload()).unwrap();
            assert_eq!(result.status, ResizeStatus::Applied);
            revision = result.revision;
        }
        (control, revision)
    });
    assert_eq!(final_revision, resize.revision + 65);
    let old_cursor = encode_request(&Request::Watch {
        cursor: initial.cursor,
        limit: 64,
        wait_ms: 0,
    })
    .unwrap();
    let (mut events, expired) = drive(&mut driver, move || {
        let frame = events.request(&old_cursor);
        (events, frame)
    });
    assert_eq!(expired.kind(), MessageKind::Response);
    assert_eq!(
        decode_watch_reply(expired.payload()).unwrap().status,
        WatchStatus::ResyncRequired
    );

    // A timeout response after reconciliation is stable and read-only.
    let latest = drive(&mut driver, move || {
        let layout = control.request(&encode_layout_query());
        assert_eq!(layout.kind(), MessageKind::Response);
        (control, decode_layout_reply(layout.payload()).unwrap())
    });
    control = latest.0;
    assert_eq!(latest.1.revision, final_revision);
    let next_cursor = WorkspaceCursor {
        sequence: changed.cursor.sequence + 65,
        ..initial.cursor
    };
    let timeout_payload = encode_request(&Request::Watch {
        cursor: next_cursor,
        limit: 8,
        wait_ms: 0,
    })
    .unwrap();
    let (mut events, timed_out) = drive(&mut driver, move || {
        let frame = events.request(&timeout_payload);
        (events, frame)
    });
    assert_eq!(timed_out.kind(), MessageKind::Response);
    let timed_out = decode_watch_reply(timed_out.payload()).unwrap();
    assert_eq!(timed_out.status, WatchStatus::TimedOut);
    assert_eq!(timed_out.cursor.sequence, next_cursor.sequence);

    let stop_payload = encode_request(&Request::Watch {
        cursor: timed_out.cursor,
        limit: 8,
        wait_ms: 1_000,
    })
    .unwrap();
    let stopping_watch = thread::spawn(move || {
        let _id = events.send(&stop_payload);
        events.stream.read_frame(Instant::now() + IO_DEADLINE).ok()
    });
    let pending_deadline = Instant::now() + Duration::from_secs(3);
    while driver.pending_watches() == 0 {
        let _ = driver.tick(|| 100);
        assert!(
            Instant::now() < pending_deadline,
            "second watch entered pending set"
        );
        thread::sleep(Duration::from_millis(1));
    }
    driver.stop_connections(|| 100);
    assert_eq!(
        driver.pending_watches(),
        0,
        "stop clears pending watch state"
    );
    if let Some(stopped) = stopping_watch.join().expect("stopped workspace worker") {
        assert_eq!(stopped.kind(), MessageKind::Error);
        assert_eq!(stopped.payload(), b"server_stopping");
    }
    drop(control);
}
