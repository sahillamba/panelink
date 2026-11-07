//! Opt-in, bounded, private local diagnostics for Panelink processes.
//!
//! Diagnostics are off by default and local only: they are enabled only when
//! `PANELINK_TRACE_DIR` names an absolute, private (mode 0700) directory owned
//! by the effective user. A client process writes `client.log*`; a server
//! process writes `server.log*`. Records are never written to the terminal or
//! to any JSON pipeline, so existing CLI and wire output contracts stay
//! untouched.
//!
//! Bounded output contracts:
//!
//! - Producers format records on their own thread and hand pre-formatted
//!   bytes to a bounded, always nonblocking queue of at most
//!   [`RECORD_QUEUE_CAPACITY`] fragments, each capped at
//!   [`RECORD_MAX_BYTES`]. When the queue is full the record is dropped and
//!   counted; a producer is never blocked or failed by diagnostics.
//! - The current log file rotates at [`ROTATION_BYTES`] and at most
//!   [`RETENTION_GENERATIONS`] rotated files are kept beside the current one,
//!   so on-disk diagnostics never exceed the byte cap times its bounded
//!   generations per role.
//! - Background writer failures are only counted (sink errors never cancel
//!   admitted input or hold a PTY owner); the writer retries the current file
//!   but never follows a symlink and never widens a mode.
//! - Shutdown is bounded: [`shutdown_after_restore`] stops the writer within
//!   the caller's budget and is invoked after terminal restoration.
//!
//! Privacy: records carry fixed operation/stage/outcome labels, durations,
//! byte/count totals and approved numeric correlation identifiers
//! (generation, lease, wire request id) only. No input, paste, screen,
//! argument, path, credential, environment value, provider conversation id or
//! raw `Debug` content is ever expressed through this module's record type,
//! and no tracing field is ever formatted with `?`.
//!
//! Correlation reuses wire identifiers only: a request is joined by
//! `(generation, lease, wire request id)`; a wire request id alone is
//! connection-local. No new protocol field was added for tracing. Durations
//! are monotonic `Instant` differences measured inside one process;
//! independently measured cross-process timestamps are never subtracted.
//! Spans are created at admission and passed explicitly inside in-process
//! envelopes (a shared global subscriber does not propagate current spans
//! between threads) and are entered only while processing that request.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

/// Fixed target for every diagnostic event emitted by Panelink.
pub const TARGET: &str = "panelink::diag";

/// Environment variable that opts a Panelink process into local diagnostics.
pub const TRACE_DIR_ENVIRONMENT: &str = "PANELINK_TRACE_DIR";

/// Reserve inside the existing client-worker command/result byte budget for
/// the diagnostic span handle and monotonic enqueue instant carried by one
/// queued envelope. Diagnostics can therefore never grow the bounded queue.
pub const ENVELOPE_TRACE_BYTES: usize = 64;

/// Reserve inside the existing client-worker result reservation for the
/// result-side trace (operation label and monotonic completion instant).
pub const RESULT_TRACE_BYTES: usize = std::mem::size_of::<(&'static str, Instant)>();

/// Maximum queued producer fragments (one formatted write each).
const RECORD_QUEUE_CAPACITY: usize = 128;
/// Maximum bytes per record fragment before the remainder is counted, not
/// written.
const RECORD_MAX_BYTES: usize = 4096;
/// Current-file byte cap before rotation.
const ROTATION_BYTES: u64 = 4 * 1024 * 1024;
/// Rotated files retained beside the current one.
const RETENTION_GENERATIONS: usize = 2;
/// Required mode of the diagnostics directory.
const TRACE_DIR_MODE: u32 = 0o700;
/// Required mode of diagnostics log files.
const LOG_FILE_MODE: u32 = 0o600;
/// Bounded writer-thread poll between records (also bounds stop latency).
const WRITER_POLL: Duration = Duration::from_millis(20);

/// Which process role this one is: it selects the fixed log file stem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// An attach/workspace client (`client.log*`).
    Client,
    /// A `serve`/`server-probe` process (`server.log*`).
    Server,
}

