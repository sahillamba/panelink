//! Bounded wire codec for the experimental schema-9 explicit pane close.
//!
//! This module validates and encodes close requests, close status queries and
//! close results. It performs no process, layout, ledger or dispatch work and
//! authorizes nothing. The exact request bytes cover the key, the complete
//! target and the expected layout revision, so exact-payload operation
//! deduplication compares the whole payload.
//!
//! All integers are big-endian. Offsets are byte offsets from the start of the
//! payload.
//!
//! Request (kind 1), exactly 80 bytes:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 2 | schema, [`CLOSE_SCHEMA`] |
//! | 2 | 2 | kind, 1 |
//! | 4 | 24 | operation key: generation, lease, sequence (`u64` each) |
//! | 28 | 40 | target: generation, workspace, pane, session, occupant revision |
//! | 68 | 8 | expected layout revision, nonzero |
//! | 76 | 4 | reserved, zero |
//!
//! Status query (kind 2), exactly 32 bytes: schema, kind, operation key and
//! four zero reserved bytes.
//!
//! Result (kind `0x8001`), exactly 96 bytes:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 2 | schema, [`CLOSE_SCHEMA`] |
//! | 2 | 2 | kind, `0x8001` |
//! | 4 | 24 | operation key |
//! | 28 | 2 | outcome: 1 Exited, 2 Escalated, 3 Unknown, 4 Rejected |
//! | 30 | 2 | rejection: 0 unless Rejected; then 1 NoLayout, 2 StaleTarget, 3 StaleLayout, 4 AlreadyClosed, 5 NotAdmitted |
//! | 32 | 8 | layout revision after the operation; 0 only for NoLayout/NotAdmitted |
//! | 40 | 2 | flags: bit 0 target present, bit 1 survivor resize pending |
//! | 42 | 6 | reserved, zero |
//! | 48 | 40 | target, all zero when absent |
//! | 88 | 8 | reserved, zero |

use crate::control_protocol::TargetRef;
use crate::protocol::{OperationClass, OperationKey, Outcome};
use std::error::Error;
use std::fmt;
use std::io::{self, Write};

/// Wire schema used for the experimental pane-close payloads.
pub const CLOSE_SCHEMA: u16 = 9;

/// Exact size of an encoded close request.
pub const CLOSE_REQUEST_BYTES: usize = 80;

/// Exact size of an encoded close status query.
pub const CLOSE_STATUS_QUERY_BYTES: usize = 32;

/// Exact size of an encoded close result.
pub const CLOSE_RESULT_BYTES: usize = 96;

const KIND_CLOSE: u16 = 1;
const KIND_STATUS_QUERY: u16 = 2;
const KIND_RESULT: u16 = 0x8001;

const FLAG_TARGET_PRESENT: u16 = 1;
const FLAG_RESIZE_PENDING: u16 = 2;

/// Result of one bounded explicit stop of a managed child process group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopOutcome {
    /// The child exited after HUP, or had already exited.
    Exited,
    /// KILL was required after the HUP window and the child then exited.
    Escalated,
    /// Exit could not be confirmed within the bounded windows, or the
    /// effects adapter could not stop the child.
    Unknown,
}

/// Fixed reason a close was recorded without stopping any process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseRejection {
    /// No pane layout exists yet.
    NoLayout,
    /// The target does not identify a current pane.
    StaleTarget,
    /// The expected layout revision is not the current revision.
    StaleLayout,
    /// The target named a session that was already closed or exited.
    AlreadyClosed,
    /// Status only: the key was never admitted in the live ledger.
    NotAdmitted,
}

/// Outcome of one close operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseOutcome {
    /// The stop effect ran with this observed result.
    Stopped(StopOutcome),
    /// The request was rejected before any process effect.
    Rejected(CloseRejection),
}

/// A schema-9 explicit close of one pane session.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CloseRequest {
    /// Operation key owned by the caller's own lease.
    pub key: OperationKey,
    /// Complete identity of the pane to stop.
    pub target: TargetRef,
    /// Layout revision captured together with the target.
    pub expected_layout_revision: u64,
}

