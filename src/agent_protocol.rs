//! Strict schema-14 codecs for recognized-agent observations and submissions.
//!
//! These codecs validate wire data only. They do not identify processes,
//! authorize input, or infer provider turn completion.

use crate::control_protocol::{TargetRef, MAX_INPUT_TEXT_BYTES};
use crate::protocol::OperationKey;
use std::error::Error as StdError;
use std::fmt;
use std::str;

/// Schema used by recognized-agent observation and submission messages.
pub const AGENT_SCHEMA_VERSION: u16 = 14;

const QUERY_BYTES: usize = 48;
const OBSERVATION_BYTES: usize = 96;
const SUBMIT_PREFIX_BYTES: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 65_536;

const KIND_QUERY: u16 = 1;
const KIND_SUBMIT: u16 = 2;
const KIND_OBSERVATION: u16 = 0x8001;

/// A provider recognized from the server-verified foreground process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AgentKind {
    /// No supported provider was recognized.
    Unknown = 0,
    /// Codex CLI.
    Codex = 1,
    /// OpenCode.
    OpenCode = 2,
}

impl AgentKind {
    /// Stable, nonlocalized label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }
}

/// A conservative screen-derived provider state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AgentState {
    /// The screen does not establish a supported state.
    Unknown = 0,
    /// A provider-specific input prompt appears ready.
    Idle = 1,
    /// A known provider activity marker is visible.
    Working = 2,
    /// A provider approval or permission marker is visible.
    Blocked = 3,
    /// The recognized process has exited.
    Exited = 4,
}

impl AgentState {
    /// Stable, nonlocalized label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Exited => "exited",
        }
    }
}

/// Kernel process birth identity. Debug output intentionally reveals no IDs.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AgentIdentity {
    /// Foreground process ID.
    pub pid: u64,
    /// Kernel birth time in whole seconds.
    pub start_seconds: u64,
    /// Kernel birth time's microsecond component.
    pub start_microseconds: u64,
    /// Server-verified executable kind.
    pub kind: AgentKind,
}

impl fmt::Debug for AgentIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AgentIdentity([redacted])")
    }
}

/// One process-bound screen observation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Observation {
    /// Full target identity.
    pub target: TargetRef,
    /// Foreground process identity.
    pub identity: AgentIdentity,
    /// Conservative screen state.
    pub state: AgentState,
    /// Revision of the screen used for classification.
    pub screen_revision: u64,
    /// Monotonic sample time in milliseconds.
    pub sampled_at_mono_ms: u64,
}

impl fmt::Debug for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observation")
            .field("target", &"[redacted]")
            .field("identity", &self.identity)
            .field("state", &self.state)
            .field("screen_revision", &"[redacted]")
            .field("sampled_at_mono_ms", &"[redacted]")
            .finish()
    }
}

/// One guarded, deduplicated text submission.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct GuardedSubmit<'a> {
    /// Existing operation-ledger key.
    pub key: OperationKey,
    /// Full target identity.
    pub target: TargetRef,
    /// Actual input-owner revision supplied by the claimant.
    pub owner_revision: u64,
    /// Server-verified provider process identity.
    pub identity: AgentIdentity,
    /// Screen revision on which the caller based the submission.
    pub screen_revision: u64,
    /// Borrowed UTF-8 text to submit.
    pub text: &'a str,
}

impl fmt::Debug for GuardedSubmit<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardedSubmit")
            .field("key", &"[redacted]")
            .field("target", &"[redacted]")
            .field("owner_revision", &"[redacted]")
            .field("identity", &self.identity)
            .field("screen_revision", &"[redacted]")
            .field("text_len", &self.text.len())
            .finish()
    }
}

/// A decoded schema-14 request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Request<'a> {
    /// Query one immutable target.
    Query(TargetRef),
    /// Submit text against an exact process and screen observation.
    Submit(GuardedSubmit<'a>),
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query(_) => f.write_str("Request::Query([redacted])"),
            Self::Submit(submit) => f.debug_tuple("Request::Submit").field(submit).finish(),
        }
    }
}