impl Role {
    /// Fixed file stem for this role's diagnostics files.
    pub(crate) fn file_stem(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    /// Fixed role label carried in every diagnostic record.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

static ROLE: OnceLock<Role> = OnceLock::new();

fn process_role() -> &'static str {
    ROLE.get()
        .map(|role| role.label())
        .unwrap_or("uninitialized")
}

/// One correlated stage record: the approved diagnostic shape.
///
/// Every field is a fixed `&'static str` label, a numeric total or an approved
/// numeric identifier. Content-sensitive data cannot be expressed through
/// this type. Call sites fill what they know; unused values stay default.
#[derive(Clone, Copy, Default)]
pub struct StageEvent {
    /// Fixed operation label (for example `keys`, `attach`).
    pub op: &'static str,
    /// Fixed stage label (for example `enqueue`, `pty_accept`).
    pub stage: &'static str,
    /// Fixed outcome or fixed server error label; `""` when not terminal.
    pub outcome: &'static str,
    /// Generation link.
    pub generation: u64,
    /// Lease link.
    pub lease: u64,
    /// Wire request id (connection-local).
    pub request: u64,
    /// Monotonic queue wait within this process, in microseconds.
    pub wait_us: u64,
    /// Monotonic service time within this process, in microseconds.
    pub service_us: u64,
    /// Byte total carried by this operation.
    pub bytes: u64,
    /// Named count total slot 0 (`("", 0)` when unused).
    pub count0: (&'static str, u64),
    /// Named count total slot 1 (`("", 0)` when unused).
    pub count1: (&'static str, u64),
}

impl StageEvent {
    /// Emit this record. Disabled subscribers suppress formatting and output.
    /// Callers still construct the event and any explicit timing values.
    pub fn record(self) {
        tracing::event!(
            target: TARGET,
            tracing::Level::INFO,
            process = process_role(),
            op = self.op,
            stage = self.stage,
            outcome = self.outcome,
            gen = self.generation,
            lease = self.lease,
            request = self.request,
            wait_us = self.wait_us,
            service_us = self.service_us,
            bytes = self.bytes,
            count0_label = self.count0.0,
            count0_value = self.count0.1,
            count1_label = self.count1.0,
            count1_value = self.count1.1,
        );
    }
}

/// Whether the current subscriber accepts Panelink diagnostics.
pub fn enabled() -> bool {
    tracing::enabled!(target: TARGET, tracing::Level::INFO)
}

/// Exclude routine screen/layout polling from the request trace stream.
pub fn trace_request(payload: &[u8]) -> bool {
    if !enabled() {
        return false;
    }
    !matches!(
        crate::ui_protocol::decode_ui_request(payload),
        Ok(crate::ui_protocol::UiRequest::LayoutQuery
            | crate::ui_protocol::UiRequest::ScreenQuery { .. })
    ) && !matches!(
        crate::pane_operations_protocol::decode_request(payload),
        Ok(crate::pane_operations_protocol::Request::ViewQuery)
    )
}

/// Monotonic elapsed time of a stage, in whole microseconds (saturating).
pub fn micros_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Fixed lowercase label for the shared wire `Outcome` vocabulary, so
/// call sites never carry content-bearing outcome strings into records.
pub fn wire_outcome_label(outcome: crate::protocol::Outcome) -> &'static str {
    match outcome {
        crate::protocol::Outcome::Accepted => "accepted",
        crate::protocol::Outcome::Applied => "applied",
        crate::protocol::Outcome::Failed => "failed",
        crate::protocol::Outcome::Partial => "partial",
        crate::protocol::Outcome::Unknown => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Bounded, lossy producer side.
// ---------------------------------------------------------------------------

/// Producer end of the diagnostic pipeline: bounded and always nonblocking.
struct SharedSink {
    sender: SyncSender<Vec<u8>>,
    stopping: Arc<AtomicBool>,
    dropped: AtomicU64,
    truncated: AtomicU64,
    sink_errors: AtomicU64,
    record_cap: usize,
}

impl SharedSink {
    /// Create the bounded queue; the receiver end goes to the writer thread
    /// or the test that inspects the pipeline.
    fn new(queue_capacity: usize, record_cap: usize) -> (Arc<Self>, Receiver<Vec<u8>>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(queue_capacity.max(1));
        (
            Arc::new(Self {
                sender,
                stopping: Arc::new(AtomicBool::new(false)),
                dropped: AtomicU64::new(0),
                truncated: AtomicU64::new(0),
                sink_errors: AtomicU64::new(0),
                record_cap: record_cap.max(1),
            }),
            receiver,
        )
    }

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn truncated(&self) -> u64 {
        self.truncated.load(Ordering::Relaxed)
    }

    fn sink_errors(&self) -> u64 {
        self.sink_errors.load(Ordering::Relaxed)
    }

    /// Queue one fragment; lossy on overflow and after stop, never blocking.
    fn enqueue(&self, bytes: &[u8]) {
        if self.stopping.load(Ordering::Relaxed) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut record = Vec::with_capacity(bytes.len());
        record.extend_from_slice(bytes);
        match self.sender.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The `fmt` layer creates one guard per record; the byte cap is per record.
/// Newtype so the `MakeWriter` impl observes the orphan rules.
struct SinkWriter(Arc<SharedSink>);

impl<'a> MakeWriter<'a> for SinkWriter {
    type Writer = RecordGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RecordGuard {
            sink: self.0.as_ref(),
            buffer: Vec::with_capacity(self.0.record_cap),
        }
    }
}

struct RecordGuard<'a> {
    sink: &'a SharedSink,
    buffer: Vec<u8>,
}

impl Drop for RecordGuard<'_> {
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            self.sink.enqueue(&self.buffer);
        }
    }
}

impl Write for RecordGuard<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let take = bytes.len().min(self.sink.record_cap - self.buffer.len());
        if take > 0 {
            self.buffer.extend_from_slice(&bytes[..take]);
        }
        if take < bytes.len() {
            self.sink.truncated.fetch_add(1, Ordering::Relaxed);
        }
        // Consume the whole buffer; overflow was counted instead so the
        // producer path never stalls.
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bounded disk side (private files only; failures are never propagated).
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const O_NOFOLLOW_FLAG: i32 = libc::O_NOFOLLOW;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const O_NOFOLLOW_FLAG: i32 = 0;

/// Validate (or create) the private diagnostics directory.
///
/// Rejects absolute paths outside the contract: symlinks, non-directories,
/// wrong owner and anything wider than mode 0700. A missing directory is
/// created with mode 0700, and the resulting mode and owner are verified.
fn ensure_private_dir(path: &Path) -> Result<(), &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{
            DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
        };

