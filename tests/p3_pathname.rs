#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CAPTURE_LIMIT: usize = 16 * 1024;
const STARTUP_LINE_LIMIT: usize = 4096;
const PROCESS_LIMIT: Duration = Duration::from_secs(8);
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(4);
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    terminfo: PathBuf,
    context: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        for _ in 0..32 {
            let suffix = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = PathBuf::from(format!(
                "/private/tmp/panelink-p3-pathname-{}-{suffix}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
                    let runtime = root.join("runtime");
                    let terminfo = root.join("terminfo");
                    fs::create_dir(&runtime)?;
                    fs::create_dir(&terminfo)?;
                    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
                    fs::set_permissions(&terminfo, fs::Permissions::from_mode(0o700))?;
                    return Ok(Self {
                        context: root.join("caller.context"),
                        root,
                        runtime,
                        terminfo,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a unique pathname fixture directory",
        ))
    }

    fn write_capture(&self, label: &str, stream: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.root.join(format!("{label}.{stream}.log")))?;
        file.write_all(bytes)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct BoundedCapture {
    bytes: Mutex<Vec<u8>>,
    overflowed: AtomicBool,
}

impl BoundedCapture {
    fn new() -> Self {
        Self {
            bytes: Mutex::new(Vec::with_capacity(CAPTURE_LIMIT)),
            overflowed: AtomicBool::new(false),
        }
    }

    fn append(&self, input: &[u8]) {
        let mut bytes = self.bytes.lock().unwrap_or_else(|error| error.into_inner());
        let remaining = CAPTURE_LIMIT.saturating_sub(bytes.len());
        bytes.extend_from_slice(&input[..input.len().min(remaining)]);
        if input.len() > remaining {
            self.overflowed.store(true, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        self.bytes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Relaxed)
    }
}

struct ManagedChild<'a> {
    child: Child,
    stdout: Arc<BoundedCapture>,
    stderr: Arc<BoundedCapture>,
    stdout_reader: Option<JoinHandle<io::Result<()>>>,
    stderr_reader: Option<JoinHandle<io::Result<()>>>,
    startup: Option<mpsc::Receiver<Result<Vec<u8>, ()>>>,
    fixture: &'a Fixture,
    label: String,
    persisted: bool,
}

impl<'a> ManagedChild<'a> {
    fn spawn(
        fixture: &'a Fixture,
        label: &str,
        command: &mut Command,
        capture_startup: bool,
    ) -> io::Result<Self> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout_pipe = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "stdout pipe was not created")
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "stderr pipe was not created")
        })?;
        let stdout = Arc::new(BoundedCapture::new());
        let stderr = Arc::new(BoundedCapture::new());
        let (startup, startup_receiver) = if capture_startup {
            let (sender, receiver) = mpsc::sync_channel(1);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let stdout_reader = Some(spawn_reader(stdout_pipe, Arc::clone(&stdout), startup));
        let stderr_reader = Some(spawn_reader(stderr_pipe, Arc::clone(&stderr), None));
        Ok(Self {
            child,
            stdout,
            stderr,
            stdout_reader,
            stderr_reader,
            startup: startup_receiver,
            fixture,
            label: label.to_owned(),
            persisted: false,
        })
    }

    fn startup_line(&mut self, deadline: Instant) -> io::Result<Vec<u8>> {
        let receiver = self.startup.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "startup capture is disabled")
        })?;
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(line)) => return Ok(line),
                Ok(Err(())) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "server startup line exceeded its bound",
                    ))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "server stdout closed before startup JSON",
                    ))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "server did not emit startup JSON before deadline",
                        ));
                    }
                    if self.child.try_wait()?.is_some() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "server exited before startup JSON",
                        ));
                    }
                }
            }
        }
    }

    fn wait(&mut self, deadline: Instant) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.join_readers(deadline)?;
                self.persist()?;
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child process exceeded its deadline",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop_with_sigterm(&mut self, deadline: Instant) -> io::Result<ExitStatus> {
        let pid = self.child.id();
        // SAFETY: pid comes from this guard's own spawned child and is signaled
        // only for the bounded, explicit fixture shutdown.
        let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if sent != 0 {
            if let Some(status) = self.child.try_wait()? {
                self.join_readers(deadline)?;
                self.persist()?;
                return Ok(status);
            }
            return Err(io::Error::last_os_error());
        }
        self.wait(deadline)
    }

    fn startup_error_context(&self) -> String {
        format!(
            "stdout={:?}; stderr={:?}",
            String::from_utf8_lossy(&self.stdout.snapshot()),
            String::from_utf8_lossy(&self.stderr.snapshot())
        )
    }

    fn persist(&mut self) -> io::Result<()> {
        if !self.persisted {
            self.fixture
                .write_capture(&self.label, "stdout", &self.stdout.snapshot())?;
            self.fixture
                .write_capture(&self.label, "stderr", &self.stderr.snapshot())?;
            self.persisted = true;
        }
        Ok(())
    }

    fn join_readers(&mut self, deadline: Instant) -> io::Result<()> {
        for reader in [&mut self.stdout_reader, &mut self.stderr_reader] {
            while reader.as_ref().is_some_and(|handle| !handle.is_finished()) {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "bounded output reader did not finish before deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            if let Some(handle) = reader.take() {
                handle
                    .join()
                    .map_err(|_| io::Error::other("output reader panicked"))??;
            }
        }
        Ok(())
    }

    fn assert_capture_within_limit(&self) -> io::Result<()> {
        if self.stdout.overflowed() || self.stderr.overflowed() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "child output exceeded the per-stream capture limit",
            ));
        }
        Ok(())
    }
}

