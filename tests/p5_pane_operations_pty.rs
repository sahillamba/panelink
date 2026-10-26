#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real four-PTY process continuity acceptance for schema-13 pane operations.
//!
//! The transport and owner are the production runtime path. The test uses
//! real zsh processes, actual PTY `stty size` output, and private fixture
//! storage; no fake resize effects or user clipboard are involved.

use panelink::control_protocol::TargetRef;
use panelink::handshake_protocol::{
    decode_welcome, Hello, Welcome, CAPABILITY_INPUT, CAPABILITY_PANE_OPERATIONS, CAPABILITY_READ,
};
use panelink::launch::{Environment, LaunchContext};
use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::pane_layout::{SplitDirection, MAX_PANES};
use panelink::pane_operations_protocol::{
    decode_request, decode_view, encode_request, PaneDividerSet, PaneOperationsView, PaneSwap,
    PaneZoomSet, Request as PaneRequest,
};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
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
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const IO_DEADLINE: Duration = Duration::from_secs(4);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(50);
const ROOT_ROWS: u16 = 40;
const ROOT_COLUMNS: u16 = 120;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
static NEXT_NONCE: AtomicU64 = AtomicU64::new(10_000);

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new(label: &str) -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p5-pane-pty-{label}-{}-{serial}",
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

fn zsh_context(home: &Path) -> LaunchContext {
    LaunchContext {
        working_directory: home.to_path_buf(),
        environment: Environment::from([
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (OsString::from("ZDOTDIR"), home.as_os_str().to_owned()),
            (OsString::from("PS1"), OsString::from("P5PO$ ")),
        ]),
    }
}

fn zsh_arguments(marker: &str) -> Vec<OsString> {
    vec![
        OsString::from("-f"),
        OsString::from("-c"),
        OsString::from(format!(
            "printf 'P5PO-READY-{marker}-PID-%s-END\\n' \"$$\"; exec /bin/zsh -f -i"
        )),
    ]
}

struct Client {
    stream: AuthenticatedStream,
    welcome: Welcome,
    target: Option<TargetRef>,
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

    fn pane_view(&mut self) -> PaneOperationsView {
        let payload = encode_request(&PaneRequest::ViewQuery).unwrap();
        decode_view(&self.request(&payload)).expect("schema-13 PaneOperationsView")
    }

    fn describe(&mut self) -> SessionInfo {
        decode_session_info(
            &self.request(&encode_session_request(&SessionRequest::Describe).unwrap()),
        )
        .expect("SessionInfo")
    }

    fn claim(&mut self, target: TargetRef) {
        let info = self.describe();
        assert_eq!(info.target, target);
        assert_eq!(info.owner_lease, 0, "fixture begins with no input owner");
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
    }

    fn keys(&mut self, target: TargetRef, text: &str) -> OperationStatus {
        // Claims are capped at 30 seconds. Refresh the same connection's
        // lease before every input so later continuity checks do not depend
        // on the full scenario finishing inside one claim TTL.
        let info = self.describe();
        assert_eq!(info.target, target);
        assert!(
            info.owner_lease == 0 || info.owner_lease == self.welcome.lease,
            "fixture input owner is vacant or remains this connection's lease"
        );
        let claim_key = self.key();
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        assert_eq!(
            status(&self.request(&claim), claim_key),
            OperationStatus::Completed(Outcome::Applied),
            "refresh input ownership on the same lease before sending keys"
        );
        let owner_revision = self.describe().owner_revision;
        let mut keys: Vec<Key> = text.chars().map(Key::Char).collect();
        keys.push(Key::Enter);
        for chunk in keys.chunks(panelink::ui_protocol::MAX_KEYS) {
            let key = self.key();
            let payload = encode_key_input(&KeyInput {
                key,
                target,
                expected_owner_revision: owner_revision,
                keys: chunk,
            })
            .unwrap();
            let result = status(&self.request(&payload), key);
            if result != OperationStatus::Completed(Outcome::Applied) {
                return result;
            }
        }
        OperationStatus::Completed(Outcome::Applied)
    }

