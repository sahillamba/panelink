#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::env;
use std::ffi::CString;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{self, Mode, OFlags};
use rustix::io::{self as rio, FdFlags};
use rustix::termios::{self, Winsize};

const TEST_NAME: &str = "atomic_pty_opener_has_cloexec_private_io_and_window_size";
const PROBE_ENV: &str = "PANELINK_P3_ATOMIC_PTY_PROBE_V1";
const CHILD_PROBE_EXIT: i32 = 42;
const PTY_PATH_BUFFER_LEN: usize = 128;
const PROBE_DEADLINE: Duration = Duration::from_secs(5);
const IO_DEADLINE: Duration = Duration::from_secs(2);
const IO_BUFFER_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    rdevice: u64,
    mode: u64,
}

impl FileIdentity {
    fn from_stat(stat: &libc::stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
            rdevice: stat.st_rdev as u64,
            mode: stat.st_mode as u64,
        }
    }

    fn encode(self) -> String {
        format!(
            "{},{},{},{}",
            self.device, self.inode, self.rdevice, self.mode
        )
    }

    fn decode(value: &str) -> Option<Self> {
        let mut fields = value.split(',');
        let device = fields.next()?.parse().ok()?;
        let inode = fields.next()?.parse().ok()?;
        let rdevice = fields.next()?.parse().ok()?;
        let mode = fields.next()?.parse().ok()?;
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            device,
            inode,
            rdevice,
            mode,
        })
    }
}

fn posix_openpt_cloexec() -> io::Result<OwnedFd> {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: `posix_openpt` takes an integer flag set and returns either a
    // new descriptor or -1. On success, ownership is transferred exactly once
    // to `OwnedFd` below. The CLOEXEC bit is passed to the creation call.
    let raw_fd = unsafe { libc::posix_openpt(flags) };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `posix_openpt` returned a fresh, valid descriptor which has not
    // yet been wrapped or closed.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

fn duplicate_without_cloexec(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fd` is a live PTY master owned by this test. F_DUPFD creates a
    // new descriptor at or above zero and, by contract, does not copy
    // FD_CLOEXEC. The returned descriptor is wrapped exactly once below.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD, 0) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful F_DUPFD returned a new descriptor owned by caller.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn bounded_slave_name(master: RawFd) -> io::Result<CString> {
    let mut bytes = [0_u8; PTY_PATH_BUFFER_LEN];
    // SAFETY: `master` is an open PTY master owned by this test. The
    // `TIOCPTYGNAME` request encodes a 128-byte output buffer, and `bytes` is
    // exactly that size and writable for the duration of the call.
    let result = unsafe {
        libc::ioctl(
            master,
            libc::TIOCPTYGNAME as libc::c_ulong,
            bytes.as_mut_ptr().cast::<libc::c_char>(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }

    let Some(length) = bytes.iter().position(|byte| *byte == 0) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PTY slave name was not NUL-terminated within 128 bytes",
        ));
    };
    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PTY slave name was empty",
        ));
    }
    CString::new(&bytes[..length]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "PTY slave name contained an unexpected NUL byte",
        )
    })
}

fn grant_unlock(master: RawFd) -> io::Result<()> {
    // SAFETY: `master` is an open PTY master owned by this test. These libc
    // wrappers operate only on that descriptor and have no pointer arguments.
    if unsafe { libc::grantpt(master) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same live PTY master, still owned by this test.
    if unsafe { libc::unlockpt(master) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn raw_fd_identity(raw_fd: RawFd) -> io::Result<FileIdentity> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` points to writable, correctly aligned storage for
    // `libc::stat`; `fstat` initializes it on success. Callers either pass an
    // owned PTY descriptor or probe one descriptor number in the exec child.
    if unsafe { libc::fstat(raw_fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful `fstat` initialized the full `libc::stat` value.
    let stat = unsafe { stat.assume_init() };
    Ok(FileIdentity::from_stat(&stat))
}

fn assert_cloexec(fd: &OwnedFd, label: &str) {
    let flags = rio::fcntl_getfd(fd)
        .unwrap_or_else(|error| panic!("could not read {label} descriptor flags: {error}"));
    assert!(
        flags.contains(FdFlags::CLOEXEC),
        "{label} descriptor was returned without FD_CLOEXEC"
    );
}

fn encode_probe(checkpoint: &str, expect_inherited: bool, descriptors: &[&OwnedFd]) -> String {
    let records = descriptors
        .iter()
        .map(|fd| {
            format!(
                "{}:{}",
                fd.as_raw_fd(),
                raw_fd_identity(fd.as_raw_fd())
                    .expect("fstat PTY descriptor")
                    .encode()
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!("{checkpoint}|{}|{records}", u8::from(expect_inherited))
}

fn run_child_probe(
    checkpoint: &str,
    expect_inherited: bool,
    descriptors: &[&OwnedFd],
) -> io::Result<()> {
    let executable = env::current_exe()?;
    let child = Command::new(executable)
        .args(["--exact", TEST_NAME])
        .env(
            PROBE_ENV,
            encode_probe(checkpoint, expect_inherited, descriptors),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = ChildGuard(child);
    let deadline = Instant::now() + PROBE_DEADLINE;

    loop {
        match child.0.try_wait()? {
            Some(status) if status.code() == Some(CHILD_PROBE_EXIT) => return Ok(()),
            Some(status) => {
                return Err(io::Error::other(format!(
                    "exec-child descriptor probe failed at checkpoint {checkpoint}: {status}"
                )))
            }
            None if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("exec-child descriptor probe timed out at {checkpoint}"),
                ));
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn parse_probe_record(record: &str) -> Option<(RawFd, FileIdentity)> {
    let (fd, identity) = record.split_once(':')?;
    Some((fd.parse().ok()?, FileIdentity::decode(identity)?))
}

fn run_child_descriptor_probe(value: &str) {
    let Some((checkpoint, remainder)) = value.split_once('|') else {
        panic!("malformed PTY descriptor probe marker");
    };
    let Some((expect_inherited, records)) = remainder.split_once('|') else {
        panic!("malformed PTY descriptor probe expectation");
    };
    let expect_inherited = match expect_inherited {
        "0" => false,
        "1" => true,
        _ => panic!("invalid PTY descriptor probe expectation"),
    };
    assert!(!checkpoint.is_empty(), "empty PTY probe checkpoint");
    for record in records.split(';') {
        let Some((raw_fd, expected)) = parse_probe_record(record) else {
            panic!("malformed PTY descriptor identity record");
        };
        match raw_fd_identity(raw_fd) {
            Ok(actual) if expect_inherited => assert_eq!(
                actual, expected,
                "known non-CLOEXEC descriptor was not found after exec at {checkpoint}"
            ),
            Ok(actual) => assert_ne!(
                actual, expected,
                "PTY descriptor identity survived exec at checkpoint {checkpoint}"
            ),
            Err(error) if error.raw_os_error() == Some(libc::EBADF) && !expect_inherited => {}
            Err(error) if error.raw_os_error() == Some(libc::EBADF) => {
                panic!("known non-CLOEXEC descriptor was absent after exec at {checkpoint}")
            }
            Err(error) => {
                panic!("could not inspect descriptor slot at checkpoint {checkpoint}: {error}")
            }
        }
    }
    // A successful helper must prove that the exact test ran. Cargo's test
    // harness exits zero when an `--exact` filter matches no tests, so zero is
    // deliberately not the protocol success value.
    std::process::exit(CHILD_PROBE_EXIT);
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = fs::fcntl_getfl(fd)?;
    fs::fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(io::Error::other)
}

fn write_all_before(fd: &OwnedFd, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        match rio::write(fd, bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "PTY write returned zero bytes",
                ))
            }
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error == rio::Errno::AGAIN => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "PTY write exceeded its deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    Ok(())
}

fn read_until_before(fd: &OwnedFd, marker: &[u8], deadline: Instant) -> io::Result<()> {
    let mut captured = Vec::with_capacity(IO_BUFFER_LIMIT);
    let mut chunk = [0_u8; 64];
    loop {
        match rio::read(fd, &mut chunk) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "PTY reached EOF before the expected bytes arrived",
                ))
            }
            Ok(read) => {
                if captured.len().saturating_add(read) > IO_BUFFER_LIMIT {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "PTY test exceeded its 256-byte capture bound",
                    ));
                }
                captured.extend_from_slice(&chunk[..read]);
                if captured
                    .windows(marker.len())
                    .any(|window| window == marker)
                {
                    return Ok(());
                }
            }
            Err(error) if error == rio::Errno::AGAIN => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "PTY did not deliver expected bytes before its deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(io::Error::other(error)),
        }
    }
}

