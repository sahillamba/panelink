//! macOS/aarch64 PTY pair creation with close-on-exec set at descriptor creation.
//!
//! This module changes only PTY pair opening. It does not coordinate unrelated
//! process spawns or make the other macOS socket/pipe creation paths atomic.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use rustix::fs::{self, Mode, OFlags};
use rustix::termios::{tcsetwinsize, Winsize};

use super::PtyPair;

const PTY_NAME_BUFFER_LEN: usize = 128;

pub(super) fn open_pty(winsize: &Winsize) -> io::Result<PtyPair> {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: `posix_openpt` receives only its integer flag set. On success it
    // returns a fresh descriptor, and ownership is transferred once below.
    // O_CLOEXEC is supplied as part of creation; unsupported flags fail closed.
    let raw_controller = unsafe { libc::posix_openpt(flags) };
    if raw_controller < 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `posix_openpt` returned a fresh, valid descriptor that has not
    // been wrapped or closed by any other code.
    let controller = unsafe { OwnedFd::from_raw_fd(raw_controller) };
    grantpt(controller.as_raw_fd())?;
    unlockpt(controller.as_raw_fd())?;

    let slave_name = bounded_slave_name(controller.as_raw_fd())?;
    let user = fs::open(
        &slave_name,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;

    // The old `openpty(None, Some(winsize))` passes a null termios pointer.
    // Keep the PTY's platform default line discipline by leaving it untouched.
    tcsetwinsize(&user, *winsize).map_err(io::Error::from)?;

    Ok(PtyPair { controller, user })
}

fn grantpt(controller: RawFd) -> io::Result<()> {
    // SAFETY: `controller` is the live PTY master owned by `open_pty`; grantpt
    // takes no pointers and does not transfer descriptor ownership.
    if unsafe { libc::grantpt(controller) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn unlockpt(controller: RawFd) -> io::Result<()> {
    // SAFETY: `controller` is the live PTY master owned by `open_pty`; unlockpt
    // takes no pointers and does not transfer descriptor ownership.
    if unsafe { libc::unlockpt(controller) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn bounded_slave_name(controller: RawFd) -> io::Result<CString> {
    let mut bytes = [0_u8; PTY_NAME_BUFFER_LEN];
    // SAFETY: the private TIOCPTYGNAME request writes at most its documented
    // 128-byte buffer to `bytes`, which is writable for this call's duration.
    let result = unsafe {
        libc::ioctl(
            controller,
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

    CString::new(bytes[..length].to_vec()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "PTY slave name contained an unexpected NUL byte",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::io::{fcntl_getfd, FdFlags};
    use rustix::termios::{tcgetattr, tcgetwinsize, LocalModes};

    #[test]
    fn pair_is_cloexec_with_the_requested_size_and_default_line_discipline() {
        let requested = Winsize {
            ws_row: 19,
            ws_col: 71,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pair = open_pty(&requested).expect("open production PTY pair");

        for (label, descriptor) in [("controller", &pair.controller), ("user", &pair.user)] {
            let flags = fcntl_getfd(descriptor)
                .unwrap_or_else(|error| panic!("read {label} descriptor flags: {error}"));
            assert!(
                flags.contains(FdFlags::CLOEXEC),
                "{label} descriptor must be CLOEXEC when returned"
            );
            assert_eq!(
                tcgetwinsize(descriptor).expect("read PTY window size"),
                requested,
                "the requested size must be visible from both PTY ends"
            );
        }

        let termios = tcgetattr(&pair.user).expect("read default slave termios");
        assert!(termios.local_modes.contains(LocalModes::ICANON));
        assert!(termios.local_modes.contains(LocalModes::ECHO));
    }
}
