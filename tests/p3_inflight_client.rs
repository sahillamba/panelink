#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-CLI acceptance for a killed input caller after the server has already
//! applied its submission and produced the terminal response.

use panelink::client_context::ContextFile;
use panelink::control_protocol::{decode_request_payload, Command, TargetRef};
use panelink::handshake_protocol::decode_welcome;
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::transport::{AuthenticatedStream, PrivateListener};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROCESS_DEADLINE: Duration = Duration::from_secs(10);
const STARTUP_DEADLINE: Duration = Duration::from_secs(12);
const STOP_DEADLINE: Duration = Duration::from_secs(6);
const IO_DEADLINE: Duration = Duration::from_secs(2);
const PROXY_ACCEPT_POLL: Duration = Duration::from_millis(50);
const PROXY_HOLD_DEADLINE: Duration = Duration::from_secs(4);
const CAPTURE_LIMIT: usize = 32 * 1024;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
    terminfo: PathBuf,
    server: Option<Child>,
    output_id: u64,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-p3-inflight-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        private_dir(&root);
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            terminfo: root.join("terminfo"),
            root,
            server: None,
            output_id: 0,
        };
        for path in [&fixture.state, &fixture.home, &fixture.terminfo] {
            private_dir(path);
        }
        private_dir(&fixture.server_directory());
        fixture
    }

    fn server_directory(&self) -> PathBuf {
        self.state.join("server")
    }

    fn proxy_directory(&self) -> PathBuf {
        self.root.join("proxy")
    }

    fn context_path(&self) -> PathBuf {
        self.root.join("caller.context")
    }

    fn observer_context_path(&self) -> PathBuf {
        self.root.join("observer.context")
    }

    fn record_path(&self) -> PathBuf {
        self.root.join("handled.log")
    }

    fn command(&self) -> ProcessCommand {
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", "/bin/sh")
            .env("PS1", "INFLIGHT$ ")
            .env("TERM", "panelink-probe")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn output_paths(&mut self, label: &str) -> (PathBuf, PathBuf) {
        self.output_id += 1;
        (
            self.root.join(format!("{label}-{}.stdout", self.output_id)),
            self.root.join(format!("{label}-{}.stderr", self.output_id)),
        )
    }

    fn attach_capture(command: &mut ProcessCommand, out: &Path, err: &Path) {
        command
            .stdout(Stdio::from(create_capture(out)))
            .stderr(Stdio::from(create_capture(err)));
    }

    fn read_capture(path: &Path) -> Vec<u8> {
        let file = File::open(path).expect("open bounded child capture");
        let mut bytes = Vec::new();
        file.take((CAPTURE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .expect("read bounded child capture");
        assert!(
            bytes.len() <= CAPTURE_LIMIT,
            "child capture exceeded its fixed bound"
        );
        bytes
    }

    fn start(&mut self, script: &str) {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let tic_status = bounded_status(
            {
                let mut command = ProcessCommand::new("/usr/bin/tic");
                command
                    .args(["-x", "-o"])
                    .arg(&self.terminfo)
                    .arg(source)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin");
                command
            },
            PROCESS_DEADLINE,
        );
        assert!(
            tic_status.success(),
            "compile private terminfo: {tic_status}"
        );

        let (out, err) = self.output_paths("server");
        let mut command = self.command();
        command
            .args(["serve", "--directory"])
            .arg(self.server_directory())
            .arg("--terminfo")
            .arg(&self.terminfo)
            .args([
                "--initialize",
                "--allow-input",
                "--",
                "/bin/sh",
                "-c",
                script,
            ]);
        Self::attach_capture(&mut command, &out, &err);
        self.server = Some(
            command
                .spawn()
                .expect("start the real private serve process"),
        );
        let deadline = Instant::now() + STARTUP_DEADLINE;
        loop {
            if let Some(line) = first_capture_line(&out, 4096) {
                let startup = String::from_utf8(line).expect("serve startup JSON is UTF-8");
                assert!(
                    startup.contains("\"ok\":true") && startup.contains("\"state\":\"running\""),
                    "serve did not report running: startup={startup:?}; stderr={:?}",
                    String::from_utf8_lossy(&Self::read_capture(&err))
                );
                assert!(
                    self.server_directory().join("control.sock").exists(),
                    "serve startup did not leave its private control socket"
                );
                self.assert_live_server();
                return;
            }
            if let Some(status) = self
                .server
                .as_mut()
                .unwrap()
                .try_wait()
                .expect("poll serve startup")
            {
                let stderr = String::from_utf8_lossy(&Self::read_capture(&err)).into_owned();
                panic!("serve exited before startup JSON: status={status}; stderr={stderr:?}");
            }
            if Instant::now() >= deadline {
                panic!(
                    "serve startup exceeded its bound; stderr={:?}",
                    String::from_utf8_lossy(&Self::read_capture(&err))
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn control(&mut self, directory: &Path, context: &Path, action: &[&str]) -> Output {
        let mut command = self.command();
        command
            .args(["control", "--directory"])
            .arg(directory)
            .arg("--context")
            .arg(context)
            .args(action);
        let (out, err) = self.output_paths("control");
        Self::attach_capture(&mut command, &out, &err);
        let mut child = command.spawn().expect("spawn real control CLI process");
        let status = wait_owned_child(&mut child, PROCESS_DEADLINE);
        Output {
            status,
            stdout: Self::read_capture(&out),
            stderr: Self::read_capture(&err),
        }
    }

    fn assert_live_server(&mut self) {
        let server = self.server.as_mut().expect("owned server child");
        assert!(server.try_wait().expect("poll owned server").is_none());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let server_live = self
            .server
            .as_mut()
            .is_some_and(|server| server.try_wait().ok().flatten().is_none());
        if server_live {
            if let Some(server) = self.server.as_mut() {
                // SAFETY: this is the still-live, unreaped server Child owned
                // by the fixture, never a PID loaded from persistent metadata.
                let _ = unsafe { libc::kill(server.id() as libc::pid_t, libc::SIGTERM) };
                let deadline = Instant::now() + STOP_DEADLINE;
                while server.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                if server.try_wait().ok().flatten().is_none() {
                    let _ = server.kill();
                    let _ = server.wait();
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct OwnedClient {
    child: Child,
}

impl OwnedClient {
    fn spawn(fixture: &mut Fixture, args: &[String], label: &str) -> (Self, PathBuf, PathBuf) {
        let (out, err) = fixture.output_paths(label);
        let mut command = fixture.command();
        command.args(args);
        Fixture::attach_capture(&mut command, &out, &err);
        let child = command
            .spawn()
            .expect("spawn exact CLI child owned by this test");
        (Self { child }, out, err)
    }

    fn assert_running(&mut self) {
        assert!(
            self.child
                .try_wait()
                .expect("poll submitting CLI child")
                .is_none(),
            "submit child must still be waiting for the held response"
        );
    }

    fn kill_with_sigkill_and_reap(&mut self) -> ExitStatus {
        let pid = self.child.id();
        // SAFETY: this PID comes from this test-owned Child and is signaled only
        // to establish the requested OS process-death boundary.
        let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        assert_eq!(result, 0, "SIGKILL the exact submitting Child");
        let status = wait_owned_child(&mut self.child, Duration::from_secs(3));
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        status
    }
}

impl Drop for OwnedClient {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HeldResponse {
    key: OperationKey,
    target: TargetRef,
    request_id: u64,
}

#[derive(Clone, Copy, Debug)]
struct HeldSubmit {
    key: OperationKey,
    target: TargetRef,
    request_id: u64,
}

struct ExpectedSubmit {
    target: TargetRef,
    owner_revision: u64,
    text: String,
}

struct Proxy {
    cancel: Arc<AtomicBool>,
    release: Option<mpsc::Sender<()>>,
    barrier: mpsc::Receiver<Result<HeldResponse, String>>,
    worker: Option<JoinHandle<()>>,
}

impl Proxy {
    fn start(
        proxy_dir: PathBuf,
        server_dir: PathBuf,
        expected_target: TargetRef,
        owner_revision: u64,
        text: String,
    ) -> Self {
        let listener =
            PrivateListener::bind(&proxy_dir).expect("bind private one-connection proxy");
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let (barrier_tx, barrier) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let expected = ExpectedSubmit {
                target: expected_target,
                owner_revision,
                text,
            };
            let result = proxy_submit_once(
                listener,
                &server_dir,
                &expected,
                &thread_cancel,
                &release_rx,
                &barrier_tx,
            );
            if let Err(error) = result {
                let _ = barrier_tx.send(Err(error));
            }
        });
        Self {
            cancel,
            release: Some(release),
            barrier,
            worker: Some(worker),
        }
    }

    fn held_response(&self) -> HeldResponse {
        match self.barrier.recv_timeout(STARTUP_DEADLINE) {
            Ok(Ok(held)) => held,
            Ok(Err(error)) => panic!("proxy rejected submit exchange: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("proxy did not reach Applied barrier"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("proxy ended before Applied barrier")
            }
        }
    }

    fn release_and_join(&mut self, require_join: bool) {
        self.cancel.store(true, Ordering::Release);
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let result = self.worker.take().expect("proxy worker").join();
            if require_join {
                result.expect("bounded proxy worker");
            }
        } else if require_join {
            panic!("proxy worker did not terminate within its bounded teardown window");
        } else {
            // Its accept/read/write/hold waits each have explicit deadlines;
            // dropping detaches only after the cleanup window was exhausted.
            self.worker.take();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.release_and_join(false);
    }
}

struct PreAdmissionProxy {
    cancel: Arc<AtomicBool>,
    release: Option<mpsc::Sender<()>>,
    barrier: mpsc::Receiver<Result<HeldSubmit, String>>,
    worker: Option<JoinHandle<()>>,
}

impl PreAdmissionProxy {
    fn start(
        proxy_dir: PathBuf,
        server_dir: PathBuf,
        expected_target: TargetRef,
        owner_revision: u64,
        text: String,
    ) -> Self {
        let listener = PrivateListener::bind(&proxy_dir).expect("bind private pre-admission proxy");
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = Arc::clone(&cancel);
        let (barrier_tx, barrier) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let expected = ExpectedSubmit {
                target: expected_target,
                owner_revision,
                text,
            };
            let result = proxy_submit_before_admission(
                listener,
                &server_dir,
                &expected,
                &thread_cancel,
                &release_rx,
                &barrier_tx,
            );
            if let Err(error) = result {
                let _ = barrier_tx.send(Err(error));
            }
        });
        Self {
            cancel,
            release: Some(release),
            barrier,
            worker: Some(worker),
        }
    }

    fn held_submit(&self) -> HeldSubmit {
        match self.barrier.recv_timeout(STARTUP_DEADLINE) {
            Ok(Ok(held)) => held,
            Ok(Err(error)) => panic!("pre-admission proxy rejected Submit: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("pre-admission proxy did not reach Submit barrier")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("pre-admission proxy ended before Submit barrier")
            }
        }
    }

    fn release_and_join(&mut self, require_join: bool) {
        self.cancel.store(true, Ordering::Release);
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let result = self
                .worker
                .take()
                .expect("pre-admission proxy worker")
                .join();
            if require_join {
                result.expect("bounded pre-admission proxy worker");
            }
        } else if require_join {
            panic!("pre-admission proxy did not terminate within bounded teardown");
        } else {
            self.worker.take();
        }
    }
}

impl Drop for PreAdmissionProxy {
    fn drop(&mut self) {
        self.release_and_join(false);
    }
}

fn proxy_submit_once(
    listener: PrivateListener,
    server_dir: &Path,
    expected: &ExpectedSubmit,
    cancel: &AtomicBool,
    release: &mpsc::Receiver<()>,
    barrier: &mpsc::SyncSender<Result<HeldResponse, String>>,
) -> Result<(), String> {
    let accept_deadline = Instant::now() + STARTUP_DEADLINE;
    let mut client = loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let slice = (Instant::now() + PROXY_ACCEPT_POLL).min(accept_deadline);
        match listener.accept(slice) {
            Ok(stream) => break stream,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                if Instant::now() >= accept_deadline {
                    return Err("proxy accept deadline elapsed".to_owned());
                }
            }
            Err(error) => return Err(format!("proxy accept failed: {error}")),
        }
    };
    let mut server = AuthenticatedStream::connect(server_dir, Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("connect proxy to real server: {error}"))?;

    let hello = client
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read client Hello: {error}"))?;
    if hello.kind() != MessageKind::Hello || hello.request_id() != 0 {
        return Err("client did not begin with one Hello".to_owned());
    }
    write_frame(
        &mut server,
        hello.kind(),
        hello.request_id(),
        hello.payload(),
    )?;
    let welcome = server
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read real server Welcome: {error}"))?;
    if welcome.kind() != MessageKind::Welcome || welcome.request_id() != 0 {
        return Err("real server did not return one Welcome".to_owned());
    }
    decode_welcome(welcome.payload()).map_err(|error| format!("validate Welcome: {error}"))?;
    write_frame(
        &mut client,
        welcome.kind(),
        welcome.request_id(),
        welcome.payload(),
    )?;

    let request = client
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read one submit request: {error}"))?;
    if request.kind() != MessageKind::Request || request.request_id() == 0 {
        return Err("submit was not one nonzero-ID Request frame".to_owned());
    }
    let decoded = decode_request_payload(request.payload())
        .map_err(|error| format!("decode forwarded control request: {error}"))?;
    match decoded.command {
        Command::Submit {
            owner_revision,
            text,
        } if decoded.target == expected.target
            && owner_revision == expected.owner_revision
            && text == expected.text => {}
        _ => return Err("client request was not the exact expected Submit".to_owned()),
    }
    let held = HeldResponse {
        key: decoded.key,
        target: decoded.target,
        request_id: request.request_id(),
    };
    write_frame(
        &mut server,
        request.kind(),
        request.request_id(),
        request.payload(),
    )?;
    let response = server
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read server terminal response: {error}"))?;
    if response.kind() != MessageKind::Response || response.request_id() != held.request_id {
        return Err("server response does not match the forwarded request".to_owned());
    }
    let decoded_response = decode_response_payload(response.payload())
        .map_err(|error| format!("decode server terminal response: {error}"))?;
    if decoded_response.key != held.key
        || decoded_response.body
            != ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    {
        return Err("server response did not report exact Completed(Applied)".to_owned());
    }
    barrier
        .send(Ok(held))
        .map_err(|_| "parent dropped the response barrier".to_owned())?;
    // Keep the caller stream open while the parent verifies and SIGKILLs its
    // Child. The terminal response remains in this worker's local frame and is
    // deliberately never written to the client.
    match release.recv_timeout(PROXY_HOLD_DEADLINE) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => Ok(()),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("held response teardown deadline elapsed".to_owned())
        }
    }
}

fn proxy_submit_before_admission(
    listener: PrivateListener,
    server_dir: &Path,
    expected: &ExpectedSubmit,
    cancel: &AtomicBool,
    release: &mpsc::Receiver<()>,
    barrier: &mpsc::SyncSender<Result<HeldSubmit, String>>,
) -> Result<(), String> {
    let accept_deadline = Instant::now() + STARTUP_DEADLINE;
    let mut client = loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let slice = (Instant::now() + PROXY_ACCEPT_POLL).min(accept_deadline);
        match listener.accept(slice) {
            Ok(stream) => break stream,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                if Instant::now() >= accept_deadline {
                    return Err("pre-admission proxy accept deadline elapsed".to_owned());
                }
            }
            Err(error) => return Err(format!("pre-admission proxy accept failed: {error}")),
        }
    };
    let mut server = AuthenticatedStream::connect(server_dir, Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("connect pre-admission proxy to real server: {error}"))?;

    let hello = client
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read client Hello: {error}"))?;
    if hello.kind() != MessageKind::Hello || hello.request_id() != 0 {
        return Err("client did not begin with one Hello".to_owned());
    }
    write_frame(
        &mut server,
        hello.kind(),
        hello.request_id(),
        hello.payload(),
    )?;
    let welcome = server
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read real server Welcome: {error}"))?;
    if welcome.kind() != MessageKind::Welcome || welcome.request_id() != 0 {
        return Err("real server did not return one Welcome".to_owned());
    }
    decode_welcome(welcome.payload())
        .map_err(|error| format!("validate pre-admission Welcome: {error}"))?;
    write_frame(
        &mut client,
        welcome.kind(),
        welcome.request_id(),
        welcome.payload(),
    )?;

    let request = client
        .read_frame(Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("read pre-admission Submit: {error}"))?;
    if request.kind() != MessageKind::Request || request.request_id() == 0 {
        return Err("Submit was not one nonzero-ID Request frame".to_owned());
    }
    let decoded = decode_request_payload(request.payload())
        .map_err(|error| format!("decode pre-admission Submit: {error}"))?;
    match decoded.command {
        Command::Submit {
            owner_revision,
            text,
        } if decoded.target == expected.target
            && owner_revision == expected.owner_revision
            && text == expected.text => {}
        _ => return Err("client request was not the exact expected Submit".to_owned()),
    }
    let held = HeldSubmit {
        key: decoded.key,
        target: decoded.target,
        request_id: request.request_id(),
    };
    barrier
        .send(Ok(held))
        .map_err(|_| "parent dropped the pre-admission barrier".to_owned())?;

    // Keep the decoded Submit frame local and never forward it. After the
    // parent kills and reaps the submitting Child, returning drops the frame
    // and both transport streams without presenting the key to server ingress.
    match release.recv_timeout(PROXY_HOLD_DEADLINE) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            drop(request);
            Ok(())
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("pre-admission barrier teardown deadline elapsed".to_owned())
        }
    }
}

