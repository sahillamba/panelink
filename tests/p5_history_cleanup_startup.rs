#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::history_store::HistoryStore;
use panelink::screen_checkpoint::{self, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const STARTUP_LIMIT: Duration = Duration::from_secs(15);
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(5);
const CAPTURE_LIMIT: Duration = Duration::from_secs(12);
const OUTPUT_LINE_LIMIT: usize = 4096;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    history: PathBuf,
    terminfo: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let parent = Path::new("/private/tmp");
        if fs::canonicalize(parent)? != parent {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture parent is not the canonical private temp directory",
            ));
        }
        for _ in 0..64 {
            let root = parent.join(format!(
                "panelink-p5-history-startup-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::DirBuilder::new().mode(0o700).create(&root) {
                Ok(()) => {
                    let runtime = root.join("server");
                    let terminfo = root.join("terminfo");
                    let history = runtime.join("history");
                    let fixture = Self {
                        root,
                        runtime,
                        history,
                        terminfo,
                    };
                    let setup = (|| {
                        fs::DirBuilder::new().mode(0o700).create(&fixture.runtime)?;
                        fs::DirBuilder::new()
                            .mode(0o700)
                            .create(&fixture.terminfo)?;

                        // Create the real server identity and then release its
                        // lock before starting the pathname runtime process.
                        drop(ServerStorage::initialize(&fixture.runtime)?);
                        fs::DirBuilder::new().mode(0o700).create(&fixture.history)?;
                        Ok(())
                    })();
                    if let Err(error) = setup {
                        drop(fixture);
                        return Err(error);
                    }
                    return Ok(fixture);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a private startup fixture",
        ))
    }

    fn compile_terminfo(&self) -> io::Result<()> {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.root)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "tic could not compile the private terminfo",
            ));
        }
        if !self.terminfo.join("p/panelink-probe").is_file()
            && !self.terminfo.join("70/panelink-probe").is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "compiled Panelink terminfo entry is missing",
            ));
        }
        Ok(())
    }

    fn start_server(&self, gate: &Path, marker: &str) -> io::Result<ServerProcess> {
        let script = "while [ ! -e \"$1\" ]; do /bin/sleep 0.01; done; printf '%s\\n' \"$2\"; exec /bin/sleep 60";
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .arg("serve")
            .arg("--directory")
            .arg(&self.runtime)
            .arg("--terminfo")
            .arg(&self.terminfo)
            .arg("--allow-input")
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("panelink-startup-fixture")
            .arg(gate)
            .arg(marker)
            .env_clear()
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        ServerProcess::spawn(&mut command)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ServerProcess {
    child: Child,
    startup: Receiver<Result<String, String>>,
    readers: Vec<JoinHandle<()>>,
}

impl ServerProcess {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "server stdout was not piped")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "server stderr was not piped")
        })?;
        let (sender, startup) = mpsc::sync_channel(1);
        let stdout_reader = thread::spawn(move || drain_startup(stdout, sender));
        let stderr_reader = thread::spawn(move || drain(stderr));
        Ok(Self {
            child,
            startup,
            readers: vec![stdout_reader, stderr_reader],
        })
    }

    fn startup_line(&mut self) -> io::Result<String> {
        let deadline = Instant::now() + STARTUP_LIMIT;
        loop {
            match self.startup.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(line)) => return Ok(line),
                Ok(Err(message)) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, message))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "server exited before startup JSON",
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(status) = self.child.try_wait()? {
                        return Err(io::Error::other(format!(
                            "server exited before startup JSON: {status}"
                        )));
                    }
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "server startup JSON deadline elapsed",
                        ));
                    }
                }
            }
        }
    }

    fn assert_running(&mut self) -> io::Result<()> {
        if let Some(status) = self.child.try_wait()? {
            return Err(io::Error::other(format!(
                "server exited after startup: {status}"
            )));
        }
        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            let pid = self.child.id();
            // SAFETY: this PID was returned by this fixture's server child.
            if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
                let error = io::Error::last_os_error();
                if self.child.try_wait()?.is_none() {
                    return Err(error);
                }
            }
            let deadline = Instant::now() + SHUTDOWN_LIMIT;
            while self.child.try_wait()?.is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait()?.is_none() {
                // SAFETY: bounded fallback signal to this fixture's server PID.
                let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                let _ = self.child.wait();
            }
        }
        for reader in self.readers.drain(..) {
            if reader.join().is_err() {
                return Err(io::Error::other("server output reader panicked"));
            }
        }
        Ok(())
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn drain_startup<R: Read>(mut reader: R, sender: mpsc::SyncSender<Result<String, String>>) {
    let mut line = Vec::with_capacity(256);
    let mut first_line = Some(sender);
    let mut buffer = [0_u8; 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if let Some(sender) = first_line.take() {
                    let _ = sender.send(Err(format!("read server stdout: {error}")));
                }
                return;
            }
        };
        if first_line.is_none() {
            continue;
        }
        for byte in &buffer[..read] {
            if line.len() == OUTPUT_LINE_LIMIT {
                if let Some(sender) = first_line.take() {
                    let _ = sender.send(Err("startup JSON exceeded its bound".to_owned()));
                }
                break;
            }
            if *byte == b'\n' {
                let result = String::from_utf8(std::mem::take(&mut line))
                    .map_err(|_| "startup JSON was not UTF-8".to_owned());
                if let Some(sender) = first_line.take() {
                    let _ = sender.send(result);
                }
                break;
            }
            line.push(*byte);
        }
    }
    if let Some(sender) = first_line {
        let _ = sender.send(Err("server stdout closed before startup JSON".to_owned()));
    }
}

