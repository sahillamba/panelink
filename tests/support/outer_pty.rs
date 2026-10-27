//! Shared outer-PTY client mechanics for the P3A host integration tests.
//!
//! Callers build the scenario-specific supervisor command and launch argv;
//! this module owns the PTY session, bounded output observation and cleanup.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::{tcgetattr, LocalModes, Termios, Winsize};
use rustix_openpty::openpty;

const STEP: Duration = Duration::from_secs(10);
const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    Read(String),
    Overflow {
        limit: usize,
        captured: usize,
        dropped: usize,
    },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "outer PTY capture read failed: {error}"),
            Self::Overflow {
                limit,
                captured,
                dropped,
            } => write!(
                formatter,
                "outer PTY capture exceeded {limit} bytes (kept {captured}, dropped {dropped})"
            ),
        }
    }
}

struct CaptureState {
    bytes: Vec<u8>,
    error: Option<CaptureError>,
}

/// An attach client running beneath a session-leading supervisor on a private
/// outer PTY. Exit publication is independent of the PTY drain thread.
pub struct Client {
    supervisor: Child,
    status_path: PathBuf,
    release_path: PathBuf,
    writer: fs::File,
    slave: std::os::fd::OwnedFd,
    initial: Termios,
    rows: u16,
    columns: u16,
    capture: Arc<Mutex<CaptureState>>,
    stop: Arc<AtomicBool>,
    drain_paused: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
}

impl Client {
    /// Create the PTY and let the consumer build its launch wrapper. Its
    /// command and arguments remain scenario-specific; the supplied stdio all
    /// refer to the allocated outer PTY except an optional explicit stderr.
    pub fn spawn(
        root: &Path,
        rows: u16,
        columns: u16,
        stderr: Option<std::os::fd::OwnedFd>,
        build: impl FnOnce(&Path, &Path, Stdio, Stdio, Stdio) -> Command,
    ) -> Self {
        Self::spawn_with_drain_gate(root, rows, columns, stderr, false, build)
    }

    fn spawn_with_drain_gate(
        root: &Path,
        rows: u16,
        columns: u16,
        stderr: Option<std::os::fd::OwnedFd>,
        drain_paused: bool,
        build: impl FnOnce(&Path, &Path, Stdio, Stdio, Stdio) -> Command,
    ) -> Self {
        let winsize = Winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open outer PTY");
        let master = fs::File::from(pty.controller);
        let flags = fcntl_getfl(&master).expect("get outer PTY flags");
        fcntl_setfl(&master, flags | OFlags::NONBLOCK).expect("set outer PTY nonblocking");
        let slave = pty.user;
        let initial = tcgetattr(&slave).expect("save outer termios");
        let controlling = slave
            .as_fd()
            .try_clone_to_owned()
            .expect("clone controlling PTY");
        let clone_slave = || {
            Stdio::from(fs::File::from(
                slave
                    .as_fd()
                    .try_clone_to_owned()
                    .expect("clone outer PTY slave"),
            ))
        };
        // Keep the session leader alive after the attach client exits. On
        // macOS this keeps restored termios observable until test cleanup.
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let status_path = root.join(format!("attach-{serial}.status"));
        let release_path = root.join(format!("release-{serial}"));
        let command = build(
            &status_path,
            &release_path,
            clone_slave(),
            clone_slave(),
            stderr.map_or_else(clone_slave, |fd| Stdio::from(fs::File::from(fd))),
        );
        let mut command = command;
        // SAFETY: these reviewed rustix wrappers create a new session and set
        // the inherited PTY slave as its controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let supervisor = command.spawn().expect("spawn outer PTY supervisor");
        let capture = Arc::new(Mutex::new(CaptureState {
            bytes: Vec::new(),
            error: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(drain_paused));
        let mut reader = master.try_clone().expect("clone outer PTY master");
        let drain = {
            let capture = Arc::clone(&capture);
            let stop = Arc::clone(&stop);
            let paused = Arc::clone(&paused);
            thread::spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                while !stop.load(Ordering::Acquire) {
                    if paused.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(count) => {
                            let mut state = capture.lock().unwrap();
                            let available = MAX_CAPTURE.saturating_sub(state.bytes.len());
                            let kept = available.min(count);
                            state.bytes.extend_from_slice(&buffer[..kept]);
                            if kept < count {
                                state.error = Some(CaptureError::Overflow {
                                    limit: MAX_CAPTURE,
                                    captured: state.bytes.len(),
                                    dropped: count - kept,
                                });
                                break;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            capture.lock().unwrap().error =
                                Some(CaptureError::Read(error.to_string()));
                            break;
                        }
                    }
                }
            })
        };
        Self {
            supervisor,
            status_path,
            release_path,
            writer: master,
            slave,
            initial,
            rows,
            columns,
            capture,
            stop,
            drain_paused: paused,
            drain: Some(drain),
        }
    }

    /// Read the supervisor's atomically published observation of the attach
    /// process exit. This does not imply that all PTY output has been drained.
    pub fn attach_exit(&mut self) -> Option<i32> {
        let text = fs::read_to_string(&self.status_path).ok()?;
        text.strip_suffix('\n')?.parse().ok()
    }

    pub fn wait_attach_exit(&mut self) -> i32 {
        self.wait_attach_exit_within(STEP)
    }

    pub fn wait_attach_exit_within(&mut self, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(code) = self.attach_exit() {
                return code;
            }
            assert!(Instant::now() < deadline, "attach did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let deadline = Instant::now() + STEP;
        let mut cursor = 0;
        while cursor < bytes.len() {
            match self.writer.write(&bytes[cursor..]) {
                Ok(count) => cursor += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "outer PTY input stalled");
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("write outer PTY: {error}"),
            }
        }
    }

