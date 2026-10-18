//! Pure schema-2 session-control and session-info payloads.
//!
//! This module encodes bounded metadata only. It does not authenticate peers,
//! issue or renew leases, check target ownership, admit ledger operations, or
//! detach a connection. Runtime dispatch and effect ordering remain separate.

use crate::control_protocol::TargetRef;
use crate::handshake_protocol::{decode_hello, encode_hello, HandshakeError, Hello};
use crate::protocol::OperationKey;
use std::error::Error;
use std::fmt;

/// Schema version used by session requests and session-info responses.
pub const SESSION_SCHEMA_VERSION: u16 = 2;

/// Exact size of a fresh schema-1 Hello payload.
pub const SESSION_HELLO_FRESH_BYTES: usize = 28;

/// Exact size of a resumable schema-2 Hello payload.
pub const SESSION_HELLO_RESUME_BYTES: usize = 44;

/// Maximum encoded size of one session request.
pub const MAX_SESSION_REQUEST_BYTES: usize = 88;

/// Exact encoded size of the initial session-info response.
pub const SESSION_INFO_BYTES: usize = 72;

const SESSION_INFO_KIND: u16 = 1;
const REQUEST_DESCRIBE: u16 = 1;
const REQUEST_CLAIM: u16 = 2;
const REQUEST_RELEASE: u16 = 3;
const REQUEST_HANDOFF: u16 = 4;
const REQUEST_OPERATION_STATUS: u16 = 5;
const REQUEST_RECEIPT: u16 = 6;
const REQUEST_DETACH: u16 = 7;
const MIN_OWNER_TTL_SECONDS: u32 = 1;
const MAX_OWNER_TTL_SECONDS: u32 = 30;

/// Previous server generation and client lease offered for same-generation resume.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LeaseRef {
    /// Server-start generation that issued the lease.
    pub generation: u64,
    /// Client lease issued within that generation.
    pub lease: u64,
}

impl LeaseRef {
    /// Validate the nonzero generation and lease identity.
    pub fn validate(&self) -> Result<(), SessionProtocolError> {
        if self.generation == 0 || self.lease == 0 {
            return Err(SessionProtocolError::ZeroIdentifier);
        }
        Ok(())
    }
}

impl fmt::Debug for LeaseRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseRef")
            .field("generation", &"<redacted>")
            .field("lease", &"<redacted>")
            .finish()
    }
}

/// Initial Hello fields with an optional prior lease for reconnect.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SessionHello {
    /// Existing schema-1 Hello capability request and nonce.
    pub hello: Hello,
    /// Present only when explicitly requesting resume of an old lease.
    pub resume: Option<LeaseRef>,
}

impl SessionHello {
    /// Validate the existing Hello fields and optional prior lease identity.
    pub fn validate(&self) -> Result<(), SessionProtocolError> {
        self.hello
            .validate()
            .map_err(SessionProtocolError::Handshake)?;
        if let Some(resume) = self.resume {
            resume.validate()?;
        }
        Ok(())
    }
}

impl fmt::Debug for SessionHello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionHello")
            .field("hello", &"<redacted>")
            .field(
                "resume",
                &if self.resume.is_some() {
                    "provided"
                } else {
                    "new"
                },
            )
            .finish()
    }
}

/// One deterministic schema-2 session request payload.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SessionRequest {
    /// Observe the current single-session inventory.
    Describe,
    /// Claim input ownership for a client lease and expected owner revision.
    Claim {
        /// Operation identity for deduplication.
        key: OperationKey,
        /// Full immutable pane/session identity.
        target: TargetRef,
        /// Compare-and-swap revision expected by the caller.
        expected_owner_revision: u64,
        /// Requested ownership TTL, in the prototype range 1 through 30.
        ttl_seconds: u32,
    },
    /// Release the caller's current input ownership.
    Release {
        /// Operation identity for deduplication.
        key: OperationKey,
        /// Full immutable pane/session identity.
        target: TargetRef,
        /// Compare-and-swap revision expected by the caller.
        expected_owner_revision: u64,
    },
    /// Hand input ownership to another client lease.
    Handoff {
        /// Operation identity for deduplication.
        key: OperationKey,
        /// Full immutable pane/session identity.
        target: TargetRef,
        /// Compare-and-swap revision expected by the caller.
        expected_owner_revision: u64,
        /// Recipient client lease. Human-owner handoff is not encoded here.
        recipient_lease: u64,
        /// Requested recipient ownership TTL, in the range 1 through 30.
        ttl_seconds: u32,
    },
    /// Observe the ledger status for an operation key.
    OperationStatus {
        /// Operation identity to inspect.
        key: OperationKey,
    },
    /// Retire completed operation results through a cumulative sequence.
    Receipt {
        /// Server-start generation containing the lease.
        generation: u64,
        /// Client lease whose completed results are being retired.
        lease: u64,
        /// Highest completed sequence to retire.
        through_sequence: u64,
    },
    /// Close this client attachment only.
    Detach,
}