/// Fixed, redacted errors returned by schema-14 codecs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A required field or fixed-size message is truncated.
    Truncated,
    /// Bytes remain after the declared message.
    TrailingBytes,
    /// The encoded payload exceeds its byte ceiling.
    PayloadTooLarge,
    /// The schema number is unsupported.
    UnsupportedSchema,
    /// The message discriminator is unknown.
    UnknownKind,
    /// A reserved byte is nonzero.
    UnsupportedReserved,
    /// A required identifier is zero.
    ZeroIdentifier,
    /// The operation and target server generations differ.
    GenerationMismatch,
    /// The identity fields are inconsistent with the provider kind.
    InvalidIdentity,
    /// An enum tag is not defined by this schema.
    UnknownEnumTag,
    /// An unknown provider claims an actionable readiness state.
    UnknownKindState,
    /// Input text is longer than the accepted UTF-8 byte limit.
    InputTextTooLarge,
    /// Text bytes are not valid UTF-8.
    InvalidUtf8,
    /// Text contains a Unicode control scalar.
    InvalidText,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "agent payload is truncated",
            Self::TrailingBytes => "bytes follow the agent payload",
            Self::PayloadTooLarge => "agent payload exceeds its limit",
            Self::UnsupportedSchema => "agent schema is unsupported",
            Self::UnknownKind => "agent message kind is unknown",
            Self::UnsupportedReserved => "agent reserved bytes must be zero",
            Self::ZeroIdentifier => "agent identifiers must be nonzero",
            Self::GenerationMismatch => "agent operation and target generations differ",
            Self::InvalidIdentity => "agent process identity is invalid",
            Self::UnknownEnumTag => "agent enum tag is unknown",
            Self::UnknownKindState => "unknown agent kind cannot claim readiness",
            Self::InputTextTooLarge => "agent input text exceeds its byte limit",
            Self::InvalidUtf8 => "agent input text is not valid UTF-8",
            Self::InvalidText => "agent input text contains a control scalar",
        };
        f.write_str(message)
    }
}

impl StdError for Error {}

/// Encode a 48-byte schema-14 observation query.
pub fn encode_query(target: TargetRef) -> Result<Vec<u8>, Error> {
    validate_target(target)?;
    let mut bytes = Vec::with_capacity(QUERY_BYTES);
    write_u16(&mut bytes, AGENT_SCHEMA_VERSION);
    write_u16(&mut bytes, KIND_QUERY);
    bytes.extend_from_slice(&[0; 4]);
    write_target(&mut bytes, target);
    Ok(bytes)
}

/// Encode a fixed 96-byte process-bound observation.
pub fn encode_observation(observation: &Observation) -> Result<Vec<u8>, Error> {
    validate_target(observation.target)?;
    validate_identity(observation.identity)?;
    validate_kind_state(observation.identity.kind, observation.state)?;

    let mut bytes = Vec::with_capacity(OBSERVATION_BYTES);
    write_u16(&mut bytes, AGENT_SCHEMA_VERSION);
    write_u16(&mut bytes, KIND_OBSERVATION);
    bytes.extend_from_slice(&[0; 4]);
    write_target(&mut bytes, observation.target);
    write_identity(&mut bytes, observation.identity);
    bytes.push(observation.state as u8);
    bytes.extend_from_slice(&[0; 6]);
    write_u64(&mut bytes, observation.screen_revision);
    write_u64(&mut bytes, observation.sampled_at_mono_ms);
    debug_assert_eq!(bytes.len(), OBSERVATION_BYTES);
    Ok(bytes)
}