fn drain<R: Read>(mut reader: R) {
    let mut buffer = [0_u8; 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn target(generation: u64) -> TargetRef {
    TargetRef {
        generation,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    }
}

fn fields(target: TargetRef) -> String {
    format!(
        "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn temporary_name(target: TargetRef, counter: u64) -> String {
    format!(
        ".{}.rows-{}-{counter}.tmp",
        fields(target),
        std::process::id()
    )
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

fn startup_target(line: &str) -> TargetRef {
    let value = line
        .split_once("\"target\":\"")
        .and_then(|(_, rest)| rest.split_once('\"').map(|(value, _)| value))
        .expect("startup response contains target");
    let fields = value
        .split(':')
        .map(|field| field.parse::<u64>().expect("numeric target field"))
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 5);
    TargetRef {
        generation: fields[0],
        workspace: fields[1],
        pane: fields[2],
        session: fields[3],
        occupant_revision: fields[4],
    }
}

fn screen_has_marker(record: &[u8], marker: &str) -> bool {
    let Ok(checkpoint) = screen_checkpoint::decode(record) else {
        return false;
    };
    checkpoint.buffer() == ScreenBuffer::Normal
        && checkpoint
            .cells()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.text());
                text
            })
            .contains(marker)
}

fn wait_for_marker(store: &HistoryStore, target: TargetRef, marker: &str) {
    let deadline = Instant::now() + CAPTURE_LIMIT;
    loop {
        let screens = store
            .load_screens(target)
            .expect("read committed startup history screens");
        if screens[0]
            .as_deref()
            .is_some_and(|record| screen_has_marker(record, marker))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "capture did not persist the post-startup shell marker"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn pathname_startup_cleans_orphan_before_post_ready_capture() {
    let fixture = Fixture::new().expect("private pathname fixture");
    fixture
        .compile_terminfo()
        .expect("compile private terminfo");

    let storage = ServerStorage::open(&fixture.runtime).expect("open initialized server storage");
    let orphan_target = target(storage.generation());
    drop(storage);
    let orphan = fixture.history.join(temporary_name(orphan_target, 1));
    write_private(&orphan, b"incomplete private writer bytes").unwrap();

    let gate = fixture.root.join("release-child-output");
    let marker = format!("P5STARTUP_CAPTURE_{}", std::process::id());
    let mut server = fixture
        .start_server(&gate, &marker)
        .expect("spawn actual pathname serve runtime");
    let startup = server.startup_line().expect("read bounded startup JSON");
    assert!(startup.contains("\"action\":\"serve\""), "{startup}");
    assert!(startup.contains("\"state\":\"running\""), "{startup}");
    assert!(
        startup.contains("\"history_capture\":\"enabled\""),
        "{startup}"
    );
    assert!(
        !orphan.exists(),
        "startup left the recognized orphan temporary"
    );
    server
        .assert_running()
        .expect("server remains live after startup");

    // The child emits only after the runtime's readiness line, so this
    // checkpoint proves the actual startup-installed capture path is active.
    let capture_target = startup_target(&startup);
    write_private(&gate, b"release").expect("release fixture-owned shell");
    let store = HistoryStore::open(&fixture.history).expect("open history observer");
    wait_for_marker(&store, capture_target, &marker);
    assert!(!orphan.exists(), "capture recreated the orphan temporary");

    server.stop().expect("bounded graceful server shutdown");
}

#[test]
fn pathname_startup_keeps_shell_but_refuses_capture_for_unsafe_temp_metadata() {
    let fixture = Fixture::new().expect("private pathname fixture");
    fixture
        .compile_terminfo()
        .expect("compile private terminfo");

    let storage = ServerStorage::open(&fixture.runtime).expect("open initialized server storage");
    let orphan_target = target(storage.generation());
    drop(storage);
    let safe_temp = fixture.history.join(temporary_name(orphan_target, 1));
    write_private(&safe_temp, b"safe candidate must remain on refusal").unwrap();
    let unsafe_temp = fixture.history.join(temporary_name(orphan_target, 2));
    write_private(&unsafe_temp, b"unsafe metadata must remain").unwrap();
    fs::set_permissions(&unsafe_temp, fs::Permissions::from_mode(0o644)).unwrap();

    let gate = fixture.root.join("release-child-output");
    let mut server = fixture
        .start_server(&gate, "unsafe-temp-shell-still-running")
        .expect("spawn actual pathname serve runtime");
    let startup = server.startup_line().expect("read bounded startup JSON");
    assert!(startup.contains("\"action\":\"serve\""), "{startup}");
    assert!(startup.contains("\"state\":\"running\""), "{startup}");
    assert!(
        startup.contains("\"history_capture\":\"unavailable\""),
        "unsafe recognized temp must fail closed: {startup}"
    );
    server
        .assert_running()
        .expect("unsafe history temp does not stop the shell server");
    assert!(
        safe_temp.exists(),
        "preflight deleted a safe candidate early"
    );
    let unsafe_metadata =
        fs::symlink_metadata(&unsafe_temp).expect("unsafe candidate remains present");
    assert!(unsafe_metadata.file_type().is_file());
    assert_eq!(unsafe_metadata.permissions().mode() & 0o777, 0o644);
    assert_eq!(
        fs::read(&unsafe_temp).unwrap(),
        b"unsafe metadata must remain"
    );

    server.stop().expect("bounded graceful server shutdown");
}
