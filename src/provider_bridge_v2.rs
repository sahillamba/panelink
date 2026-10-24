//! Challenge-bound, fresh snapshot extension for the scoped provider bridge.
//!
//! Every snapshot response is tied to a challenge written by the reader before
//! the source callback runs. This proves ordering for cooperating source code;
//! it is not provider attestation or a current-at-crash guarantee.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::TargetRef;
use crate::peer_identity::require_same_user;
use crate::provider_bridge::Frame;
use crate::provider_bridge_protocol::{decode, encode, Hello, Message, MAX_MESSAGE_BYTES};
use crate::provider_bridge_transport::Grant;
use crate::provider_recovery_control::{
    self as recovery, NavigateOutcome, NavigationDecision, ValidationDecision, ValidationOutcome,
};
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::{fcntl_getfd, fcntl_setfd, FdFlags};
use rustix::net::sockopt::set_socket_nosigpipe;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub const HEADER_BYTES: usize = 24;
const MAGIC: &[u8; 8] = b"PLBRSN02";
const VERSION: u16 = 2;
const HELLO: u16 = 1;
const WELCOME: u16 = 2;
const SNAPSHOT_REQUEST: u16 = 3;
const SNAPSHOT_RESPONSE: u16 = 4;
const PROVISION: u16 = 5;

/// Deliver a just-issued scoped Hello to an already-connected same-user source.
///
/// The stream carries no persisted credential; callers should create the grant
/// only after the managed target is known, then move this stream into the
/// source's `FreshSender::connect_provisioned` call.
pub fn provision(stream: &mut UnixStream, grant: &Grant, deadline: Instant) -> io::Result<()> {
    check_deadline(deadline)?;
    require_same_user(stream)?;
    configure_stream(stream)?;
    let hello = grant.hello()?;
    let payload = encode(&Message::Hello(hello))
        .map_err(|_| invalid_input("fresh bridge provisioning is invalid"))?;
    write_outer(stream, PROVISION, 0, &payload, deadline)
}

/// A reader that can request scoped, challenge-bound source snapshots.
pub struct FreshReader {
    stream: UnixStream,
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    instance: u64,
    next_challenge: u64,
    validated_session: Option<String>,
    navigation_attempted: bool,
    poisoned: bool,
}

impl FreshReader {
    /// Authenticate the peer and complete a grant-scoped version-2 handshake.
    pub fn accept(stream: UnixStream, grant: Grant, deadline: Instant) -> io::Result<Self> {
        check_deadline(deadline)?;
        require_same_user(&stream)?;
        configure_stream(&stream)?;
        let hello = grant.hello()?;
        let mut reader = Self {
            stream,
            target: hello.target(),
            project: hello.project_identity().to_vec(),
            version: hello.provider_version().to_owned(),
            instance: hello.bridge_instance(),
            next_challenge: 1,
            validated_session: None,
            navigation_attempted: false,
            poisoned: false,
        };
        let result = (|| {
            match reader.read_packet(deadline)? {
                Packet::Hello(received) if received == hello => {}
                _ => return Err(permission_denied("fresh bridge grant does not match")),
            }
            reader.write_packet(Packet::Welcome(reader.instance), 0, deadline)
        })();
        drop(hello);
        drop(grant);
        if let Err(error) = result {
            reader.poison();
            return Err(error);
        }
        Ok(reader)
    }

    /// Request a new snapshot and require a response carrying that exact challenge.
    pub fn snapshot(&mut self, deadline: Instant) -> io::Result<Frame> {
        let result = (|| {
            self.ensure_healthy()?;
            check_deadline(deadline)?;
            ensure_no_pending_bytes(&self.stream, deadline)?;
            let challenge = self.allocate_challenge()?;
            self.write_packet(Packet::SnapshotRequest(challenge), challenge, deadline)?;
            match self.read_packet(deadline)? {
                Packet::SnapshotResponse {
                    challenge: response_challenge,
                    frame,
                } if response_challenge == challenge
                    && frame.target() == self.target
                    && frame.project_identity() == self.project
                    && frame.provider_version() == self.version
                    && frame.bridge_instance() == self.instance =>
                {
                    Ok(frame)
                }
                Packet::SnapshotResponse { .. } => {
                    Err(invalid_data("snapshot response scope is invalid"))
                }
                _ => Err(invalid_data("snapshot response kind is invalid")),
            }
        })();
        self.finish(result)
    }

    /// Validate an explicit session ID without changing the provider route.
    /// The result is bounded to the granted project and is not a selected snapshot.
    pub fn validate_session(
        &mut self,
        session_id: &str,
        deadline: Instant,
    ) -> io::Result<ValidationOutcome> {
        let result = (|| {
            self.ensure_healthy()?;
            let payload = recovery::encode_validate_request(session_id)
                .ok_or_else(|| invalid_input("recovery session ID is invalid"))?;
            let deadline = control_deadline(deadline)?;
            ensure_no_pending_bytes(&self.stream, deadline)?;
            let challenge = self.allocate_challenge()?;
            self.write_packet(
                Packet::RecoveryRequest {
                    kind: recovery::VALIDATE_REQUEST,
                    challenge,
                    payload,
                },
                challenge,
                deadline,
            )?;
            match self.read_packet(deadline)? {
                Packet::RecoveryResponse {
                    kind,
                    challenge: response_challenge,
                    payload,
                } if kind == recovery::VALIDATE_RESPONSE && response_challenge == challenge => {
                    let outcome =
                        recovery::decode_validate_response(session_id, &self.project, &payload)
                            .ok_or_else(|| {
                                invalid_data("recovery validation response is invalid")
                            })?;
                    self.validated_session = match &outcome {
                        ValidationOutcome::Verified(session) => {
                            Some(session.session_id().to_owned())
                        }
                        _ => None,
                    };
                    Ok(outcome)
                }
                _ => Err(invalid_data("recovery validation response kind is invalid")),
            }
        })();
        self.finish(result)
    }

