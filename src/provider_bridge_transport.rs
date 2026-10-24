//! Scoped, same-user Unix transport for provider observations.
//!
//! This module accepts only an already-connected stream. It has no listener,
//! provider activation, process authority, or raw-stream escape hatch.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::TargetRef;
use crate::peer_identity::require_same_user;
use crate::provider_bridge::Frame;
use crate::provider_bridge_protocol::{
    decode, decode_header, encode, Hello, Message, HEADER_BYTES,
};
use rustix::fs::{fcntl_setfl, OFlags};
use rustix::io::{fcntl_getfd, fcntl_setfd, FdFlags};
use rustix::net::sockopt::set_socket_nosigpipe;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Instant;

const SUPPORTED_VERSION: &str = "2.0.32";

/// A one-use capability for one exact provider source and managed target.
///
/// Possession proves that the peer received this issuer-created grant. It is
/// not provider executable attestation or same-user process isolation.
pub struct Grant {
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    instance: u64,
    secret: [u8; 32],
}

impl Grant {
    /// Build a grant from secret material delivered by a trusted issuer.
    pub fn new(
        target: TargetRef,
        project: &[u8],
        version: &str,
        instance: u64,
        secret: [u8; 32],
    ) -> io::Result<Self> {
        if !valid_target(target)
            || project.is_empty()
            || project.len() > 4096
            || project.contains(&0)
            || version != SUPPORTED_VERSION
            || instance == 0
            || secret.iter().all(|byte| *byte == 0)
        {
            return Err(invalid_input("bridge grant configuration is invalid"));
        }
        Ok(Self {
            target,
            project: project.to_vec(),
            version: version.to_owned(),
            instance,
            secret,
        })
    }

    /// Issue a grant using 32 bytes read from the operating system RNG.
    pub fn issue(
        target: TargetRef,
        project: &[u8],
        version: &str,
        instance: u64,
    ) -> io::Result<Self> {
        let mut secret = [0u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut secret)?;
        Self::new(target, project, version, instance, secret)
    }

    /// Construct the matching typed Hello for delivery to the source endpoint.
    ///
    /// The Hello contains the capability secret, but its Debug implementation
    /// is redacted. This copies the secret so the issuer can provision both
    /// endpoints without exposing the stream or transport internals.
    pub fn hello(&self) -> io::Result<Hello> {
        Hello::new(
            self.target,
            &self.project,
            &self.version,
            self.instance,
            self.secret,
        )
        .map_err(|_| invalid_input("bridge grant configuration is invalid"))
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.secret.fill(0);
    }
}

impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Grant { <redacted> }")
    }
}

/// Authenticated receiving endpoint. It yields only scoped observations.
pub struct BridgeReader {
    stream: UnixStream,
    poisoned: bool,
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    instance: u64,
}

impl BridgeReader {
    /// Authenticate the peer, validate one Hello, and consume the grant.
    pub fn accept(stream: UnixStream, grant: Grant, deadline: Instant) -> io::Result<Self> {
        check_deadline(deadline)?;
        require_same_user(&stream)?;
        configure_stream(&stream)?;
        let target = grant.target;
        let project = grant.project.clone();
        let version = grant.version.clone();
        let instance = grant.instance;
        let mut expected_secret = grant.secret;
        let mut reader = Self {
            stream,
            poisoned: false,
            target,
            project,
            version,
            instance,
        };
        let result = (|| {
            let hello = match reader.read_message(deadline)? {
                Message::Hello(hello) => hello,
                _ => return Err(invalid_data("bridge handshake message kind is invalid")),
            };
            if hello.target() != reader.target
                || hello.project_identity() != reader.project
                || hello.provider_version() != reader.version
                || hello.bridge_instance() != reader.instance
                || hello.secret() != &expected_secret
            {
                return Err(permission_denied("bridge grant does not match"));
            }
            reader.write_message(
                &Message::Welcome {
                    bridge_instance: reader.instance,
                },
                deadline,
            )
        })();
        expected_secret.fill(0);
        drop(grant);
        if let Err(error) = result {
            reader.poison();
            return Err(error);
        }
        Ok(reader)
    }