impl CloseRequest {
    /// Classify this request under the existing operation protocol.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::DurableMutation
    }

    /// Validate every field and return the fixed encoded size.
    pub fn encoded_len(&self) -> Result<usize, CloseCodecError> {
        validate_key(self.key)?;
        validate_target(self.key, self.target)?;
        if self.expected_layout_revision == 0 {
            return Err(CloseCodecError::ZeroLayoutRevision);
        }
        Ok(CLOSE_REQUEST_BYTES)
    }
}

impl fmt::Debug for CloseRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CloseRequest { <redacted> }")
    }
}

/// A status query for the original close operation key.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CloseStatusQuery {
    /// Original operation key whose result is observed.
    pub key: OperationKey,
}

impl CloseStatusQuery {
    /// Classify status as an observation that does not reserve an operation.
    pub const fn operation_class(self) -> OperationClass {
        OperationClass::Query
    }

    /// Validate the operation key and return the fixed encoded size.
    pub fn encoded_len(self) -> Result<usize, CloseCodecError> {
        validate_key(self.key)?;
        Ok(CLOSE_STATUS_QUERY_BYTES)
    }
}

impl fmt::Debug for CloseStatusQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CloseStatusQuery { <redacted> }")
    }
}

/// Typed result for one pane-close operation.
///
/// `target` is absent only for [`CloseRejection::NotAdmitted`].
/// `layout_revision` is the current revision after the operation and is zero
/// only for [`CloseRejection::NoLayout`] and [`CloseRejection::NotAdmitted`].
/// `survivor_resize_pending` may be set only for an `Exited`/`Escalated`
/// stop that collapsed a sibling pane whose absolute resize did not succeed.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CloseResult {
    /// Key identifying the close operation.
    pub key: OperationKey,
    /// Stop or rejection outcome.
    pub outcome: CloseOutcome,
    /// Complete identity of the requested pane.
    pub target: Option<TargetRef>,
    /// Layout revision after this operation.
    pub layout_revision: u64,
    /// Whether the surviving pane's PTY resize is still pending.
    pub survivor_resize_pending: bool,
}

impl CloseResult {
    /// Validate this result and return its fixed encoded size.
    pub fn encoded_len(&self) -> Result<usize, CloseCodecError> {
        validate_result(self)?;
        Ok(CLOSE_RESULT_BYTES)
    }

    /// Terminal ledger outcome recorded for this result.
    ///
    /// `Exited`/`Escalated` are `Applied`, or `Partial` when the survivor's
    /// resize is pending; `Unknown` is `Unknown`; rejections are `Failed`.
    pub const fn ledger_outcome(&self) -> Outcome {
        match self.outcome {
            CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated) => {
                if self.survivor_resize_pending {
                    Outcome::Partial
                } else {
                    Outcome::Applied
                }
            }
            CloseOutcome::Stopped(StopOutcome::Unknown) => Outcome::Unknown,
            CloseOutcome::Rejected(_) => Outcome::Failed,
        }
    }
}

impl fmt::Debug for CloseResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloseResult")
            .field("key", &"<redacted>")
            .field("outcome", &self.outcome)
            .field("target", &"<redacted>")
            .field("layout_revision", &"<redacted>")
            .field("survivor_resize_pending", &self.survivor_resize_pending)
            .finish()
    }
}

/// Fixed, redacted validation errors for pane-close payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseCodecError {
    /// A fixed-size payload is truncated.
    Truncated,
    /// Bytes remain after a fixed-size payload.
    TrailingBytes,
    /// The schema is unsupported.
    UnsupportedSchema,
    /// The request or result kind is unknown.
    UnknownKind,
    /// A reserved field contains a nonzero byte.
    NonzeroReserved,
    /// An operation-key component is zero.
    ZeroKeyComponent,
    /// A target identity component is zero.
    InvalidTarget,
    /// A target generation differs from the operation-key generation.
    GenerationMismatch,
    /// The expected layout revision is zero.
    ZeroLayoutRevision,
    /// An outcome code is unsupported.
    UnknownOutcome,
    /// A rejection code is unsupported or inconsistent with the outcome.
    UnknownRejection,
    /// Unknown flag bits are set.
    InvalidFlags,
    /// Outcome, revision, target and pending flag do not agree.
    OutcomeMismatch,
}