impl Drop for ManagedChild<'_> {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let pid = self.child.id();
            // SAFETY: this guard stores only the PID returned by its own child.
            let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            let graceful_deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < graceful_deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
                let kill_deadline = Instant::now() + Duration::from_millis(500);
                while Instant::now() < kill_deadline {
                    if self.child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        let reader_deadline = Instant::now() + Duration::from_millis(500);
        let _ = self.join_readers(reader_deadline);
        let _ = self.persist();
    }
}

fn spawn_reader<R>(
    mut reader: R,
    capture: Arc<BoundedCapture>,
    startup: Option<mpsc::SyncSender<Result<Vec<u8>, ()>>>,
) -> JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        let mut startup_line = Vec::with_capacity(128);
        let mut startup_sender = startup;
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            let chunk = &buffer[..read];
            capture.append(chunk);
            if startup_sender.is_some() {
                let mut startup_result = None;
                for byte in chunk {
                    if startup_line.len() == STARTUP_LINE_LIMIT {
                        startup_result = Some(Err(()));
                        break;
                    }
                    if *byte == b'\n' {
                        startup_result = Some(Ok(std::mem::take(&mut startup_line)));
                        break;
                    }
                    startup_line.push(*byte);
                }
                if let Some(result) = startup_result {
                    if let Some(sender) = startup_sender.take() {
                        let _ = sender.send(result);
                    }
                }
            }
        }
        if let Some(sender) = startup_sender {
            let _ = sender.send(Err(()));
        }
        Ok(())
    })
}

