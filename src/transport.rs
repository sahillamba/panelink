//! Authenticated, bounded transport over a private macOS Unix socket.
//!
//! This module is intentionally limited to the first local connection boundary:
//! kernel same-user authentication, v1.0 control framing, finite nonblocking
//! deadlines, and a private pathname listener. Same-user checks do not isolate
//! agents or replace the runtime's grants, target checks, or input ownership.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::event_protocol::{decode_event, EVENT_PAYLOAD_BYTES};
use crate::peer_identity::require_same_user;
use crate::protocol_wire::{
    decode_header, encode_frame, Frame, FrameHeader, MessageKind, WireError, HEADER_BYTES,
    MAX_CONTROL_PAYLOAD_BYTES,
};
use rustix::fs::{
    chmodat, fchmod, fstat, mkdirat, openat, statat, unlinkat, AtFlags, FileType, Mode, OFlags,
};
use rustix::io::Errno;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const SOCKET_BASENAME: &str = "control.sock";
const DIRECTORY_MODE: u16 = 0o700;
const SOCKET_MODE: u16 = 0o600;
const POLL_INTERVAL: Duration = Duration::from_millis(1);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Prepare a protected directory before generation initialization or binding.
/// Ancestors are validated before creation; chmod acts on the pinned descriptor.
pub fn prepare_directory(directory: &Path) -> io::Result<()> {
    let directory = validate_path(directory)?;
    let (_opened, identity) = open_private_directory(&directory, true)?;
    verify_directory_path(&directory, identity)
}

/// A complete owned frame returned by the authenticated stream.
///
/// The payload is never included in Debug output. Callers can inspect it only
/// through the explicit borrowed accessor.
pub struct OwnedFrame {
    header: FrameHeader,
    payload: Vec<u8>,
}

impl OwnedFrame {
    /// Return validated message kind, request identifier, and payload length.
    pub const fn header(&self) -> FrameHeader {
        self.header
    }

    /// Return the validated message kind.
    pub const fn kind(&self) -> MessageKind {
        self.header.kind()
    }

    /// Return the request identifier from the validated header.
    pub const fn request_id(&self) -> u64 {
        self.header.request_id()
    }

    /// Borrow the owned payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Transfer the validated body to the owner queue without a second copy.
    pub(crate) fn into_payload(self) -> Vec<u8> {
        self.payload
    }
}

impl fmt::Debug for OwnedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedFrame")
            .field("kind", &self.header.kind())
            .field("request_id", &self.header.request_id())
            .field("payload_len", &self.payload.len())
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// A same-user-authenticated Unix stream with bounded v1.0 frame I/O.
///
/// Construction checks the kernel peer UID before changing the stream mode or
/// allowing any frame bytes to be read. The stream stays private; this type
/// does not expose raw descriptors, clones, or unauthenticated readers.
pub struct AuthenticatedStream {
    stream: UnixStream,
    poisoned: bool,
    metadata_events_enabled: bool,
}