fn write_frame(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
) -> Result<(), String> {
    let frame = Frame::new(kind, request_id, payload)
        .map_err(|error| format!("construct forwarded frame: {error}"))?;
    stream
        .write_frame(&frame, Instant::now() + IO_DEADLINE)
        .map_err(|error| format!("write forwarded frame: {error}"))
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|error| panic!("create private directory {}: {error}", path.display()));
}

fn create_capture(path: &Path) -> File {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| panic!("create private output capture: {error}"))
}

fn first_capture_line(path: &Path, maximum: usize) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    assert!(
        bytes.len() <= maximum,
        "startup capture exceeded its fixed bound"
    );
    let newline = bytes.iter().position(|byte| *byte == b'\n')?;
    Some(bytes[..newline].to_vec())
}

fn wait_owned_child(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll owned Child") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("owned Child exceeded {timeout:?} deadline");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn bounded_status(mut command: ProcessCommand, timeout: Duration) -> ExitStatus {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn bounded helper process");
    wait_owned_child(&mut child, timeout)
}

fn assert_success(output: &Output, label: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{label}: status={} stdout={stdout:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn json_u64(json: &str, name: &str) -> u64 {
    let marker = format!("\"{name}\":");
    let start = json
        .find(&marker)
        .unwrap_or_else(|| panic!("missing JSON field {name}"))
        + marker.len();
    let digits: String = json[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("invalid JSON integer field {name}"))
}

fn json_string(json: &str, name: &str) -> String {
    let marker = format!("\"{name}\":\"");
    let start = json
        .find(&marker)
        .unwrap_or_else(|| panic!("missing JSON string {name}"))
        + marker.len();
    let mut result = String::new();
    let mut escaped = false;
    for ch in json[start..].chars() {
        if escaped {
            result.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'b' => '\u{0008}',
                'f' => '\u{000c}',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return result;
        } else {
            result.push(ch);
        }
    }
    panic!("unterminated JSON string field {name}");
}

fn json_bool(json: &str, name: &str) -> bool {
    let marker = format!("\"{name}\":");
    let start = json
        .find(&marker)
        .unwrap_or_else(|| panic!("missing JSON field {name}"))
        + marker.len();
    match &json[start..] {
        value if value.starts_with("true") => true,
        value if value.starts_with("false") => false,
        _ => panic!("invalid JSON boolean field {name}"),
    }
}

fn assert_target(json: &str, expected: &str) {
    assert_eq!(json_string(json, "target"), expected);
}

#[test]
fn killed_submit_client_resyncs_applied_input_without_replay() {
    let mut fixture = Fixture::new();
    let token = format!("INFLIGHT-TOKEN-{}", std::process::id());
    let check = format!("INFLIGHT-CHECK-{}", std::process::id());
    let record = fixture.record_path();
    let script = format!(
        "/bin/stty -echo; printf 'INFLIGHT-READY:%s\\n' \"$$\"; handled=0; \\
         while IFS= read -r received; do \\
           if [ \"$received\" = '{token}' ]; then \\
             handled=$((handled + 1)); printf '%s\\n' '{token}' >> '{}'; \\
             printf 'INFLIGHT-EFFECT:%s\\n' \"$$\"; \\
           elif [ \"$received\" = '{check}' ]; then \\
             printf 'INFLIGHT-CHECK:%s:%s\\n' \"$$\" \"$handled\"; \\
           fi; \\
         done",
        record.display()
    );
    fixture.start(&script);

    let server_dir = fixture.server_directory();
    let caller_context = fixture.context_path();
    let init = fixture.control(&server_dir, &caller_context, &["init"]);
    assert_success(&init, "initialize persistent caller context");
    let initial_generation = json_u64(&String::from_utf8_lossy(&init.stdout), "generation");
    let initial_lease = json_u64(&String::from_utf8_lossy(&init.stdout), "lease");
    assert!(initial_generation > 0 && initial_lease > 0);

    let describe = fixture.control(&server_dir, &caller_context, &["describe"]);
    let describe_json = assert_success(&describe, "describe controlled shell");
    let target = json_string(&describe_json, "target");
    let owner_revision = json_u64(&describe_json, "owner_revision");
    assert!(!json_bool(&describe_json, "child_exited"));

    let observer_context = fixture.observer_context_path();
    let observer_init = fixture.control(&server_dir, &observer_context, &["init", target.as_str()]);
    assert_success(
        &observer_init,
        "initialize separate read-only observer context",
    );
    let observer_describe = fixture.control(&server_dir, &observer_context, &["describe"]);
    let observer_describe_json = assert_success(&observer_describe, "describe through observer");
    assert_target(&observer_describe_json, &target);
    let ready_pid = poll_ready_pid(&mut fixture, &server_dir, &observer_context, &target);

    let claim = fixture.control(
        &server_dir,
        &caller_context,
        &["claim", target.as_str(), &owner_revision.to_string()],
    );
    let claim_json = assert_success(&claim, "claim input ownership");
    assert_eq!(json_string(&claim_json, "status"), "completed");
    assert_eq!(json_string(&claim_json, "outcome"), "applied");
    let claimed_describe = fixture.control(&server_dir, &caller_context, &["describe"]);
    let claimed_json = assert_success(&claimed_describe, "confirm caller owns input");
    assert_target(&claimed_json, &target);
    let claimed_revision = json_u64(&claimed_json, "owner_revision");
    assert_eq!(json_u64(&claimed_json, "owner_lease"), initial_lease);

    let before_submit = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("open caller before the submitting invocation");
    assert_eq!(before_submit.context().uncertain(), None);
    let next_before_submit = before_submit.context().next_sequence();
    drop(before_submit);

    // Create the proxy endpoint separately from the server endpoint. The
    // durable caller context carries only generation/lease, so this CLI
    // invocation routes through the test-owned one-connection proxy.
    let proxy_dir = fixture.proxy_directory();
    private_dir(&proxy_dir);
    let parsed_target = parse_target(&target);
    let mut proxy = Proxy::start(
        proxy_dir.clone(),
        server_dir.clone(),
        parsed_target,
        claimed_revision,
        token.clone(),
    );
    let submit_args = vec![
        "control".to_owned(),
        "--directory".to_owned(),
        proxy_dir.display().to_string(),
        "--context".to_owned(),
        caller_context.display().to_string(),
        "submit".to_owned(),
        target.clone(),
        claimed_revision.to_string(),
        token.clone(),
    ];
    let (mut submit_child, submit_stdout, submit_stderr) =
        OwnedClient::spawn(&mut fixture, &submit_args, "submit");

    let held = proxy.held_response();
    assert_eq!(held.target, parsed_target);
    submit_child.assert_running();
    let kill_status = submit_child.kill_with_sigkill_and_reap();
    assert_eq!(kill_status.signal(), Some(libc::SIGKILL));
    assert_eq!(held.key.generation, initial_generation);
    assert_eq!(held.key.lease, initial_lease);
    assert_eq!(held.key.sequence, next_before_submit);
    proxy.release_and_join(true);

    let submit_out = Fixture::read_capture(&submit_stdout);
    let submit_err = Fixture::read_capture(&submit_stderr);
    assert!(
        submit_out.is_empty(),
        "killed CLI emitted no completion JSON"
    );
    assert!(
        submit_err.is_empty(),
        "killed CLI emitted no diagnostic JSON"
    );

    let uncertain = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller context after killed process released its lock");
    let state = uncertain.context();
    assert_eq!(state.generation(), initial_generation);
    assert_eq!(state.lease(), initial_lease);
    assert_eq!(state.uncertain(), Some(held.key));
    assert_eq!(state.next_sequence(), held.key.sequence + 1);
    drop(uncertain);

    let observed = poll_observer_read(
        &mut fixture,
        &server_dir,
        &observer_context,
        &target,
        &ready_pid,
        &token,
    );
    assert_target(&observed, &target);
    let observed_text = json_string(&observed, "text");
    assert!(
        observed_text.contains(&format!("INFLIGHT-EFFECT:{ready_pid}")),
        "separate observer sees the handled token marker"
    );
    assert_eq!(
        fs::read_to_string(fixture.record_path()).expect("private append record"),
        format!("{token}\n")
    );
    let observed_describe = fixture.control(&server_dir, &observer_context, &["describe"]);
    let observed_describe_json = assert_success(
        &observed_describe,
        "confirm same server target after submit",
    );
    assert_target(&observed_describe_json, &target);
    assert!(!json_bool(&observed_describe_json, "child_exited"));

    let status = fixture.control(
        &server_dir,
        &caller_context,
        &["status", &held.key.sequence.to_string()],
    );
    let status_json = assert_success(&status, "read exact uncertain operation status");
    assert_eq!(json_u64(&status_json, "generation"), held.key.generation);
    assert_eq!(json_u64(&status_json, "lease"), held.key.lease);
    assert_eq!(json_u64(&status_json, "sequence"), held.key.sequence);
    assert_eq!(json_string(&status_json, "status"), "completed");
    assert_eq!(json_string(&status_json, "outcome"), "applied");
    let still_uncertain = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller after read-only status");
    assert_eq!(still_uncertain.context().uncertain(), Some(held.key));
    assert_eq!(
        still_uncertain.context().next_sequence(),
        held.key.sequence + 1
    );
    drop(still_uncertain);

    let resync = fixture.control(&server_dir, &caller_context, &["resync"]);
    let resync_json = assert_success(&resync, "resync exact uncertain operation");
    assert_eq!(json_u64(&resync_json, "generation"), held.key.generation);
    assert_eq!(json_u64(&resync_json, "lease"), held.key.lease);
    assert_eq!(json_u64(&resync_json, "sequence"), held.key.sequence);
    assert_eq!(json_string(&resync_json, "status"), "completed");
    assert_eq!(json_string(&resync_json, "outcome"), "applied");
    assert!(!json_bool(&resync_json, "uncertain"));
    assert!(json_bool(&resync_json, "context_cleared"));
    let recovered = fixture.control(&server_dir, &caller_context, &["show"]);
    let recovered_json = assert_success(&recovered, "show recovered caller context");
    assert_eq!(json_u64(&recovered_json, "generation"), initial_generation);
    assert_eq!(json_u64(&recovered_json, "lease"), initial_lease);
    assert_eq!(
        json_u64(&recovered_json, "next_sequence"),
        held.key.sequence + 1
    );
    assert!(recovered_json.contains("\"uncertain\":null"));
    let second_resync = fixture.control(&server_dir, &caller_context, &["resync"]);
    let second_resync_json = assert_success(&second_resync, "verify repeated resync is clear");
    assert!(json_bool(&second_resync_json, "ok"));
    assert_eq!(json_string(&second_resync_json, "action"), "resync");
    assert!(!json_bool(&second_resync_json, "uncertain"));
    assert!(!json_bool(&second_resync_json, "context_cleared"));
    let clear = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller after repeated resync");
    assert_eq!(clear.context().generation(), initial_generation);
    assert_eq!(clear.context().lease(), initial_lease);
    assert_eq!(clear.context().uncertain(), None);
    assert_eq!(clear.context().next_sequence(), next_before_submit + 1);
    drop(clear);

    let check_submit = fixture.control(
        &server_dir,
        &caller_context,
        &[
            "submit",
            target.as_str(),
            &claimed_revision.to_string(),
            check.as_str(),
        ],
    );
    let check_json = assert_success(
        &check_submit,
        "submit explicit count check through recovered caller",
    );
    assert_eq!(json_string(&check_json, "status"), "completed");
    assert_eq!(json_string(&check_json, "outcome"), "applied");
    let check_observed = poll_observer_read(
        &mut fixture,
        &server_dir,
        &observer_context,
        &target,
        &ready_pid,
        &check,
    );
    let check_text = json_string(&check_observed, "text");
    assert!(
        check_text.contains(&format!("INFLIGHT-CHECK:{ready_pid}:1")),
        "explicit CHECK reports exactly one handled token"
    );
    assert_eq!(
        check_text
            .matches(&format!("INFLIGHT-EFFECT:{ready_pid}"))
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(fixture.record_path()).expect("private append record after check"),
        format!("{token}\n")
    );
    let final_info = fixture.control(&server_dir, &observer_context, &["describe"]);
    let final_info_json = assert_success(
        &final_info,
        "confirm same live child and target after recovery",
    );
    assert_target(&final_info_json, &target);
    assert!(!json_bool(&final_info_json, "child_exited"));
    fixture.assert_live_server();

    // Server shutdown is an explicit fixture action; Fixture::drop also has a
    // bounded fallback for assertion unwinds.
    let server = fixture.server.as_mut().expect("owned server child");
    assert!(server
        .try_wait()
        .expect("server live before explicit stop")
        .is_none());
    // SAFETY: signal only this test's still-live, unreaped server Child.
    assert_eq!(
        unsafe { libc::kill(server.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let stopped = wait_owned_child(server, STOP_DEADLINE);
    assert!(stopped.success(), "explicit SIGTERM shutdown: {stopped}");
    wait_for_server_exit(&mut fixture);
    assert!(
        !server_dir.join("control.sock").exists(),
        "explicit stop removes the owned server endpoint"
    );
}

#[test]
fn killed_submit_client_before_admission_resyncs_without_sending_submit() {
    let mut fixture = Fixture::new();
    let token = format!("INFLIGHT-TOKEN-PRE-ADMISSION-{}", std::process::id());
    let check = format!("INFLIGHT-CHECK-PRE-ADMISSION-{}", std::process::id());
    let record = fixture.record_path();
    let script = format!(
        "/bin/stty -echo; printf 'INFLIGHT-READY:%s\\n' \"$$\"; handled=0; \\
         while IFS= read -r received; do \\
           if [ \"$received\" = '{token}' ]; then \\
             handled=$((handled + 1)); printf '%s\\n' '{token}' >> '{}'; \\
             printf 'INFLIGHT-EFFECT:%s\\n' \"$$\"; \\
           elif [ \"$received\" = '{check}' ]; then \\
             printf 'INFLIGHT-CHECK:%s:%s\\n' \"$$\" \"$handled\"; \\
           fi; \\
         done",
        record.display()
    );
    fixture.start(&script);

    let server_dir = fixture.server_directory();
    let caller_context = fixture.context_path();
    let init = fixture.control(&server_dir, &caller_context, &["init"]);
    assert_success(&init, "initialize pre-admission caller context");
    let initial_generation = json_u64(&String::from_utf8_lossy(&init.stdout), "generation");
    let initial_lease = json_u64(&String::from_utf8_lossy(&init.stdout), "lease");
    assert!(initial_generation > 0 && initial_lease > 0);

    let describe = fixture.control(&server_dir, &caller_context, &["describe"]);
    let describe_json = assert_success(&describe, "describe pre-admission target");
    let target = json_string(&describe_json, "target");
    let owner_revision = json_u64(&describe_json, "owner_revision");
    assert!(!json_bool(&describe_json, "child_exited"));

    let observer_context = fixture.observer_context_path();
    let observer_init = fixture.control(&server_dir, &observer_context, &["init", target.as_str()]);
    assert_success(&observer_init, "initialize pre-admission observer context");
    let observer_describe = fixture.control(&server_dir, &observer_context, &["describe"]);
    let observer_describe_json = assert_success(&observer_describe, "describe target as observer");
    assert_target(&observer_describe_json, &target);
    let ready_pid = poll_ready_pid(&mut fixture, &server_dir, &observer_context, &target);

    let claim = fixture.control(
        &server_dir,
        &caller_context,
        &["claim", target.as_str(), &owner_revision.to_string()],
    );
    let claim_json = assert_success(&claim, "claim pre-admission input ownership");
    assert_eq!(json_string(&claim_json, "status"), "completed");
    assert_eq!(json_string(&claim_json, "outcome"), "applied");
    let claimed_describe = fixture.control(&server_dir, &caller_context, &["describe"]);
    let claimed_json = assert_success(&claimed_describe, "confirm pre-admission owner");
    assert_target(&claimed_json, &target);
    let claimed_revision = json_u64(&claimed_json, "owner_revision");
    assert_eq!(json_u64(&claimed_json, "owner_lease"), initial_lease);

    let before_submit = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("open caller before pre-admission Submit");
    assert_eq!(before_submit.context().uncertain(), None);
    let next_before_submit = before_submit.context().next_sequence();
    drop(before_submit);

    let proxy_dir = fixture.proxy_directory();
    private_dir(&proxy_dir);
    let parsed_target = parse_target(&target);
    let mut proxy = PreAdmissionProxy::start(
        proxy_dir.clone(),
        server_dir.clone(),
        parsed_target,
        claimed_revision,
        token.clone(),
    );
    let submit_args = vec![
        "control".to_owned(),
        "--directory".to_owned(),
        proxy_dir.display().to_string(),
        "--context".to_owned(),
        caller_context.display().to_string(),
        "submit".to_owned(),
        target.clone(),
        claimed_revision.to_string(),
        token.clone(),
    ];
    let (mut submit_child, submit_stdout, submit_stderr) =
        OwnedClient::spawn(&mut fixture, &submit_args, "pre-admission-submit");

    let held = proxy.held_submit();
    assert_eq!(held.target, parsed_target);
    assert!(held.request_id > 0);
    submit_child.assert_running();
    let kill_status = submit_child.kill_with_sigkill_and_reap();
    assert_eq!(kill_status.signal(), Some(libc::SIGKILL));
    assert_eq!(held.key.generation, initial_generation);
    assert_eq!(held.key.lease, initial_lease);
    assert_eq!(held.key.sequence, next_before_submit);
    proxy.release_and_join(true);

    assert!(Fixture::read_capture(&submit_stdout).is_empty());
    assert!(Fixture::read_capture(&submit_stderr).is_empty());
    let uncertain = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller after exact CLI child released its context lock");
    let state = uncertain.context();
    assert_eq!(state.generation(), initial_generation);
    assert_eq!(state.lease(), initial_lease);
    assert_eq!(state.uncertain(), Some(held.key));
    assert_eq!(state.next_sequence(), held.key.sequence + 1);
    drop(uncertain);

    // Status and resync use the real server directory directly. The proxy
    // captured the Submit but never wrote it upstream.
    let status = fixture.control(
        &server_dir,
        &caller_context,
        &["status", &held.key.sequence.to_string()],
    );
    let status_json = String::from_utf8_lossy(&status.stdout).into_owned();
    assert_eq!(status.status.code(), Some(1));
    assert!(status.stderr.is_empty());
    assert!(!json_bool(&status_json, "ok"));
    assert_eq!(json_u64(&status_json, "generation"), held.key.generation);
    assert_eq!(json_u64(&status_json, "lease"), held.key.lease);
    assert_eq!(json_u64(&status_json, "sequence"), held.key.sequence);
    assert_eq!(json_string(&status_json, "status"), "not_admitted");
    let still_uncertain = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller after read-only NotAdmitted status");
    assert_eq!(still_uncertain.context().uncertain(), Some(held.key));
    assert_eq!(
        still_uncertain.context().next_sequence(),
        held.key.sequence + 1
    );
    drop(still_uncertain);

    let resync = fixture.control(&server_dir, &caller_context, &["resync"]);
    let resync_json = String::from_utf8_lossy(&resync.stdout).into_owned();
    assert_eq!(resync.status.code(), Some(1));
    assert!(resync.stderr.is_empty());
    assert!(!json_bool(&resync_json, "ok"));
    assert_eq!(json_u64(&resync_json, "generation"), held.key.generation);
    assert_eq!(json_u64(&resync_json, "lease"), held.key.lease);
    assert_eq!(json_u64(&resync_json, "sequence"), held.key.sequence);
    assert_eq!(json_string(&resync_json, "status"), "not_admitted");
    assert!(!json_bool(&resync_json, "uncertain"));
    assert!(json_bool(&resync_json, "context_cleared"));
    let recovered = fixture.control(&server_dir, &caller_context, &["show"]);
    let recovered_json = assert_success(&recovered, "show caller after NotAdmitted resync");
    assert_eq!(json_u64(&recovered_json, "generation"), initial_generation);
    assert_eq!(json_u64(&recovered_json, "lease"), initial_lease);
    assert_eq!(
        json_u64(&recovered_json, "next_sequence"),
        held.key.sequence + 1
    );
    assert!(recovered_json.contains("\"uncertain\":null"));
    let second_resync = fixture.control(&server_dir, &caller_context, &["resync"]);
    let second_json = assert_success(&second_resync, "verify repeated NotAdmitted resync");
    assert!(!json_bool(&second_json, "uncertain"));
    assert!(!json_bool(&second_json, "context_cleared"));
    let clear = ContextFile::open_existing(&caller_context, Instant::now() + IO_DEADLINE)
        .expect("reopen caller after repeated NotAdmitted resync");
    assert_eq!(clear.context().generation(), initial_generation);
    assert_eq!(clear.context().lease(), initial_lease);
    assert_eq!(clear.context().uncertain(), None);
    assert_eq!(clear.context().next_sequence(), next_before_submit + 1);
    drop(clear);

    let check_submit = fixture.control(
        &server_dir,
        &caller_context,
        &[
            "submit",
            target.as_str(),
            &claimed_revision.to_string(),
            check.as_str(),
        ],
    );
    let check_json = assert_success(&check_submit, "submit explicit zero-count CHECK");
    assert_eq!(json_string(&check_json, "status"), "completed");
    assert_eq!(json_string(&check_json, "outcome"), "applied");
    let check_observed = poll_observer_read(
        &mut fixture,
        &server_dir,
        &observer_context,
        &target,
        &ready_pid,
        &check,
    );
    assert_target(&check_observed, &target);
    let check_text = json_string(&check_observed, "text");
    assert!(
        check_text.contains(&format!("INFLIGHT-CHECK:{ready_pid}:0")),
        "explicit CHECK reports zero handled tokens"
    );
    assert!(!check_text.contains(&format!("INFLIGHT-EFFECT:{ready_pid}")));
    assert!(!fixture.record_path().exists());

    let final_info = fixture.control(&server_dir, &observer_context, &["describe"]);
    let final_json = assert_success(&final_info, "confirm original target after NotAdmitted");
    assert_target(&final_json, &target);
    assert!(!json_bool(&final_json, "child_exited"));
    let pid = ready_pid.parse::<u32>().expect("shell PID is numeric");
    let pid_status = bounded_status(
        {
            let mut command = ProcessCommand::new("/bin/kill");
            command.args(["-0", &pid.to_string()]);
            command
        },
        PROCESS_DEADLINE,
    );
    assert!(
        pid_status.success(),
        "original controlled shell PID remains live"
    );
    fixture.assert_live_server();

    let server = fixture.server.as_mut().expect("owned server child");
    assert!(server
        .try_wait()
        .expect("server live before explicit stop")
        .is_none());
    // SAFETY: signal only this test's still-live, unreaped server Child.
    assert_eq!(
        unsafe { libc::kill(server.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let stopped = wait_owned_child(server, STOP_DEADLINE);
    assert!(stopped.success(), "explicit SIGTERM shutdown: {stopped}");
    wait_for_server_exit(&mut fixture);
    assert!(
        !server_dir.join("control.sock").exists(),
        "explicit stop removes the owned server endpoint"
    );
}

fn poll_ready_pid(fixture: &mut Fixture, directory: &Path, context: &Path, target: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let read = fixture.control(directory, context, &["read", target]);
        let json = assert_success(&read, "read startup screen through observer");
        let text = json_string(&json, "text");
        if let Some(line) = text.lines().find(|line| line.contains("INFLIGHT-READY:")) {
            return line
                .split("INFLIGHT-READY:")
                .nth(1)
                .expect("ready marker PID")
                .trim()
                .to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "controlled shell readiness deadline elapsed"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn poll_observer_read(
    fixture: &mut Fixture,
    directory: &Path,
    context: &Path,
    target: &str,
    pid: &str,
    marker: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let read = fixture.control(directory, context, &["read", target]);
        let json = assert_success(&read, "read current screen through separate observer");
        let text = json_string(&json, "text");
        let has_marker = if marker.starts_with("INFLIGHT-TOKEN-") {
            text.contains(&format!("INFLIGHT-EFFECT:{pid}"))
        } else {
            text.contains(&format!("INFLIGHT-CHECK:{pid}:"))
        };
        if has_marker {
            return json;
        }
        assert!(
            Instant::now() < deadline,
            "observer marker deadline elapsed"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn parse_target(target: &str) -> TargetRef {
    let values: Vec<u64> = target
        .split(':')
        .map(|field| field.parse().expect("numeric full target field"))
        .collect();
    assert_eq!(
        values.len(),
        5,
        "target has all five immutable identity fields"
    );
    TargetRef {
        generation: values[0],
        workspace: values[1],
        pane: values[2],
        session: values[3],
        occupant_revision: values[4],
    }
}

fn wait_for_server_exit(fixture: &mut Fixture) {
    let deadline = Instant::now() + STOP_DEADLINE;
    let server = fixture.server.as_mut().expect("owned server child");
    while server
        .try_wait()
        .expect("poll explicitly stopped server")
        .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "server did not exit after explicit stop"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