impl fmt::Display for CloseCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "pane-close payload is truncated",
            Self::TrailingBytes => "bytes follow the pane-close payload",
            Self::UnsupportedSchema => "pane-close schema is unsupported",
            Self::UnknownKind => "pane-close kind is unknown",
            Self::NonzeroReserved => "pane-close reserved bytes must be zero",
            Self::ZeroKeyComponent => "pane-close key components must be nonzero",
            Self::InvalidTarget => "pane-close target components must be nonzero",
            Self::GenerationMismatch => "pane-close target generation does not match the key",
            Self::ZeroLayoutRevision => "pane-close layout revision must be nonzero",
            Self::UnknownOutcome => "pane-close outcome is unsupported",
            Self::UnknownRejection => "pane-close rejection is unsupported",
            Self::InvalidFlags => "pane-close flags are invalid",
            Self::OutcomeMismatch => "pane-close outcome, revision and target do not match",
        })
    }
}

impl Error for CloseCodecError {}

/// Encode failures keep a writer's `io::Error` out of debug and display text.
pub enum CloseEncodeError {
    /// Validation failed before the encoder wrote any bytes.
    Invalid(CloseCodecError),
    /// The destination writer rejected the encoded bytes.
    Writer(io::Error),
}

impl fmt::Debug for CloseEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => f
                .debug_tuple("CloseEncodeError::Invalid")
                .field(error)
                .finish(),
            Self::Writer(_) => f.write_str("CloseEncodeError::Writer(<redacted>)"),
        }
    }
}

impl fmt::Display for CloseEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(_) => f.write_str("pane-close codec validation failed"),
            Self::Writer(_) => f.write_str("pane-close codec writer failed"),
        }
    }
}

impl Error for CloseEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Writer(error) => Some(error),
        }
    }
}

impl From<CloseCodecError> for CloseEncodeError {
    fn from(value: CloseCodecError) -> Self {
        Self::Invalid(value)
    }
}