#[test]
fn atomic_pty_opener_has_cloexec_private_io_and_window_size() {
    if let Some(value) = env::var_os(PROBE_ENV) {
        let Some(value) = value.to_str() else {
            panic!("PTY descriptor probe marker was not UTF-8");
        };
        run_child_descriptor_probe(value);
        return;
    }

    let master = posix_openpt_cloexec().expect("posix_openpt with O_CLOEXEC");
    assert_cloexec(&master, "master");
    let non_cloexec_control =
        duplicate_without_cloexec(master.as_raw_fd()).expect("duplicate owned PTY master");
    assert!(
        !rio::fcntl_getfd(&non_cloexec_control)
            .expect("read control duplicate flags")
            .contains(FdFlags::CLOEXEC),
        "F_DUPFD control descriptor must be inheritable"
    );
    run_child_probe("known-inheritance-control", true, &[&non_cloexec_control])
        .expect("positive control for descriptor identity probe");
    drop(non_cloexec_control);
    run_child_probe("master-only", false, &[&master]).expect("master-only exec probe");

    grant_unlock(master.as_raw_fd()).expect("grant and unlock PTY slave");
    let slave_name = bounded_slave_name(master.as_raw_fd()).expect("bounded PTY slave name");
    let slave = fs::open(
        &slave_name,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("open slave with O_CLOEXEC");
    assert_cloexec(&master, "master");
    assert_cloexec(&slave, "slave");
    run_child_probe("master-and-slave", false, &[&master, &slave])
        .expect("master-and-slave exec probe");

    let expected_size = Winsize {
        ws_row: 37,
        ws_col: 101,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    termios::tcsetwinsize(&slave, expected_size).expect("set PTY window size");
    assert_eq!(
        termios::tcgetwinsize(&master).expect("read PTY window size"),
        expected_size,
        "master and slave must share the private PTY window size"
    );

    set_nonblocking(&master).expect("make PTY master nonblocking");
    set_nonblocking(&slave).expect("make PTY slave nonblocking");
    let io_deadline = Instant::now() + IO_DEADLINE;
    const TO_MASTER: &[u8] = b"panelink-master-check";
    write_all_before(&slave, TO_MASTER, io_deadline).expect("write slave to master");
    read_until_before(&master, TO_MASTER, io_deadline).expect("read slave bytes from master");

    const TO_SLAVE: &[u8] = b"panelink-slave-check\n";
    write_all_before(&master, TO_SLAVE, io_deadline).expect("write master to slave");
    read_until_before(&slave, TO_SLAVE, io_deadline).expect("read master bytes from slave");
}
