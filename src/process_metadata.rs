//! Bounded native sampling of the current managed child's process metadata.
//!
//! This module observes only the direct child PID captured at launch. It does
//! not inspect descendants, argv, or the environment. Identity is captured on
//! the launch thread; path sampling runs on one detached worker with capacity
//! for one queued job and one completion.

use crate::control_protocol::TargetRef;
use crate::pane_metadata_protocol::{Observation, PathObservation, PathStatus};
use std::fmt;
use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

const PROC_PIDTBSDINFO: i32 = 3;
const PROC_PIDVNODEPATHINFO: i32 = 9;
const BSDINFO_BYTES: usize = 136;
const BSDINFO_PID_OFFSET: usize = 12;
const BSDINFO_START_SEC_OFFSET: usize = 120;
const BSDINFO_START_USEC_OFFSET: usize = 128;
const VNODEPATHINFO_BYTES: usize = 2352;
const VNODEPATHINFO_CWD_PATH_OFFSET: usize = 152;
const PATH_MAX: usize = 4096;
const PATH_BYTES_MAX: usize = 4095;

/// Stable PID and kernel birth identity for one managed child.
///
/// Fields stay private and Debug intentionally omits their values.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ProcessIdentity {
    pid: i32,
    start_seconds: u64,
    start_microseconds: u64,
}

impl fmt::Debug for ProcessIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProcessIdentity(<redacted>)")
    }
}

/// Capture the live kernel identity immediately after a managed spawn.
pub(crate) fn capture_identity(pid: u32) -> Option<ProcessIdentity> {
    let pid = i32::try_from(pid).ok()?;
    read_identity(pid).ok()
}

/// One identity-bound request for the worker.
pub(crate) struct MetadataJob {
    pub(crate) id: u64,
    pub(crate) target: TargetRef,
    pub(crate) identity: ProcessIdentity,
}

/// Completed observation associated with its owner request identifier.
pub(crate) struct MetadataCompletion {
    pub(crate) id: u64,
    pub(crate) observation: Observation,
}

/// Single-thread native sampler with one-slot bounded channels.
pub(crate) struct MetadataWorker {
    jobs: SyncSender<MetadataJob>,
    completions: Receiver<MetadataCompletion>,
    // Dropping a JoinHandle detaches. It must never be joined because a native
    // libproc call has no cancellation contract.
    _thread: JoinHandle<()>,
}

impl MetadataWorker {
    pub(crate) fn start(origin: Instant) -> io::Result<Self> {
        Self::start_with_sampler(origin, sample)
    }