impl SessionRequest {
    /// Validate identifiers, target generation, and bounded TTL fields.
    pub fn validate(&self) -> Result<(), SessionProtocolError> {
        match self {
            Self::Describe | Self::Detach => Ok(()),
            Self::Claim {
                key,
                target,
                expected_owner_revision,
                ttl_seconds,
            } => {
                validate_key_target(*key, *target, *expected_owner_revision)?;
                validate_ttl(*ttl_seconds)
            }
            Self::Release {
                key,
                target,
                expected_owner_revision,
            } => validate_key_target(*key, *target, *expected_owner_revision),
            Self::Handoff {
                key,
                target,
                expected_owner_revision,
                recipient_lease,
                ttl_seconds,
            } => {
                validate_key_target(*key, *target, *expected_owner_revision)?;
                if *recipient_lease == 0 {
                    return Err(SessionProtocolError::ZeroIdentifier);
                }
                validate_ttl(*ttl_seconds)
            }
            Self::OperationStatus { key } => validate_key(*key),
            Self::Receipt {
                generation,
                lease,
                through_sequence,
            } => {
                if *generation == 0 || *lease == 0 || *through_sequence == 0 {
                    return Err(SessionProtocolError::ZeroIdentifier);
                }
                Ok(())
            }
        }
    }

    /// Return the exact request size after validation.
    pub fn encoded_len(&self) -> Result<usize, SessionProtocolError> {
        self.validate()?;
        Ok(match self {
            Self::Describe | Self::Detach => 4,
            Self::Claim { .. } => 80,
            Self::Release { .. } => 76,
            Self::Handoff { .. } => 88,
            Self::OperationStatus { .. } | Self::Receipt { .. } => 28,
        })
    }
}

impl fmt::Debug for SessionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Describe => "Describe",
            Self::Claim { .. } => "Claim",
            Self::Release { .. } => "Release",
            Self::Handoff { .. } => "Handoff",
            Self::OperationStatus { .. } => "OperationStatus",
            Self::Receipt { .. } => "Receipt",
            Self::Detach => "Detach",
        };
        f.debug_struct("SessionRequest")
            .field("kind", &name)
            .finish()
    }
}

/// Current read-only session and client-owner state.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SessionInfo {
    /// Full immutable pane/session identity.
    pub target: TargetRef,
    /// Current owner revision, including while vacant.
    pub owner_revision: u64,
    /// Current client lease, or zero while vacant. Human owners are not
    /// represented by this initial client-only schema.
    pub owner_lease: u64,
    /// Current client-owner expiry time, or zero while vacant.
    pub owner_expires_secs: u64,
    /// Whether the managed child has exited.
    pub child_exited: bool,
}

impl SessionInfo {
    /// Validate identifiers and the client-owner/vacant field relationship.
    pub fn validate(&self) -> Result<(), SessionProtocolError> {
        validate_target(self.target)?;
        if self.owner_revision == 0 {
            return Err(SessionProtocolError::ZeroIdentifier);
        }
        if (self.owner_lease == 0) != (self.owner_expires_secs == 0) {
            return Err(SessionProtocolError::InvalidOwnerState);
        }
        Ok(())
    }
}

impl fmt::Debug for SessionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionInfo")
            .field("target", &"<redacted>")
            .field("owner_revision", &"<redacted>")
            .field("owner_lease", &"<redacted>")
            .field("owner_expires_secs", &"<redacted>")
            .field("child_exited", &self.child_exited)
            .finish()
    }
}

/// Fixed, redacted validation errors for session payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionProtocolError {
    /// An embedded schema-1 Hello field failed validation.
    Handshake(HandshakeError),
    /// The payload does not have the exact length required by its kind.
    InvalidLength,
    /// The payload schema version is unsupported.
    UnsupportedSchema,
    /// A request opcode or session-info response kind is unknown.
    UnknownKind,
    /// A reserved field is nonzero.
    NonzeroReserved,
    /// A key, target, owner revision, lease, or sequence is zero.
    ZeroIdentifier,
    /// An operation key and target name different server generations.
    GenerationMismatch,
    /// An ownership TTL is outside the range 1 through 30 seconds.
    InvalidTtl,
    /// A vacant/client owner lease and expiry pair is inconsistent.
    InvalidOwnerState,
    /// The session-info child-exited field is not zero or one.
    InvalidBoolean,
}

impl fmt::Display for SessionProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshake(error) => write!(f, "session Hello is invalid: {error}"),
            Self::InvalidLength => f.write_str("session payload has an invalid length"),
            Self::UnsupportedSchema => f.write_str("session payload schema is unsupported"),
            Self::UnknownKind => f.write_str("session payload kind is unknown"),
            Self::NonzeroReserved => f.write_str("session reserved field must be zero"),
            Self::ZeroIdentifier => f.write_str("session identifiers must be nonzero"),
            Self::GenerationMismatch => {
                f.write_str("session operation and target generations do not match")
            }
            Self::InvalidTtl => f.write_str("session ownership TTL is outside its limit"),
            Self::InvalidOwnerState => f.write_str("session owner lease state is inconsistent"),
            Self::InvalidBoolean => f.write_str("session boolean field is invalid"),
        }
    }
}

