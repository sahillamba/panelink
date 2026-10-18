//! Child-side macOS descriptor-table probe shared by focused integration fixtures.
//!
//! `proc_pidinfo(PROC_PIDLISTFDS)` supplies the descriptor numbers, and this
//! module verifies every returned slot by `fstat`. The fixture fails closed if
//! the bounded inventory fills, becomes malformed, or cannot stabilize.
#![allow(dead_code)]

use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::ptr;

pub const PROBE_ENV: &str = "FD_PROBE_CASE";
pub const SUCCESS_EXIT: i32 = 42;

const CLEAN_VALUE: &str = "clean:v1";
const LOWER_LIMIT_ENV: &str = "FD_PROBE_NOFILE_LIMIT";
const MAX_DESCRIPTORS: usize = 4096;
const MAX_STABLE_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DescriptorIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    rdevice: u64,
}

pub fn clean_value() -> OsString {
    OsString::from(CLEAN_VALUE)
}

/// Encode the stable identity of an owned descriptor for the exec child.
pub fn inherited_value(fd: RawFd) -> io::Result<OsString> {
    let identity = descriptor_identity(fd)?;
    Ok(OsString::from(format!(
        "inode:v1:{}:{}:{}:{}",
        identity.device, identity.inode, identity.mode, identity.rdevice
    )))
}

/// Run only in the exact child helper test. Returns false in the parent suite.
pub fn run_if_requested() -> bool {
    let Some(value) = std::env::var_os(PROBE_ENV) else {
        return false;
    };

    match check_child(&value) {
        Ok(result) => {
            match result.kind {
                ProbeKind::Clean => {
                    println!("\nPANELINK_FD_PROBE_OK_CLEAN_N={:04}", result.open_count)
                }
                ProbeKind::Inherited => {
                    println!(
                        "\nPANELINK_FD_PROBE_OK_INHERITED_N={:04}",
                        result.open_count
                    )
                }
            }
            std::process::exit(SUCCESS_EXIT);
        }
        Err(reason) => {
            eprintln!("\nPANELINK_FD_PROBE_FAILED_{reason}");
            std::process::exit(SUCCESS_EXIT + 1);
        }
    }
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Clean,
    Inherited,
}

struct ProbeResult {
    kind: ProbeKind,
    open_count: usize,
}

fn check_child(value: &std::ffi::OsStr) -> Result<ProbeResult, &'static str> {
    lower_child_limit_if_requested()?;
    let (kind, expected) = if value.as_bytes() == CLEAN_VALUE.as_bytes() {
        (ProbeKind::Clean, None)
    } else {
        (ProbeKind::Inherited, Some(parse_identity(value)?))
    };

    let mut observed_expected = false;
    let mut stable_count = None;
    for _ in 0..MAX_STABLE_ATTEMPTS {
        let descriptors = descriptor_numbers()?;
        if descriptors.iter().any(|fd| *fd < 0) {
            return Err("invalid_descriptor");
        }
        let mut sorted = descriptors.clone();
        sorted.sort_unstable();
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("duplicate_descriptor");
        }

        let mut open_count = 0;
        for fd in sorted.iter().copied() {
            match descriptor_identity(fd) {
                Ok(_) if fd <= 2 => open_count += 1,
                Ok(identity) if Some(identity) == expected => {
                    observed_expected = true;
                    open_count += 1;
                }
                Ok(_) if matches!(kind, ProbeKind::Clean) && fd > 2 => {
                    return Err("unexpected_descriptor");
                }
                Ok(_) => open_count += 1,
                Err(error) if error.raw_os_error() == Some(libc::EBADF) => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    return Err("descriptor_identity_encoding");
                }
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => {
                    return Err("descriptor_stat_interrupted");
                }
                Err(error) if error.raw_os_error() == Some(libc::EACCES) => {
                    return Err("descriptor_stat_access");
                }
                Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                    return Err("descriptor_stat_permission");
                }
                Err(_) => return Err("descriptor_stat_other"),
            }
        }

        let after = descriptor_numbers()?;
        let mut after_sorted = after;
        after_sorted.sort_unstable();
        if sorted == after_sorted {
            stable_count = Some(open_count);
            break;
        }
    }
    let Some(open_count) = stable_count else {
        return Err("unstable_table");
    };

    if !stdio_share_terminal()? {
        return Err("stdio_terminal");
    }
    if expected.is_some() && !observed_expected {
        return Err("inherited_identity_missing");
    }

    Ok(ProbeResult { kind, open_count })
}

fn parse_identity(value: &std::ffi::OsStr) -> Result<DescriptorIdentity, &'static str> {
    let text = value.to_str().ok_or("invalid_identity")?;
    let mut fields = text.split(':');
    if fields.next() != Some("inode") || fields.next() != Some("v1") {
        return Err("invalid_identity");
    }
    let device = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or("invalid_identity")?;
    let inode = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or("invalid_identity")?;
    let mode = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or("invalid_identity")?;
    let rdevice = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or("invalid_identity")?;
    if fields.next().is_some() {
        return Err("invalid_identity");
    }
    Ok(DescriptorIdentity {
        device,
        inode,
        mode,
        rdevice,
    })
}