    pub(crate) fn start_with_sampler<F>(origin: Instant, sampler: F) -> io::Result<Self>
    where
        F: Fn(TargetRef, ProcessIdentity, Instant) -> Observation + Send + 'static,
    {
        let (job_tx, job_rx) = mpsc::sync_channel::<MetadataJob>(1);
        let (completion_tx, completion_rx) = mpsc::sync_channel::<MetadataCompletion>(1);
        let worker = thread::Builder::new()
            .name("panelink-process-metadata".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let observation = sampler(job.target, job.identity, origin);
                    let completion = MetadataCompletion {
                        id: job.id,
                        observation,
                    };
                    match completion_tx.try_send(completion) {
                        Ok(()) | Err(TrySendError::Disconnected(_)) => {}
                        // A full one-slot completion channel means the owner
                        // violated its one-in-flight contract. Stop rather
                        // than block or accumulate stale observations.
                        Err(TrySendError::Full(_)) => break,
                    }
                }
            })?;
        Ok(Self {
            jobs: job_tx,
            completions: completion_rx,
            _thread: worker,
        })
    }

    /// Enqueue without waiting; at most one job may be pending.
    pub(crate) fn submit(&self, job: MetadataJob) -> Result<(), ()> {
        self.jobs.try_send(job).map_err(|_| ())
    }

    /// Poll without waiting for a native query to finish.
    pub(crate) fn poll(&self) -> Result<Option<MetadataCompletion>, ()> {
        match self.completions.try_recv() {
            Ok(completion) => Ok(Some(completion)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }
}

fn sample(target: TargetRef, expected: ProcessIdentity, origin: Instant) -> Observation {
    let sampled_at_mono_ms = origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let before = match read_identity(expected.pid) {
        Ok(identity) if identity == expected => identity,
        Ok(_) => {
            return Observation::unavailable(
                target,
                sampled_at_mono_ms,
                PathStatus::IdentityMismatch,
            )
        }
        Err(status) => return Observation::unavailable(target, sampled_at_mono_ms, status),
    };

    let cwd = read_cwd(before.pid);
    let executable = read_executable(before.pid);

    match read_identity(before.pid) {
        Ok(after) if after == expected => Observation {
            target,
            sampled_at_mono_ms,
            cwd,
            executable,
        },
        Ok(_) => Observation::unavailable(target, sampled_at_mono_ms, PathStatus::IdentityMismatch),
        Err(status) => Observation::unavailable(target, sampled_at_mono_ms, status),
    }
}

#[repr(align(8))]
struct AlignedBsdInfo([u8; BSDINFO_BYTES]);

#[repr(align(8))]
struct AlignedVnodePathInfo([u8; VNODEPATHINFO_BYTES]);

#[repr(align(8))]
struct AlignedExecutablePath([u8; PATH_MAX]);

fn read_identity(pid: i32) -> Result<ProcessIdentity, PathStatus> {
    let mut info = AlignedBsdInfo([0; BSDINFO_BYTES]);
    // SAFETY: libproc writes the documented 136-byte proc_bsdinfo record to
    // this 8-byte-aligned buffer. Fields below use offsets confirmed against
    // the active macOS SDK headers and a compiled C ABI probe.
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            info.0.as_mut_ptr().cast(),
            BSDINFO_BYTES as i32,
        )
    };
    if written <= 0 {
        return Err(native_error_status());
    }
    if written != BSDINFO_BYTES as i32 {
        return Err(PathStatus::QueryFailed);
    }
    let bytes = &info.0;
    let observed_pid = read_u32(bytes, BSDINFO_PID_OFFSET).ok_or(PathStatus::QueryFailed)?;
    if observed_pid != pid as u32 {
        return Err(PathStatus::IdentityMismatch);
    }
    Ok(ProcessIdentity {
        pid,
        start_seconds: read_u64(bytes, BSDINFO_START_SEC_OFFSET).ok_or(PathStatus::QueryFailed)?,
        start_microseconds: read_u64(bytes, BSDINFO_START_USEC_OFFSET)
            .ok_or(PathStatus::QueryFailed)?,
    })
}

fn read_cwd(pid: i32) -> PathObservation {
    let mut info = AlignedVnodePathInfo([0; VNODEPATHINFO_BYTES]);
    // SAFETY: libproc writes the SDK's 2352-byte proc_vnodepathinfo record;
    // cwd's vip_path begins at byte 152 and occupies a 1024-byte array.
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDVNODEPATHINFO,
            0,
            info.0.as_mut_ptr().cast(),
            VNODEPATHINFO_BYTES as i32,
        )
    };
    if written != VNODEPATHINFO_BYTES as i32 {
        return PathObservation::unavailable(PathStatus::QueryFailed);
    }
    path_from_c_bytes(&info.0[VNODEPATHINFO_CWD_PATH_OFFSET..VNODEPATHINFO_CWD_PATH_OFFSET + 1024])
}

fn read_executable(pid: i32) -> PathObservation {
    let mut path = AlignedExecutablePath([0; PATH_MAX]);
    // SAFETY: the aligned 4096-byte output buffer and maximum length match
    // proc_pidpath's documented interface.
    let written = unsafe { proc_pidpath(pid, path.0.as_mut_ptr().cast(), PATH_MAX as u32) };
    if written <= 0 {
        return PathObservation::unavailable(native_error_status());
    }
    path_from_proc_pidpath(&path.0, written as usize)
}

fn path_from_proc_pidpath(buffer: &[u8], returned: usize) -> PathObservation {
    if returned == 0 || returned >= PATH_MAX || returned >= buffer.len() || buffer[returned] != 0 {
        return PathObservation::unavailable(PathStatus::QueryFailed);
    }
    let bytes = &buffer[..returned];
    if bytes.contains(&0) {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    }
    path_from_utf8(bytes)
}

fn path_from_c_bytes(bytes: &[u8]) -> PathObservation {
    let Some(end) = bytes.iter().position(|byte| *byte == 0) else {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    };
    if end == 0 || end > PATH_BYTES_MAX {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    }
    path_from_utf8(&bytes[..end])
}

