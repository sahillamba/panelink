#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "support/fd_probe.rs"]
mod fd_probe;

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::{Environment, ResolvedLaunch};
use panelink::terminal::{ScreenSnapshot, TerminalSession, TerminalSize};

const CHILD_TEST_NAME: &str = "child_probe_helper";
const CHILD_LIMIT_ENV: &str = "FD_PROBE_NOFILE_LIMIT";
const CHILD_SOFT_LIMIT: &str = "16";
const CHILD_FD_MINIMUM: RawFd = 64;
const PROBE_DEADLINE: Duration = Duration::from_secs(4);
const PUMP_INTERVAL: Duration = Duration::from_millis(5);

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
static ROOT_FIXTURE_LOCK: Mutex<()> = Mutex::new(());

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp")
                .join(format!("panelink-p3-fd-{}-{serial}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn environment(probe_value: OsString, lower_child_limit: bool) -> Environment {
    let mut environment = Environment::new();
    environment.insert(OsString::from("TERM"), OsString::from("panelink-probe"));
    environment.insert(OsString::from(fd_probe::PROBE_ENV), probe_value);
    if lower_child_limit {
        environment.insert(
            OsString::from(CHILD_LIMIT_ENV),
            OsString::from(CHILD_SOFT_LIMIT),
        );
    }
    environment
}

fn self_launch(cwd: &Path) -> io::Result<ResolvedLaunch> {
    let executable = std::env::current_exe()?;
    Ok(ResolvedLaunch {
        requested_path: executable.clone(),
        executable_path: executable,
        working_directory: cwd.to_path_buf(),
    })
}

fn spawn_probe(cwd: &Path, probe_value: OsString, lower_child_limit: bool) -> TerminalSession {
    let launch = self_launch(cwd).expect("resolve this integration-test executable");
    let arguments = [
        OsString::from("--exact"),
        OsString::from(CHILD_TEST_NAME),
        OsString::from("--nocapture"),
    ];
    TerminalSession::spawn(
        &launch,
        &arguments,
        &environment(probe_value, lower_child_limit),
        TerminalSize::new(10, 100).expect("valid descriptor-probe terminal size"),
    )
    .expect("spawn the exact helper test in a real Panelink PTY")
}

fn snapshot_text(snapshot: &ScreenSnapshot) -> String {
    let mut text = String::new();
    for row in 0..snapshot.rows() {
        for column in 0..snapshot.columns() {
            if let Some(cell) = snapshot.cell(row, column) {
                text.push_str(cell.text());
            }
        }
        text.push('\n');
    }
    text
}

fn finish_probe(session: &mut TerminalSession) -> (String, i32) {
    let deadline = Instant::now() + PROBE_DEADLINE;
    loop {
        let outcome = session
            .pump()
            .expect("pump the bounded child descriptor probe");
        if outcome.pty_eof && outcome.child_exited {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "descriptor-probe child exceeded its PTY deadline"
        );
        thread::sleep(PUMP_INTERVAL);
    }

    let status = session
        .exit_status()
        .expect("collect the descriptor-probe child status");
    (
        snapshot_text(session.snapshot()),
        status.code().expect("descriptor probe exits normally"),
    )
}

fn assert_success_marker(output: &str, prefix: &str) -> usize {
    assert_eq!(
        output.matches(prefix).count(),
        1,
        "child must print one fixed success marker after the scan"
    );
    let marker_start = output.find(prefix).expect("success marker found") + prefix.len();
    let count = output[marker_start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    assert_eq!(count.len(), 4, "open-fd count is fixed-width metadata");
    let count = count.parse::<usize>().expect("numeric open-fd count");
    assert!((3..4096).contains(&count));
    count
}

fn failure_metadata(output: &str) -> &str {
    const PREFIX: &str = "PANELINK_FD_PROBE_FAILED_";
    let Some(start) = output.find(PREFIX).map(|index| index + PREFIX.len()) else {
        return "no_failure_marker";
    };
    let suffix = &output[start..];
    let length = suffix
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    if length == 0 {
        "empty_failure_marker"
    } else {
        &suffix[..length]
    }
}

fn assert_cloexec(fd: RawFd) {
    // SAFETY: F_GETFD only reads the live descriptor flag for this owned test
    // fixture; it does not change flags or transfer ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags >= 0, "could not read a controlled fixture's fd flags");
    assert_ne!(flags & libc::FD_CLOEXEC, 0, "fixture fd must start CLOEXEC");
}

fn duplicate_inheritable(fd: RawFd) -> OwnedFd {
    // SAFETY: `fd` is an owned regular-file descriptor. Darwin F_DUPFD returns
    // a new descriptor with FD_CLOEXEC clear; ownership is wrapped below.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD, CHILD_FD_MINIMUM) };
    assert!(duplicate >= CHILD_FD_MINIMUM, "F_DUPFD failed for fixture");
    // SAFETY: successful F_DUPFD returned a fresh descriptor owned by caller.
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
    // SAFETY: F_GETFD only reads the newly owned duplicate's descriptor flags.
    let flags = unsafe { libc::fcntl(duplicate.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0, "could not read the duplicated fixture fd flags");
    assert_eq!(
        flags & libc::FD_CLOEXEC,
        0,
        "positive control must be inheritable"
    );
    duplicate
}

fn assert_parent_limit_supports_high_fd() {
    let mut limits = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: getrlimit initializes the supplied rlimit output structure.
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limits.as_mut_ptr()) },
        0,
        "read the parent process's fd limit"
    );
    // SAFETY: getrlimit succeeded and initialized the output structure.
    let limits = unsafe { limits.assume_init() };
    assert!(
        limits.rlim_cur > CHILD_FD_MINIMUM as libc::rlim_t,
        "host fd soft limit must permit the high-fd fixture"
    );
}