    fn screen(&mut self, target: TargetRef) -> String {
        let payload = self.send(&encode_screen_query(target).unwrap());
        let frame = self.read();
        assert_eq!(frame.request_id(), payload);
        assert_eq!(frame.kind(), MessageKind::Snapshot);
        let view = decode_screen_reply(frame.payload()).expect("ScreenReply");
        assert_eq!(view.target(), target);
        let checkpoint =
            panelink::screen_checkpoint::decode(view.checkpoint()).expect("screen checkpoint");
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

fn handshake(link: (UnixStream, Option<TargetRef>, u64), pane_operations: bool) -> Client {
    handshake_resumed(link, pane_operations, None)
}

fn handshake_resumed(
    (socket, target, nonce): (UnixStream, Option<TargetRef>, u64),
    pane_operations: bool,
    resume: Option<panelink::session_protocol::LeaseRef>,
) -> Client {
    let mut stream = AuthenticatedStream::new(socket).unwrap();
    let mut optional_capabilities = CAPABILITY_PANE_OPERATIONS;
    if !pane_operations {
        optional_capabilities &= !CAPABILITY_PANE_OPERATIONS;
    }
    let mut payload = encode_session_hello(&SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities,
        },
        resume,
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
    if pane_operations {
        assert_ne!(
            welcome.selected_capabilities & CAPABILITY_PANE_OPERATIONS,
            0,
            "runtime must select capability 64 only after owner and dispatch integration"
        );
    } else {
        assert_eq!(
            welcome.selected_capabilities & CAPABILITY_PANE_OPERATIONS,
            0,
            "nonselecting client must not acquire pane-operation dispatch"
        );
    }
    stream.enable_snapshot_reads().unwrap();
    Client {
        stream,
        welcome,
        target,
        next_request: 0,
        next_sequence: 0,
    }
}

fn resume_client(driver: &mut MultiServerDriver, started: Instant, client: &mut Client) {
    let previous = client.welcome;
    let sequence = client.next_sequence;
    let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
    let link = connect(driver, client.target, nonce);
    let mut resumed = drive(driver, started, move || {
        handshake_resumed(
            link,
            true,
            Some(panelink::session_protocol::LeaseRef {
                generation: previous.generation,
                lease: previous.lease,
            }),
        )
    });
    assert_eq!(resumed.welcome.generation, previous.generation);
    assert_eq!(resumed.welcome.lease, previous.lease);
    resumed.next_sequence = sequence;
    *client = resumed;
}

fn drive<T: Send>(
    driver: &mut MultiServerDriver,
    started: Instant,
    operation: impl FnOnce() -> T + Send,
) -> T {
    // If this deadline trips, the scoped worker can still be inside one
    // bounded four-second socket read/write before the scope joins it.
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
    let mut payload = Vec::new();
    encode_split_request(
        &SplitRequest {
            key,
            target,
            expected_layout_revision: layout.revision,
            direction,
            launch: &LaunchRequest {
                key,
                rows,
                columns,
                requested_executable: b"/bin/zsh",
                requested_working_directory: cwd,
                client_working_directory: cwd,
                arguments: &arguments,
                environment: &environment,
            },
        },
        &mut payload,
    )
    .unwrap();
    (key, payload)
}

fn wait_screen(client: &mut Client, target: TargetRef, marker: &str) -> String {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        let screen = client.screen(target);
        if screen.lines().any(|line| line.trim_end() == marker) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "marker {marker:?} missing:\n{screen}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn marker_pid(screen: &str, marker: &str) -> Option<i32> {
    screen.lines().find_map(|line| {
        let line = line.trim_end();
        let pid = line
            .strip_prefix(marker)?
            .strip_prefix("-PID-")?
            .strip_suffix("-END")?;
        if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        pid.parse::<i32>().ok().filter(|pid| *pid > 0)
    })
}

fn wait_ready_screen(client: &mut Client, target: TargetRef, marker: &str) -> (String, i32) {
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    loop {
        let screen = client.screen(target);
        if let Some(pid) = marker_pid(&screen, marker) {
            return (screen, pid);
        }
        assert!(
            Instant::now() < deadline,
            "well-formed readiness line {marker:?}-PID-<numeric>-END missing:\n{screen}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_underlying_geometry_eq(actual: &LayoutReply, expected: &LayoutReply, context: &str) {
    assert_eq!(
        (actual.rows, actual.columns),
        (expected.rows, expected.columns),
        "{context}"
    );
    assert_eq!(actual.dividers, expected.dividers, "{context}");
    for expected_pane in expected.panes.iter().flatten() {
        let actual_pane = actual
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == expected_pane.target)
            .expect("underlying target remains present");
        assert_eq!(actual_pane.rect, expected_pane.rect, "{context}");
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes existence of a fixture-owned shell.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn targets(view: &PaneOperationsView) -> [TargetRef; MAX_PANES] {
    let mut result = [None; MAX_PANES];
    for (index, pane) in view.layout.panes.iter().enumerate() {
        result[index] = pane.map(|pane| pane.target);
    }
    result.map(|target| target.expect("four occupied PTYs"))
}

fn sort_targets(mut result: Vec<TargetRef>) -> Vec<TargetRef> {
    result.sort_by_key(|target| {
        (
            target.generation,
            target.workspace,
            target.pane,
            target.session,
            target.occupant_revision,
        )
    });
    result
}

fn target_set(view: &PaneOperationsView) -> Vec<TargetRef> {
    sort_targets(targets(view).to_vec())
}

fn confirmed_size(view: &PaneOperationsView, target: TargetRef) -> (u16, u16) {
    let index = view
        .layout
        .panes
        .iter()
        .position(|slot| slot.is_some_and(|pane| pane.target == target))
        .expect("target occupies one layout slot");
    view.confirmed_sizes[index].expect("occupied PTY has confirmed dimensions")
}

fn content_size(layout: &LayoutReply, target: TargetRef) -> (u16, u16) {
    let pane = layout
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == target)
        .expect("target occupies a layout slot");
    let content = panelink::pane_layout::content_rect(pane.rect).unwrap();
    (content.rows, content.columns)
}

fn client_at<'a>(first: &'a mut Client, others: &'a mut [Client], index: usize) -> &'a mut Client {
    if index == 0 {
        first
    } else {
        &mut others[index - 1]
    }
}

fn operation_status(
    client: &mut Client,
    request: PaneRequest,
    key: OperationKey,
) -> OperationStatus {
    let payload = encode_request(&request).unwrap();
    assert_eq!(decode_request(&payload), Ok(request));
    status(&client.request(&payload), key)
}

fn assert_size_command(client: &mut Client, target: TargetRef, label: &str, size: (u16, u16)) {
    let command = format!("printf 'P5PO-SIZE-{label}-'; stty size");
    assert_eq!(
        client.keys(target, &command),
        OperationStatus::Completed(Outcome::Applied)
    );
    let marker = format!("P5PO-SIZE-{label}-{} {}", size.0, size.1);
    wait_screen(client, target, &marker);
}

fn assert_state_marker(
    client: &mut Client,
    target: TargetRef,
    index: usize,
    pid: i32,
    phase: &str,
) {
    let state = format!("S{index}-{pid}");
    let phase_code = if phase == "after-swap" { "S" } else { "U" };
    let marker = format!("{phase_code}:{state}");
    let command = format!("printf '%s:%s\\n' '{phase_code}' \"$P5PO_STATE\"");
    assert_eq!(
        client.keys(target, &command),
        OperationStatus::Completed(Outcome::Applied)
    );
    wait_screen(client, target, &marker);
}

#[test]
fn four_real_ptys_keep_processes_state_and_output_through_swap_divider_zoom_and_resize() {
    let storage_dir = PrivateDir::new("storage");
    let terminfo = PrivateDir::new("terminfo");
    let home = PrivateDir::new("home");
    compile_terminfo(&terminfo.0);
    let context = zsh_context(&home.0);
    let split_context_two = zsh_context(&home.0);
    let split_context_three = zsh_context(&home.0);
    let split_context_four = zsh_context(&home.0);
    let storage = panelink::server_storage::ServerStorage::initialize(&storage_dir.0).unwrap();
    let first_arguments = zsh_arguments("one");
    let started_server = StartedMultiServer::start(
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
    .expect("start real multi-session server");
    assert_eq!(started_server.startup().outcome, Outcome::Applied);
    let first = started_server.startup().target.unwrap();
    let mut driver = started_server.into_driver();
    let started = Instant::now();

    let first_link = connect(&mut driver, None, 701);
    let (mut first_client, first_pid) = drive(&mut driver, started, move || {
        let mut client = handshake(first_link, true);
        assert_eq!(client.layout().panes.iter().flatten().count(), 1);
        let (_, pid) = wait_ready_screen(&mut client, first, "P5PO-READY-one");
        (client, pid)
    });

    // Build the exact four-leaf 2x2 tree using the existing real split path.
    let (one_back, second) = drive(&mut driver, started, move || {
        let mut client = first_client;
        let layout = client.layout();
        let (_, split_two) = split_payload(
            &mut client,
            &layout,
            first,
            SplitDirection::Right,
            &split_context_two,
            "two",
        );
        let result_two = decode_split_result(&client.request(&split_two)).unwrap();
        assert_eq!(result_two.outcome, Outcome::Applied);
        let second = result_two.new_target.unwrap();
        (client, second)
    });
    first_client = one_back;

    // Split requests and screen queries are target-scoped: attach a client
    // pinned to the second pane before querying its screen or splitting it.
    let second_link = connect(&mut driver, Some(second), 702);
    let (mut second_client, second_pid) = drive(&mut driver, started, move || {
        let mut client = handshake(second_link, true);
        assert_eq!(client.describe().target, second);
        let (_, pid) = wait_ready_screen(&mut client, second, "P5PO-READY-two");
        (client, pid)
    });

    resume_client(&mut driver, started, &mut first_client);
    let (one_back, third) = drive(&mut driver, started, move || {
        let mut client = first_client;
        let layout = client.layout();
        let (_, split_three) = split_payload(
            &mut client,
            &layout,
            first,
            SplitDirection::Down,
            &split_context_three,
            "three",
        );
        let result_three = decode_split_result(&client.request(&split_three)).unwrap();
        assert_eq!(result_three.outcome, Outcome::Applied);
        let third = result_three.new_target.unwrap();
        (client, third)
    });
    first_client = one_back;

    let third_link = connect(&mut driver, Some(third), 703);
    let (third_client, third_pid) = drive(&mut driver, started, move || {
        let mut client = handshake(third_link, true);
        assert_eq!(client.describe().target, third);
        let (_, pid) = wait_ready_screen(&mut client, third, "P5PO-READY-three");
        (client, pid)
    });

    resume_client(&mut driver, started, &mut second_client);
    let (second_client, fourth) = drive(&mut driver, started, move || {
        let layout = second_client.layout();
        let (_, split_four) = split_payload(
            &mut second_client,
            &layout,
            second,
            SplitDirection::Down,
            &split_context_four,
            "four",
        );
        let result_four = decode_split_result(&second_client.request(&split_four)).unwrap();
        assert_eq!(result_four.outcome, Outcome::Applied);
        let fourth = result_four.new_target.unwrap();
        (second_client, fourth)
    });
    let fourth_link = connect(&mut driver, Some(fourth), 704);
    let (fourth_client, fourth_pid) = drive(&mut driver, started, move || {
        let mut client = handshake(fourth_link, true);
        assert_eq!(client.describe().target, fourth);
        let (_, pid) = wait_ready_screen(&mut client, fourth, "P5PO-READY-four");
        (client, pid)
    });

    let mut clients = vec![second_client, third_client, fourth_client];
    let pids = vec![first_pid, second_pid, third_pid, fourth_pid];
    assert_eq!(
        pids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert!(pids.iter().all(|pid| alive(*pid)));

    let all_targets = [first, second, third, fourth];
    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(all_targets)
        .enumerate()
    {
        let pid = pids[index];
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            client.claim(target);
            let state = format!("S{index}-{pid}");
            assert_eq!(
                client.keys(target, &format!("P5PO_STATE='{state}'")),
                OperationStatus::Completed(Outcome::Applied)
            );
            assert_eq!(
                client.keys(target, "printf '%s\\n' \"$P5PO_STATE\""),
                OperationStatus::Completed(Outcome::Applied)
            );
            wait_screen(client, target, &state);
        });
    }

    resume_client(&mut driver, started, &mut first_client);
    let initial = drive(&mut driver, started, move || {
        let view = first_client.pane_view();
        assert_eq!(view.layout.revision, 4);
        (view, first_client)
    });
    let (mut view, mut first_client) = initial;
    let original_layout = view.layout;
    let initial_targets = all_targets;
    let initial_target_set = sort_targets(initial_targets.to_vec());
    assert_eq!(target_set(&view), initial_target_set);
    // An unnegotiated peer can use legacy layout but cannot query or mutate
    // schema 13; the authoritative view remains unchanged.
    let baseline = view;
    let denied_link = connect(&mut driver, None, 799);
    drive(&mut driver, started, move || {
        let mut denied = handshake(denied_link, false);
        assert_eq!(denied.layout().revision, 4);
        let request = encode_request(&PaneRequest::ViewQuery).unwrap();
        assert_eq!(denied.request_error(&request), "capability_denied");
        let key = denied.key();
        let mutation = encode_request(&PaneRequest::ZoomSet(PaneZoomSet {
            key,
            expected_layout_revision: 4,
            target: first,
            zoomed: true,
        }))
        .unwrap();
        assert_eq!(denied.request_error(&mutation), "capability_denied");
    });
    for (index, client) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .enumerate()
    {
        let expected_revision = view.layout.revision;
        assert_eq!(
            client.welcome.selected_capabilities & CAPABILITY_PANE_OPERATIONS,
            CAPABILITY_PANE_OPERATIONS
        );
        resume_client(&mut driver, started, client);
        let current = drive(&mut driver, started, move || client.pane_view());
        assert_eq!(target_set(&current), initial_target_set);
        assert_eq!(current.layout.revision, expected_revision, "client {index}");
    }
    resume_client(&mut driver, started, &mut first_client);
    let (unchanged, first_client_back) = drive(&mut driver, started, move || {
        let unchanged = first_client.pane_view();
        (unchanged, first_client)
    });
    first_client = first_client_back;
    assert_eq!(
        unchanged, baseline,
        "unnegotiated query and mutation have no effects"
    );

    // Confirm real PTY sizes from shell output. This is independent evidence
    // from the owner's geometry and confirmed-size fields.
    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(initial_targets)
        .enumerate()
    {
        let content = content_size(&view.layout, target);
        assert_eq!(
            confirmed_size(&view, target),
            content,
            "owner reports the size measured from the actual PTY"
        );
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            assert_size_command(client, target, &format!("base-{index}"), content);
        });
    }

    // Swap two identities in their fixed tree locations. Exact replay after
    // the revision advances is a no-op; changing the same key's payload is a
    // conflict and cannot commit a second transition.
    let swap_key = first_client.key();
    let source = initial_targets[0];
    let destination = initial_targets[1];
    let swap = PaneRequest::Swap(PaneSwap {
        key: swap_key,
        expected_layout_revision: view.layout.revision,
        source,
        destination,
    });
    let swap_bytes = encode_request(&swap).unwrap();
    let first_swap_bytes = swap_bytes.clone();
    let swap_expected_revision = view.layout.revision;
    resume_client(&mut driver, started, &mut first_client);
    let swapped = drive(&mut driver, started, move || {
        assert_eq!(
            status(&first_client.request(&first_swap_bytes), swap_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        let after = first_client.pane_view();
        assert_eq!(after.layout.revision, swap_expected_revision + 1);
        (after, first_client)
    });
    (view, first_client) = swapped;
    assert_eq!(
        view.layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == source)
            .unwrap()
            .rect,
        original_layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == destination)
            .unwrap()
            .rect
    );
    assert_eq!(
        view.layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == destination)
            .unwrap()
            .rect,
        original_layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == source)
            .unwrap()
            .rect
    );
    assert_eq!(target_set(&view), initial_target_set);
    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(initial_targets)
        .enumerate()
    {
        let size = content_size(&view.layout, target);
        assert_eq!(confirmed_size(&view, target), size);
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            assert_size_command(client, target, &format!("swap-{index}"), size);
        });
    }
    let swapped_revision = view.layout.revision;
    resume_client(&mut driver, started, &mut first_client);
    let duplicate = drive(&mut driver, started, move || {
        assert_eq!(
            status(&first_client.request(&swap_bytes), swap_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        assert_eq!(first_client.pane_view().layout.revision, swapped_revision);
        let conflict = PaneRequest::Swap(PaneSwap {
            destination: third,
            ..match swap {
                PaneRequest::Swap(value) => value,
                _ => unreachable!(),
            }
        });
        let conflict_bytes = encode_request(&conflict).unwrap();
        assert_eq!(
            first_client.request_error(&conflict_bytes),
            "operation_conflict",
            "same key with different payload must conflict"
        );
        assert_eq!(first_client.pane_view().layout.revision, swapped_revision);
        first_client
    });
    first_client = duplicate;

    // Adjust the first spatial divider by an absolute first-span request.
    // The accepted view is checked for a changed divider rectangle and the
    // original four complete target references remain present.
    let divider_before = view.layout.dividers[0].expect("four-pane internal divider");
    let divider_key = first_client.key();
    let divider_request = PaneRequest::DividerSet(PaneDividerSet {
        key: divider_key,
        expected_layout_revision: view.layout.revision,
        target: source,
        divider_index: 0,
        first_cells: 31,
    });
    resume_client(&mut driver, started, &mut first_client);
    let divider_after = drive(&mut driver, started, move || {
        assert_eq!(
            operation_status(&mut first_client, divider_request, divider_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        let after = first_client.pane_view();
        (after, first_client)
    });
    (view, first_client) = divider_after;
    assert_eq!(view.layout.revision, 6);
    assert_eq!(target_set(&view), initial_target_set);
    assert_ne!(
        view.layout.dividers[0],
        Some(divider_before),
        "selected internal divider moved"
    );
    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(initial_targets)
        .enumerate()
    {
        let size = content_size(&view.layout, target);
        assert_eq!(confirmed_size(&view, target), size);
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            assert_size_command(client, target, &format!("divider-{index}"), size);
        });
    }

    // Set independent shell state again after the swap, so target identity
    // and process identity are both checked through every subsequent action.
    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(initial_targets)
        .enumerate()
    {
        let pid = pids[index];
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            assert_state_marker(client, target, index, pid, "after-swap");
        });
    }

    let zoom_target = initial_targets[2];
    let hidden_target = initial_targets[3];
    let hidden_index = 3;
    let hidden_command = "(i=1; while [ $i -le 8 ]; do printf 'P5PO-HIDDEN-%s\\n' \"$i\"; i=$((i+1)); sleep 0.1; done) &";

    let zoom_key = first_client.key();
    let zoom_request = PaneRequest::ZoomSet(PaneZoomSet {
        key: zoom_key,
        expected_layout_revision: view.layout.revision,
        target: zoom_target,
        zoomed: true,
    });
    let zoom_bytes = encode_request(&zoom_request).unwrap();
    let before_zoom = view;
    let underlying = view.layout;
    resume_client(&mut driver, started, &mut first_client);
    let zoomed = drive(&mut driver, started, move || {
        assert_eq!(
            status(&first_client.request(&zoom_bytes), zoom_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        let view = first_client.pane_view();
        (view, first_client)
    });
    (view, first_client) = zoomed;
    assert_eq!(view.layout.revision, 7);
    assert_eq!(view.zoomed_target, Some(zoom_target));
    assert_underlying_geometry_eq(
        &view.layout,
        &underlying,
        "zoom preserves underlying geometry",
    );
    assert_eq!(target_set(&view), initial_target_set);
    for target in initial_targets {
        if target != zoom_target {
            assert_eq!(
                confirmed_size(&view, target),
                confirmed_size(&before_zoom, target),
                "hidden PTY retains its last confirmed size while zoomed"
            );
        }
    }
    // Start the bounded producer only after the authoritative zoom view is
    // received, so its output is proven to arrive while this pane is hidden.
    resume_client(
        &mut driver,
        started,
        client_at(&mut first_client, &mut clients, hidden_index),
    );
    let (first_client_back, clients_back) = drive(&mut driver, started, move || {
        let hidden_client = client_at(&mut first_client, &mut clients, hidden_index);
        assert_eq!(
            hidden_client.keys(hidden_target, hidden_command),
            OperationStatus::Completed(Outcome::Applied)
        );
        wait_screen(hidden_client, hidden_target, "P5PO-HIDDEN-8");
        (first_client, clients)
    });
    first_client = first_client_back;
    clients = clients_back;

    let root_content = panelink::pane_layout::content_rect(panelink::pane_layout::Rect {
        x: 0,
        y: 0,
        rows: ROOT_ROWS,
        columns: ROOT_COLUMNS,
    })
    .unwrap();
    let zoom_owner_index = initial_targets
        .iter()
        .position(|target| *target == zoom_target)
        .unwrap();
    resume_client(
        &mut driver,
        started,
        client_at(&mut first_client, &mut clients, zoom_owner_index),
    );
    let (first_client_back, clients_back) = drive(&mut driver, started, move || {
        assert_size_command(
            client_at(&mut first_client, &mut clients, zoom_owner_index),
            zoom_target,
            "zoom",
            (root_content.rows, root_content.columns),
        );
        (first_client, clients)
    });
    first_client = first_client_back;
    clients = clients_back;
    // Root resize is permitted while zoomed. The zoom target follows the new
    // full-root content dimensions; hidden PTYs retain their own confirmed
    // sizes until unzoom reconciles each underlying rectangle.
    let resize_key = first_client.key();
    let resize_bytes = encode_root_resize(&RootResize {
        key: resize_key,
        expected_layout_revision: view.layout.revision,
        rows: ROOT_ROWS + 2,
        columns: ROOT_COLUMNS + 4,
    })
    .unwrap();
    resume_client(&mut driver, started, &mut first_client);
    let resized = drive(&mut driver, started, move || {
        let result = decode_resize_reply(&first_client.request(&resize_bytes)).unwrap();
        assert_eq!(result.status, ResizeStatus::Applied);
        assert_eq!(
            (result.rows, result.columns),
            (ROOT_ROWS + 2, ROOT_COLUMNS + 4)
        );
        let view = first_client.pane_view();
        (view, first_client)
    });
    (view, first_client) = resized;
    assert_eq!(view.layout.revision, 8);
    assert_eq!(view.zoomed_target, Some(zoom_target));
    assert_eq!(target_set(&view), initial_target_set);
    for target in initial_targets {
        if target != zoom_target {
            assert_eq!(
                confirmed_size(&view, target),
                confirmed_size(&before_zoom, target),
                "root resize during zoom leaves hidden PTY size unchanged"
            );
        }
    }
    let resized_underlying = view.layout;
    let resized_content = panelink::pane_layout::content_rect(panelink::pane_layout::Rect {
        x: 0,
        y: 0,
        rows: ROOT_ROWS + 2,
        columns: ROOT_COLUMNS + 4,
    })
    .unwrap();
    resume_client(
        &mut driver,
        started,
        client_at(&mut first_client, &mut clients, zoom_owner_index),
    );
    let (first_client_back, clients_back) = drive(&mut driver, started, move || {
        assert_size_command(
            client_at(&mut first_client, &mut clients, zoom_owner_index),
            zoom_target,
            "zoom-resized",
            (resized_content.rows, resized_content.columns),
        );
        (first_client, clients)
    });
    first_client = first_client_back;
    clients = clients_back;

    let unzoom_key = first_client.key();
    let unzoom_request = PaneRequest::ZoomSet(PaneZoomSet {
        key: unzoom_key,
        expected_layout_revision: view.layout.revision,
        target: zoom_target,
        zoomed: false,
    });
    resume_client(&mut driver, started, &mut first_client);
    let unzoomed = drive(&mut driver, started, move || {
        assert_eq!(
            operation_status(&mut first_client, unzoom_request, unzoom_key),
            OperationStatus::Completed(Outcome::Applied)
        );
        let view = first_client.pane_view();
        (view, first_client)
    });
    (view, first_client) = unzoomed;
    assert_eq!(view.layout.revision, 9);
    assert_eq!(view.zoomed_target, None);
    assert_eq!(target_set(&view), initial_target_set);
    assert_eq!(view.layout.rows, ROOT_ROWS + 2);
    assert_eq!(view.layout.columns, ROOT_COLUMNS + 4);
    assert_underlying_geometry_eq(
        &view.layout,
        &resized_underlying,
        "unzoom restores the resized underlying geometry",
    );

    for (index, (client, target)) in std::iter::once(&mut first_client)
        .chain(clients.iter_mut())
        .zip(initial_targets)
        .enumerate()
    {
        let pid = pids[index];
        let content = content_size(&view.layout, target);
        assert_eq!(confirmed_size(&view, target), content);
        // Inactive fixture sockets can exceed the production one-second read
        // deadline while another pane is tested. Resume the same lease and
        // preserve its next sequence; never replay an operation or input.
        resume_client(&mut driver, started, client);
        drive(&mut driver, started, move || {
            assert_size_command(client, target, &format!("unzoom-{index}"), content);
            assert_state_marker(client, target, index, pid, "after-unzoom");
        });
    }
    assert_eq!(target_set(&view), initial_target_set);
    assert!(
        pids.iter().all(|pid| alive(*pid)),
        "all four original shells remain alive"
    );

    // Close clients, let workers detach, then drop the driver for its normal
    // owner teardown and verify every fixture process has been reaped.
    drop(first_client);
    drop(clients);
    let deadline = Instant::now() + SCENARIO_DEADLINE;
    while driver.active_connections() != 0 {
        let _ = driver.tick(|| 100 + started.elapsed().as_secs());
        assert!(
            Instant::now() < deadline,
            "client workers close after disconnect"
        );
        thread::sleep(Duration::from_millis(1));
    }
    drop(driver);
    for pid in pids {
        assert!(!alive(pid), "owner teardown reaps fixture shell {pid}");
    }
}