/// Decode exactly one fixed-size schema-14 observation.
pub fn decode_observation(bytes: &[u8]) -> Result<Observation, Error> {
    if bytes.len() < OBSERVATION_BYTES {
        return Err(Error::Truncated);
    }
    if bytes.len() > OBSERVATION_BYTES {
        return Err(Error::TrailingBytes);
    }
    validate_header(bytes, KIND_OBSERVATION)?;
    validate_reserved(bytes, &[(4, 8), (74, 80)])?;

    let observation = Observation {
        target: read_target(bytes, 8)?,
        identity: read_identity(bytes, 48)?,
        state: decode_state(read_u8(bytes, 73)?)?,
        screen_revision: read_u64(bytes, 80)?,
        sampled_at_mono_ms: read_u64(bytes, 88)?,
    };
    validate_target(observation.target)?;
    validate_identity(observation.identity)?;
    validate_kind_state(observation.identity.kind, observation.state)?;
    Ok(observation)
}

/// Encode a schema-14 guarded submission and its bounded UTF-8 body.
pub fn encode_submit(submit: &GuardedSubmit<'_>) -> Result<Vec<u8>, Error> {
    validate_submit(submit)?;
    let size = SUBMIT_PREFIX_BYTES
        .checked_add(submit.text.len())
        .ok_or(Error::InputTextTooLarge)?;
    if size > MAX_PAYLOAD_BYTES {
        return Err(Error::PayloadTooLarge);
    }

    let mut bytes = Vec::with_capacity(size);
    write_u16(&mut bytes, AGENT_SCHEMA_VERSION);
    write_u16(&mut bytes, KIND_SUBMIT);
    bytes.extend_from_slice(&[0; 4]);
    write_u64(&mut bytes, submit.key.generation);
    write_u64(&mut bytes, submit.key.lease);
    write_u64(&mut bytes, submit.key.sequence);
    write_target(&mut bytes, submit.target);
    write_u64(&mut bytes, submit.owner_revision);
    write_identity(&mut bytes, submit.identity);
    bytes.extend_from_slice(&[0; 7]);
    write_u64(&mut bytes, submit.screen_revision);
    write_u32(&mut bytes, submit.text.len() as u32);
    write_u32(&mut bytes, 0);
    bytes.extend_from_slice(submit.text.as_bytes());
    debug_assert_eq!(bytes.len(), size);
    Ok(bytes)
}

/// Decode exactly one query or guarded submission, borrowing submitted text.
pub fn decode_request(bytes: &[u8]) -> Result<Request<'_>, Error> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::PayloadTooLarge);
    }
    if bytes.len() < 8 {
        return Err(Error::Truncated);
    }
    let schema = read_u16(bytes, 0)?;
    if schema != AGENT_SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema);
    }
    match read_u16(bytes, 2)? {
        KIND_QUERY => {
            check_exact_length(bytes, QUERY_BYTES)?;
            validate_reserved(bytes, &[(4, 8)])?;
            let target = read_target(bytes, 8)?;
            validate_target(target)?;
            Ok(Request::Query(target))
        }
        KIND_SUBMIT => decode_submit(bytes).map(Request::Submit),
        _ => Err(Error::UnknownKind),
    }
}

fn decode_submit(bytes: &[u8]) -> Result<GuardedSubmit<'_>, Error> {
    if bytes.len() < SUBMIT_PREFIX_BYTES {
        return Err(Error::Truncated);
    }
    validate_reserved(bytes, &[(4, 8), (105, 112), (124, 128)])?;
    let key = OperationKey {
        generation: read_u64(bytes, 8)?,
        lease: read_u64(bytes, 16)?,
        sequence: read_u64(bytes, 24)?,
    };
    let submit = GuardedSubmit {
        key,
        target: read_target(bytes, 32)?,
        owner_revision: read_u64(bytes, 72)?,
        identity: read_identity(bytes, 80)?,
        screen_revision: read_u64(bytes, 112)?,
        text: "",
    };
    let text_len = usize::try_from(read_u32(bytes, 120)?).map_err(|_| Error::InputTextTooLarge)?;
    if text_len > MAX_INPUT_TEXT_BYTES {
        return Err(Error::InputTextTooLarge);
    }
    let expected = SUBMIT_PREFIX_BYTES
        .checked_add(text_len)
        .ok_or(Error::InputTextTooLarge)?;
    check_exact_length(bytes, expected)?;
    let text =
        str::from_utf8(&bytes[SUBMIT_PREFIX_BYTES..expected]).map_err(|_| Error::InvalidUtf8)?;
    let submit = GuardedSubmit { text, ..submit };
    validate_submit(&submit)?;
    Ok(submit)
}