    /// Read one validated observation from the configured source.
    pub fn read_observation(&mut self, deadline: Instant) -> io::Result<Frame> {
        let result = self
            .read_message(deadline)
            .and_then(|message| match message {
                Message::Observation(frame)
                    if frame.target() == self.target
                        && frame.project_identity() == self.project
                        && frame.provider_version() == self.version
                        && frame.bridge_instance() == self.instance =>
                {
                    Ok(frame)
                }
                Message::Observation(_) => Err(invalid_data("bridge observation scope is invalid")),
                _ => Err(invalid_data("bridge message kind is invalid")),
            });
        self.finish_operation(result)
    }

    /// Whether a failed or incomplete operation permanently closed this channel.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn read_message(&mut self, deadline: Instant) -> io::Result<Message> {
        self.ensure_healthy()?;
        let mut header = [0; HEADER_BYTES];
        read_exact_until(&mut self.stream, &mut header, deadline)?;
        let parsed =
            decode_header(&header).map_err(|_| invalid_data("bridge header is invalid"))?;
        let mut bytes = Vec::with_capacity(HEADER_BYTES + parsed.body_len());
        bytes.extend_from_slice(&header);
        let body_start = bytes.len();
        bytes.resize(body_start + parsed.body_len(), 0);
        read_exact_until(&mut self.stream, &mut bytes[body_start..], deadline)?;
        let message = decode(&bytes).map_err(|_| invalid_data("bridge message is invalid"))?;
        check_deadline(deadline)?;
        Ok(message)
    }

    fn write_message(&mut self, message: &Message, deadline: Instant) -> io::Result<()> {
        self.ensure_healthy()?;
        let bytes = encode(message).map_err(|_| invalid_input("bridge message is invalid"))?;
        write_all_until(&mut self.stream, &bytes, deadline)
    }

    fn finish_operation<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.poison();
        }
        result
    }

    fn ensure_healthy(&self) -> io::Result<()> {
        if self.poisoned {
            Err(poisoned_error())
        } else {
            Ok(())
        }
    }

    fn poison(&mut self) {
        self.poisoned = true;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

impl fmt::Debug for BridgeReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeReader { <redacted> }")
    }
}

/// Authenticated sending endpoint with observation-only authority.
pub struct BridgeSender {
    stream: UnixStream,
    poisoned: bool,
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    instance: u64,
}

impl BridgeSender {
    /// Authenticate the peer, send Hello, and require the matching Welcome.
    pub fn connect(stream: UnixStream, hello: Hello, deadline: Instant) -> io::Result<Self> {
        check_deadline(deadline)?;
        if hello.provider_version() != SUPPORTED_VERSION {
            return Err(invalid_input("bridge provider version is unsupported"));
        }
        require_same_user(&stream)?;
        configure_stream(&stream)?;
        let mut sender = Self {
            stream,
            poisoned: false,
            target: hello.target(),
            project: hello.project_identity().to_vec(),
            version: hello.provider_version().to_owned(),
            instance: hello.bridge_instance(),
        };
        let result = (|| {
            sender.write_message(&Message::Hello(hello), deadline)?;
            match sender.read_message(deadline)? {
                Message::Welcome { bridge_instance } if bridge_instance == sender.instance => {
                    Ok(())
                }
                _ => Err(invalid_data("bridge welcome does not match")),
            }
        })();
        if let Err(error) = result {
            sender.poison();
            return Err(error);
        }
        Ok(sender)
    }

    /// Send one observation whose complete scope matches this connection.
    pub fn send_observation(&mut self, frame: &Frame, deadline: Instant) -> io::Result<()> {
        if frame.target() != self.target
            || frame.project_identity() != self.project
            || frame.provider_version() != self.version
            || frame.bridge_instance() != self.instance
        {
            self.poison();
            return Err(invalid_data("bridge observation scope is invalid"));
        }
        let result = self.write_message(&Message::Observation(frame.clone()), deadline);
        self.finish_operation(result)
    }