#[test]
#[ignore = "requires pathname Unix socket bind/accept outside restricted sandbox"]
fn pathname_server_cli_context_reuse() {
    let fixture = Fixture::new().expect("private fixture directory");
    assert_private_directory(&fixture.root);
    assert_private_directory(&fixture.runtime);
    assert_private_directory(&fixture.terminfo);

    let terminfo_source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
    let mut tic = Command::new("/usr/bin/tic");
    tic.args([OsStr::new("-x"), OsStr::new("-o")])
        .arg(&fixture.terminfo)
        .arg(&terminfo_source)
        .env_clear()
        .env("HOME", &fixture.root)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&fixture.root);
    let tic = run_short(&fixture, "tic", &mut tic).expect("compile private terminfo");
    assert!(tic.status.success(), "tic failed: {}", tic.output);
    assert!(
        fixture.terminfo.join("p/panelink-probe").is_file()
            || fixture.terminfo.join("70/panelink-probe").is_file()
    );

    let nonce = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let token = format!("P3PATH-TOKEN-{}-{nonce}", std::process::id());
    let script = format!(
        "printf 'P3PATH-READY-%s\\n' \"$$\"; handled=0; while IFS= read -r received; do if [ \"$received\" = '{token}' ]; then handled=$((handled + 1)); printf 'P3PATH-GOT-%s-%s\\n' \"$$\" \"$handled\"; fi; done"
    );
    let mut startup_command = server_command(&fixture, true, &script);
    let mut server = ManagedChild::spawn(&fixture, "server", &mut startup_command, true)
        .expect("spawn the actual Panelink binary");

    let startup_line = match server.startup_line(Instant::now() + PROCESS_LIMIT) {
        Ok(line) => line,
        Err(error) => fail_server_startup(&fixture, &mut server, &error.to_string()),
    };
    let startup_json = std::str::from_utf8(&startup_line).expect("startup JSON is UTF-8");
    if json_string(startup_json, "action").as_deref() != Some("server-probe")
        || json_string(startup_json, "state").as_deref() != Some("running")
    {
        let reason = json_string(startup_json, "error")
            .unwrap_or_else(|| "first stdout line was not running JSON".to_owned());
        fail_server_startup(&fixture, &mut server, &reason);
    }
    assert_eq!(
        json_string(startup_json, "action").as_deref(),
        Some("server-probe")
    );
    assert_eq!(
        json_string(startup_json, "state").as_deref(),
        Some("running")
    );
    assert_eq!(
        json_string(startup_json, "startup_outcome").as_deref(),
        Some("applied")
    );
    assert_eq!(json_bool(startup_json, "allow_input"), Some(true));
    assert!(server
        .child
        .try_wait()
        .expect("server liveness check")
        .is_none());

    let socket_path = fixture.runtime.join("control.sock");
    let socket_before = fs::symlink_metadata(&socket_path).expect("bound pathname socket");
    assert!(socket_before.file_type().is_socket());
    assert_eq!(socket_before.mode() & 0o7777, 0o600);
    assert_eq!(socket_before.uid(), unsafe { libc::geteuid() });

    let init = control(&fixture, "init", &[]);
    assert_ok_action(&init.output, "init");
    let generation = json_u64(&init.output, "generation").expect("init generation");
    let lease = json_u64(&init.output, "lease").expect("init lease");
    assert!(generation > 0 && lease > 0);
    assert_eq!(json_u64(startup_json, "generation"), Some(generation));
    assert!(json_null(&init.output, "uncertain"));
    assert_context_identity(&control(&fixture, "show", &[]).output, generation, lease);

    let described = control(&fixture, "describe", &[]);
    assert_ok_action(&described.output, "describe");
    let target = json_string(&described.output, "target").expect("explicit describe target");
    validate_target(&target);
    let owner_revision = json_u64(&described.output, "owner_revision").expect("owner revision");
    assert_eq!(json_bool(&described.output, "child_exited"), Some(false));

    let ready = read_until(&fixture, &target, "P3PATH-READY-", 4);
    let ready_json = parse_object(&ready).expect("ready Read JSON");
    let ready_text = json_string(ready_json, "text").expect("ready screen text");
    let server_child_pid = marker_pid(&ready_text, "P3PATH-READY-");
    assert_eq!(
        json_string(ready_json, "target").as_deref(),
        Some(target.as_str())
    );

    let claim = control(&fixture, "claim", &[&target, &owner_revision.to_string()]);
    assert_eq!(claim.status.code(), Some(0), "claim: {}", claim.output);
    assert_eq!(
        json_string(&claim.output, "status").as_deref(),
        Some("completed")
    );
    assert_eq!(
        json_string(&claim.output, "outcome").as_deref(),
        Some("applied")
    );
    assert_context_identity(&control(&fixture, "show", &[]).output, generation, lease);

    let claimed = control(&fixture, "describe", &[]);
    assert_ok_action(&claimed.output, "describe");
    assert_eq!(
        json_string(&claimed.output, "target").as_deref(),
        Some(target.as_str())
    );
    let claimed_revision =
        json_u64(&claimed.output, "owner_revision").expect("post-claim owner revision");
    assert_eq!(json_u64(&claimed.output, "owner_lease"), Some(lease));
    assert_eq!(json_bool(&claimed.output, "child_exited"), Some(false));

    let before_reconcile = fs::read(&fixture.context).expect("context before reconcile");
    let reconciled = control(&fixture, "reconcile", &[&target]);
    assert_eq!(
        reconciled.status.code(),
        Some(0),
        "reconcile: {}",
        reconciled.output
    );
    assert_ok_action(&reconciled.output, "reconcile");
    let initial_cursor_text = json_string(&reconciled.output, "cursor").expect("reconcile cursor");
    let (initial_cursor_generation, initial_cursor_sequence) =
        parse_event_cursor(&initial_cursor_text).expect("generation:sequence cursor");
    assert_eq!(initial_cursor_generation, generation);
    let initial_screen_revision =
        json_u64(&reconciled.output, "screen_revision").expect("screen revision");
    assert!(initial_screen_revision > 0);
    assert_eq!(
        json_u64(&reconciled.output, "owner_revision"),
        Some(claimed_revision)
    );
    assert_eq!(json_u64(&reconciled.output, "owner_lease"), Some(lease));
    assert_eq!(json_bool(&reconciled.output, "child_exited"), Some(false));
    assert_eq!(
        json_string(&reconciled.output, "target").as_deref(),
        Some(target.as_str())
    );
    assert_eq!(
        fs::read(&fixture.context).expect("context immediately after reconcile"),
        before_reconcile,
        "reconcile must not reserve a caller sequence"
    );

    let before_submit_reconcile =
        fs::read(&fixture.context).expect("context before submit cursor reconcile");
    let submit_cursor = control(&fixture, "reconcile", &[&target]);
    assert_eq!(
        submit_cursor.status.code(),
        Some(0),
        "reconcile: {}",
        submit_cursor.output
    );
    assert_ok_action(&submit_cursor.output, "reconcile");
    let submit_cursor_text =
        json_string(&submit_cursor.output, "cursor").expect("pre-submit cursor");
    let (submit_cursor_generation, submit_cursor_sequence) =
        parse_event_cursor(&submit_cursor_text).expect("generation:sequence cursor");
    assert_eq!(submit_cursor_generation, generation);
    assert!(submit_cursor_sequence >= initial_cursor_sequence);
    assert_eq!(
        fs::read(&fixture.context).expect("context immediately after submit cursor reconcile"),
        before_submit_reconcile,
        "reconcile must not reserve a caller sequence"
    );

    let submit = control(
        &fixture,
        "submit",
        &[&target, &claimed_revision.to_string(), &token],
    );
    assert_eq!(submit.status.code(), Some(0), "submit: {}", submit.output);
    assert_eq!(
        json_string(&submit.output, "status").as_deref(),
        Some("completed")
    );
    assert_eq!(
        json_string(&submit.output, "outcome").as_deref(),
        Some("applied")
    );
    assert_context_identity(&control(&fixture, "show", &[]).output, generation, lease);

    let got_marker = format!("P3PATH-GOT-{server_child_pid}-1");
    let got = read_until(&fixture, &target, &got_marker, 4);
    let got_object = parse_object(&got).expect("GOT Read JSON");
    let got_text = json_string(got_object, "text").expect("GOT screen text");
    assert_eq!(count_occurrences(&got_text, &got_marker), 1);
    assert_eq!(json_u64(got_object, "generation"), Some(generation));
    assert_eq!(json_u64(got_object, "lease"), Some(lease));

    let before_watch = fs::read(&fixture.context).expect("context before watch");
    let cursor_argument = format!("{submit_cursor_generation}:{submit_cursor_sequence}");
    let watch = control(
        &fixture,
        "watch",
        &[&target, &cursor_argument, "64", "1000"],
    );
    assert_eq!(watch.status.code(), Some(0), "watch: {}", watch.output);
    assert_ok_action(&watch.output, "watch");
    assert_eq!(
        json_string(&watch.output, "status").as_deref(),
        Some("changed"),
        "watch did not report a changed event: {}",
        watch.output
    );
    let events = json_field(&watch.output, "events").expect("watch events array");
    assert!(events.starts_with('['));
    assert!(
        count_occurrences(events, "\"kind\":") >= 1,
        "watch returned no events"
    );
    let watch_cursor_text = json_string(&watch.output, "cursor").expect("watch cursor");
    let (watch_cursor_generation, watch_cursor_sequence) =
        parse_event_cursor(&watch_cursor_text).expect("generation:sequence cursor");
    assert_eq!(watch_cursor_generation, generation);
    assert!(watch_cursor_sequence > submit_cursor_sequence);
    assert_eq!(
        fs::read(&fixture.context).expect("context immediately after watch"),
        before_watch,
        "watch must not reserve a caller sequence"
    );

    let timeout_baseline = control(&fixture, "reconcile", &[&target]);
    assert_eq!(
        timeout_baseline.status.code(),
        Some(0),
        "reconcile before timeout poll: {}",
        timeout_baseline.output
    );
    assert_ok_action(&timeout_baseline.output, "reconcile");
    let timeout_cursor_text =
        json_string(&timeout_baseline.output, "cursor").expect("timeout baseline cursor");
    let (timeout_cursor_generation, timeout_cursor_sequence) =
        parse_event_cursor(&timeout_cursor_text).expect("generation:sequence cursor");
    assert_eq!(timeout_cursor_generation, generation);
    let timeout_cursor_argument = format!("{timeout_cursor_generation}:{timeout_cursor_sequence}");
    let before_timeout = fs::read(&fixture.context).expect("context before timeout poll");
    let timeout = control(
        &fixture,
        "watch",
        &[&target, &timeout_cursor_argument, "64", "0"],
    );
    assert_eq!(
        timeout.status.code(),
        Some(1),
        "zero-wait timeout is an intentional CLI status 1: {}",
        timeout.output
    );
    let timeout_object = parse_object(&timeout.output).expect("timeout JSON object");
    assert_eq!(json_bool(timeout_object, "ok"), Some(false));
    assert_eq!(
        json_string(timeout_object, "action").as_deref(),
        Some("watch")
    );
    assert_eq!(
        json_string(timeout_object, "status").as_deref(),
        Some("timed_out")
    );
    assert_eq!(json_field(timeout_object, "events"), Some("[]"));
    assert_eq!(
        json_string(timeout_object, "cursor").as_deref(),
        Some(timeout_cursor_text.as_str())
    );
    assert_eq!(
        fs::read(&fixture.context).expect("context immediately after timeout poll"),
        before_timeout,
        "watch must not reserve a caller sequence"
    );

    let detach = control(&fixture, "detach", &[]);
    assert_ok_action(&detach.output, "detach");
    let reconnected = control(&fixture, "describe", &[]);
    assert_ok_action(&reconnected.output, "describe");
    assert_context_identity(&control(&fixture, "show", &[]).output, generation, lease);
    assert_eq!(
        json_string(&reconnected.output, "target").as_deref(),
        Some(target.as_str())
    );
    assert_eq!(json_u64(&reconnected.output, "owner_lease"), Some(lease));
    assert_eq!(json_bool(&reconnected.output, "child_exited"), Some(false));
    let after_detach = read_until(&fixture, &target, &got_marker, 3);
    let after_text = json_string(&after_detach, "text").expect("post-detach screen text");
    assert_eq!(count_occurrences(&after_text, &got_marker), 1);
    assert!(after_text.contains(&format!("P3PATH-READY-{server_child_pid}")));

    let second_script = "sleep 1";
    let mut second_command = server_command(&fixture, false, second_script);
    let second = run_short(&fixture, "second-server", &mut second_command)
        .expect("bounded second server attempt");
    assert!(
        !second.status.success(),
        "second server unexpectedly started"
    );
    assert_eq!(
        json_string(&second.output, "error").as_deref(),
        Some("server_storage_unavailable"),
        "second server did not fail on the held singleton storage lock: {}",
        second.output
    );
    let socket_after = fs::symlink_metadata(&socket_path).expect("original endpoint preserved");
    assert_eq!(socket_after.dev(), socket_before.dev());
    assert_eq!(socket_after.ino(), socket_before.ino());
    assert_ok_action(&control(&fixture, "describe", &[]).output, "describe");

    let stopped = server
        .stop_with_sigterm(Instant::now() + SHUTDOWN_LIMIT)
        .expect("bounded SIGTERM server shutdown");
    assert!(stopped.success(), "server exit: {stopped}");
    assert!(
        !socket_path.exists(),
        "server listener did not clean its endpoint"
    );
    server
        .assert_capture_within_limit()
        .expect("server output bound");
}

