//! Kernel credentials for connected macOS Unix streams.
//!
//! Credentials describe the connection establishment identity. Same-user checks
//! do not isolate agents from other processes belonging to that user. Socket
//! directory security, application grants and input ownership remain separate.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;

/// Kernel-reported effective credentials associated with a connection.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PeerCredentials {
    uid: u32,
    gid: u32,
}

impl PeerCredentials {
    /// Effective UID at connection establishment, not a client-supplied ID.
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// Effective GID at connection establishment; not used for authorization.
    pub const fn gid(self) -> u32 {
        self.gid
    }
}

impl std::fmt::Debug for PeerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PeerCredentials { .. }")
    }
}

/// Observe effective peer UID/GID without reading from or consuming the stream.
pub fn peer_credentials(stream: &UnixStream) -> io::Result<PeerCredentials> {
    credentials_for_fd(stream.as_fd())
}

/// Reject a connection unless its peer UID equals this process's effective UID.
///
/// A caller must perform this before accepting handshake or application input.
/// Successful credentials are not a workspace grant or an input-owner lease.
pub fn require_same_user(stream: &UnixStream) -> io::Result<PeerCredentials> {
    let credentials = peer_credentials(stream)?;
    require_uid(credentials, rustix::process::geteuid().as_raw())
}

fn credentials_for_fd(fd: BorrowedFd<'_>) -> io::Result<PeerCredentials> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: fd remains borrowed/live for the call. Both output pointers refer
    // to distinct initialized stack values of the ABI's exact uid_t/gid_t types.
    // getpeereid neither retains the pointers nor consumes/closes the descriptor.
    let result = unsafe { libc::getpeereid(fd.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PeerCredentials { uid, gid })
}

fn require_uid(credentials: PeerCredentials, expected_uid: u32) -> io::Result<PeerCredentials> {
    if credentials.uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket peer user does not match",
        ));
    }
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn socket_pair_reports_kernel_credentials_without_consuming_data() {
        let (mut left, mut right) = UnixStream::pair().expect("socket pair");
        left.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        right
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        right.write_all(b"check").unwrap();
        for stream in [&left, &right] {
            let peer = require_same_user(stream).expect("same-user pair");
            assert_eq!(peer.uid(), rustix::process::geteuid().as_raw());
            assert_eq!(peer.gid(), rustix::process::getegid().as_raw());
            assert_eq!(format!("{peer:?}"), "PeerCredentials { .. }");
        }
        let mut bytes = [0; 5];
        left.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"check");
    }

    #[test]
    fn non_socket_error_preserves_os_failure_and_file_ownership() {
        let file = File::open("/dev/null").unwrap();
        let error = credentials_for_fd(file.as_fd()).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOTSOCK));
        assert!(file.metadata().is_ok());
    }

    #[test]
    fn uid_mismatch_fails_closed_even_when_gid_matches() {
        let credentials = PeerCredentials { uid: 10, gid: 20 };
        assert_eq!(require_uid(credentials, 10).unwrap(), credentials);
        let error = require_uid(credentials, 11).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "socket peer user does not match");
    }

    #[test]
    #[ignore = "requires pathname Unix socket bind permission; sandbox returns EPERM"]
    fn pathname_connection_reports_same_user_in_a_private_fixture() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join("s"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Keep below macOS's Unix socket path limit, even when TMPDIR is long.
        let directory = PathBuf::from(format!(
            "/private/tmp/pl-peer-{}-{unique}",
            std::process::id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .expect("exclusive private directory");
        let fixture = Fixture(directory);
        let metadata = std::fs::symlink_metadata(&fixture.0).unwrap();
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.mode() & 0o777, 0o700);
        let socket_path = fixture.0.join("s");
        let listener = UnixListener::bind(&socket_path).expect("pathname bind");
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&socket_path).unwrap().mode() & 0o777,
            0o600
        );
        listener.set_nonblocking(true).unwrap();
        let client = UnixStream::connect(&socket_path).expect("local connect");
        // The connect completed before accept; nonblocking accept cannot hang.
        let (server, _) = listener.accept().expect("queued connection");
        for stream in [&client, &server] {
            assert_eq!(
                require_same_user(stream).unwrap().uid(),
                rustix::process::geteuid().as_raw()
            );
        }
        drop(server);
        drop(client);
        drop(listener);
    }
}