/// Validate a close request completely, then encode it.
///
/// Nothing is written when validation fails.
pub fn encode_close_request<W: Write>(
    request: &CloseRequest,
    writer: &mut W,
) -> Result<usize, CloseEncodeError> {
    let encoded_len = request.encoded_len()?;
    let mut bytes = [0_u8; CLOSE_REQUEST_BYTES];
    put_header(&mut bytes, KIND_CLOSE, request.key);
    put_target(&mut bytes, 28, request.target);
    put_u64(&mut bytes, 68, request.expected_layout_revision);
    writer.write_all(&bytes).map_err(CloseEncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode and validate exactly one 80-byte close request.
pub fn decode_close_request(bytes: &[u8]) -> Result<CloseRequest, CloseCodecError> {
    validate_exact_size(bytes, CLOSE_REQUEST_BYTES)?;
    validate_header(bytes, KIND_CLOSE)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    let target = read_target(bytes, 28);
    validate_target(key, target)?;
    let expected_layout_revision = read_u64(bytes, 68);
    if expected_layout_revision == 0 {
        return Err(CloseCodecError::ZeroLayoutRevision);
    }
    if bytes[76..CLOSE_REQUEST_BYTES].iter().any(|byte| *byte != 0) {
        return Err(CloseCodecError::NonzeroReserved);
    }
    Ok(CloseRequest {
        key,
        target,
        expected_layout_revision,
    })
}

/// Validate and encode a fixed-size query for an already-admitted close key.
pub fn encode_close_status_query<W: Write>(
    query: CloseStatusQuery,
    writer: &mut W,
) -> Result<usize, CloseEncodeError> {
    let encoded_len = query.encoded_len()?;
    let mut bytes = [0_u8; CLOSE_STATUS_QUERY_BYTES];
    put_header(&mut bytes, KIND_STATUS_QUERY, query.key);
    writer.write_all(&bytes).map_err(CloseEncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 32-byte close status query.
pub fn decode_close_status_query(bytes: &[u8]) -> Result<CloseStatusQuery, CloseCodecError> {
    validate_exact_size(bytes, CLOSE_STATUS_QUERY_BYTES)?;
    validate_header(bytes, KIND_STATUS_QUERY)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    if bytes[28..32].iter().any(|byte| *byte != 0) {
        return Err(CloseCodecError::NonzeroReserved);
    }
    Ok(CloseStatusQuery { key })
}

/// Validate and encode one fixed-size close result.
pub fn encode_close_result<W: Write>(
    result: &CloseResult,
    writer: &mut W,
) -> Result<usize, CloseEncodeError> {
    let encoded_len = result.encoded_len()?;
    let mut bytes = [0_u8; CLOSE_RESULT_BYTES];
    put_header(&mut bytes, KIND_RESULT, result.key);
    let (outcome, rejection) = outcome_codes(result.outcome);
    put_u16(&mut bytes, 28, outcome);
    put_u16(&mut bytes, 30, rejection);
    put_u64(&mut bytes, 32, result.layout_revision);
    let mut flags = 0;
    if let Some(target) = result.target {
        flags |= FLAG_TARGET_PRESENT;
        put_target(&mut bytes, 48, target);
    }
    if result.survivor_resize_pending {
        flags |= FLAG_RESIZE_PENDING;
    }
    put_u16(&mut bytes, 40, flags);
    writer.write_all(&bytes).map_err(CloseEncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 96-byte close result.
pub fn decode_close_result(bytes: &[u8]) -> Result<CloseResult, CloseCodecError> {
    validate_exact_size(bytes, CLOSE_RESULT_BYTES)?;
    validate_header(bytes, KIND_RESULT)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    let outcome = decode_outcome(read_u16(bytes, 28), read_u16(bytes, 30))?;
    let flags = read_u16(bytes, 40);
    if flags & !(FLAG_TARGET_PRESENT | FLAG_RESIZE_PENDING) != 0 {
        return Err(CloseCodecError::InvalidFlags);
    }
    if bytes[42..48].iter().any(|byte| *byte != 0)
        || bytes[88..CLOSE_RESULT_BYTES].iter().any(|byte| *byte != 0)
    {
        return Err(CloseCodecError::NonzeroReserved);
    }
    let target = if flags & FLAG_TARGET_PRESENT != 0 {
        Some(read_target(bytes, 48))
    } else if bytes[48..88].iter().any(|byte| *byte != 0) {
        return Err(CloseCodecError::NonzeroReserved);
    } else {
        None
    };
    let result = CloseResult {
        key,
        outcome,
        target,
        layout_revision: read_u64(bytes, 32),
        survivor_resize_pending: flags & FLAG_RESIZE_PENDING != 0,
    };
    validate_result(&result)?;
    Ok(result)
}

fn validate_result(result: &CloseResult) -> Result<(), CloseCodecError> {
    validate_key(result.key)?;
    if let Some(target) = result.target {
        validate_target(result.key, target)?;
    }
    let revision = result.layout_revision != 0;
    let consistent = match result.outcome {
        CloseOutcome::Rejected(CloseRejection::NotAdmitted) => {
            result.target.is_none() && !revision && !result.survivor_resize_pending
        }
        CloseOutcome::Rejected(CloseRejection::NoLayout) => {
            result.target.is_some() && !revision && !result.survivor_resize_pending
        }
        CloseOutcome::Rejected(_) => {
            result.target.is_some() && revision && !result.survivor_resize_pending
        }
        CloseOutcome::Stopped(StopOutcome::Unknown) => {
            result.target.is_some() && revision && !result.survivor_resize_pending
        }
        CloseOutcome::Stopped(_) => result.target.is_some() && revision,
    };
    if consistent {
        Ok(())
    } else {
        Err(CloseCodecError::OutcomeMismatch)
    }
}

fn validate_key(key: OperationKey) -> Result<(), CloseCodecError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        Err(CloseCodecError::ZeroKeyComponent)
    } else {
        Ok(())
    }
}

fn validate_target(key: OperationKey, target: TargetRef) -> Result<(), CloseCodecError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(CloseCodecError::InvalidTarget);
    }
    if target.generation != key.generation {
        return Err(CloseCodecError::GenerationMismatch);
    }
    Ok(())
}

fn validate_exact_size(bytes: &[u8], expected: usize) -> Result<(), CloseCodecError> {
    if bytes.len() < expected {
        Err(CloseCodecError::Truncated)
    } else if bytes.len() > expected {
        Err(CloseCodecError::TrailingBytes)
    } else {
        Ok(())
    }
}

/// Callers guarantee at least four bytes are present.
fn validate_header(bytes: &[u8], expected_kind: u16) -> Result<(), CloseCodecError> {
    if read_u16(bytes, 0) != CLOSE_SCHEMA {
        return Err(CloseCodecError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != expected_kind {
        return Err(CloseCodecError::UnknownKind);
    }
    Ok(())
}

const fn outcome_codes(outcome: CloseOutcome) -> (u16, u16) {
    match outcome {
        CloseOutcome::Stopped(StopOutcome::Exited) => (1, 0),
        CloseOutcome::Stopped(StopOutcome::Escalated) => (2, 0),
        CloseOutcome::Stopped(StopOutcome::Unknown) => (3, 0),
        CloseOutcome::Rejected(reason) => (
            4,
            match reason {
                CloseRejection::NoLayout => 1,
                CloseRejection::StaleTarget => 2,
                CloseRejection::StaleLayout => 3,
                CloseRejection::AlreadyClosed => 4,
                CloseRejection::NotAdmitted => 5,
            },
        ),
    }
}

fn decode_outcome(outcome: u16, rejection: u16) -> Result<CloseOutcome, CloseCodecError> {
    let stopped = |stop| {
        if rejection == 0 {
            Ok(CloseOutcome::Stopped(stop))
        } else {
            Err(CloseCodecError::UnknownRejection)
        }
    };
    match outcome {
        1 => stopped(StopOutcome::Exited),
        2 => stopped(StopOutcome::Escalated),
        3 => stopped(StopOutcome::Unknown),
        4 => Ok(CloseOutcome::Rejected(match rejection {
            1 => CloseRejection::NoLayout,
            2 => CloseRejection::StaleTarget,
            3 => CloseRejection::StaleLayout,
            4 => CloseRejection::AlreadyClosed,
            5 => CloseRejection::NotAdmitted,
            _ => return Err(CloseCodecError::UnknownRejection),
        })),
        _ => Err(CloseCodecError::UnknownOutcome),
    }
}

// Fixed-offset helpers are only called after an exact-size check.

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut field = [0_u8; 8];
    field.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_be_bytes(field)
}

fn read_key(bytes: &[u8], offset: usize) -> OperationKey {
    OperationKey {
        generation: read_u64(bytes, offset),
        lease: read_u64(bytes, offset + 8),
        sequence: read_u64(bytes, offset + 16),
    }
}

fn read_target(bytes: &[u8], offset: usize) -> TargetRef {
    TargetRef {
        generation: read_u64(bytes, offset),
        workspace: read_u64(bytes, offset + 8),
        pane: read_u64(bytes, offset + 16),
        session: read_u64(bytes, offset + 24),
        occupant_revision: read_u64(bytes, offset + 32),
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn put_header(bytes: &mut [u8], kind: u16, key: OperationKey) {
    put_u16(bytes, 0, CLOSE_SCHEMA);
    put_u16(bytes, 2, kind);
    put_u64(bytes, 4, key.generation);
    put_u64(bytes, 12, key.lease);
    put_u64(bytes, 20, key.sequence);
}

fn put_target(bytes: &mut [u8], offset: usize, target: TargetRef) {
    put_u64(bytes, offset, target.generation);
    put_u64(bytes, offset + 8, target.workspace);
    put_u64(bytes, offset + 16, target.pane);
    put_u64(bytes, offset + 24, target.session);
    put_u64(bytes, offset + 32, target.occupant_revision);
}

#[cfg(test)]
mod tests;