    /// Issue one explicit provider route action after exact-ID validation.
    /// Success means the native action was issued, not that selection was observed.
    pub fn navigate_session(
        &mut self,
        session_id: &str,
        prepared_revision: u64,
        host_ticket: &[u8; 16],
        deadline: Instant,
    ) -> io::Result<NavigateOutcome> {
        let result = (|| {
            self.ensure_healthy()?;
            if self.validated_session.as_deref() != Some(session_id) || self.navigation_attempted {
                return Err(permission_denied("recovery navigation is not prepared"));
            }
            let payload =
                recovery::encode_navigate_request(session_id, prepared_revision, host_ticket)
                    .ok_or_else(|| invalid_input("recovery navigation request is invalid"))?;
            let request = recovery::decode_navigate_request(&payload)
                .ok_or_else(|| invalid_input("recovery navigation request is invalid"))?;
            let deadline = control_deadline(deadline)?;
            ensure_no_pending_bytes(&self.stream, deadline)?;
            let challenge = self.allocate_challenge()?;
            self.navigation_attempted = true;
            self.write_packet(
                Packet::RecoveryRequest {
                    kind: recovery::NAVIGATE_REQUEST,
                    challenge,
                    payload,
                },
                challenge,
                deadline,
            )?;
            match self.read_packet(deadline)? {
                Packet::RecoveryResponse {
                    kind,
                    challenge: response_challenge,
                    payload,
                } if kind == recovery::NAVIGATE_RESPONSE && response_challenge == challenge => {
                    recovery::decode_navigate_response(&request, &payload)
                        .ok_or_else(|| invalid_data("recovery navigation response is invalid"))
                }
                _ => Err(invalid_data("recovery navigation response kind is invalid")),
            }
        })();
        self.finish(result)
    }

    fn allocate_challenge(&mut self) -> io::Result<u64> {
        let challenge = self.next_challenge;
        let next = challenge
            .checked_add(1)
            .ok_or_else(|| invalid_data("snapshot challenge exhausted"))?;
        if challenge == 0 {
            return Err(invalid_data("snapshot challenge exhausted"));
        }
        self.next_challenge = next;
        Ok(challenge)
    }