fn fail_server_startup(fixture: &Fixture, server: &mut ManagedChild<'_>, reason: &str) -> ! {
    let context = server.startup_error_context();
    let _ = server.wait(Instant::now() + PROCESS_LIMIT);
    match panelink::transport::PrivateListener::bind(&fixture.runtime) {
        Ok(listener) => {
            drop(listener);
            panic!(
                "server did not start ({reason}); independent pathname bind succeeded; {context}"
            );
        }
        Err(bind_error) if bind_error.raw_os_error() == Some(libc::EPERM) => {
            panic!("expected sandbox blocker: server startup failed ({reason}) and pathname bind returned EPERM; {context}");
        }
        Err(bind_error) => panic!(
            "server did not start ({reason}); pathname diagnostic returned {:?}; {context}",
            bind_error.raw_os_error()
        ),
    }
}

struct ShortResult {
    status: ExitStatus,
    output: String,
}

fn run_short(fixture: &Fixture, label: &str, command: &mut Command) -> io::Result<ShortResult> {
    command
        .env_clear()
        .env("HOME", &fixture.root)
        .env("PATH", "/usr/bin:/bin");
    let mut process = ManagedChild::spawn(fixture, label, command, false)?;
    let status = process.wait(Instant::now() + PROCESS_LIMIT)?;
    process.assert_capture_within_limit()?;
    let stdout = process.stdout.snapshot();
    let stderr = process.stderr.snapshot();
    let output = format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    Ok(ShortResult { status, output })
}

