#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Saved-layout driver lifecycle acceptance (P5-SAVED-LAYOUT stage 4).
//!
//! Every scenario uses real `/bin/sh` or `/bin/zsh` children, private
//! `/private/tmp` roots, and socketpair-based UI clients (never pathname
//! listeners).  Defects in production code are reported via
//! `#[ignore = "parent defect: ..."]` tests; this file never edits `src/`.

use panelink::close_protocol::{encode_close_request, CloseOutcome, CloseRequest, StopOutcome};
use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::launch::{Environment, LaunchContext};
use panelink::layout_store::{LayoutSlot, LayoutStore};
use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::server_storage::ServerStorage;
use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
use panelink::terminal::Key;
use panelink::transport::{AuthenticatedStream, OwnedFrame};
use panelink::ui_protocol::{
    decode_layout_reply, decode_resize_reply, encode_attach_target, encode_key_input,
    encode_layout_query, encode_root_resize, encode_screen_query, planned_split_size, KeyInput,
    LayoutReply, ResizeStatus, RootResize,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

// ==================================================================
// Fixture helpers
// ==================================================================

struct Fixture {
    root: PathBuf,
    server: PathBuf,
    layout: PathBuf,
    history: PathBuf,
    terminfo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-layout-lc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Self {
            server: root.join("server"),
            layout: root.join("server/layout"),
            history: root.join("server/history"),
            terminfo: root.join("terminfo"),
            root,
        };
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.server)
            .unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.terminfo)
            .unwrap();
        fixture
    }

    fn sh_start(&self, script: &str) -> (MultiServerDriver, TargetRef) {
        let storage = ServerStorage::initialize(&self.server).unwrap();
        let context = LaunchContext {
            working_directory: self.root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), self.root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("panelink-layout-lc-test"),
            self.root.as_os_str().to_owned(),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &self.terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 10,
                columns: 40,
            },
            false,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let driver = started.into_driver();
        (driver, target)
    }

    fn open_restart(&self) -> MultiServerDriver {
        // Detached history/layout workers may retain the singleton briefly
        // after Drop by design; wait boundedly instead of racing them.
        let deadline = Instant::now() + Duration::from_secs(5);
        let storage = loop {
            match ServerStorage::open(&self.server) {
                Ok(storage) => break storage,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "singleton lock not released");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("reopen storage: {error}"),
            }
        };
        let context = LaunchContext {
            working_directory: self.root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), self.root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from("printf 'RESTART-READY\\r\\n'; while :; do /bin/sleep 1; done"),
            OsString::from("panelink-layout-restart"),
            self.root.as_os_str().to_owned(),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &self.terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 10,
                columns: 40,
            },
            false,
        )
        .unwrap();
        started.into_driver()
    }

    fn layout_store(&self) -> LayoutStore {
        LayoutStore::open(&self.layout).unwrap()
    }

    fn panelink_command(&self, args: &[&str]) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = i32::try_from(child.id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let timed_out = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if Instant::now() >= deadline {
                break true;
            }
            thread::sleep(Duration::from_millis(5));
        };
        // SAFETY: negative PID names only this test-owned isolated group.
        unsafe { libc::kill(-group, libc::SIGKILL) };
        let output = child.wait_with_output().unwrap();
        assert!(!timed_out, "panelink CLI exceeded deadline: {args:?}");
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn pump_until_layout_durable(driver: &mut MultiServerDriver, expected_sequence: u64) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = driver.tick(|| 1);
        if let Some(handle) = driver.layout_status() {
            let status = handle.status();
            if status
                .last_durable
                .is_some_and(|k| k.snapshot_sequence == expected_sequence)
            {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "layout did not reach durable sequence {expected_sequence}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

// ==================================================================
// Scenarios without any socketpair client
// ==================================================================

#[test]
fn startup_one_pane_current_is_single_with_sequence_1() {
    let fixture = Fixture::new();
    let (mut driver, target) =
        fixture.sh_start("printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 1);
    assert_eq!(
        current.axis() as u8,
        panelink::saved_layout::LayoutAxis::Single as u8
    );
    let meta = current.metadata();
    assert_eq!(meta.snapshot_sequence(), 1);
    assert_eq!(meta.layout_revision(), 1);
    assert_eq!(current.panes()[0], Some(target));
    assert!(fixture
        .layout_store()
        .load(LayoutSlot::Previous)
        .unwrap()
        .is_none());
}

#[test]
fn unchanged_ticks_do_not_advance_sequence() {
    let fixture = Fixture::new();
    let (mut driver, _target) =
        fixture.sh_start("printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);

    let handle = driver.layout_status().unwrap();
    let first_observed = handle.status().last_observed.unwrap().snapshot_sequence;
    assert_eq!(first_observed, 1);

    for _ in 0..50 {
        let _ = driver.tick(|| 1);
        thread::sleep(Duration::from_millis(2));
    }
    let later = handle.status().last_observed.unwrap().snapshot_sequence;
    assert_eq!(
        later, first_observed,
        "sequence must not advance without change"
    );

    assert_eq!(handle.status().last_durable.unwrap().snapshot_sequence, 1);
}

#[test]
fn natural_exit_of_last_pane_becomes_empty_tombstone() {
    let fixture = Fixture::new();
    let (mut driver, _target) = fixture.sh_start("printf 'READY\\r\\n'; exit 0");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();

    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = driver.tick(|| 1);
        if let Some(handle) = driver.layout_status() {
            let status = handle.status();
            if status.last_durable.is_some_and(|k| {
                let store = fixture.layout_store();
                if let Ok(Some(layout)) = store.load(LayoutSlot::Current) {
                    layout.pane_count() == 0
                        && layout.metadata().snapshot_sequence() == k.snapshot_sequence
                } else {
                    false
                }
            }) {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "empty tombstone did not become durable"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 0);
    let meta = current.metadata();
    let live_revision = meta.layout_revision();
    let live_sequence = meta.snapshot_sequence();
    // Parent review: the tombstone keeps the last live revision (last-leaf
    // exit issues no new revision) and is ordered by a greater sequence.
    assert_eq!(live_revision, 1);
    assert!(live_sequence >= 2, "tombstone must follow the live record");

    for _ in 0..30 {
        let _ = driver.tick(|| 1);
        thread::sleep(Duration::from_millis(5));
    }
    let after = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(after.metadata().snapshot_sequence(), live_sequence);
    assert_eq!(after.metadata().layout_revision(), live_revision);
}

#[test]
fn driver_drop_with_live_panes_preserves_non_empty_topology() {
    let fixture = Fixture::new();
    let (mut driver, _target) =
        fixture.sh_start("trap 'exit 0' HUP; printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);

    drop(driver);

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(
        current.pane_count(),
        1,
        "drop must preserve last live topology"
    );
    assert_ne!(current.pane_count(), 0, "must NOT be empty tombstone");
    // Freeze-before-stop: no post-stop candidate was ever written.
    assert_eq!(current.metadata().snapshot_sequence(), 1);
}

#[test]
fn restart_rotation_preserves_previous_bytes() {
    let fixture = Fixture::new();
    let (mut driver, _target) =
        fixture.sh_start("printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);

    let prior_bytes = fixture
        .layout_store()
        .load_bytes(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    drop(driver);

    let mut driver = fixture.open_restart();
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);

    let previous = fixture
        .layout_store()
        .load_bytes(LayoutSlot::Previous)
        .unwrap()
        .unwrap();
    assert_eq!(
        previous, prior_bytes,
        "previous must equal prior current bytes"
    );

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 1);
    assert_eq!(current.metadata().snapshot_sequence(), 1);
    let meta = current.metadata();
    assert_eq!(meta.layout_revision(), 1);
    assert!(meta.generation() > 1);
}

#[test]
fn drop_with_history_and_layout_completes_within_shared_deadline() {
    let fixture = Fixture::new();
    let (mut driver, _target) =
        fixture.sh_start("trap 'exit 0' HUP; printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let layout_store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(layout_store).unwrap();

    fs::DirBuilder::new()
        .mode(0o700)
        .create(&fixture.history)
        .unwrap();
    driver.enable_history(HistoryStore::open(&fixture.history).unwrap());

    pump_until_layout_durable(&mut driver, 1);

    let started = Instant::now();
    drop(driver);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "drop took {elapsed:?}, expected < 3 s"
    );
}

#[test]
fn layout_show_cli_is_read_only_and_matches_saved_record() {
    let fixture = Fixture::new();
    let (mut driver, _target) =
        fixture.sh_start("printf 'READY\\r\\n'; while :; do /bin/sleep 1; done");
    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();
    pump_until_layout_durable(&mut driver, 1);
    drop(driver);

    let gen_path = fixture.server.join("generation");
    let gen_before = fs::read(&gen_path).unwrap();

    let output = fixture.panelink_command(&["layout", "show"]);
    assert!(
        output.status.success(),
        "layout show failed: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("saved layout: current"), "{stdout}");
    assert!(stdout.contains("orientation: single"), "{stdout}");
    assert!(stdout.contains("pane 1:"), "{stdout}");

    let gen_after = fs::read(&gen_path).unwrap();
    assert_eq!(
        gen_before, gen_after,
        "layout show must not mutate generation bytes"
    );
}

// ==================================================================
// Socketpair-based scenarios (2, 4, 5)
// ==================================================================

const IO_DEADLINE: Duration = Duration::from_secs(4);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(15);
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 81;

struct Client {
    stream: AuthenticatedStream,
    welcome: panelink::handshake_protocol::Welcome,
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

    fn layout(&mut self) -> LayoutReply {
        decode_layout_reply(&self.request(&encode_layout_query())).expect("LayoutReply")
    }

    fn screen(&mut self, target: TargetRef) -> String {
        let id = self.send(&encode_screen_query(target).unwrap());
        let frame = self.read();
        assert_eq!(frame.request_id(), id);
        assert_eq!(frame.kind(), MessageKind::Snapshot);
        let view =
            panelink::ui_protocol::decode_screen_reply(frame.payload()).expect("ScreenReply");
        assert_eq!(view.target(), target);
        let checkpoint =
            panelink::screen_checkpoint::decode(view.checkpoint()).expect("checkpoint");
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

    #[allow(dead_code)]
    fn claim(&mut self, target: TargetRef) -> u64 {
        let info = panelink::session_protocol::decode_session_info(
            &self.request(
                &panelink::session_protocol::encode_session_request(
                    &panelink::session_protocol::SessionRequest::Describe,
                )
                .unwrap(),
            ),
        )
        .unwrap();
        assert_eq!(info.target, target);
        assert_eq!(info.owner_lease, 0);
        let key = self.key();
        let payload = panelink::session_protocol::encode_session_request(
            &panelink::session_protocol::SessionRequest::Claim {
                key,
                target,
                expected_owner_revision: info.owner_revision,
                ttl_seconds: 30,
            },
        )
        .unwrap();
        assert_eq!(
            status(&self.request(&payload), key),
            OperationStatus::Completed(Outcome::Applied)
        );
        info.owner_revision + 1
    }

    #[allow(dead_code)]
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

#[allow(dead_code)]
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
    use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use panelink::session_protocol::{encode_session_hello, SessionHello};
    let mut payload = encode_session_hello(&SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_INPUT | CAPABILITY_READ,
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
    let welcome = panelink::handshake_protocol::decode_welcome(frame.payload()).unwrap();
    stream.enable_snapshot_reads().unwrap();
    Client {
        stream,
        welcome,
        next_request: 0,
        next_sequence: 0,
    }
}

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

fn zsh_context(home: &Path) -> LaunchContext {
    LaunchContext {
        working_directory: home.to_path_buf(),
        environment: Environment::from([
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (OsString::from("ZDOTDIR"), home.as_os_str().to_owned()),
            (OsString::from("PS1"), OsString::from("P5LC$ ")),
        ]),
    }
}

fn zsh_arguments(marker: &str) -> Vec<OsString> {
    vec![
        OsString::from("-f"),
        OsString::from("-c"),
        OsString::from(format!(
            "printf 'P5LC-READY-{marker}-PID-%s-END\\n' \"$$\"; exec /bin/zsh -f -i"
        )),
    ]
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
    let arguments: Vec<&[u8]> = arguments.iter().map(|a| a.as_bytes()).collect();
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

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn marker_pid(screen: &str, marker: &str) -> i32 {
    let start = screen.find(marker).expect("marker") + marker.len();
    let rest = screen[start..].strip_prefix("-PID-").expect("pid prefix");
    rest[..rest.find("-END").unwrap()].parse().unwrap()
}

fn wait_screen(client: &mut Client, target: TargetRef, marker: &str) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        let screen = client.screen(target);
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

#[test]
fn split_produces_two_panes_with_correct_axis_and_sequence() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let context = zsh_context(&home);
    let storage = ServerStorage::initialize(&fixture.server).unwrap();
    let first_args = zsh_arguments("one");
    let started = StartedMultiServer::start(
        storage,
        &fixture.terminfo,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &first_args,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .unwrap();
    let first = started.startup().target.unwrap();
    let mut driver = started.into_driver();

    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();

    let t0 = Instant::now();
    let link = connect(&mut driver, None, 11);
    let mut one = drive(&mut driver, t0, move || {
        let mut one = handshake(link);
        let layout = one.layout();
        assert_eq!(layout.panes.iter().flatten().count(), 1);
        assert_eq!(
            (layout.revision, layout.rows, layout.columns),
            (1, ROOT_ROWS, ROOT_COLUMNS)
        );
        wait_screen(&mut one, first, "P5LC-READY-one");
        one
    });

    pump_until_layout_durable(&mut driver, 1);

    let split_context = context.clone();
    let (one_back, split, split_bytes) = drive(&mut driver, t0, move || {
        let layout = one.layout();
        let (_, bytes) = split_payload(
            &mut one,
            &layout,
            first,
            SplitDirection::Right,
            &split_context,
            "two",
        );
        let result = decode_split_result(&one.request(&bytes)).expect("SplitResult");
        (one, result, bytes)
    });
    one = one_back;
    assert_eq!(split.outcome, Outcome::Applied);
    assert_eq!(split.committed_layout_revision, 2);
    let second = split.new_target.expect("new target");

    let link = connect(&mut driver, Some(second), 22);
    let second_pid = drive(&mut driver, t0, move || {
        let mut two = handshake(link);
        let ready = wait_screen(&mut two, second, "P5LC-READY-two");
        let pid = marker_pid(&ready, "P5LC-READY-two");
        (two, pid)
    })
    .1;
    assert!(alive(second_pid));

    pump_until_layout_durable(&mut driver, 2);

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 2);
    assert_eq!(
        current.axis() as u8,
        panelink::saved_layout::LayoutAxis::SideBySide as u8
    );
    let meta = current.metadata();
    assert_eq!(meta.snapshot_sequence(), 2);
    assert!(meta.layout_revision() > 1);
    assert_eq!(current.panes()[0], Some(first));
    assert_eq!(current.panes()[1], Some(second));

    let replay = drive(&mut driver, t0, move || {
        decode_split_result(&one.request(&split_bytes)).expect("replay result")
    });
    assert_eq!(replay.outcome, Outcome::Applied);

    drop(driver);
    assert!(!alive(second_pid), "owner teardown reaps child");
}

#[test]
fn root_resize_changes_geometry_and_sequence() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let context = zsh_context(&home);
    let storage = ServerStorage::initialize(&fixture.server).unwrap();
    let first_args = zsh_arguments("resize");
    let started = StartedMultiServer::start(
        storage,
        &fixture.terminfo,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &first_args,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .unwrap();
    let first = started.startup().target.unwrap();
    let mut driver = started.into_driver();

    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();

    let t0 = Instant::now();
    let link = connect(&mut driver, None, 33);
    let mut one = drive(&mut driver, t0, move || {
        let mut one = handshake(link);
        wait_screen(&mut one, first, "P5LC-READY-resize");
        one
    });

    pump_until_layout_durable(&mut driver, 1);

    let one_back = drive(&mut driver, t0, move || {
        let key = one.key();
        let payload = encode_root_resize(&RootResize {
            key,
            expected_layout_revision: 1,
            rows: 18,
            columns: 80,
        })
        .unwrap();
        let result = decode_resize_reply(&one.request(&payload)).expect("ResizeReply");
        assert_eq!(result.status, ResizeStatus::Applied);
        assert_eq!((result.revision, result.rows, result.columns), (2, 18, 80));
        one
    });
    drop(one_back);

    pump_until_layout_durable(&mut driver, 2);

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 1);
    assert_eq!(current.rows(), 18);
    assert_eq!(current.columns(), 80);
    let meta = current.metadata();
    assert_eq!(meta.snapshot_sequence(), 2);
    assert_eq!(meta.layout_revision(), 2);

    drop(driver);
}

#[test]
fn close_one_pane_of_two_leaves_survivor_and_new_sequence() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let context = zsh_context(&home);
    let storage = ServerStorage::initialize(&fixture.server).unwrap();
    let first_args = zsh_arguments("close");
    let started = StartedMultiServer::start(
        storage,
        &fixture.terminfo,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &first_args,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .unwrap();
    let first = started.startup().target.unwrap();
    let mut driver = started.into_driver();

    let store = LayoutStore::prepare(&fixture.server).unwrap();
    driver.enable_layout(store).unwrap();

    let t0 = Instant::now();
    let link = connect(&mut driver, None, 44);
    let mut one = drive(&mut driver, t0, move || {
        let mut one = handshake(link);
        wait_screen(&mut one, first, "P5LC-READY-close");
        one
    });

    pump_until_layout_durable(&mut driver, 1);

    let split_context = context.clone();
    let (one_back, split) = drive(&mut driver, t0, move || {
        let layout = one.layout();
        let (_, bytes) = split_payload(
            &mut one,
            &layout,
            first,
            SplitDirection::Right,
            &split_context,
            "close2",
        );
        let result = decode_split_result(&one.request(&bytes)).expect("SplitResult");
        (one, result)
    });
    one = one_back;
    assert_eq!(split.outcome, Outcome::Applied);
    let second = split.new_target.unwrap();

    let link = connect(&mut driver, Some(second), 55);
    let second_pid = drive(&mut driver, t0, move || {
        let mut two = handshake(link);
        let ready = wait_screen(&mut two, second, "P5LC-READY-close2");
        let pid = marker_pid(&ready, "P5LC-READY-close2");
        (two, pid)
    })
    .1;
    assert!(alive(second_pid));

    pump_until_layout_durable(&mut driver, 2);

    let one_back = drive(&mut driver, t0, move || {
        let layout = one.layout();
        let key = one.key();
        let mut payload = Vec::new();
        encode_close_request(
            &CloseRequest {
                key,
                target: second,
                expected_layout_revision: layout.revision,
            },
            &mut payload,
        )
        .unwrap();
        let result = panelink::close_protocol::decode_close_result(&one.request(&payload))
            .expect("CloseResult");
        assert!(
            matches!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited)),
            "expected stopped, got {:?}",
            result.outcome
        );
        assert_eq!(result.layout_revision, 3);
        one
    });
    drop(one_back);

    pump_until_layout_durable(&mut driver, 3);

    let current = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.pane_count(), 1);
    assert_eq!(current.panes()[0], Some(first));
    let meta = current.metadata();
    assert_eq!(meta.snapshot_sequence(), 3);
    assert_eq!(meta.layout_revision(), 3);

    drop(driver);
    assert!(!alive(second_pid), "closed pane must be reaped");
}

#[test]
fn explicit_close_of_last_pane_publishes_empty_after_settled_exit() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let context = zsh_context(&home);
    let arguments = zsh_arguments("last-close");
    let started = StartedMultiServer::start(
        ServerStorage::initialize(&fixture.server).unwrap(),
        &fixture.terminfo,
        &StartupLaunch {
            executable: OsStr::new("/bin/zsh"),
            arguments: &arguments,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .unwrap();
    let target = started.startup().target.unwrap();
    let mut driver = started.into_driver();
    driver
        .enable_layout(LayoutStore::prepare(&fixture.server).unwrap())
        .unwrap();

    let started_at = Instant::now();
    let link = connect(&mut driver, None, 66);
    let (mut client, child_pid) = drive(&mut driver, started_at, move || {
        let mut client = handshake(link);
        let screen = wait_screen(&mut client, target, "P5LC-READY-last-close");
        (client, marker_pid(&screen, "P5LC-READY-last-close"))
    });
    assert!(alive(child_pid));
    pump_until_layout_durable(&mut driver, 1);

    let (mut client, request) = drive(&mut driver, started_at, move || {
        let layout = client.layout();
        let key = client.key();
        let mut payload = Vec::new();
        encode_close_request(
            &CloseRequest {
                key,
                target,
                expected_layout_revision: layout.revision,
            },
            &mut payload,
        )
        .unwrap();
        (client, payload)
    });
    let result =
        panelink::close_protocol::decode_close_result(&drive(&mut driver, started_at, move || {
            client.request(&request)
        }))
        .expect("CloseResult");
    assert!(
        matches!(
            result.outcome,
            CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated)
        ),
        "last-pane close must settle as an exited child: {:?}",
        result.outcome
    );
    assert_eq!(
        result.layout_revision, 1,
        "last-leaf retention keeps revision"
    );

    // The owner publishes the tombstone only after close settlement recorded
    // the child as exited; ordering follows the snapshot sequence, not revision.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = driver.tick(|| 1);
        if fixture
            .layout_store()
            .load(LayoutSlot::Current)
            .unwrap()
            .is_some_and(|saved| saved.pane_count() == 0)
        {
            break;
        }
        assert!(Instant::now() < deadline, "last-pane tombstone not durable");
        thread::sleep(Duration::from_millis(5));
    }
    let saved = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(saved.metadata().snapshot_sequence(), 2);
    assert_eq!(saved.metadata().layout_revision(), 1);
    assert_eq!(saved.pane_count(), 0);
    assert!(!alive(child_pid), "settled close must reap the child");
    drop(driver);
}

#[test]
fn drop_during_phased_last_pane_close_preserves_nonempty_snapshot() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    let context = zsh_context(&home);
    let hup_marker = fixture.root.join("close-hup-seen");
    let script = format!(
        "trap 'printf hup > {}' HUP; printf 'P5LC-PENDING-CLOSE-READY-PID-%s-END\\n' \"$$\"; while :; do /bin/sleep 1; done",
        hup_marker.display()
    );
    let arguments = vec![OsString::from("-c"), OsString::from(script)];
    let started = StartedMultiServer::start(
        ServerStorage::initialize(&fixture.server).unwrap(),
        &fixture.terminfo,
        &StartupLaunch {
            executable: OsStr::new("/bin/sh"),
            arguments: &arguments,
            context: &context,
            rows: ROOT_ROWS,
            columns: ROOT_COLUMNS,
        },
        true,
    )
    .unwrap();
    let target = started.startup().target.unwrap();
    let mut driver = started.into_driver();
    driver
        .enable_layout(LayoutStore::prepare(&fixture.server).unwrap())
        .unwrap();

    let started_at = Instant::now();
    let link = connect(&mut driver, None, 67);
    let (mut client, child_pid) = drive(&mut driver, started_at, move || {
        let mut client = handshake(link);
        let screen = wait_screen(&mut client, target, "P5LC-PENDING-CLOSE-READY");
        let pid = marker_pid(&screen, "P5LC-PENDING-CLOSE-READY");
        (client, pid)
    });
    assert!(alive(child_pid));
    pump_until_layout_durable(&mut driver, 1);

    // Start a phased close and stop the driver before its HUP phase can settle.
    let (mut client, close_key, payload) = drive(&mut driver, started_at, move || {
        let layout = client.layout();
        let close_key = client.key();
        let mut payload = Vec::new();
        encode_close_request(
            &CloseRequest {
                key: close_key,
                target,
                expected_layout_revision: layout.revision,
            },
            &mut payload,
        )
        .unwrap();
        (client, close_key, payload)
    });
    client.send(&payload);
    let deadline = Instant::now() + Duration::from_secs(4);
    while !hup_marker.exists() {
        let _ = driver.tick(|| 2);
        assert!(Instant::now() < deadline, "close did not deliver HUP");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(close_key.generation, target.generation);
    assert_eq!(close_key.sequence, 1);

    let before = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(before.pane_count(), 1);
    assert_eq!(before.metadata().snapshot_sequence(), 1);

    drop(driver);

    let after = fixture
        .layout_store()
        .load(LayoutSlot::Current)
        .unwrap()
        .unwrap();
    assert_eq!(
        after.pane_count(),
        1,
        "shutdown completion cannot publish empty"
    );
    assert_eq!(after.panes()[0], Some(target));
    assert_eq!(after.metadata().snapshot_sequence(), 1);
    assert!(!alive(child_pid), "driver drop must stop the child");
}