impl AuthenticatedStream {
    /// Connect to the validated private endpoint using only nonblocking calls.
    ///
    /// Path and peer checks complete before any protocol bytes are exchanged.
    /// Connection failure never causes a request or input to be retransmitted.
    pub fn connect(directory: &Path, deadline: Instant) -> io::Result<Self> {
        use rustix::net::{self, sockopt, AddressFamily, SocketAddrUnix, SocketType};
        check_deadline(deadline)?;
        let directory = validate_path(directory)?;
        let (directory_fd, identity) = open_private_directory(&directory, false)?;
        verify_directory_path(&directory, identity)?;
        let socket_identity = entry_identity(&directory_fd, SOCKET_BASENAME)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "control socket is absent"))?;
        verify_socket_permissions(&directory_fd, socket_identity)?;
        let address = SocketAddrUnix::new(directory.join(SOCKET_BASENAME))?;
        let fd = net::socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
        // macOS does not expose SOCK_NONBLOCK/SOCK_CLOEXEC flags. Set both
        // before connect or exposing the descriptor; startup forks must be
        // serialized before client worker creation in this prototype.
        rustix::fs::fcntl_setfl(&fd, rustix::fs::fcntl_getfl(&fd)? | OFlags::NONBLOCK)?;
        rustix::io::fcntl_setfd(
            &fd,
            rustix::io::fcntl_getfd(&fd)? | rustix::io::FdFlags::CLOEXEC,
        )?;
        sockopt::set_socket_nosigpipe(&fd, true)?;
        check_deadline(deadline)?;
        match net::connect(&fd, &address) {
            Ok(()) => {}
            Err(Errno::INPROGRESS) => loop {
                check_deadline(deadline)?;
                sockopt::socket_error(&fd)??;
                match net::getpeername(&fd) {
                    Ok(Some(_)) => break,
                    Ok(None) | Err(Errno::NOTCONN) => wait_for_readiness(deadline)?,
                    Err(error) => return Err(error.into()),
                }
            },
            Err(error) => return Err(error.into()),
        }
        check_deadline(deadline)?;
        verify_directory_path(&directory, identity)?;
        verify_socket_permissions(&directory_fd, socket_identity)?;
        Self::new(UnixStream::from(fd))
    }

    /// Authenticate the kernel-reported peer UID, then make the stream
    /// nonblocking.
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        require_same_user(&stream)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            poisoned: false,
            metadata_events_enabled: false,
        })
    }

    /// Return whether an incomplete, failed, malformed, or timed-out operation
    /// has made this connection unusable.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Enable fixed-size metadata Event frames after the caller has validated
    /// that the peer's Welcome selected the Events capability.
    ///
    /// Snapshot frames remain unsupported. Enabling Events on an unusable
    /// stream fails without changing its state.
    pub fn enable_metadata_events(&mut self) -> io::Result<()> {
        if self.poisoned {
            return Err(poisoned_error());
        }
        self.metadata_events_enabled = true;
        Ok(())
    }

    /// Read one complete initial-protocol frame by an absolute deadline.
    ///
    /// The fixed 24-byte header is read into stack storage and validated before
    /// a payload allocation is attempted. Only Hello, Welcome, Request,
    /// Response, and Error are accepted before a future explicit runtime
    /// capability enables other message kinds.
    pub fn read_frame(&mut self, deadline: Instant) -> io::Result<OwnedFrame> {
        if self.poisoned {
            return Err(poisoned_error());
        }

        let result = self.read_frame_inner(deadline);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn read_frame_inner(&mut self, deadline: Instant) -> io::Result<OwnedFrame> {
        let mut encoded_header = [0_u8; HEADER_BYTES];
        read_exact_until(&mut self.stream, &mut encoded_header, deadline)?;

        let header = decode_header(&encoded_header).map_err(wire_error)?;
        let kind = header.kind();
        let event_enabled = kind == MessageKind::Event && self.metadata_events_enabled;
        if !initial_kind(kind) && !event_enabled {
            return Err(invalid_data(
                "message kind is not enabled on this connection",
            ));
        }
        let payload_len = header.payload_len();
        if event_enabled && payload_len != EVENT_PAYLOAD_BYTES {
            return Err(invalid_data(
                "metadata event payload must be exactly 72 bytes",
            ));
        }
        if payload_len > MAX_CONTROL_PAYLOAD_BYTES {
            return Err(invalid_data("control payload exceeds the transport limit"));
        }

        let mut payload = Vec::new();
        payload
            .try_reserve_exact(payload_len)
            .map_err(|_| io::Error::other("frame allocation failed"))?;
        payload.resize(payload_len, 0);
        read_exact_until(&mut self.stream, &mut payload, deadline)?;

        if event_enabled {
            decode_event(&payload)
                .map_err(|_| invalid_data("metadata event payload is invalid"))?;
        }

        Ok(OwnedFrame { header, payload })
    }

    /// Validate and write one frame by an absolute deadline.
    ///
    /// Validation completes before the first socket write. The codec streams
    /// the fixed header and borrowed payload through a small nonblocking writer;
    /// this method never creates a second whole-frame buffer.
    pub fn write_frame(&mut self, frame: &Frame<'_>, deadline: Instant) -> io::Result<()> {
        if self.poisoned {
            return Err(poisoned_error());
        }
        validate_outbound(frame, self.metadata_events_enabled)?;

        let result = {
            let mut writer = DeadlineWriter {
                stream: &mut self.stream,
                deadline,
            };
            encode_frame(frame, &mut writer)
        };
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
}

/// A listener rooted in an exclusively private, same-user socket directory.
///
/// The listener owns its directory descriptor and pathname identities. Drop
/// removes only the socket inode created by this instance while both the
/// directory pathname and socket inode still match. It never removes the
/// directory and never takes over a presumed stale endpoint.
pub struct PrivateListener {
    listener: UnixListener,
    _cleanup: SocketCleanup,
    socket_path: PathBuf,
}

impl PrivateListener {
    /// Bind the fixed control socket beneath a validated private directory.
    ///
    /// Existing files, sockets, and symlinks at the endpoint are refused. The
    /// directory must be absolute and lexically unambiguous; existing
    /// ancestors must be root/current-user owned and not group/other writable,
    /// except for root-owned sticky directories.
    pub fn bind(directory: &Path) -> io::Result<Self> {
        let directory = validate_path(directory)?;
        let (directory_fd, directory_identity) = open_private_directory(&directory, true)?;
        verify_directory_path(&directory, directory_identity)?;
        if entry_identity(&directory_fd, SOCKET_BASENAME)?.is_some() {
            return Err(already_exists("control socket path already exists"));
        }

        let socket_path = directory.join(SOCKET_BASENAME);
        let listener = UnixListener::bind(&socket_path)?;

        // Capture the inode immediately after bind. If the pathname moved or
        // was replaced, cleanup fails closed and preserves the new path.
        let socket_identity = entry_identity(&directory_fd, SOCKET_BASENAME)?
            .ok_or_else(|| invalid_data("bound socket entry disappeared"))?;
        let cleanup = SocketCleanup {
            directory_fd,
            directory_path: directory.clone(),
            directory_identity,
            socket_identity,
        };

        verify_directory_path(&directory, directory_identity)?;
        verify_socket_identity(&cleanup.directory_fd, socket_identity)?;
        chmodat(
            &cleanup.directory_fd,
            SOCKET_BASENAME,
            Mode::from_raw_mode(SOCKET_MODE),
            AtFlags::SYMLINK_NOFOLLOW,
        )?;
        verify_socket_permissions(&cleanup.directory_fd, socket_identity)?;
        listener.set_nonblocking(true)?;
        verify_directory_path(&directory, directory_identity)?;

        Ok(Self {
            listener,
            _cleanup: cleanup,
            socket_path,
        })
    }

    /// Return the fixed endpoint path for local client connection setup.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Accept and kernel-authenticate one connection before returning it.
    pub fn accept(&self, deadline: Instant) -> io::Result<AuthenticatedStream> {
        loop {
            check_deadline(deadline)?;
            match self.listener.accept() {
                Ok((stream, _address)) => {
                    check_deadline(deadline)?;
                    return AuthenticatedStream::new(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_for_readiness(deadline)?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn validate_path(path: &Path) -> io::Result<PathBuf> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() > 1024 {
        return Err(invalid_input("socket directory path exceeds its limit"));
    }
    if bytes.is_empty() || bytes[0] != b'/' || bytes.contains(&0) {
        return Err(invalid_input("socket directory must be an absolute path"));
    }
    if bytes.len() > 1 && bytes.last() == Some(&b'/') {
        return Err(invalid_input(
            "socket directory must not have a trailing separator",
        ));
    }
    if bytes.windows(2).any(|pair| pair == b"//") {
        return Err(invalid_input(
            "socket directory contains an empty component",
        ));
    }
    for component in bytes[1..].split(|byte| *byte == b'/') {
        if component == b"." || component == b".." {
            return Err(invalid_input("socket directory contains a dot component"));
        }
    }
    Ok(path.to_path_buf())
}

fn open_private_directory(path: &Path, create_final: bool) -> io::Result<(File, Identity)> {
    let bytes = path.as_os_str().as_bytes();
    let components: Vec<&OsStr> = if bytes == b"/" {
        Vec::new()
    } else {
        bytes[1..]
            .split(|byte| *byte == b'/')
            .map(OsStr::from_bytes)
            .collect()
    };

    let mut current = File::open("/")?;
    if components.is_empty() {
        validate_final_directory(&current)?;
        let identity = Identity::from_stat(fstat(&current)?);
        return Ok((current, identity));
    }

    for component in &components[..components.len() - 1] {
        validate_ancestor_directory(&current)?;
        let opened = openat(&current, *component, DIRECTORY_FLAGS, Mode::empty())?;
        current = File::from(opened);
        validate_ancestor_directory(&current)?;
    }
    validate_ancestor_directory(&current)?;

    let final_name = components[components.len() - 1];
    let created = if create_final {
        match mkdirat(&current, final_name, Mode::from_raw_mode(DIRECTORY_MODE)) {
            Ok(()) => true,
            Err(error) if error == Errno::EXIST => false,
            Err(error) => return Err(error.into()),
        }
    } else {
        false
    };

    let before = statat(&current, final_name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::Directory {
        return Err(permission_denied(
            "socket directory component is not a directory",
        ));
    }
    let opened = openat(&current, final_name, DIRECTORY_FLAGS, Mode::empty())?;
    let directory = File::from(opened);
    let opened_stat = fstat(&directory)?;
    if Identity::from_stat(before) != Identity::from_stat(opened_stat) {
        return Err(permission_denied("socket directory changed while opening"));
    }

    if created {
        if opened_stat.st_uid as u32 != rustix::process::geteuid().as_raw() {
            return Err(permission_denied(
                "created socket directory has an unexpected owner",
            ));
        }
        fchmod(&directory, Mode::from_raw_mode(DIRECTORY_MODE))?;
    }
    validate_final_directory(&directory)?;
    let identity = Identity::from_stat(fstat(&directory)?);
    let after = statat(&current, final_name, AtFlags::SYMLINK_NOFOLLOW)?;
    if Identity::from_stat(after) != identity {
        return Err(permission_denied(
            "socket directory changed while validating",
        ));
    }
    Ok((directory, identity))
}

fn validate_ancestor_directory(directory: &File) -> io::Result<()> {
    let stat = fstat(directory)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(permission_denied("socket path ancestor is not a directory"));
    }
    let uid = stat.st_uid as u32;
    let mode = stat.st_mode as u32;
    let owned_by_trusted_user = uid == 0 || uid == rustix::process::geteuid().as_raw();
    let writable_by_others = mode & 0o022 != 0;
    let root_sticky = uid == 0 && mode & 0o1000 != 0;
    if !owned_by_trusted_user || (writable_by_others && !root_sticky) {
        return Err(permission_denied(
            "socket path ancestor permissions are unsafe",
        ));
    }
    Ok(())
}

fn validate_final_directory(directory: &File) -> io::Result<()> {
    let stat = fstat(directory)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid as u32 != rustix::process::geteuid().as_raw()
        || (stat.st_mode as u32 & 0o7777) != u32::from(DIRECTORY_MODE)
    {
        return Err(permission_denied(
            "socket directory must be owned by this user with mode 0700",
        ));
    }
    Ok(())
}

fn verify_directory_path(path: &Path, expected: Identity) -> io::Result<()> {
    let (opened, actual) = open_private_directory(path, false)?;
    if actual != expected || Identity::from_stat(fstat(&opened)?) != expected {
        return Err(permission_denied("socket directory pathname changed"));
    }
    Ok(())
}

fn entry_identity(directory: &File, name: &str) -> io::Result<Option<Identity>> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(Identity::from_stat(stat))),
        Err(error) if error == Errno::NOENT => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn verify_socket_identity(directory: &File, expected: Identity) -> io::Result<()> {
    let stat = statat(directory, SOCKET_BASENAME, AtFlags::SYMLINK_NOFOLLOW)?;
    if Identity::from_stat(stat) != expected
        || FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || stat.st_uid as u32 != rustix::process::geteuid().as_raw()
    {
        return Err(permission_denied("control socket identity changed"));
    }
    Ok(())
}

fn verify_socket_permissions(directory: &File, expected: Identity) -> io::Result<()> {
    verify_socket_identity(directory, expected)?;
    let stat = statat(directory, SOCKET_BASENAME, AtFlags::SYMLINK_NOFOLLOW)?;
    if (stat.st_mode as u32 & 0o7777) != u32::from(SOCKET_MODE) {
        return Err(permission_denied("control socket mode is not 0600"));
    }
    Ok(())
}

struct SocketCleanup {
    directory_fd: File,
    directory_path: PathBuf,
    directory_identity: Identity,
    socket_identity: Identity,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if Identity::from_stat(match fstat(&self.directory_fd) {
            Ok(stat) => stat,
            Err(_) => return,
        }) != self.directory_identity
        {
            return;
        }
        if verify_directory_path(&self.directory_path, self.directory_identity).is_err() {
            return;
        }
        if verify_socket_identity(&self.directory_fd, self.socket_identity).is_err() {
            return;
        }
        let _ = unlinkat(&self.directory_fd, SOCKET_BASENAME, AtFlags::empty());
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn from_stat(stat: rustix::fs::Stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        }
    }
}

fn validate_outbound(frame: &Frame<'_>, metadata_events_enabled: bool) -> io::Result<()> {
    let kind = frame.header().kind();
    let event_enabled = kind == MessageKind::Event && metadata_events_enabled;
    if !initial_kind(kind) && !event_enabled {
        return Err(invalid_input(
            "message kind is not enabled on this connection",
        ));
    }
    if frame.header().payload_len() != frame.payload().len()
        || frame.payload().len() > MAX_CONTROL_PAYLOAD_BYTES
    {
        return Err(invalid_input("control frame has an invalid payload length"));
    }
    if event_enabled {
        if frame.payload().len() != EVENT_PAYLOAD_BYTES {
            return Err(invalid_input(
                "metadata event payload must be exactly 72 bytes",
            ));
        }
        decode_event(frame.payload())
            .map_err(|_| invalid_input("metadata event payload is invalid"))?;
    }
    Ok(())
}

const fn initial_kind(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::Hello
            | MessageKind::Welcome
            | MessageKind::Request
            | MessageKind::Response
            | MessageKind::Error
    )
}

fn read_exact_until(
    stream: &mut UnixStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        check_deadline(deadline)?;
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "socket closed before the frame was complete",
                ));
            }
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_for_readiness(deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct DeadlineWriter<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}

impl Write for DeadlineWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut offset = 0;
        while offset < buffer.len() {
            check_deadline(self.deadline)?;
            match self.stream.write(&buffer[offset..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "socket accepted no frame bytes",
                    ));
                }
                Ok(written) => offset += written,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    wait_for_readiness(self.deadline)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(offset)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(timeout_error())
    } else {
        Ok(())
    }
}