impl Error for SessionProtocolError {}

/// Encode a fresh schema-1 Hello or a schema-2 resumable Hello.
pub fn encode_session_hello(session_hello: &SessionHello) -> Result<Vec<u8>, SessionProtocolError> {
    session_hello.validate()?;
    match session_hello.resume {
        None => {
            let bytes =
                encode_hello(&session_hello.hello).map_err(SessionProtocolError::Handshake)?;
            Ok(bytes.to_vec())
        }
        Some(resume) => {
            let hello_bytes =
                encode_hello(&session_hello.hello).map_err(SessionProtocolError::Handshake)?;
            let mut bytes = [0_u8; SESSION_HELLO_RESUME_BYTES];
            bytes[..SESSION_HELLO_FRESH_BYTES].copy_from_slice(&hello_bytes);
            write_u16(&mut bytes, 0, SESSION_SCHEMA_VERSION);
            write_u64(&mut bytes, 28, resume.generation);
            write_u64(&mut bytes, 36, resume.lease);
            Ok(bytes.to_vec())
        }
    }
}

/// Decode a fresh schema-1 Hello or exact schema-2 resumable Hello.
pub fn decode_session_hello(bytes: &[u8]) -> Result<SessionHello, SessionProtocolError> {
    if bytes.len() == SESSION_HELLO_FRESH_BYTES {
        let hello = decode_hello(bytes).map_err(SessionProtocolError::Handshake)?;
        return Ok(SessionHello {
            hello,
            resume: None,
        });
    }
    if bytes.len() != SESSION_HELLO_RESUME_BYTES {
        return Err(SessionProtocolError::InvalidLength);
    }
    if read_u16(bytes, 0) != SESSION_SCHEMA_VERSION {
        return Err(SessionProtocolError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != 0 {
        return Err(SessionProtocolError::NonzeroReserved);
    }
    let hello = Hello {
        nonce: read_u64(bytes, 4),
        required_capabilities: read_u64(bytes, 12),
        optional_capabilities: read_u64(bytes, 20),
    };
    hello.validate().map_err(SessionProtocolError::Handshake)?;
    let resume = LeaseRef {
        generation: read_u64(bytes, 28),
        lease: read_u64(bytes, 36),
    };
    let session_hello = SessionHello {
        hello,
        resume: Some(resume),
    };
    session_hello.validate()?;
    Ok(session_hello)
}

/// Encode one request into its exact bounded schema-2 payload.
///
/// All fields are validated before the bounded output buffer is allocated.
pub fn encode_session_request(request: &SessionRequest) -> Result<Vec<u8>, SessionProtocolError> {
    let encoded_len = request.encoded_len()?;
    let mut bytes = Vec::with_capacity(encoded_len);
    push_u16(&mut bytes, SESSION_SCHEMA_VERSION);
    push_u16(&mut bytes, request_opcode(request));
    match request {
        SessionRequest::Describe | SessionRequest::Detach => {}
        SessionRequest::Claim {
            key,
            target,
            expected_owner_revision,
            ttl_seconds,
        } => {
            encode_key(&mut bytes, *key);
            encode_target(&mut bytes, *target);
            push_u64(&mut bytes, *expected_owner_revision);
            push_u32(&mut bytes, *ttl_seconds);
        }
        SessionRequest::Release {
            key,
            target,
            expected_owner_revision,
        } => {
            encode_key(&mut bytes, *key);
            encode_target(&mut bytes, *target);
            push_u64(&mut bytes, *expected_owner_revision);
        }
        SessionRequest::Handoff {
            key,
            target,
            expected_owner_revision,
            recipient_lease,
            ttl_seconds,
        } => {
            encode_key(&mut bytes, *key);
            encode_target(&mut bytes, *target);
            push_u64(&mut bytes, *expected_owner_revision);
            push_u64(&mut bytes, *recipient_lease);
            push_u32(&mut bytes, *ttl_seconds);
        }
        SessionRequest::OperationStatus { key } => encode_key(&mut bytes, *key),
        SessionRequest::Receipt {
            generation,
            lease,
            through_sequence,
        } => {
            push_u64(&mut bytes, *generation);
            push_u64(&mut bytes, *lease);
            push_u64(&mut bytes, *through_sequence);
        }
    }
    debug_assert_eq!(bytes.len(), encoded_len);
    Ok(bytes)
}

/// Decode exactly one schema-2 session request without allocating.
pub fn decode_session_request(bytes: &[u8]) -> Result<SessionRequest, SessionProtocolError> {
    if bytes.len() < 4 {
        return Err(SessionProtocolError::InvalidLength);
    }
    if read_u16(bytes, 0) != SESSION_SCHEMA_VERSION {
        return Err(SessionProtocolError::UnsupportedSchema);
    }
    let opcode = read_u16(bytes, 2);
    let expected_len = request_len(opcode).ok_or(SessionProtocolError::UnknownKind)?;
    if bytes.len() != expected_len {
        return Err(SessionProtocolError::InvalidLength);
    }
    let request = match opcode {
        REQUEST_DESCRIBE => SessionRequest::Describe,
        REQUEST_CLAIM => SessionRequest::Claim {
            key: decode_key(bytes, 4),
            target: decode_target(bytes, 28),
            expected_owner_revision: read_u64(bytes, 68),
            ttl_seconds: read_u32(bytes, 76),
        },
        REQUEST_RELEASE => SessionRequest::Release {
            key: decode_key(bytes, 4),
            target: decode_target(bytes, 28),
            expected_owner_revision: read_u64(bytes, 68),
        },
        REQUEST_HANDOFF => SessionRequest::Handoff {
            key: decode_key(bytes, 4),
            target: decode_target(bytes, 28),
            expected_owner_revision: read_u64(bytes, 68),
            recipient_lease: read_u64(bytes, 76),
            ttl_seconds: read_u32(bytes, 84),
        },
        REQUEST_OPERATION_STATUS => SessionRequest::OperationStatus {
            key: decode_key(bytes, 4),
        },
        REQUEST_RECEIPT => SessionRequest::Receipt {
            generation: read_u64(bytes, 4),
            lease: read_u64(bytes, 12),
            through_sequence: read_u64(bytes, 20),
        },
        REQUEST_DETACH => SessionRequest::Detach,
        _ => return Err(SessionProtocolError::UnknownKind),
    };
    request.validate()?;
    Ok(request)
}

/// Encode fixed-size schema-2 single-session info.
pub fn encode_session_info(
    info: &SessionInfo,
) -> Result<[u8; SESSION_INFO_BYTES], SessionProtocolError> {
    info.validate()?;
    let mut bytes = [0_u8; SESSION_INFO_BYTES];
    write_u16(&mut bytes, 0, SESSION_SCHEMA_VERSION);
    write_u16(&mut bytes, 2, SESSION_INFO_KIND);
    write_target(&mut bytes, 4, info.target);
    write_u64(&mut bytes, 44, info.owner_revision);
    write_u64(&mut bytes, 52, info.owner_lease);
    write_u64(&mut bytes, 60, info.owner_expires_secs);
    bytes[68] = u8::from(info.child_exited);
    Ok(bytes)
}

/// Decode exactly one fixed-size session-info response without allocating.
pub fn decode_session_info(bytes: &[u8]) -> Result<SessionInfo, SessionProtocolError> {
    if bytes.len() != SESSION_INFO_BYTES {
        return Err(SessionProtocolError::InvalidLength);
    }
    if read_u16(bytes, 0) != SESSION_SCHEMA_VERSION {
        return Err(SessionProtocolError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != SESSION_INFO_KIND {
        return Err(SessionProtocolError::UnknownKind);
    }
    if bytes[69..72] != [0, 0, 0] {
        return Err(SessionProtocolError::NonzeroReserved);
    }
    let child_exited = match bytes[68] {
        0 => false,
        1 => true,
        _ => return Err(SessionProtocolError::InvalidBoolean),
    };
    let info = SessionInfo {
        target: decode_target(bytes, 4),
        owner_revision: read_u64(bytes, 44),
        owner_lease: read_u64(bytes, 52),
        owner_expires_secs: read_u64(bytes, 60),
        child_exited,
    };
    info.validate()?;
    Ok(info)
}

fn request_opcode(request: &SessionRequest) -> u16 {
    match request {
        SessionRequest::Describe => REQUEST_DESCRIBE,
        SessionRequest::Claim { .. } => REQUEST_CLAIM,
        SessionRequest::Release { .. } => REQUEST_RELEASE,
        SessionRequest::Handoff { .. } => REQUEST_HANDOFF,
        SessionRequest::OperationStatus { .. } => REQUEST_OPERATION_STATUS,
        SessionRequest::Receipt { .. } => REQUEST_RECEIPT,
        SessionRequest::Detach => REQUEST_DETACH,
    }
}

const fn request_len(opcode: u16) -> Option<usize> {
    match opcode {
        REQUEST_DESCRIBE | REQUEST_DETACH => Some(4),
        REQUEST_CLAIM => Some(80),
        REQUEST_RELEASE => Some(76),
        REQUEST_HANDOFF => Some(88),
        REQUEST_OPERATION_STATUS | REQUEST_RECEIPT => Some(28),
        _ => None,
    }
}

fn validate_key_target(
    key: OperationKey,
    target: TargetRef,
    expected_owner_revision: u64,
) -> Result<(), SessionProtocolError> {
    validate_key(key)?;
    validate_target(target)?;
    if expected_owner_revision == 0 {
        return Err(SessionProtocolError::ZeroIdentifier);
    }
    if key.generation != target.generation {
        return Err(SessionProtocolError::GenerationMismatch);
    }
    Ok(())
}

fn validate_key(key: OperationKey) -> Result<(), SessionProtocolError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        return Err(SessionProtocolError::ZeroIdentifier);
    }
    Ok(())
}

fn validate_target(target: TargetRef) -> Result<(), SessionProtocolError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(SessionProtocolError::ZeroIdentifier);
    }
    Ok(())
}

