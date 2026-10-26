//! Attached-client-only asynchronous writes to the host clipboard.
//!
//! Selection text is owned only by this bounded queue and its worker. It is
//! never formatted, logged, or included in a diagnostic. The service is not
//! wired into the event loop yet.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::{self as rio, Errno};

use panelink::terminal_selection::MAX_SELECTION_BYTES;

const PBCOPY: &str = "/usr/bin/pbcopy";
const WRITE_DEADLINE: Duration = Duration::from_secs(3);
const WORKER_POLL: Duration = Duration::from_millis(2);
const QUEUE_CAPACITY: usize = 1;
const RESULT_CAPACITY: usize = 2;

/// Fixed, redacted result classes suitable for display by the attached UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardStatus {
    Copied,
    Unavailable,
    TimedOut,
    WriteFailed,
    Busy,
    TooLarge,
    Cancelled,
}

impl ClipboardStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Copied => "clipboard copied",
            Self::Unavailable => "clipboard unavailable",
            Self::TimedOut => "clipboard timed out",
            Self::WriteFailed => "clipboard write failed",
            Self::Busy => "clipboard busy",
            Self::TooLarge => "clipboard selection too large",
            Self::Cancelled => "clipboard cancelled",
        }
    }
}

/// Testable writer boundary. Implementations receive the same absolute
/// deadline and cancellation flag as the process implementation.
trait ClipboardSink: Send + 'static {
    fn write(&mut self, bytes: &[u8], cancel: &AtomicBool, deadline: Instant) -> ClipboardStatus;
}

struct ProcessSink {
    executable: PathBuf,
    timeout: Duration,
}

impl ProcessSink {
    fn production() -> Self {
        Self {
            executable: PathBuf::from(PBCOPY),
            timeout: WRITE_DEADLINE,
        }
    }

    fn kill_and_reap(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn write_process(
        &self,
        bytes: &[u8],
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> ClipboardStatus {
        let mut child = match Command::new(&self.executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return ClipboardStatus::Unavailable,
        };

        let Some(stdin) = child.stdin.take() else {
            Self::kill_and_reap(&mut child);
            return ClipboardStatus::WriteFailed;
        };
        let mut stdin = Some(stdin);
        let flags = match fcntl_getfl(stdin.as_ref().expect("stdin is present")) {
            Ok(flags) => flags,
            Err(_) => {
                drop(stdin.take());
                Self::kill_and_reap(&mut child);
                return ClipboardStatus::WriteFailed;
            }
        };
        if fcntl_setfl(
            stdin.as_ref().expect("stdin is present"),
            flags | OFlags::NONBLOCK,
        )
        .is_err()
        {
            drop(stdin.take());
            Self::kill_and_reap(&mut child);
            return ClipboardStatus::WriteFailed;
        }

        let mut offset = 0;
        let mut stdin_open = true;
        loop {
            if cancel.load(Ordering::Acquire) {
                drop(stdin.take());
                Self::kill_and_reap(&mut child);
                return ClipboardStatus::Cancelled;
            }
            if Instant::now() >= deadline {
                drop(stdin.take());
                Self::kill_and_reap(&mut child);
                return ClipboardStatus::TimedOut;
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    drop(stdin.take());
                    if status.success() && offset == bytes.len() {
                        return ClipboardStatus::Copied;
                    }
                    return ClipboardStatus::WriteFailed;
                }
                Ok(None) => {}
                Err(_) => {
                    drop(stdin.take());
                    Self::kill_and_reap(&mut child);
                    return ClipboardStatus::WriteFailed;
                }
            }

            if offset < bytes.len() {
                match rio::write(stdin.as_ref().expect("stdin is present"), &bytes[offset..]) {
                    Ok(0) => {
                        drop(stdin.take());
                        Self::kill_and_reap(&mut child);
                        return ClipboardStatus::WriteFailed;
                    }
                    Ok(count) => offset += count,
                    Err(error) if error == Errno::AGAIN || error == Errno::INTR => {
                        thread::sleep(WORKER_POLL);
                    }
                    Err(_) => {
                        drop(stdin.take());
                        Self::kill_and_reap(&mut child);
                        return ClipboardStatus::WriteFailed;
                    }
                }
            } else if stdin_open {
                drop(stdin.take());
                stdin_open = false;
            } else {
                thread::sleep(WORKER_POLL);
            }
        }
    }
}

impl ClipboardSink for ProcessSink {
    fn write(&mut self, bytes: &[u8], cancel: &AtomicBool, deadline: Instant) -> ClipboardStatus {
        self.write_process(bytes, cancel, deadline.min(Instant::now() + self.timeout))
    }
}

struct Payload(Vec<u8>);

impl Drop for Payload {
    fn drop(&mut self) {
        // Keep selection bytes out of panic/debug paths and shorten their
        // lifetime in this worker-owned buffer after use.
        self.0.fill(0);
    }
}

#[derive(Default)]
struct Accounting {
    closing: bool,
    active_bytes: usize,
    queued_bytes: usize,
}

/// One attached client's bounded clipboard writer.
pub(crate) struct ClipboardWriter {
    sender: Option<SyncSender<Payload>>,
    results: Receiver<ClipboardStatus>,
    accounting: Arc<Mutex<Accounting>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardWriter {
    /// Start the production writer, which invokes `/usr/bin/pbcopy` directly.
    pub(crate) fn new() -> Result<Self, ClipboardStatus> {
        Self::start(ProcessSink::production())
    }

    fn start<S: ClipboardSink>(mut sink: S) -> Result<Self, ClipboardStatus> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (result_sender, results) = mpsc::sync_channel(RESULT_CAPACITY);
        let accounting = Arc::new(Mutex::new(Accounting::default()));
        let worker_accounting = Arc::clone(&accounting);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::Builder::new()
            .name("panelink-clipboard".into())
            .spawn(move || {
                clipboard_loop(
                    &mut sink,
                    receiver,
                    result_sender,
                    worker_accounting,
                    worker_cancel,
                )
            })
            .map_err(|_| ClipboardStatus::Unavailable)?;
        Ok(Self {
            sender: Some(sender),
            results,
            accounting,
            cancel,
            thread: Some(thread),
        })
    }