        if !path.is_absolute() {
            return Err("PANELINK_TRACE_DIR must be an absolute path");
        }
        let euid = rustix::process::geteuid().as_raw();
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("PANELINK_TRACE_DIR must not be a symlink");
                }
                if !metadata.is_dir() {
                    return Err("PANELINK_TRACE_DIR is not a directory");
                }
                if metadata.permissions().mode() & 0o777 != TRACE_DIR_MODE {
                    return Err("PANELINK_TRACE_DIR must have mode 0700");
                }
                if metadata.uid() != euid {
                    return Err("PANELINK_TRACE_DIR must be owned by the current user");
                }
                let directory = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(path)
                    .map_err(|_| "PANELINK_TRACE_DIR is unavailable")?;
                crate::private_acl::require_private(&directory)
                    .map_err(|_| "PANELINK_TRACE_DIR has an unsafe ACL")?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new()
                    .mode(TRACE_DIR_MODE)
                    .create(path)
                    .map_err(|_| "PANELINK_TRACE_DIR could not be created")?;
                let metadata = fs::symlink_metadata(path)
                    .map_err(|_| "PANELINK_TRACE_DIR could not be read back")?;
                if metadata.is_dir()
                    && metadata.permissions().mode() & 0o777 == TRACE_DIR_MODE
                    && metadata.uid() == euid
                {
                    ensure_private_dir(path)
                } else {
                    // The mode was narrowed below the contract (umask): fail
                    // closed rather than trusting a weaker directory.
                    Err("PANELINK_TRACE_DIR could not be created with mode 0700")
                }
            }
            Err(_) => Err("PANELINK_TRACE_DIR is unavailable"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("local diagnostics require a Unix private directory")
    }
}

fn backup_path(path: &Path, generation: usize) -> PathBuf {
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("log");
    let parent = path.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.{generation}"))
}

/// The current diagnostics file plus bounded rotation bookkeeping.
struct LogFile {
    file: Option<fs::File>,
    path: PathBuf,
    bytes: u64,
    rotation_cap: u64,
    retention: usize,
    rotations: u64,
}