    /// Whether a failed or incomplete operation permanently closed this channel.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn read_message(&mut self, deadline: Instant) -> io::Result<Message> {
        self.ensure_healthy()?;
        let mut header = [0; HEADER_BYTES];
        read_exact_until(&mut self.stream, &mut header, deadline)?;
        let parsed =
            decode_header(&header).map_err(|_| invalid_data("bridge header is invalid"))?;
        let mut bytes = Vec::with_capacity(HEADER_BYTES + parsed.body_len());
        bytes.extend_from_slice(&header);
        let body_start = bytes.len();
        bytes.resize(body_start + parsed.body_len(), 0);
        read_exact_until(&mut self.stream, &mut bytes[body_start..], deadline)?;
        let message = decode(&bytes).map_err(|_| invalid_data("bridge message is invalid"))?;
        check_deadline(deadline)?;
        Ok(message)
    }

    fn write_message(&mut self, message: &Message, deadline: Instant) -> io::Result<()> {
        self.ensure_healthy()?;
        let bytes = encode(message).map_err(|_| invalid_input("bridge message is invalid"))?;
        write_all_until(&mut self.stream, &bytes, deadline)
    }

    fn finish_operation<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.poison();
        }
        result
    }

    fn ensure_healthy(&self) -> io::Result<()> {
        if self.poisoned {
            Err(poisoned_error())
        } else {
            Ok(())
        }
    }

    fn poison(&mut self) {
        self.poisoned = true;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

impl fmt::Debug for BridgeSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeSender { <redacted> }")
    }
}

fn configure_stream(stream: &UnixStream) -> io::Result<()> {
    set_socket_nosigpipe(stream.as_fd(), true)?;
    let fd = stream.as_fd();
    fcntl_setfd(fd, fcntl_getfd(fd)? | FdFlags::CLOEXEC)?;
    fcntl_setfl(fd, rustix::fs::fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
    Ok(())
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
                    "bridge stream ended",
                ));
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(error) => return Err(error),
        }
    }
    check_deadline(deadline)
}

fn write_all_until(stream: &mut UnixStream, buffer: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < buffer.len() {
        check_deadline(deadline)?;
        match stream.write(&buffer[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "bridge stream accepted no bytes",
                ));
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(error) => return Err(error),
        }
    }
    check_deadline(deadline)
}

fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "bridge deadline expired",
        ))
    } else {
        Ok(())
    }
}

fn wait(deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "bridge deadline expired",
        ));
    }
    std::thread::sleep(remaining.min(std::time::Duration::from_millis(1)));
    check_deadline(deadline)
}