fn descriptor_numbers() -> Result<Vec<RawFd>, &'static str> {
    let bytes_per_entry = size_of::<libc::proc_fdinfo>();
    if bytes_per_entry != libc::PROC_PIDLISTFD_SIZE as usize || bytes_per_entry == 0 {
        return Err("descriptor_abi");
    }
    let byte_capacity = MAX_DESCRIPTORS
        .checked_mul(bytes_per_entry)
        .ok_or("descriptor_capacity")?;
    let byte_capacity = i32::try_from(byte_capacity).map_err(|_| "descriptor_capacity")?;
    let word_capacity = usize::try_from(byte_capacity)
        .map_err(|_| "descriptor_capacity")?
        .div_ceil(size_of::<u64>());
    let mut words = vec![0_u64; word_capacity];

    // SAFETY: `words` is suitably aligned and has at least `byte_capacity`
    // writable bytes; the PID, flavor and byte count match Apple's libproc API.
    let copied = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDLISTFDS,
            0,
            words.as_mut_ptr().cast(),
            byte_capacity,
        )
    };
    if copied <= 0 || copied > byte_capacity || copied % bytes_per_entry as i32 != 0 {
        return Err("descriptor_inventory");
    }
    if copied == byte_capacity {
        return Err("descriptor_inventory_full");
    }

    let count = copied as usize / bytes_per_entry;
    let entries = words.as_ptr().cast::<libc::proc_fdinfo>();
    let mut descriptors = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: proc_pidinfo reported `count` complete entries in `words`.
        let entry = unsafe { ptr::read(entries.add(index)) };
        descriptors.push(entry.proc_fd);
    }
    Ok(descriptors)
}

fn descriptor_identity(fd: RawFd) -> io::Result<DescriptorIdentity> {
    // SAFETY: `fstat` only writes to the initialized `stat` output structure;
    // callers pass a descriptor number obtained from the OS or an owned handle.
    unsafe {
        let mut status = std::mem::zeroed::<libc::stat>();
        if libc::fstat(fd, &mut status) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(DescriptorIdentity {
            device: u64::from(u32::from_ne_bytes(status.st_dev.to_ne_bytes())),
            inode: status.st_ino,
            mode: u32::from(status.st_mode),
            rdevice: u64::from(u32::from_ne_bytes(status.st_rdev.to_ne_bytes())),
        })
    }
}

fn stdio_share_terminal() -> Result<bool, &'static str> {
    let mut session = None;
    let mut identity = None;
    for fd in 0..=2 {
        // SAFETY: tcgetsid reads tty state for this inherited descriptor and
        // does not retain pointers or change descriptor flags.
        let current = unsafe { libc::tcgetsid(fd) };
        // SAFETY: getsid reads the calling child's session ID without changing
        // process state or retaining pointers.
        let child_session = unsafe { libc::getsid(0) };
        // SAFETY: isatty only inspects the inherited descriptor's tty state.
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        if current < 0 || child_session < 0 || current != child_session || !is_tty {
            return Ok(false);
        }
        if session.is_some_and(|expected| expected != current) {
            return Ok(false);
        }
        session = Some(current);
        let current_identity = descriptor_identity(fd).map_err(|_| "stdio_stat")?;
        if identity.is_some_and(|expected| expected != current_identity) {
            return Ok(false);
        }
        identity = Some(current_identity);
    }
    Ok(session.is_some() && identity.is_some())
}

fn lower_child_limit_if_requested() -> Result<(), &'static str> {
    let Some(value) = std::env::var_os(LOWER_LIMIT_ENV) else {
        return Ok(());
    };
    let requested = value
        .to_str()
        .and_then(|text| text.parse::<libc::rlim_t>().ok())
        .ok_or("invalid_child_limit")?;
    let mut current = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: getrlimit writes one initialized rlimit structure to this pointer.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, current.as_mut_ptr()) } != 0 {
        return Err("child_limit_read");
    }
    // SAFETY: getrlimit succeeded and initialized the output structure.
    let mut limits = unsafe { current.assume_init() };
    if requested == 0 || requested > limits.rlim_cur {
        return Err("child_limit_invalid");
    }
    limits.rlim_cur = requested;
    // SAFETY: setrlimit receives a valid resource and initialized structure;
    // this call changes only this child process's soft limit.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limits) } != 0 {
        return Err("child_limit_set");
    }
    let mut observed = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: getrlimit writes the child process's updated limit into a valid
    // output structure.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, observed.as_mut_ptr()) } != 0 {
        return Err("child_limit_verify");
    }
    // SAFETY: the preceding successful getrlimit initialized the structure.
    if unsafe { observed.assume_init() }.rlim_cur != requested {
        return Err("child_limit_mismatch");
    }
    Ok(())
}