fn validate_ttl(ttl_seconds: u32) -> Result<(), SessionProtocolError> {
    if !(MIN_OWNER_TTL_SECONDS..=MAX_OWNER_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(SessionProtocolError::InvalidTtl);
    }
    Ok(())
}

fn encode_key(bytes: &mut Vec<u8>, key: OperationKey) {
    push_u64(bytes, key.generation);
    push_u64(bytes, key.lease);
    push_u64(bytes, key.sequence);
}

fn encode_target(bytes: &mut Vec<u8>, target: TargetRef) {
    push_u64(bytes, target.generation);
    push_u64(bytes, target.workspace);
    push_u64(bytes, target.pane);
    push_u64(bytes, target.session);
    push_u64(bytes, target.occupant_revision);
}

fn write_target(bytes: &mut [u8], offset: usize, target: TargetRef) {
    write_u64(bytes, offset, target.generation);
    write_u64(bytes, offset + 8, target.workspace);
    write_u64(bytes, offset + 16, target.pane);
    write_u64(bytes, offset + 24, target.session);
    write_u64(bytes, offset + 32, target.occupant_revision);
}

fn decode_key(bytes: &[u8], offset: usize) -> OperationKey {
    OperationKey {
        generation: read_u64(bytes, offset),
        lease: read_u64(bytes, offset + 8),
        sequence: read_u64(bytes, offset + 16),
    }
}