fn wait_for_readiness(deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(timeout_error());
    }
    thread::sleep(remaining.min(POLL_INTERVAL));
    check_deadline(deadline)
}

fn wire_error(error: WireError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn permission_denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn already_exists(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}

fn timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "socket operation deadline expired")
}

fn poisoned_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "socket transport is poisoned",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::time::SystemTime;

    fn deadline_after(duration: Duration) -> Instant {
        Instant::now() + duration
    }

    fn encoded(kind: MessageKind, request_id: u64, payload: &[u8]) -> Vec<u8> {
        let frame = Frame::new(kind, request_id, payload).expect("valid frame");
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).expect("encode frame");
        bytes
    }

    fn metadata_event_payload() -> [u8; EVENT_PAYLOAD_BYTES] {
        use crate::control_protocol::TargetRef;
        use crate::event_protocol::{encode_event, Event, EventBody};

        encode_event(&Event {
            cursor: 1,
            target: TargetRef {
                generation: 1,
                workspace: 2,
                pane: 3,
                session: 4,
                occupant_revision: 5,
            },
            body: EventBody::ChildExited,
        })
        .expect("valid metadata event")
    }

    fn private_test_directory() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = PathBuf::from(format!(
            "/private/tmp/pl-transport-{}-{unique}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(u32::from(DIRECTORY_MODE))
            .create(&path)
            .expect("create private test directory");
        path
    }

    fn remove_test_tree(path: &Path) {
        let _ = fs::remove_file(path.join(SOCKET_BASENAME));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn construction_authenticates_without_consuming_already_queued_bytes() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let frame_bytes = encoded(MessageKind::Request, 19, b"auth-before-read");
        sender.write_all(&frame_bytes).expect("queue frame");

        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        let frame = authenticated
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect("read queued frame");
        assert_eq!(frame.kind(), MessageKind::Request);
        assert_eq!(frame.request_id(), 19);
        assert_eq!(frame.payload(), b"auth-before-read");
        assert!(!authenticated.is_poisoned());
    }

    #[test]
    fn reads_a_valid_frame_and_redacts_payload_debug() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        sender
            .write_all(&encoded(MessageKind::Response, 7, b"payload-canary"))
            .expect("write frame");

        let frame = authenticated
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect("read frame");
        let debug = format!("{frame:?}");
        assert!(debug.contains("Response"));
        assert!(debug.contains("payload_len"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("payload-canary"));
    }

    #[test]
    fn partial_header_timeout_poisons_the_connection() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        sender
            .write_all(b"PLNKpartial")
            .expect("write partial header");

        let error = authenticated
            .read_frame(deadline_after(Duration::from_millis(5)))
            .expect_err("partial header must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(authenticated.is_poisoned());
        assert_eq!(
            authenticated
                .read_frame(deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn partial_body_timeout_poisons_the_connection() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        let bytes = encoded(MessageKind::Request, 3, b"body");
        sender
            .write_all(&bytes[..HEADER_BYTES + 1])
            .expect("partial body");

        let error = authenticated
            .read_frame(deadline_after(Duration::from_millis(5)))
            .expect_err("partial body must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(authenticated.is_poisoned());
    }

    #[test]
    fn eof_during_header_or_body_poisons_the_connection() {
        for body_case in [false, true] {
            let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
            let mut authenticated =
                AuthenticatedStream::new(receiver).expect("same-user socket pair");
            if body_case {
                let bytes = encoded(MessageKind::Request, 4, b"four");
                sender.write_all(&bytes[..HEADER_BYTES + 1]).unwrap();
            } else {
                sender.write_all(b"short").unwrap();
            }
            drop(sender);
            let error = authenticated
                .read_frame(deadline_after(Duration::from_secs(1)))
                .expect_err("closed stream must fail");
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
            assert!(authenticated.is_poisoned());
        }
    }

    #[test]
    fn rejects_snapshot_and_event_headers_before_waiting_for_their_payloads() {
        for kind in [MessageKind::Snapshot, MessageKind::Event] {
            let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
            let mut authenticated =
                AuthenticatedStream::new(receiver).expect("same-user socket pair");
            let header_only = encoded(kind, 0, b"")
                .into_iter()
                .take(HEADER_BYTES)
                .collect::<Vec<_>>();
            sender.write_all(&header_only).expect("write header");
            let started = Instant::now();
            let error = authenticated
                .read_frame(deadline_after(Duration::from_secs(1)))
                .expect_err("kind must be rejected before payload read");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(started.elapsed() < Duration::from_millis(100));
            assert!(authenticated.is_poisoned());
        }
    }

    #[test]
    fn negotiated_metadata_events_round_trip_but_snapshot_stays_denied() {
        let (server_socket, client_socket) = UnixStream::pair().expect("socket pair");
        let mut server = AuthenticatedStream::new(server_socket).expect("server stream");
        let mut client = AuthenticatedStream::new(client_socket).expect("client stream");
        server
            .enable_metadata_events()
            .expect("enable server events");
        client
            .enable_metadata_events()
            .expect("enable client events");

        let payload = metadata_event_payload();
        let event = Frame::new(MessageKind::Event, 0, &payload).expect("event frame");
        client
            .write_frame(&event, deadline_after(Duration::from_secs(1)))
            .expect("write negotiated metadata event");
        let received = server
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect("read negotiated metadata event");
        assert_eq!(received.kind(), MessageKind::Event);
        assert_eq!(received.request_id(), 0);
        assert_eq!(received.payload(), payload);

        let snapshot =
            Frame::new(MessageKind::Snapshot, 1, b"unsupported").expect("snapshot frame envelope");
        assert_eq!(
            server
                .write_frame(&snapshot, deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!server.is_poisoned());
    }

    #[test]
    fn enabled_event_header_length_is_rejected_before_body_allocation() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user stream");
        authenticated.enable_metadata_events().unwrap();

        let mut header = encoded(MessageKind::Event, 0, &[0; EVENT_PAYLOAD_BYTES]);
        let declared_len = 20_u32 + EVENT_PAYLOAD_BYTES as u32 + 1;
        header[..4].copy_from_slice(&declared_len.to_be_bytes());
        sender
            .write_all(&header[..HEADER_BYTES])
            .expect("write oversize event header only");

        let started = Instant::now();
        let error = authenticated
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect_err("non-exact Event length must fail before body read");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(authenticated.is_poisoned());
        assert_eq!(
            authenticated.enable_metadata_events().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn malformed_enabled_event_body_poisons_without_a_capability_bypass() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user stream");
        authenticated.enable_metadata_events().unwrap();
        let malformed = [0_u8; EVENT_PAYLOAD_BYTES];
        sender
            .write_all(&encoded(MessageKind::Event, 0, &malformed))
            .expect("write malformed event body");

        assert_eq!(
            authenticated
                .read_frame(deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(authenticated.is_poisoned());
        assert_eq!(
            authenticated
                .read_frame(deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );

        let (sender, _receiver) = UnixStream::pair().expect("second socket pair");
        let mut outbound = AuthenticatedStream::new(sender).expect("outbound stream");
        let mut invalid_payload = metadata_event_payload();
        invalid_payload[4] = 1;
        outbound.enable_metadata_events().unwrap();
        let invalid_event = Frame::new(MessageKind::Event, 0, &invalid_payload).unwrap();
        assert_eq!(
            outbound
                .write_frame(&invalid_event, deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!outbound.is_poisoned());
    }

    #[test]
    fn rejects_oversized_control_header_before_body_allocation() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        let mut header = encoded(MessageKind::Request, 8, b"");
        let declared_len = 20_u32 + (MAX_CONTROL_PAYLOAD_BYTES as u32) + 1;
        header[..4].copy_from_slice(&declared_len.to_be_bytes());
        sender
            .write_all(&header[..HEADER_BYTES])
            .expect("write oversized header only");
        let started = Instant::now();
        let error = authenticated
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect_err("oversized declaration must be rejected before reading a body");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(authenticated.is_poisoned());
    }

    #[test]
    fn accepts_the_full_control_payload_limit() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let payload = vec![0x5a; MAX_CONTROL_PAYLOAD_BYTES];
        let bytes = encoded(MessageKind::Request, 1, &payload);
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        let send = thread::spawn(move || sender.write_all(&bytes));
        let frame = authenticated
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect("read max control frame");
        send.join()
            .expect("writer thread")
            .expect("write max frame");
        assert_eq!(frame.payload().len(), MAX_CONTROL_PAYLOAD_BYTES);
        assert_eq!(frame.payload(), payload);
    }

    #[test]
    fn expired_read_deadline_poisoning_does_not_enter_the_read_loop() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(receiver).expect("same-user socket pair");
        sender
            .write_all(&encoded(MessageKind::Request, 9, b"already-ready"))
            .expect("queue complete frame before deadline");
        let error = authenticated
            .read_frame(Instant::now() - Duration::from_millis(1))
            .expect_err("expired deadline must win even with data available");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(authenticated.is_poisoned());
    }

    #[test]
    fn outbound_validation_rejects_unnegotiated_kinds_before_io() {
        let (sender, mut receiver) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(sender).expect("same-user socket pair");
        receiver.set_nonblocking(true).unwrap();
        let snapshot = Frame::new(MessageKind::Snapshot, 1, b"no").unwrap();
        let error = authenticated
            .write_frame(&snapshot, deadline_after(Duration::from_secs(1)))
            .expect_err("snapshot not enabled");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!authenticated.is_poisoned());
        let mut byte = [0];
        assert_eq!(
            receiver.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        let event_payload = metadata_event_payload();
        let event = Frame::new(MessageKind::Event, 0, &event_payload).unwrap();
        assert_eq!(
            authenticated
                .write_frame(&event, deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!authenticated.is_poisoned());
    }

    #[test]
    fn expired_write_deadline_writes_nothing_and_poisons() {
        let (sender, mut receiver) = UnixStream::pair().expect("socket pair");
        let mut authenticated = AuthenticatedStream::new(sender).expect("same-user socket pair");
        receiver.set_nonblocking(true).unwrap();
        let frame = Frame::new(MessageKind::Request, 2, b"never-sent").unwrap();
        let error = authenticated
            .write_frame(&frame, Instant::now() - Duration::from_millis(1))
            .expect_err("expired deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(authenticated.is_poisoned());
        assert_eq!(
            receiver.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn stalled_writer_times_out_and_cannot_replay_the_frame() {
        let (sender, _receiver) = UnixStream::pair().expect("socket pair");
        let send_buffer: libc::c_int = 1_024;
        // SAFETY: the descriptor is a live Unix stream, the value and length
        // match setsockopt(SO_SNDBUF), and the kernel copies the input value.
        let result = unsafe {
            libc::setsockopt(
                std::os::fd::AsRawFd::as_raw_fd(&sender),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&send_buffer as *const libc::c_int).cast(),
                std::mem::size_of_val(&send_buffer) as libc::socklen_t,
            )
        };
        assert_eq!(result, 0, "shrink socket send buffer");

        let mut authenticated = AuthenticatedStream::new(sender).expect("same-user socket pair");
        let payload = vec![0x7c; MAX_CONTROL_PAYLOAD_BYTES];
        let frame = Frame::new(MessageKind::Request, 5, &payload).unwrap();
        let error = authenticated
            .write_frame(&frame, deadline_after(Duration::from_millis(10)))
            .expect_err("unread peer must stall the write");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(authenticated.is_poisoned());
        assert_eq!(
            authenticated
                .write_frame(&frame, deadline_after(Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn validates_socket_directory_path_syntax_without_binding() {
        for path in [
            "",
            "relative",
            "/private/tmp/",
            "/private//tmp",
            "/private/./tmp",
            "/private/../tmp",
        ] {
            assert_eq!(
                validate_path(Path::new(path)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "path {path:?}"
            );
        }
        assert!(validate_path(Path::new("/")).is_ok());
    }

    #[test]
    fn opens_owned_private_directory_and_rejects_unsafe_ancestors() {
        let root = private_test_directory();
        let nested = root.join("parent").join("endpoint");
        fs::create_dir(root.join("parent")).expect("create parent");
        fs::set_permissions(root.join("parent"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(&nested).expect("create endpoint");
        fs::set_permissions(
            &nested,
            fs::Permissions::from_mode(u32::from(DIRECTORY_MODE)),
        )
        .unwrap();
        let (_, identity) =
            open_private_directory(&nested, false).expect("valid private directory");
        assert_ne!(identity.inode, 0);

        let created = root.join("created");
        let (created_fd, _) =
            open_private_directory(&created, true).expect("create final directory exclusively");
        assert_eq!(
            fstat(&created_fd).unwrap().st_mode as u32 & 0o7777,
            u32::from(DIRECTORY_MODE)
        );

        fs::set_permissions(root.join("parent"), fs::Permissions::from_mode(0o770)).unwrap();
        let error = match open_private_directory(&nested, false) {
            Ok(_) => panic!("group-writable non-sticky ancestor must fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        remove_test_tree(&root);
    }

    #[test]
    fn refuses_existing_socket_path_files_and_symlinks_without_unlinking() {
        for symlink in [false, true] {
            let directory = private_test_directory();
            let endpoint = directory.join(SOCKET_BASENAME);
            let victim = directory.join("existing");
            fs::write(&victim, b"preserve").unwrap();
            if symlink {
                std::os::unix::fs::symlink(&victim, &endpoint).unwrap();
            } else {
                fs::write(&endpoint, b"stale").unwrap();
            }
            let error = match PrivateListener::bind(&directory) {
                Ok(_) => panic!("existing path must be refused"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert!(fs::symlink_metadata(&endpoint).is_ok());
            assert_eq!(fs::read(&victim).unwrap(), b"preserve");
            remove_test_tree(&directory);
        }
    }

    #[test]
    #[ignore = "pathname socket bind is denied with EPERM in this sandbox; rerun on Apple Silicon macOS"]
    fn pathname_listener_authenticates_and_removes_only_its_socket() {
        let directory = private_test_directory();
        let listener = PrivateListener::bind(&directory).expect("pathname bind");
        let accept_error = match listener.accept(deadline_after(Duration::from_millis(5))) {
            Ok(_) => panic!("accept without a queued peer must time out"),
            Err(error) => error,
        };
        assert_eq!(accept_error.kind(), io::ErrorKind::TimedOut);
        let metadata = fs::symlink_metadata(listener.socket_path()).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.mode() & 0o7777, u32::from(SOCKET_MODE));

        let mut client =
            AuthenticatedStream::connect(&directory, deadline_after(Duration::from_secs(1)))
                .expect("bounded authenticated client connect");
        let mut accepted = listener
            .accept(deadline_after(Duration::from_secs(1)))
            .expect("accept same-user client");
        let outbound = Frame::new(MessageKind::Request, 13, b"pathname-auth").unwrap();
        client
            .write_frame(&outbound, deadline_after(Duration::from_secs(1)))
            .unwrap();
        let frame = accepted
            .read_frame(deadline_after(Duration::from_secs(1)))
            .expect("read authenticated frame");
        assert_eq!(frame.payload(), b"pathname-auth");
        drop(accepted);
        drop(client);
        drop(listener);
        assert!(fs::symlink_metadata(directory.join(SOCKET_BASENAME)).is_err());
        assert!(directory.exists());
        remove_test_tree(&directory);

        let replacement_dir = private_test_directory();
        let listener = PrivateListener::bind(&replacement_dir).expect("second pathname bind");
        let endpoint = replacement_dir.join(SOCKET_BASENAME);
        fs::remove_file(&endpoint).unwrap();
        fs::write(&endpoint, b"replacement").unwrap();
        drop(listener);
        assert_eq!(fs::read(&endpoint).unwrap(), b"replacement");
        remove_test_tree(&replacement_dir);
    }

    #[test]
    fn expired_connect_deadline_does_not_touch_an_invalid_path() {
        let error = match AuthenticatedStream::connect(
            Path::new("/missing/no-such-private-dir"),
            Instant::now(),
        ) {
            Ok(_) => panic!("expired connect cannot succeed"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