    /// Whether a failed or incomplete operation permanently closed this channel.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn read_packet(&mut self, deadline: Instant) -> io::Result<Packet> {
        self.ensure_healthy()?;
        let mut header = [0u8; HEADER_BYTES];
        read_exact_until(&mut self.stream, &mut header, deadline)?;
        let (kind, len, challenge) = parse_header(&header)?;
        let mut payload = vec![0; len];
        read_exact_until(&mut self.stream, &mut payload, deadline)?;
        check_deadline(deadline)?;
        match kind {
            HELLO if challenge == 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Hello(hello) => Ok(Packet::Hello(hello)),
                _ => Err(invalid_data("fresh bridge Hello payload is invalid")),
            },
            WELCOME if challenge == 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Welcome { bridge_instance } => Ok(Packet::Welcome(bridge_instance)),
                _ => Err(invalid_data("fresh bridge Welcome payload is invalid")),
            },
            SNAPSHOT_REQUEST if len == 0 && challenge != 0 => {
                Ok(Packet::SnapshotRequest(challenge))
            }
            SNAPSHOT_RESPONSE if challenge != 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Observation(frame) => Ok(Packet::SnapshotResponse { challenge, frame }),
                _ => Err(invalid_data("fresh bridge snapshot payload is invalid")),
            },
            recovery::VALIDATE_REQUEST | recovery::NAVIGATE_REQUEST if challenge != 0 => {
                recovery::decode_recovery_request(kind, &payload)
                    .ok_or_else(|| invalid_data("fresh bridge recovery request is invalid"))?;
                Ok(Packet::RecoveryRequest {
                    kind,
                    challenge,
                    payload,
                })
            }
            recovery::VALIDATE_RESPONSE | recovery::NAVIGATE_RESPONSE if challenge != 0 => {
                Ok(Packet::RecoveryResponse {
                    kind,
                    challenge,
                    payload,
                })
            }
            _ => Err(invalid_data("fresh bridge packet is invalid")),
        }
    }

    fn write_packet(
        &mut self,
        packet: Packet,
        challenge: u64,
        deadline: Instant,
    ) -> io::Result<()> {
        let (kind, payload) = match packet {
            Packet::Hello(hello) => (
                HELLO,
                encode(&Message::Hello(hello))
                    .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::Welcome(instance) => (
                WELCOME,
                encode(&Message::Welcome {
                    bridge_instance: instance,
                })
                .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::SnapshotRequest(_) => (SNAPSHOT_REQUEST, Vec::new()),
            Packet::SnapshotResponse { frame, .. } => (
                SNAPSHOT_RESPONSE,
                encode(&Message::Observation(frame))
                    .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::RecoveryRequest { kind, payload, .. }
            | Packet::RecoveryResponse { kind, payload, .. } => (kind, payload),
        };
        if payload.len() > MAX_MESSAGE_BYTES
            || (kind <= WELCOME && challenge != 0)
            || (kind >= SNAPSHOT_REQUEST && challenge == 0)
        {
            return Err(invalid_input("fresh bridge packet is invalid"));
        }
        let mut header = [0u8; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&kind.to_be_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        header[16..24].copy_from_slice(&challenge.to_be_bytes());
        write_all_until(&mut self.stream, &header, deadline)?;
        write_all_until(&mut self.stream, &payload, deadline)
    }

    fn finish<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
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

impl fmt::Debug for FreshReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FreshReader { <redacted> }")
    }
}

/// A source endpoint which samples only after receiving a valid fresh challenge.
pub struct FreshSender {
    stream: UnixStream,
    target: TargetRef,
    project: Vec<u8>,
    version: String,
    instance: u64,
    last_challenge: u64,
    validated_session: Option<String>,
    navigation_attempted: bool,
    poisoned: bool,
}

impl FreshSender {
    /// Authenticate the peer, send the provisioned Hello, and require Welcome.
    pub fn connect(stream: UnixStream, hello: Hello, deadline: Instant) -> io::Result<Self> {
        check_deadline(deadline)?;
        require_same_user(&stream)?;
        configure_stream(&stream)?;
        let mut sender = Self {
            stream,
            target: hello.target(),
            project: hello.project_identity().to_vec(),
            version: hello.provider_version().to_owned(),
            instance: hello.bridge_instance(),
            last_challenge: 0,
            validated_session: None,
            navigation_attempted: false,
            poisoned: false,
        };
        let result = (|| {
            sender.write_packet(Packet::Hello(hello), 0, deadline)?;
            match sender.read_packet(deadline)? {
                Packet::Welcome(instance) if instance == sender.instance => Ok(()),
                _ => Err(invalid_data("fresh bridge Welcome is invalid")),
            }
        })();
        if let Err(error) = result {
            sender.poison();
            return Err(error);
        }
        Ok(sender)
    }

    /// Receive the server-issued kind-5 Hello, then complete the normal handshake.
    pub fn connect_provisioned(mut stream: UnixStream, deadline: Instant) -> io::Result<Self> {
        check_deadline(deadline)?;
        require_same_user(&stream)?;
        configure_stream(&stream)?;
        let parsed = (|| {
            let (kind, challenge, payload) = read_outer(&mut stream, deadline)?;
            if kind != PROVISION || challenge != 0 {
                return Err(invalid_data("fresh bridge provisioning packet is invalid"));
            }
            match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge provisioning payload is invalid"))?
            {
                Message::Hello(hello) => Ok(hello),
                _ => Err(invalid_data("fresh bridge provisioning payload is invalid")),
            }
        })();
        let hello = match parsed {
            Ok(hello) => hello,
            Err(error) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return Err(error);
            }
        };
        // `connect` rechecks the peer before it writes the normal Hello and
        // poisons the stream if that handshake fails.
        Self::connect(stream, hello, deadline)
    }

    /// Answer one request. `sample` runs exactly once and only after a valid, increasing challenge.
    pub fn respond_once<E, F>(&mut self, deadline: Instant, sample: F) -> io::Result<()>
    where
        F: FnOnce() -> Result<Frame, E>,
    {
        let result = (|| {
            self.ensure_healthy()?;
            let challenge = match self.read_packet(deadline)? {
                Packet::SnapshotRequest(value) if value > self.last_challenge => value,
                Packet::SnapshotRequest(_) => {
                    return Err(invalid_data("snapshot challenge did not advance"))
                }
                _ => return Err(invalid_data("snapshot request is invalid")),
            };
            self.last_challenge = challenge;
            let frame = sample().map_err(|_| invalid_data("source snapshot failed"))?;
            if frame.target() != self.target
                || frame.project_identity() != self.project
                || frame.provider_version() != self.version
                || frame.bridge_instance() != self.instance
            {
                return Err(invalid_data("snapshot response scope is invalid"));
            }
            self.write_packet(
                Packet::SnapshotResponse { challenge, frame },
                challenge,
                deadline,
            )
        })();
        self.finish(result)
    }

    /// Handle one typed exact-ID validation request. The callback receives only
    /// a bounded session ID, after the challenge and request have been checked.
    pub fn respond_validate_once<F>(&mut self, deadline: Instant, validate: F) -> io::Result<()>
    where
        F: FnOnce(&str) -> ValidationDecision,
    {
        let result = (|| {
            self.ensure_healthy()?;
            let deadline = control_deadline(deadline)?;
            let (challenge, request) = match self
                .read_recovery_request(recovery::VALIDATE_REQUEST, deadline)?
            {
                (challenge, recovery::RecoveryRequest::Validate(request)) => (challenge, request),
                _ => return Err(invalid_data("recovery validation request is invalid")),
            };
            let decision = if check_deadline(deadline).is_ok() {
                validate(request.session_id())
            } else {
                ValidationDecision::Uncertain
            };
            let payload = recovery::encode_validate_response(&request, decision, &self.project)
                .ok_or_else(|| invalid_data("recovery validation result is invalid"))?;
            let outcome =
                recovery::decode_validate_response(request.session_id(), &self.project, &payload)
                    .ok_or_else(|| invalid_data("recovery validation result is invalid"))?;
            self.validated_session = match outcome {
                ValidationOutcome::Verified(session) => Some(session.session_id().to_owned()),
                _ => None,
            };
            self.write_packet(
                Packet::RecoveryResponse {
                    kind: recovery::VALIDATE_RESPONSE,
                    challenge,
                    payload,
                },
                challenge,
                deadline,
            )
        })();
        self.finish(result)
    }

    /// Handle one navigation callback only after this connection validated the
    /// same exact session ID. The response reports action issuance, not selection.
    pub fn respond_navigate_once<F>(&mut self, deadline: Instant, navigate: F) -> io::Result<()>
    where
        F: FnOnce(&str, u64, &[u8; 16]) -> NavigationDecision,
    {
        let result = (|| {
            self.ensure_healthy()?;
            let deadline = control_deadline(deadline)?;
            let (challenge, request) = match self
                .read_recovery_request(recovery::NAVIGATE_REQUEST, deadline)?
            {
                (challenge, recovery::RecoveryRequest::Navigate(request)) => (challenge, request),
                _ => return Err(invalid_data("recovery navigation request is invalid")),
            };
            if self.navigation_attempted
                || self.validated_session.as_deref() != Some(request.session_id())
            {
                self.navigation_attempted = true;
                return Err(permission_denied("recovery navigation is not validated"));
            }
            self.navigation_attempted = true;
            let decision = if check_deadline(deadline).is_ok() {
                navigate(
                    request.session_id(),
                    request.prepared_revision(),
                    request.host_ticket(),
                )
            } else {
                NavigationDecision::NotIssued
            };
            let payload = recovery::encode_navigate_response(&request, decision);
            self.write_packet(
                Packet::RecoveryResponse {
                    kind: recovery::NAVIGATE_RESPONSE,
                    challenge,
                    payload,
                },
                challenge,
                deadline,
            )
        })();
        self.finish(result)
    }

    fn read_recovery_request(
        &mut self,
        expected_kind: u16,
        deadline: Instant,
    ) -> io::Result<(u64, recovery::RecoveryRequest)> {
        let (challenge, kind, payload) = match self.read_packet(deadline)? {
            Packet::RecoveryRequest {
                kind,
                challenge,
                payload,
            } => (challenge, kind, payload),
            _ => return Err(invalid_data("recovery request is invalid")),
        };
        if challenge <= self.last_challenge || kind != expected_kind {
            return Err(invalid_data(
                "recovery challenge or request kind is invalid",
            ));
        }
        let request = recovery::decode_recovery_request(kind, &payload)
            .ok_or_else(|| invalid_data("recovery request payload is invalid"))?;
        self.last_challenge = challenge;
        Ok((challenge, request))
    }

    /// Whether a failed or incomplete operation permanently closed this channel.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    fn read_packet(&mut self, deadline: Instant) -> io::Result<Packet> {
        self.ensure_healthy()?;
        let mut header = [0u8; HEADER_BYTES];
        read_exact_until(&mut self.stream, &mut header, deadline)?;
        let (kind, len, challenge) = parse_header(&header)?;
        let mut payload = vec![0; len];
        read_exact_until(&mut self.stream, &mut payload, deadline)?;
        check_deadline(deadline)?;
        match kind {
            HELLO if challenge == 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Hello(hello) => Ok(Packet::Hello(hello)),
                _ => Err(invalid_data("fresh bridge Hello payload is invalid")),
            },
            WELCOME if challenge == 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Welcome { bridge_instance } => Ok(Packet::Welcome(bridge_instance)),
                _ => Err(invalid_data("fresh bridge Welcome payload is invalid")),
            },
            SNAPSHOT_REQUEST if len == 0 && challenge != 0 => {
                Ok(Packet::SnapshotRequest(challenge))
            }
            SNAPSHOT_RESPONSE if challenge != 0 => match decode(&payload)
                .map_err(|_| invalid_data("fresh bridge payload is invalid"))?
            {
                Message::Observation(frame) => Ok(Packet::SnapshotResponse { challenge, frame }),
                _ => Err(invalid_data("fresh bridge snapshot payload is invalid")),
            },
            recovery::VALIDATE_REQUEST | recovery::NAVIGATE_REQUEST if challenge != 0 => {
                recovery::decode_recovery_request(kind, &payload)
                    .ok_or_else(|| invalid_data("fresh bridge recovery request is invalid"))?;
                Ok(Packet::RecoveryRequest {
                    kind,
                    challenge,
                    payload,
                })
            }
            recovery::VALIDATE_RESPONSE | recovery::NAVIGATE_RESPONSE if challenge != 0 => {
                Ok(Packet::RecoveryResponse {
                    kind,
                    challenge,
                    payload,
                })
            }
            _ => Err(invalid_data("fresh bridge packet is invalid")),
        }
    }
    fn write_packet(
        &mut self,
        packet: Packet,
        challenge: u64,
        deadline: Instant,
    ) -> io::Result<()> {
        let (kind, payload) = match packet {
            Packet::Hello(hello) => (
                HELLO,
                encode(&Message::Hello(hello))
                    .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::Welcome(instance) => (
                WELCOME,
                encode(&Message::Welcome {
                    bridge_instance: instance,
                })
                .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::SnapshotRequest(_) => (SNAPSHOT_REQUEST, Vec::new()),
            Packet::SnapshotResponse { frame, .. } => (
                SNAPSHOT_RESPONSE,
                encode(&Message::Observation(frame))
                    .map_err(|_| invalid_input("fresh bridge message is invalid"))?,
            ),
            Packet::RecoveryRequest { kind, payload, .. }
            | Packet::RecoveryResponse { kind, payload, .. } => (kind, payload),
        };
        if payload.len() > MAX_MESSAGE_BYTES
            || (kind <= WELCOME && challenge != 0)
            || (kind >= SNAPSHOT_REQUEST && challenge == 0)
        {
            return Err(invalid_input("fresh bridge packet is invalid"));
        }
        let mut header = [0u8; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&kind.to_be_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        header[16..24].copy_from_slice(&challenge.to_be_bytes());
        write_all_until(&mut self.stream, &header, deadline)?;
        write_all_until(&mut self.stream, &payload, deadline)
    }
    fn finish<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
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

impl fmt::Debug for FreshSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FreshSender { <redacted> }")
    }
}

enum Packet {
    Hello(Hello),
    Welcome(u64),
    SnapshotRequest(u64),
    SnapshotResponse {
        challenge: u64,
        frame: Frame,
    },
    RecoveryRequest {
        kind: u16,
        challenge: u64,
        payload: Vec<u8>,
    },
    RecoveryResponse {
        kind: u16,
        challenge: u64,
        payload: Vec<u8>,
    },
}

fn parse_header(h: &[u8; HEADER_BYTES]) -> io::Result<(u16, usize, u64)> {
    if &h[..8] != MAGIC || u16::from_be_bytes([h[8], h[9]]) != VERSION {
        return Err(invalid_data("fresh bridge header is invalid"));
    }
    let kind = u16::from_be_bytes([h[10], h[11]]);
    let len = u32::from_be_bytes([h[12], h[13], h[14], h[15]]) as usize;
    let challenge = u64::from_be_bytes(
        h[16..24]
            .try_into()
            .map_err(|_| invalid_data("fresh bridge header is invalid"))?,
    );
    if len > MAX_MESSAGE_BYTES
        || (kind == SNAPSHOT_REQUEST && len != 0)
        || !matches!(
            kind,
            HELLO
                | WELCOME
                | SNAPSHOT_REQUEST
                | SNAPSHOT_RESPONSE
                | PROVISION
                | recovery::VALIDATE_REQUEST
                | recovery::VALIDATE_RESPONSE
                | recovery::NAVIGATE_REQUEST
                | recovery::NAVIGATE_RESPONSE
        )
        || (matches!(kind, HELLO | WELCOME | PROVISION) && challenge != 0)
        || (matches!(kind, SNAPSHOT_REQUEST | SNAPSHOT_RESPONSE) && challenge == 0)
        || (matches!(
            kind,
            recovery::VALIDATE_REQUEST
                | recovery::VALIDATE_RESPONSE
                | recovery::NAVIGATE_REQUEST
                | recovery::NAVIGATE_RESPONSE
        ) && challenge == 0)
        || (kind == HELLO && len < 32)
        || (kind == WELCOME && len < 32)
        || (kind == SNAPSHOT_RESPONSE && len < 32)
        || (kind == PROVISION && len < 32)
    {
        return Err(invalid_data("fresh bridge header is invalid"));
    }
    Ok((kind, len, challenge))
}

fn read_outer(stream: &mut UnixStream, deadline: Instant) -> io::Result<(u16, u64, Vec<u8>)> {
    let mut header = [0u8; HEADER_BYTES];
    read_exact_until(stream, &mut header, deadline)?;
    let (kind, len, challenge) = parse_header(&header)?;
    let mut payload = vec![0; len];
    read_exact_until(stream, &mut payload, deadline)?;
    check_deadline(deadline)?;
    Ok((kind, challenge, payload))
}

fn write_outer(
    stream: &mut UnixStream,
    kind: u16,
    challenge: u64,
    payload: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    if payload.len() > MAX_MESSAGE_BYTES
        || (matches!(kind, HELLO | WELCOME | PROVISION) && challenge != 0)
        || (matches!(kind, SNAPSHOT_REQUEST | SNAPSHOT_RESPONSE) && challenge == 0)
        || (kind == SNAPSHOT_REQUEST && !payload.is_empty())
    {
        return Err(invalid_input("fresh bridge packet is invalid"));
    }
    let mut header = [0u8; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_be_bytes());
    header[10..12].copy_from_slice(&kind.to_be_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    header[16..24].copy_from_slice(&challenge.to_be_bytes());
    write_all_until(stream, &header, deadline)?;
    write_all_until(stream, payload, deadline)
}

fn configure_stream(stream: &UnixStream) -> io::Result<()> {
    set_socket_nosigpipe(stream.as_fd(), true)?;
    let fd = stream.as_fd();
    fcntl_setfd(fd, fcntl_getfd(fd)? | FdFlags::CLOEXEC)?;
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
    Ok(())
}
fn read_exact_until(stream: &mut UnixStream, out: &mut [u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < out.len() {
        check_deadline(deadline)?;
        match stream.read(&mut out[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "fresh bridge stream ended",
                ))
            }
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(e) => return Err(e),
        }
    }
    check_deadline(deadline)
}
fn write_all_until(stream: &mut UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        check_deadline(deadline)?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "fresh bridge write was empty",
                ))
            }
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(e) => return Err(e),
        }
    }
    check_deadline(deadline)
}
fn ensure_no_pending_bytes(stream: &UnixStream, deadline: Instant) -> io::Result<()> {
    let mut byte = [0u8; 1];
    loop {
        check_deadline(deadline)?;
        // SAFETY: the stream keeps its borrowed descriptor live for the call;
        // `byte` is writable storage of the declared one-byte length. MSG_PEEK
        // leaves any queued protocol data untouched.
        let result = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                byte.as_mut_ptr().cast(),
                byte.len(),
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if result >= 0 {
            match result {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "fresh bridge peer closed",
                    ));
                }
                _ => return Err(invalid_data("fresh bridge data preceded snapshot request")),
            }
        } else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
    }
}
fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "fresh bridge deadline expired",
        ))
    } else {
        Ok(())
    }
}
fn control_deadline(deadline: Instant) -> io::Result<Instant> {
    check_deadline(deadline)?;
    Ok(deadline.min(Instant::now() + Duration::from_millis(recovery::CONTROL_DEADLINE_MS)))
}
fn wait(deadline: Instant) -> io::Result<()> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "fresh bridge deadline expired",
        ));
    }
    std::thread::sleep(left.min(Duration::from_millis(1)));
    check_deadline(deadline)
}
fn invalid_data(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn invalid_input(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, s)
}
fn permission_denied(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, s)
}
fn poisoned_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "fresh bridge channel is poisoned",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_bridge::{FrameScope, Selection, SUPPORTED_PROVIDER_VERSION};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::thread;

    fn target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 12,
            pane: 13,
            session: 14,
            occupant_revision: 15,
        }
    }
    fn grant() -> Grant {
        Grant::new(
            target(),
            b"/work/project",
            SUPPORTED_PROVIDER_VERSION,
            19,
            [7; 32],
        )
        .unwrap()
    }
    fn frame(seq: u64) -> Frame {
        Frame::new(
            1,
            target(),
            b"/work/project",
            SUPPORTED_PROVIDER_VERSION,
            19,
            seq,
            FrameScope::TopLevel,
            Selection::Home,
        )
        .unwrap()
    }
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }

    fn request_packet(challenge: u64) -> [u8; HEADER_BYTES] {
        let mut header = [0; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&SNAPSHOT_REQUEST.to_be_bytes());
        header[16..24].copy_from_slice(&challenge.to_be_bytes());
        header
    }

    #[test]
    fn paired_snapshot_callback_runs_after_each_new_request_and_returns_scoped_frames() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = grant().hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            for expected in 1..=2 {
                let calls_in = calls.clone();
                sender
                    .respond_once(deadline(), || {
                        assert_eq!(calls_in.load(Ordering::SeqCst), expected - 1);
                        calls_in.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ()>(frame(expected as u64))
                    })
                    .unwrap();
            }
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        assert_eq!(reader.snapshot(deadline()).unwrap(), frame(1));
        assert_eq!(reader.snapshot(deadline()).unwrap(), frame(2));
        peer.join().unwrap();
    }

    #[test]
    fn server_provisioned_grant_establishes_fresh_channel_without_secret_env_handoff() {
        let (server_stream, client_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let server = thread::spawn(move || {
            let mut stream = server_stream;
            provision(&mut stream, &use_grant, deadline()).unwrap();
            let mut reader = FreshReader::accept(stream, use_grant, deadline()).unwrap();
            reader.snapshot(deadline()).unwrap()
        });
        let client = thread::spawn(move || {
            let mut sender = FreshSender::connect_provisioned(client_stream, deadline()).unwrap();
            sender
                .respond_once(deadline(), || Ok::<_, ()>(frame(1)))
                .unwrap();
        });
        assert_eq!(server.join().unwrap(), frame(1));
        client.join().unwrap();
    }

    #[test]
    fn prebuffered_response_with_old_challenge_is_rejected_and_never_sampled() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut stream = sender_stream;
            write_raw(
                &mut stream,
                HELLO,
                0,
                &encode(&Message::Hello(hello)).unwrap(),
            );
            let (kind, _, _) = read_raw(&mut stream);
            assert_eq!(kind, WELCOME);
            let payload = encode(&Message::Observation(frame(1))).unwrap();
            // Deliberately send a plausible first response before the reader
            // issues challenge 1. The reader must see queued bytes and refuse.
            write_raw(&mut stream, SNAPSHOT_RESPONSE, 1, &payload);
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        peer.join().unwrap();
        let err = reader.snapshot(deadline()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(reader.is_poisoned());
    }

    fn write_raw(stream: &mut UnixStream, kind: u16, challenge: u64, payload: &[u8]) {
        let mut header = [0; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&kind.to_be_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        header[16..24].copy_from_slice(&challenge.to_be_bytes());
        stream.write_all(&header).unwrap();
        stream.write_all(payload).unwrap();
    }

    fn read_raw(stream: &mut UnixStream) -> (u16, Vec<u8>, u64) {
        let mut header = [0; HEADER_BYTES];
        stream.read_exact(&mut header).unwrap();
        let len = u32::from_be_bytes(header[12..16].try_into().unwrap()) as usize;
        let kind = u16::from_be_bytes(header[10..12].try_into().unwrap());
        let challenge = u64::from_be_bytes(header[16..24].try_into().unwrap());
        let mut payload = vec![0; len];
        stream.read_exact(&mut payload).unwrap();
        (kind, payload, challenge)
    }

    #[test]
    fn repeated_challenge_poisoning_does_not_run_callback_twice() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let count_in = count.clone();
            sender
                .respond_once(deadline(), || {
                    count_in.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(frame(1))
                })
                .unwrap();
            let count_in = count.clone();
            let error = sender
                .respond_once(deadline(), || {
                    count_in.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(frame(2))
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert!(sender.is_poisoned());
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        reader
            .write_packet(Packet::SnapshotRequest(5), 5, deadline())
            .unwrap();
        let response = reader.read_packet(deadline()).unwrap();
        assert!(matches!(
            response,
            Packet::SnapshotResponse { challenge: 5, .. }
        ));
        reader
            .write_packet(Packet::SnapshotRequest(4), 4, deadline())
            .unwrap();
        peer.join().unwrap();
    }

    #[test]
    fn invalid_request_never_calls_sampler_and_wrong_scope_poisoned_after_one_sample() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let hello = grant().hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let count_in = count.clone();
            let error = sender
                .respond_once(deadline(), || {
                    count_in.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(frame(1))
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(sender.is_poisoned());
        });
        let mut reader = FreshReader::accept(reader_stream, grant(), deadline()).unwrap();
        reader
            .write_packet(
                Packet::SnapshotResponse {
                    challenge: 1,
                    frame: frame(1),
                },
                1,
                deadline(),
            )
            .unwrap();
        peer.join().unwrap();

        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let count_in = count.clone();
            let error = sender
                .respond_once(deadline(), || {
                    count_in.fetch_add(1, Ordering::SeqCst);
                    Frame::new(
                        1,
                        TargetRef {
                            pane: 99,
                            ..target()
                        },
                        b"/work/project",
                        SUPPORTED_PROVIDER_VERSION,
                        19,
                        1,
                        FrameScope::TopLevel,
                        Selection::Home,
                    )
                    .map_err(|_| ())
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            assert!(sender.is_poisoned());
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        reader
            .write_packet(Packet::SnapshotRequest(1), 1, deadline())
            .unwrap();
        peer.join().unwrap();

        let (reader_stream, mut raw_peer) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            write_raw(
                &mut raw_peer,
                HELLO,
                0,
                &encode(&Message::Hello(hello)).unwrap(),
            );
            let (kind, _, _) = read_raw(&mut raw_peer);
            assert_eq!(kind, WELCOME);
            let (kind, _, challenge) = read_raw(&mut raw_peer);
            assert_eq!(kind, SNAPSHOT_REQUEST);
            let wrong = Frame::new(
                1,
                TargetRef {
                    pane: 99,
                    ..target()
                },
                b"/work/project",
                SUPPORTED_PROVIDER_VERSION,
                19,
                1,
                FrameScope::TopLevel,
                Selection::Home,
            )
            .unwrap();
            write_raw(
                &mut raw_peer,
                SNAPSHOT_RESPONSE,
                challenge,
                &encode(&Message::Observation(wrong)).unwrap(),
            );
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        let error = reader.snapshot(deadline()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(reader.is_poisoned());
        peer.join().unwrap();
    }

    #[test]
    fn partial_header_and_expired_snapshot_deadline_poison_the_reader() {
        let (reader_stream, mut peer) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let partial_peer = thread::spawn(move || {
            write_raw(
                &mut peer,
                HELLO,
                0,
                &encode(&Message::Hello(hello)).unwrap(),
            );
            let (kind, _, _) = read_raw(&mut peer);
            assert_eq!(kind, WELCOME);
            let (kind, _, _) = read_raw(&mut peer);
            assert_eq!(kind, SNAPSHOT_REQUEST);
            peer.write_all(&MAGIC[..7]).unwrap();
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        let error = reader.snapshot(deadline()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert!(reader.is_poisoned());
        partial_peer.join().unwrap();

        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let quiet_peer = thread::spawn(move || {
            let mut stream = sender_stream;
            write_raw(
                &mut stream,
                HELLO,
                0,
                &encode(&Message::Hello(hello)).unwrap(),
            );
            let (kind, _, _) = read_raw(&mut stream);
            assert_eq!(kind, WELCOME);
            let (kind, _, _) = read_raw(&mut stream);
            assert_eq!(kind, SNAPSHOT_REQUEST);
            thread::sleep(Duration::from_millis(80));
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        let expired = Instant::now() + Duration::from_millis(10);
        let error = reader.snapshot(expired).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(reader.is_poisoned());
        quiet_peer.join().unwrap();
    }

    #[test]
    fn header_bounds_version_kind_and_challenge_are_exact() {
        assert!(parse_header(&request_packet(1)).is_ok());
        assert!(parse_header(&request_packet(0)).is_err());
        let mut bad = request_packet(1);
        bad[8..10].copy_from_slice(&1u16.to_be_bytes());
        assert!(parse_header(&bad).is_err());
        let mut bad = request_packet(1);
        bad[10..12].copy_from_slice(&99u16.to_be_bytes());
        assert!(parse_header(&bad).is_err());
        let mut bad = request_packet(1);
        bad[12..16].copy_from_slice(&1u32.to_be_bytes());
        assert!(parse_header(&bad).is_err());
        let mut bad = request_packet(1);
        bad[12..16].copy_from_slice(&((MAX_MESSAGE_BYTES + 1) as u32).to_be_bytes());
        assert!(parse_header(&bad).is_err());
        assert!(format!("{:?}", grant()).contains("redacted"));
        assert!(format!(
            "{:?}",
            FreshReader {
                stream: UnixStream::pair().unwrap().0,
                target: target(),
                project: b"secret-project".to_vec(),
                version: SUPPORTED_PROVIDER_VERSION.to_owned(),
                instance: 19,
                next_challenge: 1,
                validated_session: None,
                navigation_attempted: false,
                poisoned: false
            }
        )
        .contains("redacted"));
    }

    #[test]
    fn recovery_validation_navigation_and_snapshot_share_one_challenge_sequence() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            sender
                .respond_validate_once(deadline(), |session_id| {
                    assert_eq!(session_id, "ses_exact");
                    ValidationDecision::Verified {
                        project_identity: b"/work/project".to_vec(),
                    }
                })
                .unwrap();
            sender
                .respond_navigate_once(deadline(), |session_id, revision, ticket| {
                    assert_eq!(session_id, "ses_exact");
                    assert_eq!(revision, 44);
                    assert_eq!(ticket, &[5; 16]);
                    NavigationDecision::ActionIssued
                })
                .unwrap();
            sender
                .respond_once(deadline(), || Ok::<_, ()>(frame(9)))
                .unwrap();
            assert_eq!(sender.last_challenge, 3);
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        let validation = reader.validate_session("ses_exact", deadline()).unwrap();
        assert!(matches!(validation, ValidationOutcome::Verified(_)));
        assert_eq!(
            reader
                .navigate_session("ses_exact", 44, &[5; 16], deadline())
                .unwrap(),
            NavigateOutcome::ActionIssued
        );
        assert_eq!(reader.snapshot(deadline()).unwrap(), frame(9));
        peer.join().unwrap();
    }

    #[test]
    fn navigation_requires_validation_and_is_one_shot_even_when_not_issued() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            sender
                .respond_validate_once(deadline(), |_| ValidationDecision::Verified {
                    project_identity: b"/work/project".to_vec(),
                })
                .unwrap();
            sender
                .respond_navigate_once(deadline(), |_, _, _| NavigationDecision::NotIssued)
                .unwrap();
            assert!(sender.navigation_attempted);
        });
        let mut reader = FreshReader::accept(reader_stream, use_grant, deadline()).unwrap();
        reader.validate_session("ses_exact", deadline()).unwrap();
        assert_eq!(
            reader
                .navigate_session("ses_exact", 44, &[5; 16], deadline())
                .unwrap(),
            NavigateOutcome::NotIssued
        );
        let err = reader
            .navigate_session("ses_exact", 44, &[5; 16], deadline())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(reader.is_poisoned());
        peer.join().unwrap();
    }

    #[test]
    fn malformed_recovery_request_poisoned_before_source_callback() {
        let (reader_stream, sender_stream) = UnixStream::pair().unwrap();
        let use_grant = grant();
        let hello = use_grant.hello().unwrap();
        let peer = thread::spawn(move || {
            let mut sender = FreshSender::connect(sender_stream, hello, deadline()).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_in = calls.clone();
            let error = sender
                .respond_validate_once(deadline(), move |_| {
                    calls_in.fetch_add(1, Ordering::SeqCst);
                    ValidationDecision::Uncertain
                })
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(sender.is_poisoned());
        });
        let mut stream = reader_stream;
        let (kind, _, _) = read_raw(&mut stream);
        assert_eq!(kind, HELLO);
        write_raw(
            &mut stream,
            WELCOME,
            0,
            &encode(&Message::Welcome {
                bridge_instance: 19,
            })
            .unwrap(),
        );
        write_raw(&mut stream, recovery::VALIDATE_REQUEST, 1, &[0, 0]);
        peer.join().unwrap();
    }
}
