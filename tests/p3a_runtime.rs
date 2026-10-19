#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Socketpair proof of the multi-session runtime: real zsh children, the
//! schema-7 split route and the schema-8 UI protocol over authenticated
//! streams, with every PTY effect on the driver-owning test thread.

use panelink::control_protocol::TargetRef;
use panelink::handshake_protocol::{
    decode_welcome, Hello, Welcome, CAPABILITY_INPUT, CAPABILITY_READ,
};
use panelink::launch::{Environment, LaunchContext};
use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::screen_checkpoint::decode as decode_checkpoint;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{
    decode_session_info, encode_session_hello, encode_session_request, SessionHello, SessionInfo,
    SessionRequest,
};
use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
use panelink::terminal::Key;
use panelink::transport::{AuthenticatedStream, OwnedFrame};
use panelink::ui_protocol::{
    decode_layout_reply, decode_resize_reply, decode_screen_reply, encode_attach_target,
    encode_key_input, encode_layout_query, encode_root_resize, encode_screen_query,
    planned_split_size, KeyInput, LayoutReply, ResizeStatus, RootResize,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const IO_DEADLINE: Duration = Duration::from_secs(4);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(20);
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 81;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new(label: &str) -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p3a-{label}-{}-{serial}",
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
        .arg(&source)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .status()
        .expect("run tic");
    assert!(status.success(), "compile private terminfo");
}

/// Explicit zsh context: empty private HOME/ZDOTDIR, nothing inherited.
fn zsh_context(home: &Path) -> LaunchContext {
    LaunchContext {
        working_directory: home.to_path_buf(),
        environment: Environment::from([
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (OsString::from("ZDOTDIR"), home.as_os_str().to_owned()),
            (OsString::from("PS1"), OsString::from("P3A$ ")),
        ]),
    }
}

/// zsh -f: print READY with its PID, then run an interactive shell.
fn zsh_arguments(marker: &str) -> Vec<OsString> {
    vec![
        OsString::from("-f"),
        OsString::from("-c"),
        OsString::from(format!(
            "printf 'P3A-READY-{marker}-PID-%s-END\\n' \"$$\"; exec /bin/zsh -f -i"
        )),
    ]
}

struct Client {
    stream: AuthenticatedStream,
    welcome: Welcome,
    next_request: u64,
    next_sequence: u64,
}

impl Client {
    fn key(&mut self) -> OperationKey {
        self.next_sequence += 1;
        OperationKey {
            generation: self.welcome.generation,
            lease: self.welcome.lease,
            sequence: self.next_sequence,
        }
    }

    fn send(&mut self, payload: &[u8]) -> u64 {
        self.next_request += 1;
        let frame = Frame::new(MessageKind::Request, self.next_request, payload).unwrap();
        self.stream
            .write_frame(&frame, Instant::now() + IO_DEADLINE)
            .expect("write request");
        self.next_request
    }

    fn read(&mut self) -> OwnedFrame {
        self.stream
            .read_frame(Instant::now() + IO_DEADLINE)
            .expect("read reply")
    }

    fn request(&mut self, payload: &[u8]) -> Vec<u8> {
        let id = self.send(payload);
        let frame = self.read();
        assert_eq!(frame.request_id(), id);
        assert_eq!(
            frame.kind(),
            MessageKind::Response,
            "expected Response, got {:?}: {:?}",
            frame.kind(),
            String::from_utf8_lossy(frame.payload())
        );
        frame.payload().to_vec()
    }

    fn request_error(&mut self, payload: &[u8]) -> String {
        let id = self.send(payload);
        let frame = self.read();
        assert_eq!(frame.request_id(), id);
        assert_eq!(frame.kind(), MessageKind::Error);
        String::from_utf8(frame.payload().to_vec()).unwrap()
    }

    fn layout(&mut self) -> LayoutReply {
        decode_layout_reply(&self.request(&encode_layout_query())).expect("LayoutReply")
    }

    fn describe(&mut self) -> SessionInfo {
        decode_session_info(
            &self.request(&encode_session_request(&SessionRequest::Describe).unwrap()),
        )
        .unwrap()
    }