    /// Queue a selection without waiting for the worker or a child process.
    /// One payload may be active and one payload may wait in the bounded queue.
    pub(crate) fn try_send(&self, text: String) -> Result<(), ClipboardStatus> {
        let bytes = text.into_bytes();
        if bytes.len() > MAX_SELECTION_BYTES {
            return Err(ClipboardStatus::TooLarge);
        }
        let mut accounting = self
            .accounting
            .lock()
            .map_err(|_| ClipboardStatus::Unavailable)?;
        if accounting.closing {
            return Err(ClipboardStatus::Cancelled);
        }
        let byte_len = bytes.len();
        let total = accounting
            .active_bytes
            .saturating_add(accounting.queued_bytes);
        if total.saturating_add(bytes.len()) > MAX_SELECTION_BYTES.saturating_mul(2) {
            return Err(ClipboardStatus::Busy);
        }
        let Some(sender) = self.sender.as_ref() else {
            return Err(ClipboardStatus::Cancelled);
        };
        match sender.try_send(Payload(bytes)) {
            Ok(()) => {
                accounting.queued_bytes += byte_len;
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(ClipboardStatus::Busy),
            Err(TrySendError::Disconnected(_)) => Err(ClipboardStatus::Cancelled),
        }
    }

    /// Return the next fixed status, if the worker has completed a write.
    pub(crate) fn poll_result(&self) -> Option<ClipboardStatus> {
        self.results.try_recv().ok()
    }

    /// Stop accepting writes, cancel the active child, discard the queued
    /// payload, and reap the worker before returning.
    pub(crate) fn request_shutdown(&mut self) {
        if let Ok(mut accounting) = self.accounting.lock() {
            accounting.closing = true;
            accounting.queued_bytes = 0;
        }
        self.cancel.store(true, Ordering::Release);
        self.sender.take();
    }

    /// Join only after the host terminal has been restored.
    pub(crate) fn shutdown(&mut self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ClipboardWriter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn clipboard_loop<S: ClipboardSink>(
    sink: &mut S,
    receiver: Receiver<Payload>,
    results: SyncSender<ClipboardStatus>,
    accounting: Arc<Mutex<Accounting>>,
    cancel: Arc<AtomicBool>,
) {
    loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let payload = match receiver.recv_timeout(WORKER_POLL) {
            Ok(payload) => payload,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let byte_len = payload.0.len();
        let should_write = if let Ok(mut state) = accounting.lock() {
            state.queued_bytes = state.queued_bytes.saturating_sub(byte_len);
            if state.closing || cancel.load(Ordering::Acquire) {
                false
            } else {
                state.active_bytes = byte_len;
                true
            }
        } else {
            false
        };
        if !should_write {
            continue;
        }
        let status = sink.write(&payload.0, &cancel, Instant::now() + WRITE_DEADLINE);
        drop(payload);
        if let Ok(mut state) = accounting.lock() {
            state.active_bytes = 0;
        }
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let _ = results.try_send(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        directory: PathBuf,
        executable: PathBuf,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let id = NEXT_FIXTURE.fetch_add(1, AtomicOrdering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "panelink-clipboard-test-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&directory).expect("create private test directory");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("set private test directory mode");
            let executable = directory.join("fake-pbcopy");
            fs::write(&executable, format!("#!/bin/sh\n{body}\n"))
                .expect("write fake clipboard executable");
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
                .expect("set fake executable mode");
            Self {
                directory,
                executable,
            }
        }

        fn sink(&self, timeout: Duration) -> ProcessSink {
            ProcessSink {
                executable: self.executable.clone(),
                timeout,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn wait_result(writer: &ClipboardWriter, limit: Duration) -> ClipboardStatus {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if let Some(status) = writer.poll_result() {
                return status;
            }
            thread::sleep(WORKER_POLL);
        }
        panic!("clipboard result did not arrive before test deadline");
    }

    #[test]
    fn injected_sink_receives_exact_bytes_without_debugging_payload() {
        struct Recorder(Arc<Mutex<Vec<Vec<u8>>>>);
        impl ClipboardSink for Recorder {
            fn write(
                &mut self,
                bytes: &[u8],
                _cancel: &AtomicBool,
                _deadline: Instant,
            ) -> ClipboardStatus {
                self.0.lock().unwrap().push(bytes.to_vec());
                ClipboardStatus::Copied
            }
        }

        let observed = Arc::new(Mutex::new(Vec::new()));
        let writer = ClipboardWriter::start(Recorder(Arc::clone(&observed))).unwrap();
        writer.try_send("private text ✓".to_owned()).unwrap();
        assert_eq!(
            wait_result(&writer, Duration::from_secs(1)),
            ClipboardStatus::Copied
        );
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            &["private text ✓".as_bytes()]
        );
    }

    #[test]
    fn process_fixture_success_writes_to_child_and_reaps_it() {
        let fixture = Fixture::new("cat >/dev/null");
        let writer = ClipboardWriter::start(fixture.sink(Duration::from_secs(1))).unwrap();
        writer.try_send("clipboard fixture".to_owned()).unwrap();
        assert_eq!(
            wait_result(&writer, Duration::from_secs(2)),
            ClipboardStatus::Copied
        );
    }

    #[test]
    fn process_fixture_early_exit_is_a_redacted_write_failure() {
        let fixture = Fixture::new("exec /usr/bin/false");
        let writer = ClipboardWriter::start(fixture.sink(Duration::from_secs(1))).unwrap();
        writer.try_send("x".repeat(MAX_SELECTION_BYTES)).unwrap();
        assert_eq!(
            wait_result(&writer, Duration::from_secs(2)),
            ClipboardStatus::WriteFailed
        );
    }

    #[test]
    fn process_fixture_blocked_stdin_times_out_and_is_reaped() {
        let fixture = Fixture::new("exec /usr/bin/sleep 30");
        let writer = ClipboardWriter::start(fixture.sink(Duration::from_millis(120))).unwrap();
        writer.try_send("x".repeat(MAX_SELECTION_BYTES)).unwrap();
        assert_eq!(
            wait_result(&writer, Duration::from_secs(2)),
            ClipboardStatus::TimedOut
        );
    }

    #[test]
    fn shutdown_cancels_active_process_and_discards_queued_write() {
        let fixture = Fixture::new("exec /usr/bin/sleep 30");
        let mut writer = ClipboardWriter::start(fixture.sink(Duration::from_secs(5))).unwrap();
        writer.try_send("x".repeat(MAX_SELECTION_BYTES)).unwrap();
        let start = Instant::now();
        while Instant::now() - start < Duration::from_secs(1) {
            let active = writer
                .accounting
                .lock()
                .map(|state| state.active_bytes != 0)
                .unwrap_or(false);
            if active {
                break;
            }
            thread::sleep(WORKER_POLL);
        }
        writer.try_send("queued".to_owned()).unwrap();
        let start = Instant::now();
        writer.shutdown();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(
            writer.try_send("after detach".to_owned()),
            Err(ClipboardStatus::Cancelled)
        );
    }

    #[test]
    fn queue_full_is_nonblocking_and_byte_cap_is_enforced() {
        struct Blocker(Arc<AtomicBool>);
        impl ClipboardSink for Blocker {
            fn write(
                &mut self,
                _bytes: &[u8],
                cancel: &AtomicBool,
                _deadline: Instant,
            ) -> ClipboardStatus {
                self.0.store(true, Ordering::Release);
                while !cancel.load(Ordering::Acquire) {
                    thread::sleep(WORKER_POLL);
                }
                ClipboardStatus::Cancelled
            }
        }

        let started = Arc::new(AtomicBool::new(false));
        let mut writer = ClipboardWriter::start(Blocker(Arc::clone(&started))).unwrap();
        writer.try_send("active".to_owned()).unwrap();
        let start = Instant::now();
        while !started.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(1) {
            thread::sleep(WORKER_POLL);
        }
        writer.try_send("queued".to_owned()).unwrap();
        assert_eq!(
            writer.try_send("full".to_owned()),
            Err(ClipboardStatus::Busy)
        );
        assert_eq!(
            writer.try_send("x".repeat(MAX_SELECTION_BYTES + 1)),
            Err(ClipboardStatus::TooLarge)
        );
        writer.shutdown();
    }

    #[test]
    fn production_command_is_fixed_and_never_accepts_a_text_path() {
        let sink = ProcessSink::production();
        assert_eq!(sink.executable, Path::new(PBCOPY));
        assert_eq!(
            ClipboardStatus::WriteFailed.label(),
            "clipboard write failed"
        );
    }
}