fn path_from_utf8(bytes: &[u8]) -> PathObservation {
    if bytes.is_empty() || bytes.len() > PATH_BYTES_MAX {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    }
    let Ok(path) = std::str::from_utf8(bytes) else {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    };
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return PathObservation::unavailable(PathStatus::InvalidPath);
    }
    match PathObservation::available(path.to_owned()) {
        Ok(observation) => observation,
        Err(_) => PathObservation::unavailable(PathStatus::InvalidPath),
    }
}

fn native_error_status() -> PathStatus {
    native_error_status_from_errno(io::Error::last_os_error().raw_os_error())
}

fn native_error_status_from_errno(errno: Option<i32>) -> PathStatus {
    match errno {
        Some(libc::EACCES) | Some(libc::EPERM) => PathStatus::PermissionDenied,
        Some(libc::ESRCH) => PathStatus::ProcessGone,
        _ => PathStatus::QueryFailed,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[link(name = "proc", kind = "dylib")]
unsafe extern "C" {
    fn proc_pidinfo(
        pid: i32,
        flavor: i32,
        arg: u64,
        buffer: *mut std::ffi::c_void,
        buffersize: i32,
    ) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, buffersize: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    fn target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        }
    }

    fn spawn_shell(script: &str) -> Child {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap()
    }

    struct ChildGroup(Child);

    impl Drop for ChildGroup {
        fn drop(&mut self) {
            let pgid = self.0.id() as i32;
            // SAFETY: each fixture starts in its own process group; signaling
            // that negative PID terminates only the fixture and descendants.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    fn observation_for(child: &Child) -> Observation {
        let identity = capture_identity(child.id()).unwrap();
        sample(target(), identity, Instant::now())
    }

    #[test]
    fn samples_initial_cwd_and_direct_executable() {
        let child = ChildGroup(spawn_shell("exec /bin/sleep 30"));
        let deadline = Instant::now() + Duration::from_secs(2);
        let observation = loop {
            let observation = observation_for(&child.0);
            if observation
                .executable
                .value
                .as_deref()
                .is_some_and(|path| path.ends_with("/sleep"))
                || Instant::now() >= deadline
            {
                break observation;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            observation.cwd.value.as_deref(),
            Some(
                std::fs::canonicalize(std::env::current_dir().unwrap())
                    .unwrap()
                    .to_str()
                    .unwrap()
            )
        );
        assert!(observation
            .executable
            .value
            .as_deref()
            .is_some_and(|path| path.ends_with("/sleep")));
    }

    #[test]
    fn observes_cwd_after_child_changes_directory() {
        let child = ChildGroup(spawn_shell("cd /tmp; exec /bin/sleep 30"));
        let expected = std::fs::canonicalize("/tmp").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let observation = loop {
            let observation = observation_for(&child.0);
            if observation.cwd.value.as_deref() == expected.to_str() || Instant::now() >= deadline {
                break observation;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(observation.cwd.value.as_deref(), expected.to_str());
    }

    #[test]
    fn direct_child_identity_stays_on_shell_when_descendant_runs() {
        let child = ChildGroup(spawn_shell("/bin/sleep 30 & wait"));
        let deadline = Instant::now() + Duration::from_secs(2);
        let observation = loop {
            let observation = observation_for(&child.0);
            if observation.executable.value.as_deref() == Some("/bin/sh")
                || Instant::now() >= deadline
            {
                break observation;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(
            observation
                .executable
                .value
                .as_deref()
                .is_some_and(|path| path.ends_with("/sh")),
            "{observation:?}"
        );
    }

    #[test]
    fn worker_drop_does_not_wait_for_late_native_work() {
        use std::sync::mpsc;
        use std::sync::Arc;

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let worker =
            MetadataWorker::start_with_sampler(Instant::now(), move |target, _, origin| {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Observation::unavailable(
                    target,
                    origin.elapsed().as_millis() as u64,
                    PathStatus::QueryFailed,
                )
            })
            .unwrap();
        let identity = capture_identity(std::process::id()).unwrap();
        worker
            .submit(MetadataJob {
                id: 1,
                target: target(),
                identity,
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        drop(worker);
        assert!(started.elapsed() < Duration::from_millis(50));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn worker_accepts_one_queued_job_and_rejects_more() {
        use std::sync::mpsc;
        use std::sync::Arc;

        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let worker =
            MetadataWorker::start_with_sampler(Instant::now(), move |target, _, origin| {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Observation::unavailable(
                    target,
                    origin.elapsed().as_millis() as u64,
                    PathStatus::QueryFailed,
                )
            })
            .unwrap();
        let identity = capture_identity(std::process::id()).unwrap();
        worker
            .submit(MetadataJob {
                id: 1,
                target: target(),
                identity,
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker
            .submit(MetadataJob {
                id: 2,
                target: target(),
                identity,
            })
            .unwrap();
        assert!(worker
            .submit(MetadataJob {
                id: 3,
                target: target(),
                identity,
            })
            .is_err());

        release_tx.send(()).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        // Drain the first completion before releasing the second call: the
        // completion channel deliberately has capacity one, just like the
        // production driver which admits only one in-flight query.
        let first = worker.poll().unwrap().unwrap();
        assert_eq!(first.id, 1);
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut completions = vec![first.id];
        while completions.len() < 2 && Instant::now() < deadline {
            if let Some(completion) = worker.poll().unwrap() {
                completions.push(completion.id);
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(completions, [1, 2]);
    }

    #[test]
    fn identity_debug_does_not_disclose_pid_or_birth_time() {
        let identity = capture_identity(std::process::id()).unwrap();
        let debug = format!("{identity:?}");
        assert_eq!(debug, "ProcessIdentity(<redacted>)");
        assert!(!debug.contains(&std::process::id().to_string()));
    }

    #[test]
    fn changed_birth_identity_is_rejected_before_paths_are_published() {
        let identity = capture_identity(std::process::id()).unwrap();
        let wrong_identity = ProcessIdentity {
            start_seconds: identity.start_seconds.wrapping_add(1),
            ..identity
        };
        let observation = sample(target(), wrong_identity, Instant::now());
        assert_eq!(observation.cwd.status, PathStatus::IdentityMismatch);
        assert_eq!(observation.executable.status, PathStatus::IdentityMismatch);
        assert!(observation.cwd.value.is_none());
        assert!(observation.executable.value.is_none());
    }

    #[test]
    fn missing_pid_is_reported_as_process_gone() {
        let observation = sample(
            target(),
            ProcessIdentity {
                pid: i32::MAX,
                start_seconds: 1,
                start_microseconds: 1,
            },
            Instant::now(),
        );
        assert_eq!(observation.cwd.status, PathStatus::ProcessGone);
        assert_eq!(observation.executable.status, PathStatus::ProcessGone);
    }

    #[test]
    fn path_parsers_reject_malformed_cwd_and_executable_buffers() {
        assert_eq!(
            path_from_utf8(b"relative/path").status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_utf8(b"/bad\npath").status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_utf8(b"/bad\0path").status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_utf8(b"/bad\xffpath").status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_utf8(&vec![b'a'; PATH_BYTES_MAX + 1]).status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_c_bytes(b"/no-terminator").status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_c_bytes(b"relative\0").status,
            PathStatus::InvalidPath
        );

        let mut buffer = [0u8; 16];
        buffer[..3].copy_from_slice(b"/ok");
        buffer[3] = 0;
        assert_eq!(
            path_from_proc_pidpath(&buffer, 3).status,
            PathStatus::Available
        );
        buffer[4] = 1;
        assert_eq!(
            path_from_proc_pidpath(&buffer, 4).status,
            PathStatus::QueryFailed
        );
        buffer[4] = 0;
        buffer[1] = 0;
        assert_eq!(
            path_from_proc_pidpath(&buffer, 3).status,
            PathStatus::InvalidPath
        );
        assert_eq!(
            path_from_proc_pidpath(&buffer, 0).status,
            PathStatus::QueryFailed
        );
    }

    #[test]
    fn native_errors_map_without_errno_fallback_leaks() {
        assert_eq!(
            native_error_status_from_errno(Some(libc::EACCES)),
            PathStatus::PermissionDenied
        );
        assert_eq!(
            native_error_status_from_errno(Some(libc::EPERM)),
            PathStatus::PermissionDenied
        );
        assert_eq!(
            native_error_status_from_errno(Some(libc::ESRCH)),
            PathStatus::ProcessGone
        );
        assert_eq!(
            native_error_status_from_errno(None),
            PathStatus::QueryFailed
        );
        assert_eq!(
            native_error_status_from_errno(Some(libc::EINVAL)),
            PathStatus::QueryFailed
        );
    }
}