fn valid_target(target: TargetRef) -> bool {
    target.generation != 0
        && target.workspace != 0
        && target.pane != 0
        && target.session != 0
        && target.occupant_revision != 0
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn permission_denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn poisoned_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "bridge channel is poisoned",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_bridge::{FrameScope, Selection};
    use crate::provider_bridge_protocol::{encode, Message, MAX_BODY_BYTES};
    use std::os::fd::AsRawFd;
    use std::thread;
    use std::time::Duration;

    fn target(pane: u64) -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 12,
            pane,
            session: 14,
            occupant_revision: 15,
        }
    }

    fn frame(target: TargetRef, instance: u64, sequence: u64) -> Frame {
        Frame::new(
            1,
            target,
            b"/work/project",
            SUPPORTED_VERSION,
            instance,
            sequence,
            FrameScope::TopLevel,
            Selection::Home,
        )
        .expect("valid observation")
    }

    fn connected() -> (BridgeReader, BridgeSender) {
        let (reader_stream, sender_stream) = UnixStream::pair().expect("socket pair");
        let secret = [0xA5; 32];
        let grant = Grant::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret)
            .expect("valid grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(2),
            )
        });
        let hello = Hello::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret)
            .expect("valid hello");
        let sender = BridgeSender::connect(
            sender_stream,
            hello,
            Instant::now() + Duration::from_secs(2),
        )
        .expect("authenticated sender");
        let reader = acceptor.join().expect("accept thread").expect("reader");
        (reader, sender)
    }

    fn raw_connected() -> (BridgeReader, UnixStream) {
        let (reader_stream, mut peer) = UnixStream::pair().expect("socket pair");
        let secret = [0x6A; 32];
        let grant =
            Grant::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret).expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(2),
            )
        });
        let hello =
            Hello::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret).expect("hello");
        peer.write_all(&encode(&Message::Hello(hello)).expect("encoded hello"))
            .expect("send hello");
        let mut header = [0; HEADER_BYTES];
        peer.read_exact(&mut header).expect("welcome header");
        let parsed =
            crate::provider_bridge_protocol::decode_header(&header).expect("valid welcome header");
        let mut bytes = header.to_vec();
        let body_start = bytes.len();
        bytes.resize(body_start + parsed.body_len(), 0);
        peer.read_exact(&mut bytes[body_start..])
            .expect("welcome body");
        assert!(matches!(
            crate::provider_bridge_protocol::decode(&bytes).expect("welcome"),
            Message::Welcome {
                bridge_instance: 19
            }
        ));
        let reader = acceptor.join().expect("accept thread").expect("reader");
        (reader, peer)
    }

    #[test]
    fn authenticated_socketpair_carries_only_matching_observation() {
        let (mut reader, mut sender) = connected();
        let expected = frame(target(13), 19, 1);
        sender
            .send_observation(&expected, Instant::now() + Duration::from_secs(1))
            .expect("send observation");
        let actual = reader
            .read_observation(Instant::now() + Duration::from_secs(1))
            .expect("read observation");
        assert_eq!(actual, expected);
        assert!(!reader.is_poisoned());
        assert!(!sender.is_poisoned());
    }

    #[test]
    fn issued_grant_builds_sender_hello_for_real_handshake() {
        let (reader_stream, sender_stream) = UnixStream::pair().expect("socket pair");
        let grant = Grant::issue(target(13), b"/work/project", SUPPORTED_VERSION, 19)
            .expect("OS-random grant");
        let hello = grant.hello().expect("matching typed hello");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(2),
            )
        });
        let sender = BridgeSender::connect(
            sender_stream,
            hello,
            Instant::now() + Duration::from_secs(2),
        )
        .expect("issued grant handshake");
        let reader = acceptor.join().expect("accept thread").expect("reader");
        assert!(!sender.is_poisoned());
        assert!(!reader.is_poisoned());
    }

    #[test]
    fn unsupported_sender_version_fails_before_handshake_io() {
        let (reader_stream, sender_stream) = UnixStream::pair().expect("socket pair");
        let hello = Hello::new(target(13), b"/work/project", "2.0.31", 19, [0xA5; 32])
            .expect("well-formed but unsupported hello");
        let error = BridgeSender::connect(
            sender_stream,
            hello,
            Instant::now() + Duration::from_secs(1),
        )
        .expect_err("unsupported configured version");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let mut byte = [0; 1];
        assert_eq!(
            (&reader_stream)
                .read(&mut byte)
                .expect("peer still has no hello"),
            0
        );
    }

    #[test]
    fn scope_mismatch_poison_closes_sender_without_writing() {
        let (mut reader, mut sender) = connected();
        let wrong = frame(target(99), 19, 1);
        assert_eq!(
            sender
                .send_observation(&wrong, Instant::now() + Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(sender.is_poisoned());
        assert_eq!(
            reader
                .read_observation(Instant::now() + Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(reader.is_poisoned());
    }

    #[test]
    fn stalled_output_poison_closes_sender() {
        let (reader_stream, sender_stream) = UnixStream::pair().expect("socket pair");
        let send_buffer: libc::c_int = 1_024;
        // SAFETY: the live socket descriptor and pointer to the initialized
        // integer are valid for setsockopt's duration.
        let result = unsafe {
            libc::setsockopt(
                sender_stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&send_buffer as *const libc::c_int).cast(),
                std::mem::size_of_val(&send_buffer) as libc::socklen_t,
            )
        };
        assert_eq!(result, 0, "set small socket send buffer");
        let secret = [0x91; 32];
        let grant =
            Grant::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret).expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(2),
            )
        });
        let hello =
            Hello::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret).expect("hello");
        let mut sender = BridgeSender::connect(
            sender_stream,
            hello,
            Instant::now() + Duration::from_secs(2),
        )
        .expect("sender");
        let _reader = acceptor.join().expect("accept thread").expect("reader");
        let started = Instant::now();
        let mut failure = None;
        for sequence in 1..=10_000 {
            let candidate = frame(target(13), 19, sequence);
            if let Err(error) =
                sender.send_observation(&candidate, Instant::now() + Duration::from_millis(15))
            {
                failure = Some(error);
                break;
            }
        }
        let error = failure.expect("full socket send buffer must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(sender.is_poisoned());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stalled_observation_deadline_permanently_poisons_reader() {
        let (mut reader, _sender) = connected();
        let error = reader
            .read_observation(Instant::now() + Duration::from_millis(10))
            .expect_err("idle peer must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(reader.is_poisoned());
        assert_eq!(
            reader
                .read_observation(Instant::now() + Duration::from_secs(1))
                .unwrap_err()
                .kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn wrong_handshake_secret_is_rejected_without_welcome() {
        let (reader_stream, mut peer) = UnixStream::pair().expect("socket pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x11; 32],
        )
        .expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(1),
            )
        });
        let hello = Hello::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x22; 32],
        )
        .expect("hello");
        let bytes = encode(&Message::Hello(hello)).expect("encoded hello");
        peer.write_all(&bytes).expect("send hello");
        let error = acceptor
            .join()
            .expect("accept thread")
            .expect_err("wrong grant secret");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn wrong_handshake_target_project_version_and_instance_are_rejected() {
        let secret = [0x2A; 32];
        let cases = [
            (
                target(99),
                b"/work/project".as_slice(),
                SUPPORTED_VERSION,
                19,
            ),
            (target(13), b"/work/other".as_slice(), SUPPORTED_VERSION, 19),
            (target(13), b"/work/project".as_slice(), "2.0.31", 19),
            (
                target(13),
                b"/work/project".as_slice(),
                SUPPORTED_VERSION,
                20,
            ),
        ];
        for (hello_target, hello_project, hello_version, hello_instance) in cases {
            let (reader_stream, mut peer) = UnixStream::pair().expect("socket pair");
            let grant = Grant::new(target(13), b"/work/project", SUPPORTED_VERSION, 19, secret)
                .expect("grant");
            let acceptor = thread::spawn(move || {
                BridgeReader::accept(
                    reader_stream,
                    grant,
                    Instant::now() + Duration::from_secs(1),
                )
            });
            let hello = Hello::new(
                hello_target,
                hello_project,
                hello_version,
                hello_instance,
                secret,
            )
            .expect("valid but wrong-scope hello");
            peer.write_all(&encode(&Message::Hello(hello)).expect("encoded hello"))
                .expect("send hello");
            assert_eq!(
                acceptor
                    .join()
                    .expect("accept thread")
                    .expect_err("wrong handshake scope")
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn wrong_handshake_role_and_wrong_observation_scope_are_rejected() {
        let (reader_stream, mut peer) = UnixStream::pair().expect("wrong-role pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x51; 32],
        )
        .expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(1),
            )
        });
        let bytes = encode(&Message::Welcome {
            bridge_instance: 19,
        })
        .unwrap();
        peer.write_all(&bytes).unwrap();
        assert_eq!(
            acceptor
                .join()
                .expect("accept thread")
                .expect_err("wrong handshake role")
                .kind(),
            io::ErrorKind::InvalidData
        );

        let (mut reader, mut peer) = raw_connected();
        let wrong = frame(target(99), 19, 1);
        peer.write_all(&encode(&Message::Observation(wrong)).expect("frame"))
            .expect("send wrong-scope observation");
        assert_eq!(
            reader
                .read_observation(Instant::now() + Duration::from_secs(1))
                .expect_err("wrong observation scope")
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(reader.is_poisoned());
    }

    #[test]
    fn partial_handshake_eof_and_oversized_header_fail_before_body_read() {
        let (reader_stream, mut peer) = UnixStream::pair().expect("EOF pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x31; 32],
        )
        .expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(1),
            )
        });
        peer.write_all(&[0x50; 7]).expect("partial header");
        thread::sleep(Duration::from_millis(5));
        drop(peer);
        let error = acceptor
            .join()
            .expect("accept thread")
            .expect_err("partial header EOF");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "{error:?}");

        let (mut reader, mut peer) = raw_connected();
        let bytes =
            encode(&Message::Observation(frame(target(13), 19, 1))).expect("observation bytes");
        peer.write_all(&bytes[..HEADER_BYTES + 2])
            .expect("partial observation body");
        peer.shutdown(std::net::Shutdown::Write)
            .expect("close after partial body");
        assert_eq!(
            reader
                .read_observation(Instant::now() + Duration::from_secs(1))
                .expect_err("partial body EOF")
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert!(reader.is_poisoned());

        let (reader_stream, mut peer) = UnixStream::pair().expect("oversize pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x41; 32],
        )
        .expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_secs(1),
            )
        });
        let hello = Hello::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x41; 32],
        )
        .expect("hello");
        let mut bytes = encode(&Message::Hello(hello)).expect("encoded hello");
        bytes[12..16].copy_from_slice(&((MAX_BODY_BYTES as u32) + 1).to_be_bytes());
        let started = Instant::now();
        peer.write_all(&bytes[..HEADER_BYTES])
            .expect("oversized fixed header");
        let error = acceptor
            .join()
            .expect("accept thread")
            .expect_err("oversized body length");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn stalled_or_fragmented_handshake_uses_one_absolute_deadline() {
        let (reader_stream, _peer) = UnixStream::pair().expect("stalled pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x61; 32],
        )
        .expect("grant");
        let started = Instant::now();
        assert_eq!(
            BridgeReader::accept(reader_stream, grant, started + Duration::from_millis(15),)
                .expect_err("stalled handshake")
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_millis(300));

        let (reader_stream, mut peer) = UnixStream::pair().expect("fragmented pair");
        let grant = Grant::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x71; 32],
        )
        .expect("grant");
        let acceptor = thread::spawn(move || {
            BridgeReader::accept(
                reader_stream,
                grant,
                Instant::now() + Duration::from_millis(25),
            )
        });
        let hello = Hello::new(
            target(13),
            b"/work/project",
            SUPPORTED_VERSION,
            19,
            [0x71; 32],
        )
        .expect("hello");
        let bytes = encode(&Message::Hello(hello)).unwrap();
        let started = Instant::now();
        for byte in bytes {
            if peer.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            acceptor
                .join()
                .expect("accept thread")
                .expect_err("fragmented message exceeded one deadline")
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn grant_and_transport_debug_redact_configured_scope_and_secret() {
        let grant = Grant::new(
            target(13),
            b"/private/project-name",
            SUPPORTED_VERSION,
            19,
            [0xD3; 32],
        )
        .expect("valid grant");
        let debug = format!("{grant:?}");
        assert_eq!(debug, "Grant { <redacted> }");
        assert!(!debug.contains("private"));
        assert!(!debug.contains("211"));
        let (reader, sender) = connected();
        assert_eq!(format!("{reader:?}"), "BridgeReader { <redacted> }");
        assert_eq!(format!("{sender:?}"), "BridgeSender { <redacted> }");
    }
}