fn validate_submit(submit: &GuardedSubmit<'_>) -> Result<(), Error> {
    if submit.key.generation == 0
        || submit.key.lease == 0
        || submit.key.sequence == 0
        || submit.owner_revision == 0
    {
        return Err(Error::ZeroIdentifier);
    }
    validate_target(submit.target)?;
    if submit.key.generation != submit.target.generation {
        return Err(Error::GenerationMismatch);
    }
    validate_identity(submit.identity)?;
    if submit.identity.kind == AgentKind::Unknown {
        return Err(Error::InvalidIdentity);
    }
    validate_text(submit.text)
}

fn validate_target(target: TargetRef) -> Result<(), Error> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        Err(Error::ZeroIdentifier)
    } else {
        Ok(())
    }
}

fn validate_identity(identity: AgentIdentity) -> Result<(), Error> {
    match identity.kind {
        AgentKind::Unknown => {
            if identity.pid == 0 && identity.start_seconds == 0 && identity.start_microseconds == 0
            {
                Ok(())
            } else {
                Err(Error::InvalidIdentity)
            }
        }
        AgentKind::Codex | AgentKind::OpenCode => {
            if identity.pid != 0
                && identity.start_seconds != 0
                && identity.start_microseconds < 1_000_000
            {
                Ok(())
            } else {
                Err(Error::InvalidIdentity)
            }
        }
    }
}

fn validate_kind_state(kind: AgentKind, state: AgentState) -> Result<(), Error> {
    if kind == AgentKind::Unknown
        && matches!(
            state,
            AgentState::Idle | AgentState::Working | AgentState::Blocked
        )
    {
        Err(Error::UnknownKindState)
    } else {
        Ok(())
    }
}

fn validate_text(text: &str) -> Result<(), Error> {
    if text.len() > MAX_INPUT_TEXT_BYTES {
        return Err(Error::InputTextTooLarge);
    }
    if text.chars().any(char::is_control) {
        return Err(Error::InvalidText);
    }
    if SUBMIT_PREFIX_BYTES + text.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::PayloadTooLarge);
    }
    Ok(())
}

fn validate_header(bytes: &[u8], expected_kind: u16) -> Result<(), Error> {
    if read_u16(bytes, 0)? != AGENT_SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema);
    }
    if read_u16(bytes, 2)? != expected_kind {
        return Err(Error::UnknownKind);
    }
    Ok(())
}

fn validate_reserved(bytes: &[u8], ranges: &[(usize, usize)]) -> Result<(), Error> {
    for &(start, end) in ranges {
        let field = bytes.get(start..end).ok_or(Error::Truncated)?;
        if field.iter().any(|byte| *byte != 0) {
            return Err(Error::UnsupportedReserved);
        }
    }
    Ok(())
}

fn check_exact_length(bytes: &[u8], expected: usize) -> Result<(), Error> {
    if bytes.len() < expected {
        Err(Error::Truncated)
    } else if bytes.len() > expected {
        Err(Error::TrailingBytes)
    } else {
        Ok(())
    }
}

fn decode_kind(tag: u8) -> Result<AgentKind, Error> {
    match tag {
        0 => Ok(AgentKind::Unknown),
        1 => Ok(AgentKind::Codex),
        2 => Ok(AgentKind::OpenCode),
        _ => Err(Error::UnknownEnumTag),
    }
}

fn decode_state(tag: u8) -> Result<AgentState, Error> {
    match tag {
        0 => Ok(AgentState::Unknown),
        1 => Ok(AgentState::Idle),
        2 => Ok(AgentState::Working),
        3 => Ok(AgentState::Blocked),
        4 => Ok(AgentState::Exited),
        _ => Err(Error::UnknownEnumTag),
    }
}