fn decode_target(bytes: &[u8], offset: usize) -> TargetRef {
    TargetRef {
        generation: read_u64(bytes, offset),
        workspace: read_u64(bytes, offset + 8),
        pane: read_u64(bytes, offset + 16),
        session: read_u64(bytes, offset + 24),
        occupant_revision: read_u64(bytes, offset + 32),
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH_HELLO_GOLDEN: &[u8] = &[
        0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2,
    ];

    const RESUME_HELLO_GOLDEN: &[u8] = &[
        0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0,
        0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 4,
    ];

    const CLAIM_GOLDEN: &[u8] = &[
        0, 2, 0, 2, // schema and opcode
        0, 0, 0, 0, 0, 0, 0, 1, // key generation
        0, 0, 0, 0, 0, 0, 0, 2, // key lease
        0, 0, 0, 0, 0, 0, 0, 3, // key sequence
        0, 0, 0, 0, 0, 0, 0, 1, // target generation
        0, 0, 0, 0, 0, 0, 0, 4, // workspace
        0, 0, 0, 0, 0, 0, 0, 5, // pane
        0, 0, 0, 0, 0, 0, 0, 6, // session
        0, 0, 0, 0, 0, 0, 0, 7, // occupant revision
        0, 0, 0, 0, 0, 0, 0, 8, // expected owner revision
        0, 0, 0, 10, // TTL
    ];

    const INFO_GOLDEN: &[u8] = &[
        0, 2, 0, 1, // schema and response kind
        0, 0, 0, 0, 0, 0, 0, 1, // target generation
        0, 0, 0, 0, 0, 0, 0, 4, // workspace
        0, 0, 0, 0, 0, 0, 0, 5, // pane
        0, 0, 0, 0, 0, 0, 0, 6, // session
        0, 0, 0, 0, 0, 0, 0, 7, // occupant revision
        0, 0, 0, 0, 0, 0, 0, 8, // owner revision
        0, 0, 0, 0, 0, 0, 0, 9, // client owner lease
        0, 0, 0, 0, 0, 0, 0, 10, // owner expiry
        1, 0, 0, 0, // child exited and reserved bytes
    ];

    fn sample_hello() -> Hello {
        Hello {
            nonce: 1,
            required_capabilities: 1,
            optional_capabilities: 2,
        }
    }

    fn sample_key() -> OperationKey {
        OperationKey {
            generation: 1,
            lease: 2,
            sequence: 3,
        }
    }

    fn sample_target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 4,
            pane: 5,
            session: 6,
            occupant_revision: 7,
        }
    }

    fn sample_info() -> SessionInfo {
        SessionInfo {
            target: sample_target(),
            owner_revision: 8,
            owner_lease: 9,
            owner_expires_secs: 10,
            child_exited: true,
        }
    }

    fn sample_requests() -> [SessionRequest; 7] {
        [
            SessionRequest::Describe,
            SessionRequest::Claim {
                key: sample_key(),
                target: sample_target(),
                expected_owner_revision: 8,
                ttl_seconds: 10,
            },
            SessionRequest::Release {
                key: sample_key(),
                target: sample_target(),
                expected_owner_revision: 8,
            },
            SessionRequest::Handoff {
                key: sample_key(),
                target: sample_target(),
                expected_owner_revision: 8,
                recipient_lease: 11,
                ttl_seconds: 30,
            },
            SessionRequest::OperationStatus { key: sample_key() },
            SessionRequest::Receipt {
                generation: 1,
                lease: 2,
                through_sequence: 3,
            },
            SessionRequest::Detach,
        ]
    }

    fn set_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn literal_golden_hello_claim_and_info_layouts_are_stable() {
        let fresh = SessionHello {
            hello: sample_hello(),
            resume: None,
        };
        assert_eq!(encode_session_hello(&fresh).unwrap(), FRESH_HELLO_GOLDEN);
        assert_eq!(decode_session_hello(FRESH_HELLO_GOLDEN), Ok(fresh));

        let resumed = SessionHello {
            hello: sample_hello(),
            resume: Some(LeaseRef {
                generation: 3,
                lease: 4,
            }),
        };
        assert_eq!(encode_session_hello(&resumed).unwrap(), RESUME_HELLO_GOLDEN);
        assert_eq!(decode_session_hello(RESUME_HELLO_GOLDEN), Ok(resumed));

        let claim = SessionRequest::Claim {
            key: sample_key(),
            target: sample_target(),
            expected_owner_revision: 8,
            ttl_seconds: 10,
        };
        assert_eq!(encode_session_request(&claim).unwrap(), CLAIM_GOLDEN);
        assert_eq!(decode_session_request(CLAIM_GOLDEN), Ok(claim));

        assert_eq!(encode_session_info(&sample_info()).unwrap(), INFO_GOLDEN);
        assert_eq!(decode_session_info(INFO_GOLDEN), Ok(sample_info()));
    }

    #[test]
    fn fresh_hello_uses_schema_one_and_resume_hello_uses_schema_two() {
        let fresh = SessionHello {
            hello: sample_hello(),
            resume: None,
        };
        assert_eq!(encode_session_hello(&fresh).unwrap().len(), 28);
        assert_eq!(
            encode_hello(&sample_hello()).unwrap().as_slice(),
            FRESH_HELLO_GOLDEN
        );

        let resumed = SessionHello {
            hello: sample_hello(),
            resume: Some(LeaseRef {
                generation: 3,
                lease: 4,
            }),
        };
        assert_eq!(encode_session_hello(&resumed).unwrap().len(), 44);

        let unknown_bits = Hello {
            nonce: 5,
            required_capabilities: 1 << 40,
            optional_capabilities: 1 << 41,
        };
        let encoded = encode_session_hello(&SessionHello {
            hello: unknown_bits,
            resume: Some(LeaseRef {
                generation: 6,
                lease: 7,
            }),
        })
        .unwrap();
        assert_eq!(decode_session_hello(&encoded).unwrap().hello, unknown_bits);
    }

    #[test]
    fn hello_lengths_versions_reserved_fields_and_identifiers_fail_closed() {
        for len in 0..FRESH_HELLO_GOLDEN.len() {
            assert_eq!(
                decode_session_hello(&FRESH_HELLO_GOLDEN[..len]),
                Err(SessionProtocolError::InvalidLength)
            );
        }
        for len in 0..RESUME_HELLO_GOLDEN.len() {
            let expected = if len == SESSION_HELLO_FRESH_BYTES {
                Err(SessionProtocolError::Handshake(
                    HandshakeError::UnsupportedSchema,
                ))
            } else {
                Err(SessionProtocolError::InvalidLength)
            };
            assert_eq!(decode_session_hello(&RESUME_HELLO_GOLDEN[..len]), expected);
        }
        let mut trailing_fresh = FRESH_HELLO_GOLDEN.to_vec();
        trailing_fresh.push(0);
        assert_eq!(
            decode_session_hello(&trailing_fresh),
            Err(SessionProtocolError::InvalidLength)
        );
        let mut trailing_resume = RESUME_HELLO_GOLDEN.to_vec();
        trailing_resume.push(0);
        assert_eq!(
            decode_session_hello(&trailing_resume),
            Err(SessionProtocolError::InvalidLength)
        );

        let mut bad_version = RESUME_HELLO_GOLDEN.to_vec();
        bad_version[1] = 1;
        assert_eq!(
            decode_session_hello(&bad_version),
            Err(SessionProtocolError::UnsupportedSchema)
        );
        let mut bad_reserved = RESUME_HELLO_GOLDEN.to_vec();
        bad_reserved[3] = 1;
        assert_eq!(
            decode_session_hello(&bad_reserved),
            Err(SessionProtocolError::NonzeroReserved)
        );
        let mut overlapping_capabilities = RESUME_HELLO_GOLDEN.to_vec();
        overlapping_capabilities[27] = 1;
        assert_eq!(
            decode_session_hello(&overlapping_capabilities),
            Err(SessionProtocolError::Handshake(
                HandshakeError::OverlappingCapabilities
            ))
        );

        let mut zero_nonce = RESUME_HELLO_GOLDEN.to_vec();
        zero_nonce[11] = 0;
        assert_eq!(
            decode_session_hello(&zero_nonce),
            Err(SessionProtocolError::Handshake(
                HandshakeError::ZeroIdentifier
            ))
        );
        for offset in [28, 36] {
            let mut zero_resume_id = RESUME_HELLO_GOLDEN.to_vec();
            zero_resume_id[offset..offset + 8].fill(0);
            assert_eq!(
                decode_session_hello(&zero_resume_id),
                Err(SessionProtocolError::ZeroIdentifier)
            );
        }
        let invalid = SessionHello {
            hello: sample_hello(),
            resume: Some(LeaseRef {
                generation: 0,
                lease: 1,
            }),
        };
        assert_eq!(
            encode_session_hello(&invalid),
            Err(SessionProtocolError::ZeroIdentifier)
        );
        let overlapping = SessionHello {
            hello: Hello {
                nonce: 1,
                required_capabilities: 1,
                optional_capabilities: 1,
            },
            resume: None,
        };
        assert_eq!(
            encode_session_hello(&overlapping),
            Err(SessionProtocolError::Handshake(
                HandshakeError::OverlappingCapabilities
            ))
        );
    }

    #[test]
    fn every_session_request_round_trips_with_its_exact_length() {
        let expected = [4, 80, 76, 88, 28, 28, 4];
        for (index, (request, expected_len)) in
            sample_requests().into_iter().zip(expected).enumerate()
        {
            let bytes = encode_session_request(&request).unwrap();
            assert_eq!(bytes.len(), expected_len);
            assert_eq!(&bytes[..4], &[0, 2, 0, u8::try_from(index + 1).unwrap()]);
            assert_eq!(request.encoded_len(), Ok(expected_len));
            assert_eq!(decode_session_request(&bytes), Ok(request));
        }
        assert_eq!(MAX_SESSION_REQUEST_BYTES, 88);
    }

    #[test]
    fn session_request_rejects_wrong_schema_unknown_opcode_and_all_wrong_lengths() {
        for request in sample_requests() {
            let bytes = encode_session_request(&request).unwrap();
            for len in 0..bytes.len() {
                assert_eq!(
                    decode_session_request(&bytes[..len]),
                    Err(SessionProtocolError::InvalidLength)
                );
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert_eq!(
                decode_session_request(&trailing),
                Err(SessionProtocolError::InvalidLength)
            );
        }
        let mut bad_schema = encode_session_request(&SessionRequest::Describe).unwrap();
        bad_schema[1] = 1;
        assert_eq!(
            decode_session_request(&bad_schema),
            Err(SessionProtocolError::UnsupportedSchema)
        );
        let mut unknown_opcode = encode_session_request(&SessionRequest::Describe).unwrap();
        unknown_opcode[3] = 8;
        assert_eq!(
            decode_session_request(&unknown_opcode),
            Err(SessionProtocolError::UnknownKind)
        );
    }

    #[test]
    fn claim_and_release_validate_every_key_target_and_owner_identifier() {
        for offset in [4, 12, 20, 28, 36, 44, 52, 60, 68] {
            let mut bytes = CLAIM_GOLDEN.to_vec();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_session_request(&bytes),
                Err(SessionProtocolError::ZeroIdentifier)
            );
        }
        let mut mismatch = CLAIM_GOLDEN.to_vec();
        mismatch[35] = 2;
        assert_eq!(
            decode_session_request(&mismatch),
            Err(SessionProtocolError::GenerationMismatch)
        );

        let mut release = encode_session_request(&SessionRequest::Release {
            key: sample_key(),
            target: sample_target(),
            expected_owner_revision: 8,
        })
        .unwrap();
        release[68..76].fill(0);
        assert_eq!(
            decode_session_request(&release),
            Err(SessionProtocolError::ZeroIdentifier)
        );
    }

    #[test]
    fn owner_ttls_and_handoff_recipient_are_bounded() {
        for ttl in [0, 31, u32::MAX] {
            let claim = SessionRequest::Claim {
                key: sample_key(),
                target: sample_target(),
                expected_owner_revision: 8,
                ttl_seconds: ttl,
            };
            assert_eq!(claim.validate(), Err(SessionProtocolError::InvalidTtl));
        }
        for ttl in [1, 30] {
            assert!(SessionRequest::Claim {
                key: sample_key(),
                target: sample_target(),
                expected_owner_revision: 8,
                ttl_seconds: ttl,
            }
            .validate()
            .is_ok());
        }
        let mut zero_recipient = encode_session_request(&SessionRequest::Handoff {
            key: sample_key(),
            target: sample_target(),
            expected_owner_revision: 8,
            recipient_lease: 11,
            ttl_seconds: 30,
        })
        .unwrap();
        zero_recipient[76..84].fill(0);
        assert_eq!(
            decode_session_request(&zero_recipient),
            Err(SessionProtocolError::ZeroIdentifier)
        );

        let mut bad_ttl = CLAIM_GOLDEN.to_vec();
        bad_ttl[79] = 31;
        assert_eq!(
            decode_session_request(&bad_ttl),
            Err(SessionProtocolError::InvalidTtl)
        );
        let mut bad_handoff_ttl = encode_session_request(&SessionRequest::Handoff {
            key: sample_key(),
            target: sample_target(),
            expected_owner_revision: 8,
            recipient_lease: 11,
            ttl_seconds: 10,
        })
        .unwrap();
        bad_handoff_ttl[84..88].copy_from_slice(&31_u32.to_be_bytes());
        assert_eq!(
            decode_session_request(&bad_handoff_ttl),
            Err(SessionProtocolError::InvalidTtl)
        );
    }

    #[test]
    fn operation_status_and_receipt_require_nonzero_identity() {
        let status =
            encode_session_request(&SessionRequest::OperationStatus { key: sample_key() }).unwrap();
        for offset in [4, 12, 20] {
            let mut bytes = status.clone();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_session_request(&bytes),
                Err(SessionProtocolError::ZeroIdentifier)
            );
        }
        let receipt = encode_session_request(&SessionRequest::Receipt {
            generation: 1,
            lease: 2,
            through_sequence: 3,
        })
        .unwrap();
        for offset in [4, 12, 20] {
            let mut bytes = receipt.clone();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_session_request(&bytes),
                Err(SessionProtocolError::ZeroIdentifier)
            );
        }
    }

    #[test]
    fn info_rejects_all_truncations_trailing_bytes_schema_kind_and_reserved_values() {
        for len in 0..SESSION_INFO_BYTES {
            assert_eq!(
                decode_session_info(&INFO_GOLDEN[..len]),
                Err(SessionProtocolError::InvalidLength)
            );
        }
        let mut trailing = INFO_GOLDEN.to_vec();
        trailing.push(0);
        assert_eq!(
            decode_session_info(&trailing),
            Err(SessionProtocolError::InvalidLength)
        );
        let mut wrong_schema = INFO_GOLDEN.to_vec();
        wrong_schema[1] = 1;
        assert_eq!(
            decode_session_info(&wrong_schema),
            Err(SessionProtocolError::UnsupportedSchema)
        );
        let mut wrong_kind = INFO_GOLDEN.to_vec();
        wrong_kind[3] = 2;
        assert_eq!(
            decode_session_info(&wrong_kind),
            Err(SessionProtocolError::UnknownKind)
        );
        for offset in 69..72 {
            let mut reserved = INFO_GOLDEN.to_vec();
            reserved[offset] = 1;
            assert_eq!(
                decode_session_info(&reserved),
                Err(SessionProtocolError::NonzeroReserved)
            );
        }
    }

    #[test]
    fn info_validates_target_owner_revision_owner_pair_and_child_flag() {
        for offset in [4, 12, 20, 28, 36, 44] {
            let mut bytes = INFO_GOLDEN.to_vec();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_session_info(&bytes),
                Err(SessionProtocolError::ZeroIdentifier)
            );
        }
        for (lease, expiry) in [(0, 10), (9, 0)] {
            let mut bytes = INFO_GOLDEN.to_vec();
            set_u64(&mut bytes, 52, lease);
            set_u64(&mut bytes, 60, expiry);
            assert_eq!(
                decode_session_info(&bytes),
                Err(SessionProtocolError::InvalidOwnerState)
            );
        }
        let vacant = SessionInfo {
            owner_lease: 0,
            owner_expires_secs: 0,
            ..sample_info()
        };
        assert_eq!(
            decode_session_info(&encode_session_info(&vacant).unwrap()),
            Ok(vacant)
        );
        let running = SessionInfo {
            child_exited: false,
            ..sample_info()
        };
        assert_eq!(
            decode_session_info(&encode_session_info(&running).unwrap()),
            Ok(running)
        );
        let mut invalid_bool = INFO_GOLDEN.to_vec();
        invalid_bool[68] = 2;
        assert_eq!(
            decode_session_info(&invalid_bool),
            Err(SessionProtocolError::InvalidBoolean)
        );
    }

    #[test]
    fn custom_debug_omits_nonces_operation_ids_targets_and_owner_ids() {
        let hello = SessionHello {
            hello: Hello {
                nonce: 9_876_543_210,
                required_capabilities: 3,
                optional_capabilities: 4,
            },
            resume: Some(LeaseRef {
                generation: 8_765_432_109,
                lease: 7_654_321_098,
            }),
        };
        let request = SessionRequest::Claim {
            key: sample_key(),
            target: sample_target(),
            expected_owner_revision: 8,
            ttl_seconds: 10,
        };
        let info = sample_info();
        for debug in [
            format!("{hello:?}"),
            format!("{request:?}"),
            format!("{info:?}"),
        ] {
            for identifier in [
                "9876543210",
                "8765432109",
                "7654321098",
                "generation: 1",
                "lease: 2",
                "workspace: 4",
            ] {
                assert!(!debug.contains(identifier));
            }
        }
        assert_eq!(
            SessionProtocolError::InvalidTtl.to_string(),
            "session ownership TTL is outside its limit"
        );
    }
}