fn server_command(fixture: &Fixture, initialize: bool, script: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
    command
        .arg("server-probe")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--terminfo")
        .arg(&fixture.terminfo);
    if initialize {
        command.arg("--initialize");
    }
    command
        .arg("--allow-input")
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .env_clear()
        .env("HOME", &fixture.root)
        .env("PATH", "/usr/bin:/bin")
        .env("TERM", "xterm-256color")
        .current_dir(&fixture.root);
    command
}

struct ControlResult {
    status: ExitStatus,
    output: String,
}

fn control(fixture: &Fixture, action: &str, extra: &[&str]) -> ControlResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
    command
        .arg("control")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--context")
        .arg(&fixture.context)
        .arg(action)
        .args(extra)
        .current_dir(&fixture.root);
    let label = format!(
        "control-{}-{}",
        action,
        FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let result = run_short(fixture, &label, &mut command).expect("bounded control CLI process");
    ControlResult {
        status: result.status,
        output: result.output,
    }
}

fn read_until(fixture: &Fixture, target: &str, marker: &str, attempts: usize) -> String {
    for index in 0..attempts {
        let result = control(fixture, "read", &[target]);
        assert_eq!(
            result.status.code(),
            Some(0),
            "Read failed: {}",
            result.output
        );
        let object = parse_object(&result.output).expect("Read JSON object");
        let text = json_string(object, "text").expect("Read text");
        if text.contains(marker) {
            return result.output;
        }
        assert!(
            index + 1 < attempts,
            "screen marker {marker:?} absent: {text:?}"
        );
        thread::sleep(Duration::from_millis(40));
    }
    unreachable!("nonzero finite attempts always return or assert")
}