    fn screen(&mut self, target: TargetRef) -> String {
        let id = self.send(&encode_screen_query(target).unwrap());
        let frame = self.read();
        assert_eq!(frame.request_id(), id);
        assert_eq!(frame.kind(), MessageKind::Snapshot);
        let view = decode_screen_reply(frame.payload()).expect("ScreenReply");
        assert_eq!(view.target(), target);
        let checkpoint = decode_checkpoint(view.checkpoint()).expect("checkpoint");
        assert_eq!(
            checkpoint.metadata().screen_revision(),
            view.screen_revision()
        );
        let mut text = String::new();
        for row in 0..checkpoint.rows() {
            for column in 0..checkpoint.columns() {
                let cell = checkpoint.cell(row, column).unwrap();
                if !cell.is_wide_continuation() {
                    text.push_str(if cell.text().is_empty() {
                        " "
                    } else {
                        cell.text()
                    });
                }
            }
            text.push('\n');
        }
        text
    }

    fn claim(&mut self, target: TargetRef) -> u64 {
        let info = self.describe();
        assert_eq!(info.target, target);
        assert_eq!(info.owner_lease, 0, "nothing claims input implicitly");
        let key = self.key();
        let payload = encode_session_request(&SessionRequest::Claim {
            key,
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        assert_eq!(
            status(&self.request(&payload), key),
            OperationStatus::Completed(Outcome::Applied)
        );
        self.describe().owner_revision
    }

    fn keys(&mut self, target: TargetRef, owner_revision: u64, text: &str) -> OperationStatus {
        let mut keys: Vec<Key> = text.chars().map(Key::Char).collect();
        keys.push(Key::Enter);
        let key = self.key();
        let payload = encode_key_input(&KeyInput {
            key,
            target,
            expected_owner_revision: owner_revision,
            keys: &keys,
        })
        .unwrap();
        status(&self.request(&payload), key)
    }
}

fn status(payload: &[u8], key: OperationKey) -> OperationStatus {
    let response = decode_response_payload(payload).expect("schema-1 response");
    assert_eq!(response.key, key);
    match response.body {
        ResponseBody::Status(status) => status,
        other => panic!("expected status, got {other:?}"),
    }
}

fn connect(
    driver: &mut MultiServerDriver,
    target: Option<TargetRef>,
    nonce: u64,
) -> (UnixStream, Option<TargetRef>, u64) {
    let (server, client) = UnixStream::pair().unwrap();
    driver
        .attach(AuthenticatedStream::new(server).unwrap())
        .expect("admit connection worker");
    (client, target, nonce)
}

fn handshake((socket, target, nonce): (UnixStream, Option<TargetRef>, u64)) -> Client {
    let mut stream = AuthenticatedStream::new(socket).unwrap();
    let mut payload = encode_session_hello(&SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    })
    .unwrap();
    if let Some(target) = target {
        payload.extend_from_slice(&encode_attach_target(target).unwrap());
    }
    stream
        .write_frame(
            &Frame::new(MessageKind::Hello, 0, &payload).unwrap(),
            Instant::now() + IO_DEADLINE,
        )
        .unwrap();
    let frame = stream.read_frame(Instant::now() + IO_DEADLINE).unwrap();
    assert_eq!(frame.kind(), MessageKind::Welcome);
    let welcome = decode_welcome(frame.payload()).unwrap();
    stream.enable_snapshot_reads().unwrap();
    Client {
        stream,
        welcome,
        next_request: 0,
        next_sequence: 0,
    }
}

/// Run `operation` on a client thread while this thread ticks the driver.
fn drive<T: Send>(
    driver: &mut MultiServerDriver,
    started: Instant,
    operation: impl FnOnce() -> T + Send,
) -> T {
    thread::scope(|scope| {
        let client = scope.spawn(operation);
        let deadline = Instant::now() + SCENARIO_DEADLINE;
        while !client.is_finished() {
            let _ = driver.tick(|| 100 + started.elapsed().as_secs());
            assert!(
                Instant::now() < deadline,
                "socket exchange exceeded deadline"
            );
            thread::sleep(Duration::from_millis(1));
        }
        client.join().expect("client thread")
    })
}

fn wait_screen(client: &mut Client, target: TargetRef, marker: &str, forbidden: &[&str]) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        let screen = client.screen(target);
        for text in forbidden {
            assert!(
                !screen.contains(text),
                "pane must not show {text:?}:\n{screen}"
            );
        }
        if screen.contains(marker) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "marker {marker:?} missing:\n{screen}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn marker_pid(screen: &str, marker: &str) -> i32 {
    let start = screen.find(marker).expect("marker") + marker.len();
    let rest = screen[start..].strip_prefix("-PID-").expect("pid prefix");
    rest[..rest.find("-END").unwrap()].parse().unwrap()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn split_payload(
    client: &mut Client,
    layout: &LayoutReply,
    target: TargetRef,
    direction: SplitDirection,
    context: &LaunchContext,
    marker: &str,
) -> (OperationKey, Vec<u8>) {
    let (rows, columns) = planned_split_size(layout, target, direction).expect("planned size");
    let key = client.key();
    let arguments = zsh_arguments(marker);
    let arguments: Vec<&[u8]> = arguments
        .iter()
        .map(|argument| argument.as_bytes())
        .collect();
    let environment: Vec<EnvironmentEntry<'_>> = context
        .environment
        .iter()
        .map(|(name, value)| EnvironmentEntry {
            name: name.as_bytes(),
            value: value.as_bytes(),
        })
        .collect();
    let cwd = context.working_directory.as_os_str().as_bytes();
    let launch = LaunchRequest {
        key,
        rows,
        columns,
        requested_executable: b"/bin/zsh",
        requested_working_directory: cwd,
        client_working_directory: cwd,
        arguments: &arguments,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_split_request(
        &SplitRequest {
            key,
            target,
            expected_layout_revision: layout.revision,
            direction,
            launch: &launch,
        },
        &mut payload,
    )
    .unwrap();
    (key, payload)
}

#[test]
fn split_ui_keys_and_resize_over_socketpairs_with_two_real_zsh_children() {
    let storage_dir = PrivateDir::new("storage");
    let terminfo = PrivateDir::new("terminfo");
    let home = PrivateDir::new("home");
    compile_terminfo(&terminfo.0);
    assert!(
        fs::read_dir(&home.0).unwrap().next().is_none(),
        "empty HOME/ZDOTDIR"
    );
    let context = zsh_context(&home.0);
    let storage = ServerStorage::initialize(&storage_dir.0).unwrap();
    let first_arguments = zsh_arguments("one");
    let started = StartedMultiServer::start(
        storage,
        &terminfo.0,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &first_arguments,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .expect("start multi-session server");
    assert_eq!(started.startup().outcome, Outcome::Applied);
    let first = started.startup().target.unwrap();
    let mut driver = started.into_driver();
    let t0 = Instant::now();

    // Pane 1: layout, READY, claim and typed keys.
    let link = connect(&mut driver, None, 11);
    let (mut one, first_pid) = drive(&mut driver, t0, move || {
        let mut one = handshake(link);
        let layout = one.layout();
        assert_eq!(layout.panes.iter().flatten().count(), 1);
        assert_eq!(
            (layout.revision, layout.rows, layout.columns),
            (1, ROOT_ROWS, ROOT_COLUMNS)
        );
        assert_eq!(layout.panes[0].unwrap().target, first);
        let ready = wait_screen(&mut one, first, "P3A-READY-one", &[]);
        let pid = marker_pid(&ready, "P3A-READY-one");
        // Client launch (schema 6 kind 1) is not offered in this phase.
        let launch_key = one.key();
        let mut launch = Vec::new();
        panelink::managed_launch_protocol::encode_launch_request(
            &LaunchRequest {
                key: launch_key,
                rows: 24,
                columns: 80,
                requested_executable: b"/bin/zsh",
                requested_working_directory: b"/",
                client_working_directory: b"/",
                arguments: &[],
                environment: &[],
            },
            &mut launch,
        )
        .unwrap();
        assert_eq!(one.request_error(&launch), "launch_unsupported");
        // Keys before claiming input ownership are rejected, not delivered.
        let vacant = one.describe().owner_revision;
        let key = one.key();
        let payload = encode_key_input(&KeyInput {
            key,
            target: first,
            expected_owner_revision: vacant,
            keys: &[Key::Char('x')],
        })
        .unwrap();
        assert!(matches!(
            decode_response_payload(&one.request(&payload))
                .unwrap()
                .body,
            ResponseBody::Rejected(_)
        ));
        let revision = one.claim(first);
        let key = one.key();
        let mut typed: Vec<Key> = "echo ACK1".chars().map(Key::Char).collect();
        typed.push(Key::Enter);
        let payload = encode_key_input(&KeyInput {
            key,
            target: first,
            expected_owner_revision: revision,
            keys: &typed,
        })
        .unwrap();
        assert_eq!(
            status(&one.request(&payload), key),
            OperationStatus::Completed(Outcome::Applied)
        );
        let screen = wait_screen(&mut one, first, "\nACK1", &[]);
        // An exact replay returns the recorded status and is never re-sent.
        assert_eq!(
            status(&one.request(&payload), key),
            OperationStatus::Completed(Outcome::Applied)
        );
        thread::sleep(Duration::from_millis(200));
        let again = one.screen(first);
        assert_eq!(
            again.matches("\nACK1").count(),
            screen.matches("\nACK1").count()
        );
        assert_eq!(again.matches("\nACK1").count(), 1);
        (one, pid)
    });
    assert!(alive(first_pid));

    // Split Right over the socket with a client-built schema-7 request.
    let split_context = context.clone();
    let (one_back, split, split_bytes, layout) = drive(&mut driver, t0, move || {
        let layout = one.layout();
        // A stale layout revision is recorded Failed before any effect.
        let stale_layout = LayoutReply {
            revision: 9,
            ..layout
        };
        let (_, stale) = split_payload(
            &mut one,
            &stale_layout,
            first,
            SplitDirection::Right,
            &split_context,
            "stale",
        );
        let stale = decode_split_result(&one.request(&stale)).expect("stale result");
        assert_eq!(stale.outcome, Outcome::Failed);
        assert!(stale.new_target.is_none());
        assert_eq!(one.layout().revision, 1);
        let (_, bytes) = split_payload(
            &mut one,
            &layout,
            first,
            SplitDirection::Right,
            &split_context,
            "two",
        );
        let result = decode_split_result(&one.request(&bytes)).expect("SplitResult");
        let layout = one.layout();
        (one, result, bytes, layout)
    });
    one = one_back;
    assert_eq!(split.outcome, Outcome::Applied);
    assert_eq!(split.committed_layout_revision, 2);
    let second = split.new_target.expect("new target");
    assert_eq!(layout.panes.iter().flatten().count(), 2);
    assert_eq!(layout.panes[1].unwrap().target, second);
    assert_eq!(
        (
            layout.panes[0].unwrap().rect.columns,
            layout.panes[1].unwrap().rect.columns
        ),
        (40, 40)
    );
    assert!(layout.divider.is_some());

    // Pane 2 on its own connection pinned to the new target.
    let link = connect(&mut driver, Some(second), 22);
    let (mut two, second_pid) = drive(&mut driver, t0, move || {
        let mut two = handshake(link);
        assert_eq!(two.describe().target, second);
        let ready = wait_screen(
            &mut two,
            second,
            "P3A-READY-two",
            &["P3A-READY-one", "ACK1"],
        );
        let pid = marker_pid(&ready, "P3A-READY-two");
        // Pinned scope: pane 2's connection cannot read pane 1.
        assert_eq!(
            two.request_error(&encode_screen_query(first).unwrap()),
            "unauthorized"
        );
        let revision = two.claim(second);
        assert_eq!(
            two.keys(second, revision, "echo ACK2"),
            OperationStatus::Completed(Outcome::Applied)
        );
        wait_screen(&mut two, second, "\nACK2", &["ACK1"]);
        (two, pid)
    });
    assert_ne!(first_pid, second_pid);
    assert!(alive(first_pid) && alive(second_pid));

    // ACK2 never reaches pane 1; lost-ack replay of the split is a no-op.
    let (one_back, retry, third) = drive(&mut driver, t0, move || {
        let screen = one.screen(first);
        assert!(!screen.contains("ACK2") && !screen.contains("P3A-READY-two"));
        let retry = decode_split_result(&one.request(&split_bytes)).expect("retry result");
        let layout = one.layout();
        assert_eq!(layout.revision, 2);
        // Third split: a fresh key at capacity is recorded Failed.
        let fake_single = LayoutReply {
            panes: [layout.panes[0], None],
            divider: None,
            ..layout
        };
        let fake_single = LayoutReply {
            panes: [
                Some(panelink::ui_protocol::UiPane {
                    rect: panelink::pane_layout::Rect {
                        x: 0,
                        y: 0,
                        rows: layout.rows,
                        columns: layout.columns,
                    },
                    ..fake_single.panes[0].unwrap()
                }),
                None,
            ],
            ..fake_single
        };
        let (_, bytes) = split_payload(
            &mut one,
            &fake_single,
            first,
            SplitDirection::Down,
            &zsh_context(Path::new("/private/tmp")),
            "three",
        );
        let third = decode_split_result(&one.request(&bytes)).expect("third result");
        (one, retry, third)
    });
    one = one_back;
    assert_eq!(retry, split);
    assert_eq!(third.outcome, Outcome::Failed);
    assert_eq!(third.committed_layout_revision, 0);
    assert!(third.new_target.is_none());
    assert_eq!(driver.owner().layout_snapshot().unwrap().pane_count(), 2);

    // RootResize: both PTYs report their new rectangles via stty.
    let (one_back, two_back) = drive(&mut driver, t0, move || {
        let key = one.key();
        let payload = encode_root_resize(&RootResize {
            key,
            expected_layout_revision: 2,
            rows: 30,
            columns: 101,
        })
        .unwrap();
        let result = decode_resize_reply(&one.request(&payload)).expect("ResizeReply");
        assert_eq!(result.status, ResizeStatus::Applied);
        assert_eq!((result.revision, result.rows, result.columns), (3, 30, 101));
        let layout = one.layout();
        let left = layout.panes[0].unwrap().rect;
        let right = layout.panes[1].unwrap().rect;
        assert_eq!(
            (left.rows, left.columns, right.rows, right.columns),
            (30, 50, 30, 50)
        );
        let owner = one.describe().owner_revision;
        assert_eq!(
            one.keys(first, owner, "echo SZ1-$(stty size | tr ' ' x)"),
            OperationStatus::Completed(Outcome::Applied)
        );
        wait_screen(&mut one, first, "SZ1-30x50", &[]);
        let owner = two.describe().owner_revision;
        assert_eq!(
            two.keys(second, owner, "echo SZ2-$(stty size | tr ' ' x)"),
            OperationStatus::Completed(Outcome::Applied)
        );
        wait_screen(&mut two, second, "SZ2-30x50", &[]);
        // A stale layout revision is rejected without effects.
        let stale = encode_root_resize(&RootResize {
            key: one.key(),
            expected_layout_revision: 2,
            rows: 24,
            columns: 81,
        })
        .unwrap();
        assert_eq!(one.request_error(&stale), "stale_layout");
        (one, two)
    });

    // Disconnecting every client stops no child.
    drop(one_back);
    drop(two_back);
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    while driver.active_connections() != 0 {
        let _ = driver.tick(|| 100 + t0.elapsed().as_secs());
        assert!(Instant::now() < deadline, "workers close after disconnect");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        alive(first_pid) && alive(second_pid),
        "children outlive clients"
    );
    assert_eq!(driver.owner().layout_snapshot().unwrap().revision, 3);

    // Teardown reaps both children and releases the storage lock.
    drop(driver);
    for pid in [first_pid, second_pid] {
        assert!(!alive(pid), "owner teardown reaps child");
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
    // Bounded: a concurrent fork elsewhere may briefly share the lock fd.
    let deadline = Instant::now() + Duration::from_secs(3);
    while ServerStorage::open(&storage_dir.0).is_err() {
        assert!(
            Instant::now() < deadline,
            "storage lock released after drop"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