    pub fn type_slowly(&mut self, text: &[u8]) {
        for byte in text {
            self.write(&[*byte]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn capture_result(&self) -> Result<Vec<u8>, CaptureError> {
        let state = self.capture.lock().unwrap();
        match &state.error {
            Some(error) => Err(error.clone()),
            None => Ok(state.bytes.clone()),
        }
    }

    pub fn capture(&self) -> Vec<u8> {
        self.capture_result().expect("bounded outer PTY capture")
    }

    pub fn wait_capture_contains(&self, pattern: &[u8], limit: Duration) -> Duration {
        self.wait_capture_contains_after(0, pattern, limit)
            .expect("outer PTY bytes not captured before deadline")
    }

    pub fn wait_capture_contains_after(
        &self,
        from: usize,
        pattern: &[u8],
        limit: Duration,
    ) -> Option<Duration> {
        let started = Instant::now();
        loop {
            let capture = self.capture();
            if capture[from.min(capture.len())..]
                .windows(pattern.len())
                .any(|window| window == pattern)
            {
                return Some(started.elapsed());
            }
            if started.elapsed() >= limit {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(self.rows, self.columns, 0);
        parser.process(&self.capture());
        parser.screen().contents()
    }

    pub fn wait_root_marker(&mut self, marker: &str) {
        let deadline = Instant::now() + STEP;
        loop {
            let mut parser = vt100::Parser::new(self.rows, self.columns, 0);
            parser.process(&self.capture());
            let found = (1..self.rows.saturating_sub(1)).any(|row| {
                (1..self.columns.saturating_sub(1))
                    .map(|column| {
                        parser.screen().cell(row, column).map_or(" ", |cell| {
                            if cell.contents().is_empty() {
                                " "
                            } else {
                                cell.contents()
                            }
                        })
                    })
                    .collect::<String>()
                    .trim()
                    == marker
            });
            if found {
                return;
            }
            if let Some(status) = self.attach_exit() {
                panic!("attach exited ({status}) waiting for root marker {marker}");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for root marker {marker}; screen:\n{}",
                self.screen()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn wait_screen(&mut self, what: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + STEP;
        loop {
            let screen = self.screen();
            if done(&screen) {
                return screen;
            }
            if let Some(status) = self.attach_exit() {
                panic!(
                    "attach exited ({status}) waiting for {what}: {:?}",
                    String::from_utf8_lossy(&self.capture())
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; screen:\n{screen}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn wait_raw_mode(&mut self) {
        let deadline = Instant::now() + STEP;
        loop {
            let termios = tcgetattr(&self.slave).expect("read outer PTY termios");
            if !termios.local_modes.contains(LocalModes::ICANON) {
                return;
            }
            if let Some(status) = self.attach_exit() {
                panic!(
                    "attach exited before raw mode ({status}): {:?}",
                    String::from_utf8_lossy(&self.capture())
                );
            }
            assert!(Instant::now() < deadline, "attach never entered raw mode");
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn termios_restored(&self) {
        let actual = tcgetattr(&self.slave).expect("read restored outer PTY termios");
        assert_eq!(actual.input_modes, self.initial.input_modes);
        assert_eq!(actual.output_modes, self.initial.output_modes);
        assert_eq!(actual.control_modes, self.initial.control_modes);
        assert_eq!(actual.local_modes, self.initial.local_modes);
    }

    #[allow(dead_code)] // The attach-host stalled-server probe reads termios directly.
    pub fn slave(&self) -> &std::os::fd::OwnedFd {
        &self.slave
    }

    #[allow(dead_code)] // Used by the default-open consumer for detach restore bytes.
    pub fn wait_until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + STEP;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; screen:\n{}",
                self.screen()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = fs::write(&self.release_path, b"release\n");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if self.supervisor.try_wait().ok().flatten().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = self.supervisor.kill();
                let _ = self.supervisor.wait();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.stop.store(true, Ordering::Release);
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

pub const SUPERVISOR: &str = r#"
status_path=$1
release_path=$2
shift 2
trap ':' HUP INT TERM
"$@"
code=$?
printf '%s\n' "$code" > "$status_path.tmp" && /bin/mv "$status_path.tmp" "$status_path"
while [ ! -f "$release_path" ]; do /bin/sleep 0.05; done
exit 0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_observation_precedes_a_deliberately_delayed_capture_drain() {
        let root = std::env::temp_dir().join(format!("outer-pty-drain-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut client = Client::spawn_with_drain_gate(
            &root,
            24,
            80,
            None,
            true,
            |status, release, input, output, error| {
                let mut command = Command::new("/bin/sh");
                command
                    .arg("-c")
                    .arg(SUPERVISOR)
                    .arg("outer-pty-test")
                    .arg(status)
                    .arg(release)
                    .arg("/bin/sh")
                    .arg("-c")
                    .arg("printf '\\n DELAYED  DRAIN  MARKER\\n'; exit 0")
                    .stdin(input)
                    .stdout(output)
                    .stderr(error);
                command
            },
        );
        assert_eq!(client.wait_attach_exit_within(Duration::from_secs(2)), 0);
        assert!(
            client.capture.lock().unwrap().bytes.is_empty(),
            "drain gate must hold all output"
        );
        // The regression deliberately releases the consumer only after exit
        // was observed, then synchronizes on actual captured bytes.
        client.drain_paused.store(false, Ordering::Release);
        client.wait_capture_contains(b"DELAYED  DRAIN  MARKER", Duration::from_secs(2));
        client.wait_root_marker("DELAYED  DRAIN  MARKER");
        assert!(client
            .capture()
            .windows(b"DELAYED  DRAIN  MARKER".len())
            .any(|window| window == b"DELAYED  DRAIN  MARKER"));
        drop(client);
        let _ = fs::remove_dir_all(root);
    }
}