fn assert_private_directory(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("fixture directory metadata");
    assert!(metadata.is_dir());
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
}

fn assert_ok_action(output: &str, expected_action: &str) {
    let object = parse_object(output).expect("CLI JSON object");
    assert_eq!(json_bool(object, "ok"), Some(true), "{output}");
    assert_eq!(
        json_string(object, "action").as_deref(),
        Some(expected_action)
    );
}

fn assert_context_identity(output: &str, generation: u64, lease: u64) {
    assert_ok_action(output, "show");
    let object = parse_object(output).expect("context JSON object");
    assert_eq!(json_u64(object, "generation"), Some(generation));
    assert_eq!(json_u64(object, "lease"), Some(lease));
}

fn validate_target(target: &str) {
    let parts: Vec<_> = target.split(':').collect();
    assert_eq!(
        parts.len(),
        5,
        "target must be the returned explicit five-part target"
    );
    for part in parts {
        assert!(
            part.parse::<u64>().is_ok_and(|value| value != 0),
            "invalid target field"
        );
    }
}

fn marker_pid(text: &str, marker: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(marker)?.trim().parse().ok())
        .expect("ready marker PID")
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn parse_event_cursor(value: &str) -> Option<(u64, u64)> {
    let (generation, sequence) = value.split_once(':')?;
    let generation = generation.parse::<u64>().ok()?;
    let sequence = sequence.parse::<u64>().ok()?;
    (generation != 0).then_some((generation, sequence))
}

fn parse_object(input: &str) -> Option<&str> {
    let start = input.find('{')?;
    let end = input.rfind('}')?;
    input.get(start..=end)
}