fn read_target(bytes: &[u8], offset: usize) -> Result<TargetRef, Error> {
    Ok(TargetRef {
        generation: read_u64(bytes, offset)?,
        workspace: read_u64(bytes, offset + 8)?,
        pane: read_u64(bytes, offset + 16)?,
        session: read_u64(bytes, offset + 24)?,
        occupant_revision: read_u64(bytes, offset + 32)?,
    })
}

fn write_target(bytes: &mut Vec<u8>, target: TargetRef) {
    write_u64(bytes, target.generation);
    write_u64(bytes, target.workspace);
    write_u64(bytes, target.pane);
    write_u64(bytes, target.session);
    write_u64(bytes, target.occupant_revision);
}

fn read_identity(bytes: &[u8], offset: usize) -> Result<AgentIdentity, Error> {
    Ok(AgentIdentity {
        pid: read_u64(bytes, offset)?,
        start_seconds: read_u64(bytes, offset + 8)?,
        start_microseconds: read_u64(bytes, offset + 16)?,
        kind: decode_kind(read_u8(bytes, offset + 24)?)?,
    })
}

fn write_identity(bytes: &mut Vec<u8>, identity: AgentIdentity) {
    write_u64(bytes, identity.pid);
    write_u64(bytes, identity.start_seconds);
    write_u64(bytes, identity.start_microseconds);
    bytes.push(identity.kind as u8);
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Truncated)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    let field = bytes.get(offset..offset + 2).ok_or(Error::Truncated)?;
    Ok(u16::from_be_bytes([field[0], field[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let field = bytes.get(offset..offset + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([field[0], field[1], field[2], field[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    let field = bytes.get(offset..offset + 8).ok_or(Error::Truncated)?;
    Ok(u64::from_be_bytes([
        field[0], field[1], field[2], field[3], field[4], field[5], field[6], field[7],
    ]))
}

fn write_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetRef {
        TargetRef {
            generation: u64::MAX,
            workspace: u64::MAX,
            pane: 1,
            session: u64::MAX,
            occupant_revision: u64::MAX,
        }
    }

    fn identity(kind: AgentKind) -> AgentIdentity {
        AgentIdentity {
            pid: u64::MAX,
            start_seconds: u64::MAX,
            start_microseconds: 999_999,
            kind,
        }
    }

    #[test]
    fn query_and_observation_roundtrip_max_ids_and_reject_bad_lengths() {
        let query = encode_query(target()).unwrap();
        assert_eq!(query.len(), QUERY_BYTES);
        assert_eq!(decode_request(&query), Ok(Request::Query(target())));

        let observation = Observation {
            target: target(),
            identity: identity(AgentKind::Codex),
            state: AgentState::Working,
            screen_revision: u64::MAX,
            sampled_at_mono_ms: u64::MAX,
        };
        let bytes = encode_observation(&observation).unwrap();
        assert_eq!(bytes.len(), OBSERVATION_BYTES);
        assert_eq!(decode_observation(&bytes), Ok(observation));
        assert_eq!(decode_observation(&bytes[..95]), Err(Error::Truncated));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode_observation(&trailing), Err(Error::TrailingBytes));
    }

    #[test]
    fn query_schema_kind_reserved_and_target_ids_are_strict() {
        let query = encode_query(target()).unwrap();
        let mut malformed = query.clone();
        malformed[0] = 1;
        assert_eq!(decode_request(&malformed), Err(Error::UnsupportedSchema));
        malformed = query.clone();
        malformed[3] = 9;
        assert_eq!(decode_request(&malformed), Err(Error::UnknownKind));
        malformed = query.clone();
        malformed[4] = 1;
        assert_eq!(decode_request(&malformed), Err(Error::UnsupportedReserved));
        malformed = query;
        malformed[8..16].fill(0);
        assert_eq!(decode_request(&malformed), Err(Error::ZeroIdentifier));
    }

    #[test]
    fn submit_roundtrip_checks_reserved_tags_generation_and_utf8() {
        let submit = GuardedSubmit {
            key: OperationKey {
                generation: u64::MAX,
                lease: u64::MAX,
                sequence: u64::MAX,
            },
            target: target(),
            owner_revision: u64::MAX,
            identity: identity(AgentKind::OpenCode),
            screen_revision: u64::MAX,
            text: "hello λ",
        };
        let bytes = encode_submit(&submit).unwrap();
        assert_eq!(bytes.len(), SUBMIT_PREFIX_BYTES + submit.text.len());
        assert_eq!(decode_request(&bytes), Ok(Request::Submit(submit)));

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode_request(&trailing), Err(Error::TrailingBytes));
        let mut reserved = bytes.clone();
        reserved[105] = 1;
        assert_eq!(decode_request(&reserved), Err(Error::UnsupportedReserved));
        let mut unknown = bytes.clone();
        unknown[104] = 9;
        assert_eq!(decode_request(&unknown), Err(Error::UnknownEnumTag));
        let mut mismatch = bytes.clone();
        mismatch[8..16].copy_from_slice(&1u64.to_be_bytes());
        assert_eq!(decode_request(&mismatch), Err(Error::GenerationMismatch));
        let mut invalid_utf8 = bytes.clone();
        *invalid_utf8.last_mut().unwrap() = 0xff;
        assert_eq!(decode_request(&invalid_utf8), Err(Error::InvalidUtf8));
        let mut invalid_birth = bytes.clone();
        invalid_birth[96..104].copy_from_slice(&1_000_000u64.to_be_bytes());
        assert_eq!(decode_request(&invalid_birth), Err(Error::InvalidIdentity));
        let mut zero_owner = bytes;
        zero_owner[72..80].fill(0);
        assert_eq!(decode_request(&zero_owner), Err(Error::ZeroIdentifier));
    }

    #[test]
    fn limits_identity_and_debug_redaction_are_enforced() {
        let too_long = "x".repeat(MAX_INPUT_TEXT_BYTES + 1);
        let submit = GuardedSubmit {
            key: OperationKey {
                generation: 1,
                lease: 1,
                sequence: 1,
            },
            target: TargetRef {
                generation: 1,
                workspace: 1,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            },
            owner_revision: 1,
            identity: AgentIdentity {
                pid: 1,
                start_seconds: 1,
                start_microseconds: 0,
                kind: AgentKind::Codex,
            },
            screen_revision: 0,
            text: &too_long,
        };
        assert_eq!(encode_submit(&submit), Err(Error::InputTextTooLarge));
        let debug = format!("{:?}", submit);
        assert!(debug.contains("text_len"));
        assert!(!debug.contains(&too_long));
        assert!(!format!("{:?}", submit.identity).contains("pid"));
        assert_eq!(AgentKind::Codex.as_str(), "codex");
        assert_eq!(AgentState::Blocked.as_str(), "blocked");

        let mut unknown = encode_observation(&Observation {
            target: target(),
            identity: AgentIdentity {
                pid: 0,
                start_seconds: 0,
                start_microseconds: 0,
                kind: AgentKind::Unknown,
            },
            state: AgentState::Unknown,
            screen_revision: 0,
            sampled_at_mono_ms: 0,
        })
        .unwrap();
        unknown[73] = AgentState::Idle as u8;
        assert_eq!(decode_observation(&unknown), Err(Error::UnknownKindState));
        unknown[72] = 7;
        assert_eq!(decode_observation(&unknown), Err(Error::UnknownEnumTag));
    }

    #[test]
    fn observation_reserved_bytes_and_unknown_identity_shape_are_strict() {
        let mut bytes = encode_observation(&Observation {
            target: target(),
            identity: AgentIdentity {
                pid: 0,
                start_seconds: 0,
                start_microseconds: 0,
                kind: AgentKind::Unknown,
            },
            state: AgentState::Unknown,
            screen_revision: 0,
            sampled_at_mono_ms: 0,
        })
        .unwrap();
        bytes[74] = 1;
        assert_eq!(decode_observation(&bytes), Err(Error::UnsupportedReserved));
        bytes[74] = 0;
        bytes[56] = 1;
        assert_eq!(decode_observation(&bytes), Err(Error::InvalidIdentity));
    }
}