impl LogFile {
    /// Open the fixed log file: mode 0600, never following a symlink, always
    /// owned by this user (existing wider or foreign files are rejected).
    fn open(path: PathBuf) -> Result<Self, &'static str> {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(LOG_FILE_MODE)
            .custom_flags(O_NOFOLLOW_FLAG | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| "diagnostics log file is unavailable or unsafe")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            let metadata = file
                .metadata()
                .map_err(|_| "diagnostics log file could not be inspected")?;
            if metadata.nlink() != 1 || !metadata.is_file() {
                return Err("diagnostics log path must be a regular file");
            }
            if metadata.permissions().mode() & 0o777 != LOG_FILE_MODE {
                return Err("diagnostics log file must have mode 0600");
            }
            if metadata.uid() != rustix::process::geteuid().as_raw() {
                return Err("diagnostics log file must be owned by the current user");
            }
            crate::private_acl::require_private(&file)
                .map_err(|_| "diagnostics log file has an unsafe ACL")?;
        }
        let bytes = file
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|_| "diagnostics log file could not be sized")?;
        Ok(Self {
            file: Some(file),
            path,
            bytes,
            rotation_cap: ROTATION_BYTES,
            retention: RETENTION_GENERATIONS,
            rotations: 0,
        })
    }

    /// Constructor for sink-failure probes backed by any writable descriptor.
    #[cfg(test)]
    fn from_file(path: PathBuf, file: fs::File, rotation_cap: u64, retention: usize) -> Self {
        Self {
            file: Some(file),
            path,
            bytes: 0,
            rotation_cap,
            retention,
            rotations: 0,
        }
    }

    fn write_record(&mut self, record: &[u8]) -> std::io::Result<()> {
        if record.len() as u64 > self.rotation_cap {
            return Err(std::io::Error::other("record exceeds rotation cap"));
        }
        if self.bytes.saturating_add(record.len() as u64) > self.rotation_cap {
            self.rotate()?;
        }
        if self.file.is_none() {
            self.reopen()?;
        }
        let Some(file) = self.file.as_mut() else {
            return Err(std::io::Error::other("diagnostics log unavailable"));
        };
        file.write_all(record)?;
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(record.len()).unwrap_or(u64::MAX));
        Ok(())
    }

    /// Reopen the current file after a sink failure; same bounded contract.
    fn reopen(&mut self) -> std::io::Result<()> {
        let reopened = Self::open(self.path.clone()).map_err(std::io::Error::other)?;
        self.file = reopened.file;
        Ok(())
    }

    /// Bounded retention shift: drop the oldest backup, move `.n` up, then
    /// reopen a fresh current file. Only counted-local renames inside the
    /// already-private directory, so no symlink can redirect them.
    fn rotate(&mut self) -> std::io::Result<()> {
        self.file = None;
        ignore_not_found(fs::remove_file(backup_path(&self.path, self.retention)))?;
        for generation in (1..self.retention).rev() {
            ignore_not_found(fs::rename(
                backup_path(&self.path, generation),
                backup_path(&self.path, generation + 1),
            ))?;
        }
        ignore_not_found(fs::rename(&self.path, backup_path(&self.path, 1)))?;
        self.reopen()?;
        self.bytes = 0;
        self.rotations = self.rotations.saturating_add(1);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bounded background writer (bounded drain on stop; failures only counted).
// ---------------------------------------------------------------------------

fn ignore_not_found(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn write_one(log: &mut LogFile, sink: &SharedSink, record: &[u8]) {
    if log.write_record(record).is_err() {
        sink.sink_errors.fetch_add(1, Ordering::Relaxed);
    }
}

fn write_summary(log: &mut LogFile, sink: &SharedSink, marker: &str) {
    let line = format!(
        "panelink diagnostics ({}): {marker} dropped={} truncated={} sink_errors={}\n",
        process_role(),
        sink.dropped(),
        sink.truncated(),
        sink.sink_errors(),
    );
    write_one(log, sink, line.as_bytes());
}

fn writer_loop(mut log: LogFile, receiver: Receiver<Vec<u8>>, sink: Arc<SharedSink>) {
    loop {
        match receiver.recv_timeout(WRITER_POLL) {
            Ok(record) => {
                write_one(&mut log, &sink, &record);
                if sink.stopping.load(Ordering::Acquire) {
                    // Drain what already arrived (bounded by the queue
                    // capacity), summarize, then stop.
                    while let Ok(record) = receiver.try_recv() {
                        write_one(&mut log, &sink, &record);
                    }
                    write_summary(&mut log, &sink, "stopped");
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if sink.stopping.load(Ordering::Acquire) {
                    while let Ok(record) = receiver.try_recv() {
                        write_one(&mut log, &sink, &record);
                    }
                    write_summary(&mut log, &sink, "stopped");
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                write_summary(&mut log, &sink, "pipeline closed");
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Process lifetime.
// ---------------------------------------------------------------------------

struct LiveDiagnostics {
    sink: Arc<SharedSink>,
    thread: Mutex<Option<JoinHandle<()>>>,
    _lock: fs::File,
}

static STATE: OnceLock<LiveDiagnostics> = OnceLock::new();
static INIT_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Enable diagnostics exactly once per process when `PANELINK_TRACE_DIR`
/// satisfies the private-directory contract; otherwise leave the process
/// uninstrumented (all stage records are then cheap interest-gated no-ops).
///
/// Never affects the caller's work: enablement failures disable diagnostics
/// locally rather than erroring any command.
pub fn init_process(role: Role) {
    // One attempted enablement per process, so a failed attempt cannot tear
    // down a concurrent success.
    if INIT_ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = ROLE.set(role);
    let Some(directory) = std::env::var_os(TRACE_DIR_ENVIRONMENT) else {
        return;
    };
    let directory = PathBuf::from(directory);
    if ensure_private_dir(&directory).is_err() {
        return;
    }
    let Ok(directory) = directory.canonicalize() else {
        return;
    };
    if directory.ancestors().any(|ancestor| {
        use std::os::unix::fs::MetadataExt;
        crate::private_acl::check_ancestor_path(ancestor).is_err()
            || fs::metadata(ancestor).map_or(true, |m| {
                (m.uid() != 0 && m.uid() != rustix::process::geteuid().as_raw())
                    || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
            })
    }) {
        return;
    }
    let stem = role.file_stem();
    let Ok(lock) = LogFile::open(directory.join(format!("{stem}.lock"))) else {
        return;
    };
    let Some(lock) = lock.file else {
        return;
    };
    if lock.try_lock().is_err() {
        return;
    }

    let path = directory.join(format!("{stem}.log"));
    // Open before the pipeline so a bad path disables without half-enable.
    let mut log = match LogFile::open(path) {
        Ok(log) => log,
        Err(_) => return,
    };
    for generation in 0..=RETENTION_GENERATIONS {
        let path = if generation == 0 {
            log.path.clone()
        } else {
            backup_path(&log.path, generation)
        };
        if fs::symlink_metadata(path).is_ok_and(|m| m.len() > ROTATION_BYTES || !m.is_file()) {
            return;
        }
    }
    // Startup rotation keeps the on-disk history bounded even when this
    // process runs after a long previous one.
    if log.bytes >= log.rotation_cap && log.rotate().is_err() {
        return;
    }
    let header = format!(
        "panelink diagnostics {} enabled (version={}, pid={})\n",
        role.label(),
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
    );
    if log.write_record(header.as_bytes()).is_err() {
        return;
    }
    let (sink, receiver) = SharedSink::new(RECORD_QUEUE_CAPACITY, RECORD_MAX_BYTES);
    let writer_sink = Arc::clone(&sink);
    let thread = match std::thread::Builder::new()
        .name("panelink-diagnostics".to_owned())
        .spawn(move || writer_loop(log, receiver, writer_sink))
    {
        Ok(thread) => thread,
        Err(_) => return,
    };
    {
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(SinkWriter(Arc::clone(&sink)))
                .with_ansi(false)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target(TARGET, tracing::Level::INFO),
                ),
        );
        if tracing::subscriber::set_global_default(subscriber).is_err() {
            // Another subscriber owns the process (never expected for the
            // executable boundary): stop the writer instead of leaking it.
            sink.stopping.store(true, Ordering::Release);
            // Drop the handle: a blocked sink must never hold startup.
            drop(thread);
            return;
        }
    }
    let _ = STATE.set(LiveDiagnostics {
        sink,
        thread: Mutex::new(Some(thread)),
        _lock: lock,
    });
}

/// Disable diagnostics and stop the writer within `budget`.
///
/// Client callers run this after terminal restoration (never before), and the
/// server after owner shutdown. It never blocks longer than `budget`; a still
/// draining writer exits on its own schedule (bounded by the writer poll) and
/// its handle is left for a retry or process exit, so no join can block the
/// caller's terminal or PTY work.
pub fn shutdown_after_restore(budget: Duration) {
    let Some(state) = STATE.get() else { return };
    state.sink.stopping.store(true, Ordering::Release);
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let finished = state
            .thread
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(JoinHandle::is_finished))
            .unwrap_or(true);
        if finished {
            if let Ok(mut guard) = state.thread.lock() {
                if let Some(thread) = guard.take() {
                    let _ = thread.join();
                }
            }
            return;
        }
        std::thread::sleep(
            Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::UnixStream;

    struct ScopedDir(PathBuf);

    impl ScopedDir {
        fn new(name: &str) -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after Unix epoch")
                .as_nanos();
            let stem = format!(
                "panelink-diagnostics-test-{}-{stamp}-{name}",
                std::process::id()
            );
            let path = std::env::temp_dir().join(stem);
            let _ = fs::remove_dir_all(&path);
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("private test dir");
            Self(path)
        }
    }

    impl Drop for ScopedDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Capturing subscriber: records event field names/values, flags any
    /// `Debug`-formatted value as a canary, and records span names.
    type CapturedEvents = Arc<Mutex<Vec<Vec<(String, String)>>>>;

    #[derive(Clone)]
    struct Capture {
        events: CapturedEvents,
        debug_canaries: Arc<Mutex<Vec<String>>>,
        span_names: Arc<Mutex<Vec<String>>>,
        next_id: Arc<AtomicU64>,
    }

    impl Capture {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                debug_canaries: Arc::new(Mutex::new(Vec::new())),
                span_names: Arc::new(Mutex::new(Vec::new())),
                next_id: Arc::new(AtomicU64::new(1)),
            }
        }

        fn events(&self) -> Vec<Vec<(String, String)>> {
            self.events.lock().unwrap().clone()
        }

        fn canaries(&self) -> Vec<String> {
            self.debug_canaries.lock().unwrap().clone()
        }
    }

    struct RecordingVisitor {
        fields: Vec<(String, String)>,
        canaries: Vec<String>,
    }

    impl tracing::field::Visit for RecordingVisitor {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.fields
                .push((field.name().to_owned(), value.to_owned()));
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        /// Approved emission paths never use `?`-format Debug fields; any such
        /// use surfaces here as an explicit canary instead of blending in.
        fn record_debug(&mut self, _field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.canaries.push(format!("CANARY-DEBUG:{value:?}"));
        }

        fn record_error(
            &mut self,
            field: &tracing::field::Field,
            value: &(dyn std::error::Error + 'static),
        ) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }
    }

    impl tracing::Subscriber for Capture {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::Id {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            self.span_names
                .lock()
                .unwrap()
                .push(span.metadata().name().to_owned());
            tracing::Id::from_u64(id)
        }

        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = RecordingVisitor {
                fields: Vec::new(),
                canaries: Vec::new(),
            };
            event.record(&mut visitor);
            self.debug_canaries.lock().unwrap().extend(visitor.canaries);
            self.events.lock().unwrap().push(visitor.fields);
        }

        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }

    const ALLOWED_FIELD_NAMES: &[&str] = &[
        "process",
        "op",
        "stage",
        "outcome",
        "gen",
        "lease",
        "request",
        "wait_us",
        "service_us",
        "bytes",
        "count0_label",
        "count0_value",
        "count1_label",
        "count1_value",
    ];

    fn drain_into(receiver: &Receiver<Vec<u8>>, log: &mut LogFile, sink: &SharedSink) {
        while let Ok(record) = receiver.try_recv() {
            write_one(log, sink, &record);
        }
    }

    fn read_file(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path:?}: {error}"))
    }

    #[test]
    fn bounded_queue_drops_at_capacity_and_counts_the_loss_visibly() {
        let (sink, receiver) = SharedSink::new(1, RECORD_MAX_BYTES);
        let writers = SinkWriter(Arc::clone(&sink));
        let mut guard = writers.make_writer();
        assert_eq!(guard.write(b"first line\n").unwrap(), 11);
        drop(guard);
        let mut guard = writers.make_writer();
        assert_eq!(guard.write(b"second line\n").unwrap(), 12);
        drop(guard);
        assert_eq!(sink.dropped(), 1);
        assert_eq!(sink.truncated(), 0);
        let parent = ScopedDir::new("drops");
        let mut log =
            LogFile::open(parent.0.join("client.log")).expect("private log file creates cleanly");
        drain_into(&receiver, &mut log, &sink);
    }

    #[test]
    fn producer_stays_nonblocking_and_counts_drops_when_the_sink_is_dead() {
        let (sink, receiver) = SharedSink::new(4, RECORD_MAX_BYTES);
        drop(receiver);
        let writers = SinkWriter(Arc::clone(&sink));
        let mut guard = writers.make_writer();
        for _ in 0..8 {
            assert_eq!(guard.write(b"lost-record\n").unwrap(), 12);
            drop(guard);
            guard = writers.make_writer();
        }
        assert_eq!(sink.dropped(), 8);
        assert_eq!(sink.truncated(), 0);
    }

    #[test]
    fn record_byte_cap_bounds_each_record_and_counts_truncation() {
        let (sink, _receiver) = SharedSink::new(16, 8);
        let writers = SinkWriter(Arc::clone(&sink));
        let mut guard = writers.make_writer();
        assert_eq!(guard.write(b"0123456789").unwrap(), 10);
        assert_eq!(sink.truncated(), 1);
        assert_eq!(guard.write(b"overflow").unwrap(), 8);
        assert_eq!(sink.truncated(), 2);
    }

    #[test]
    fn stop_makes_late_producers_count_drops_instead_of_queueing() {
        let (sink, _receiver) = SharedSink::new(4, RECORD_MAX_BYTES);
        sink.stopping.store(true, Ordering::Release);
        let writers = SinkWriter(Arc::clone(&sink));
        let mut guard = writers.make_writer();
        assert_eq!(guard.write(b"late\n").unwrap(), 5);
        drop(guard);
        assert_eq!(sink.dropped(), 1);
    }

    #[test]
    fn byte_capped_rotation_keeps_a_bounded_set_of_retained_files() {
        let parent = ScopedDir::new("rotation");
        let path = parent.0.join("client.log");
        let mut log = LogFile::open(path.clone()).expect("private log created");
        log.rotation_cap = 64;
        log.retention = 2;
        for index in 0..12_u8 {
            log.write_record(format!("R{index} record-0123456789\n").as_bytes())
                .expect("records always write");
        }
        assert!(path.is_file());
        for generation in [1_usize, 2] {
            let backup = parent.0.join(format!("client.log.{generation}"));
            let size = fs::metadata(&backup)
                .map(|metadata| metadata.len())
                .unwrap_or_else(|_| panic!("{backup:?} must exist"));
            assert!(size <= 64, "retained generation {backup:?} over bounds");
        }
        for generation in 3..8 {
            assert!(!parent.0.join(format!("client.log.{generation}")).exists());
        }
        assert!(read_file(&path).contains("record-0123456789"));
        assert!(read_file(&parent.0.join("client.log.1")).contains("record-0123456789"));
    }

    #[test]
    fn unsafe_log_files_are_never_followed_or_used_unmode() {
        let parent = ScopedDir::new("unsafe-files");
        // A symlinked current path is rejected, not followed.
        let destination = parent.0.join("destination.log");
        fs::write(&destination, b"real destination").unwrap();
        let symlinked = parent.0.join("symlinked.log");
        std::os::unix::fs::symlink(&destination, &symlinked).unwrap();
        assert!(LogFile::open(symlinked).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"real destination");

        // A wider-mode existing file is rejected.
        let wide = parent.0.join("wide.log");
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            fs::OpenOptions::new()
                .write(true)
                .truncate(false)
                .create(true)
                .mode(0o644)
                .open(&wide)
                .unwrap();
        }
        assert!(LogFile::open(wide).is_err());

        // A correctly private file keeps working.
        let private = parent.0.join("private.log");
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            fs::OpenOptions::new()
                .write(true)
                .truncate(false)
                .create(true)
                .mode(0o600)
                .open(&private)
                .unwrap();
        }
        let mut log = LogFile::open(PathBuf::from(&private)).expect("private file is usable");
        log.write_record(b"kept\n").unwrap();
        assert_eq!(fs::read(&private).unwrap(), b"kept\n");
    }

    #[test]
    fn directory_contracts_reject_relative_symlinked_and_wide_paths() {
        let parent = ScopedDir::new("dir");
        let fresh = parent.0.join("fresh");
        assert!(ensure_private_dir(&fresh).is_ok());
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let target = parent.0.join("target");
        fs::create_dir(&target).unwrap();
        let link = parent.0.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());

        let wide = parent.0.join("wide");
        fs::create_dir(&wide).unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&wide).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&wide, permissions).unwrap();
        }
        assert!(ensure_private_dir(&wide).is_err());

        assert!(ensure_private_dir(Path::new("relative")).is_err());
    }

    #[test]
    fn granting_acls_disable_diagnostics_without_changing_files() {
        let parent = ScopedDir::new("acl");
        let path = parent.0.join("client.log");
        let mut log = LogFile::open(path.clone()).unwrap();
        log.write_record(b"synthetic old record\n").unwrap();
        drop(log);
        let grant = |path: &Path, entry: &str| {
            let status = std::process::Command::new("/bin/chmod")
                .args(["+a", entry])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success());
        };
        grant(&path, "everyone allow read");
        assert!(LogFile::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic old record\n");
        grant(&parent.0, "everyone allow list,search");
        assert!(ensure_private_dir(&parent.0).is_err());
    }

    #[test]
    fn producer_survives_sink_failures_and_counts_them() {
        let parent = ScopedDir::new("sink-failure");
        // A pipe-backed file that dies mid-flight behaves like a failing log.
        let (writer, reader) = UnixStream::pair().expect("test pipe");
        let (sink, receiver) = SharedSink::new(8, RECORD_MAX_BYTES);
        let mut log = LogFile::from_file(
            parent.0.join("client.log"),
            std::fs::File::from(std::os::unix::io::OwnedFd::from(writer)),
            u64::MAX,
            2,
        );
        sink.sender
            .send(b"queued first\n".to_vec())
            .expect("bounded queue accepts capacity items");
        drop(reader);
        sink.sender
            .send(b"queued second\n".to_vec())
            .expect("bounded queue accepts capacity items");
        drain_into(&receiver, &mut log, &sink);
        assert_eq!(sink.sink_errors(), 2, "each failing record counts");
        assert_eq!(sink.dropped(), 0);
        // The producer path is untouched: queueing still succeeds while any
        // capacity remains in the bounded channel.
        let producer_writer = SinkWriter(Arc::clone(&sink));
        let mut guard = producer_writer.make_writer();
        assert_eq!(guard.write(b"producer alive\n").unwrap(), 15);
    }

    #[test]
    fn writer_stops_bounded_drains_and_summarizes() {
        let parent = ScopedDir::new("writer-stop");
        let path = parent.0.join("client.log");
        let mut log = LogFile::open(path.clone()).expect("private log created");
        log.rotation_cap = u64::MAX;
        let (sink, receiver) = SharedSink::new(4, RECORD_MAX_BYTES);
        let thread_sink = Arc::clone(&sink);
        let handle = std::thread::spawn(move || writer_loop(log, receiver, thread_sink));
        let stop_writer = SinkWriter(Arc::clone(&sink));
        let mut guard = stop_writer.make_writer();
        assert_eq!(guard.write(b"before stop\n").unwrap(), 12);
        drop(guard);
        sink.stopping.store(true, Ordering::Release);
        std::thread::sleep(4 * WRITER_POLL);
        assert!(
            handle.is_finished(),
            "writer stops within its bounded poll ({WRITER_POLL:?})"
        );
        handle.join().unwrap();
        let content = read_file(&path);
        assert!(content.contains("before stop\n"));
        assert!(content.contains(": stopped dropped="));
    }

    #[test]
    fn recorded_fields_are_exactly_the_allowlist_and_correlate() {
        let capture = Capture::new();
        let event = StageEvent {
            op: "keys",
            stage: "wire",
            outcome: "applied",
            generation: 42,
            lease: 7,
            request: 9,
            wait_us: 5,
            service_us: 117,
            bytes: 24,
            count0: ("keys", 3),
            count1: ("", 0),
        };
        tracing::subscriber::with_default(capture.clone(), || {
            event.record();
        });
        assert!(capture.canaries().is_empty(), "no Debug capture allowed");
        let events = capture.events();
        assert_eq!(events.len(), 1);
        for (name, _value) in &events[0] {
            assert!(
                ALLOWED_FIELD_NAMES.contains(&name.as_str()),
                "unexpected diagnostic field {name}"
            );
        }
        for (name, value) in &events[0] {
            match name.as_str() {
                "gen" => assert_eq!(value, "42"),
                "lease" => assert_eq!(value, "7"),
                "request" => assert_eq!(value, "9"),
                "outcome" => assert_eq!(value, "applied"),
                "op" => assert_eq!(value, "keys"),
                "stage" => assert_eq!(value, "wire"),
                "service_us" => assert_eq!(value, "117"),
                "count0_value" => assert_eq!(value, "3"),
                "count1_value" => assert_eq!(value, "0"),
                "count1_label" => assert_eq!(value, ""),
                _ => {}
            }
        }
    }

    #[test]
    fn disabled_path_records_nothing_even_under_a_reject_all_subscriber() {
        struct Never;
        impl tracing::Subscriber for Never {
            fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
                false
            }
            fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
                tracing::Id::from_u64(1)
            }
            fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
            fn event(&self, _event: &tracing::Event<'_>) {
                panic!("records must stay inert when no subscriber has interest");
            }
            fn enter(&self, _span: &tracing::Id) {}
            fn exit(&self, _span: &tracing::Id) {}
        }
        tracing::subscriber::with_default(Never, || {
            StageEvent {
                op: "paste",
                stage: "submit",
                ..StageEvent::default()
            }
            .record();
        });
    }

    #[test]
    fn span_entries_and_closes_are_not_arbitrary_content() {
        let capture = Capture::new();
        tracing::subscriber::with_default(capture.clone(), || {
            let span = tracing::span!(
                target: TARGET,
                tracing::Level::INFO,
                "client_command",
                process = process_role(),
                op = "paste",
            );
            span.in_scope(|| {
                StageEvent {
                    op: "paste",
                    stage: "apply",
                    outcome: "applied",
                    ..StageEvent::default()
                }
                .record();
            });
        });
        assert!(capture.canaries().is_empty());
        assert_eq!(
            capture.span_names.lock().unwrap().as_slice(),
            ["client_command"]
        );
        assert!(!capture.events().is_empty());
    }

    #[test]
    fn fmt_layer_writes_allowlisted_records_through_the_bounded_pipeline() {
        let parent = ScopedDir::new("fmt");
        let path = parent.0.join("client.log");
        let mut log = LogFile::open(path.clone()).expect("private log created");
        log.rotation_cap = u64::MAX;
        let (sink, receiver) = SharedSink::new(16, RECORD_MAX_BYTES);
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(SinkWriter(Arc::clone(&sink)))
                .with_ansi(false)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target(TARGET, tracing::Level::INFO),
                ),
        );
        tracing::subscriber::with_default(subscriber, || {
            StageEvent {
                op: "keys",
                stage: "wire",
                outcome: "applied",
                generation: 3,
                lease: 4,
                request: 5,
                wait_us: 2,
                service_us: 9,
                bytes: 12,
                count0: ("keys", 8),
                count1: ("", 0),
            }
            .record();
        });
        drain_into(&receiver, &mut log, &sink);
        let text = read_file(&path);
        assert!(text.contains("panelink"), "record was written: {text:?}");
        assert!(
            text.contains("op=\"keys\""),
            "field content present: {text:?}"
        );
        assert!(text.contains("gen=3"));
        assert!(text.contains("count0_value=8"));
        assert!(!text.contains("CANARY"), "no Debug capture: {text:?}");
    }
}