#[test]
fn cli_fields_decode_from_captured_output_as_well_as_bare_json() {
    let json = r#"{"ok":true,"action":"init","generation":18446744073709551614,"lease":2,"next_sequence":1,"uncertain":null}"#;
    let captured = format!("stdout={json}\n stderr=");
    for input in [json, captured.as_str()] {
        assert_ok_action(input, "init");
        assert_eq!(json_u64(input, "generation"), Some(u64::MAX - 1));
        assert_eq!(json_u64(input, "lease"), Some(2));
        assert!(json_null(input, "uncertain"));
        assert_eq!(json_u64(input, "missing"), None);
    }
}

fn json_string(input: &str, name: &str) -> Option<String> {
    decode_json_string(json_field(input, name)?).ok()
}

fn json_u64(input: &str, name: &str) -> Option<u64> {
    json_field(input, name)?.parse().ok()
}

fn json_bool(input: &str, name: &str) -> Option<bool> {
    match json_field(input, name)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn json_null(input: &str, name: &str) -> bool {
    json_field(input, name) == Some("null")
}

fn json_field<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    // Subprocess captures include stdout/stderr labels; all typed field readers
    // must use the same JSON object extraction as assert_ok_action.
    let input = parse_object(input)?;
    let bytes = input.as_bytes();
    let mut index = skip_space(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    index += 1;
    loop {
        index = skip_space(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            return None;
        }
        let key_start = index;
        index = skip_string(bytes, index)?;
        let key = decode_json_string(input.get(key_start..index)?).ok()?;
        index = skip_space(bytes, index);
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        index = skip_space(bytes, index + 1);
        let value_start = index;
        index = skip_value(bytes, index)?;
        if key == name {
            return input.get(value_start..index);
        }
        index = skip_space(bytes, index);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return None,
            _ => return None,
        }
    }
}

fn skip_value(bytes: &[u8], start: usize) -> Option<usize> {
    match *bytes.get(start)? {
        b'"' => skip_string(bytes, start),
        b'{' | b'[' => {
            let mut index = start;
            let mut depth = 0_usize;
            let mut in_string = false;
            let mut escaped = false;
            while let Some(byte) = bytes.get(index).copied() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else {
                    match byte {
                        b'"' => in_string = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth = depth.checked_sub(1)?;
                            if depth == 0 {
                                return Some(index + 1);
                            }
                        }
                        _ => {}
                    }
                }
                index += 1;
            }
            None
        }
        _ => {
            let mut index = start;
            while bytes.get(index).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')
            }) {
                index += 1;
            }
            (index > start).then_some(index)
        }
    }
}

fn skip_string(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut index = start + 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index).copied() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        index += 1;
    }
    index
}

fn decode_json_string(quoted: &str) -> Result<String, ()> {
    let bytes = quoted.as_bytes();
    if bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return Err(());
    }
    let mut output = String::new();
    let mut index = 1;
    while index + 1 < bytes.len() {
        if bytes[index] != b'\\' {
            let start = index;
            while index + 1 < bytes.len() && bytes[index] != b'\\' {
                index += 1;
            }
            output.push_str(std::str::from_utf8(&bytes[start..index]).map_err(|_| ())?);
            continue;
        }
        index += 1;
        match *bytes.get(index).ok_or(())? {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = parse_hex4(bytes.get(index + 1..index + 5).ok_or(())?)?;
                index += 4;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if bytes.get(index + 1..index + 3) != Some(&b"\\u"[..]) {
                        return Err(());
                    }
                    let second = parse_hex4(bytes.get(index + 3..index + 7).ok_or(())?)?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(());
                    }
                    index += 6;
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else {
                    u32::from(first)
                };
                output.push(char::from_u32(scalar).ok_or(())?);
            }
            _ => return Err(()),
        }
        index += 1;
    }
    Ok(output)
}

fn parse_hex4(bytes: &[u8]) -> Result<u16, ()> {
    if bytes.len() != 4 {
        return Err(());
    }
    bytes.iter().try_fold(0_u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => u16::from(*byte - b'0'),
            b'a'..=b'f' => u16::from(*byte - b'a' + 10),
            b'A'..=b'F' => u16::from(*byte - b'A' + 10),
            _ => return Err(()),
        };
        Ok((value << 4) | digit)
    })
}