#[test]
fn child_probe_helper() {
    let _ = fd_probe::run_if_requested();
}

#[test]
fn clean_exec_child_has_only_the_three_shared_pty_stdio_descriptors() {
    let _serial = ROOT_FIXTURE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = TempDir::new().expect("create private descriptor fixture directory");
    let file_path = directory.path().join("parent-cloexec-file");
    let cloexec_file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(file_path)
        .expect("open controlled CLOEXEC file");
    let (socket_a, socket_b) = UnixStream::pair().expect("create controlled CLOEXEC socketpair");
    assert_cloexec(cloexec_file.as_raw_fd());
    assert_cloexec(socket_a.as_raw_fd());
    assert_cloexec(socket_b.as_raw_fd());

    let mut child = spawn_probe(directory.path(), fd_probe::clean_value(), false);
    let (output, code) = finish_probe(&mut child);
    assert_eq!(
        code,
        fd_probe::SUCCESS_EXIT,
        "clean child probe must use its explicit exit code; reason={}",
        failure_metadata(&output)
    );
    let count = assert_success_marker(&output, "PANELINK_FD_PROBE_OK_CLEAN_N=");
    assert_eq!(count, 3, "the exec child should retain only PTY stdio");
    eprintln!("PANELINK_FD_TEST_CLEAN_COUNT={count:04}");

    // Keep each controlled CLOEXEC fixture open through the child exit so the
    // scan proves they were omitted by exec, rather than closed by the parent.
    assert!(cloexec_file.metadata().is_ok());
    assert!(socket_a.peer_addr().is_ok());
    assert!(socket_b.peer_addr().is_ok());
}

#[test]
fn inherited_identity_is_detected_above_the_child_soft_fd_limit() {
    let _serial = ROOT_FIXTURE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_parent_limit_supports_high_fd();
    let directory = TempDir::new().expect("create private descriptor fixture directory");
    let file_path = directory.path().join("inheritable-control-file");
    let parent_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(file_path)
        .expect("open positive-control file");
    assert_cloexec(parent_file.as_raw_fd());
    let inheritable = duplicate_inheritable(parent_file.as_raw_fd());
    assert!(inheritable.as_raw_fd() >= CHILD_FD_MINIMUM);
    let value = fd_probe::inherited_value(inheritable.as_raw_fd())
        .expect("encode the duplicate's stable fstat identity");

    let mut child = spawn_probe(directory.path(), value, true);
    drop(inheritable);
    let (output, code) = finish_probe(&mut child);
    assert_eq!(
        code,
        fd_probe::SUCCESS_EXIT,
        "positive child probe must use its explicit exit code; reason={}",
        failure_metadata(&output)
    );
    let count = assert_success_marker(&output, "PANELINK_FD_PROBE_OK_INHERITED_N=");
    assert!(
        count >= 4,
        "the child inventory includes stdio and the expected fd"
    );
    eprintln!("PANELINK_FD_TEST_INHERITED_COUNT={count:04}");

    // The CLOEXEC original stayed open in the parent; only its non-CLOEXEC
    // F_DUPFD duplicate was expected to survive the exec.
    assert!(parent_file.metadata().is_ok());
}

#[test]
fn clean_probe_rejects_a_deliberately_inheritable_descriptor() {
    let _serial = ROOT_FIXTURE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = TempDir::new().expect("create private descriptor fixture directory");
    let file_path = directory.path().join("negative-control-file");
    let parent_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(file_path)
        .expect("open negative-control file");
    assert_cloexec(parent_file.as_raw_fd());
    let inheritable = duplicate_inheritable(parent_file.as_raw_fd());

    let mut child = spawn_probe(directory.path(), fd_probe::clean_value(), false);
    drop(inheritable);
    let (output, code) = finish_probe(&mut child);
    assert_eq!(
        code,
        fd_probe::SUCCESS_EXIT + 1,
        "clean probe must reject the deliberate leak rather than accept its exit status"
    );
    assert_eq!(
        failure_metadata(&output),
        "unexpected_descriptor",
        "the child must report the bounded clean-scan failure marker"
    );
    assert!(
        !output.contains("PANELINK_FD_PROBE_OK_CLEAN_N="),
        "failed clean scan must not print a success marker"
    );
    eprintln!("PANELINK_FD_TEST_NEGATIVE_CONTROL=unexpected_descriptor");
    assert!(parent_file.metadata().is_ok());
}
