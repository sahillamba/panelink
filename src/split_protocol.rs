//! Bounded borrowed wire codec for the experimental schema-7 pane split.
//!
//! This module validates and encodes split requests, split status queries and
//! split results. It performs no filesystem, process, socket, layout,
//! persistence, ledger, or dispatch work, and it authorizes nothing. A split
//! request carries one complete embedded schema-6 managed-launch request, so
//! the exact request bytes cover the target, expected layout revision,
//! direction and every launch field. Exact-payload operation deduplication can
//! therefore compare the whole payload without a separate launch record.
//!
//! All integers are big-endian. Offsets are byte offsets from the start of the
//! payload.
//!
//! Request (kind 1), a 96-byte prefix followed by the embedded launch:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 2 | schema, [`SPLIT_SCHEMA`] |
//! | 2 | 2 | kind, 1 |
//! | 4 | 24 | operation key: generation, lease, sequence (`u64` each) |
//! | 28 | 40 | target: generation, workspace, pane, session, occupant revision |
//! | 68 | 8 | expected layout revision |
//! | 76 | 2 | direction: 1 Left, 2 Right, 3 Up, 4 Down |
//! | 78 | 18 | reserved, zero |
//! | 96 | rest | one complete schema-6 launch request |
//!
//! Status query (kind 2), exactly 32 bytes: schema, kind, operation key and
//! four zero reserved bytes.
//!
//! Result (kind `0x8001`), exactly 128 bytes:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 2 | schema, [`SPLIT_SCHEMA`] |
//! | 2 | 2 | kind, `0x8001` |
//! | 4 | 24 | operation key |
//! | 28 | 2 | outcome: 1 Accepted, 2 Applied, 3 Failed, 4 Unknown, 5 Partial |
//! | 30 | 2 | new-target presence flag, 0 or 1 |
//! | 32 | 8 | committed layout revision, 0 only for Failed |
//! | 40 | 40 | existing target, always present |
//! | 80 | 40 | new target, all zero when absent |
//! | 120 | 8 | reserved, zero |

use crate::control_protocol::TargetRef;
use crate::managed_launch_protocol::{
    decode_launch_request, encode_launch_request, CodecError, EncodeError, LaunchRequest,
    LaunchRequestView,
};
use crate::pane_layout::SplitDirection;
use crate::protocol::{OperationClass, OperationKey, Outcome, MAX_OPERATION_PAYLOAD_BYTES};
use std::error::Error;
use std::fmt;
use std::io::{self, Write};

/// Wire schema used for the experimental pane-split payloads.
pub const SPLIT_SCHEMA: u16 = 7;

/// Maximum complete encoded split request, including the embedded launch.
pub const MAX_SPLIT_REQUEST_BYTES: usize = MAX_OPERATION_PAYLOAD_BYTES;

/// Fixed size of the split request prefix before the embedded launch.
pub const SPLIT_REQUEST_PREFIX_BYTES: usize = 96;

/// Maximum embedded schema-6 launch request size inside one split request.
pub const MAX_EMBEDDED_LAUNCH_BYTES: usize = MAX_SPLIT_REQUEST_BYTES - SPLIT_REQUEST_PREFIX_BYTES;

/// Exact size of an encoded split status query.
pub const SPLIT_STATUS_QUERY_BYTES: usize = 32;

/// Exact size of an encoded split result.
pub const SPLIT_RESULT_BYTES: usize = 128;

const KIND_SPLIT: u16 = 1;
const KIND_STATUS_QUERY: u16 = 2;
const KIND_RESULT: u16 = 0x8001;

/// Borrowed typed schema-7 split request for encoding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitRequest<'a> {
    /// Operation key; it must equal the embedded launch key exactly.
    pub key: OperationKey,
    /// Complete identity of the pane captured when the split was chosen.
    pub target: TargetRef,
    /// Layout revision captured together with the target.
    pub expected_layout_revision: u64,
    /// Side of the target on which the new pane is placed.
    pub direction: SplitDirection,
    /// Complete launch request for the new pane's session.
    pub launch: &'a LaunchRequest<'a>,
}

impl SplitRequest<'_> {
    /// Classify this request under the existing operation protocol.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::DurableMutation
    }

    /// Validate every field, including the embedded launch, and return the
    /// complete encoded length.
    pub fn encoded_len(&self) -> Result<usize, SplitCodecError> {
        validate_key(self.key)?;
        validate_target(self.key, self.target)?;
        validate_revision(self.expected_layout_revision)?;
        if self.launch.key != self.key {
            return Err(SplitCodecError::LaunchKeyMismatch);
        }
        let launch_len = self
            .launch
            .encoded_len()
            .map_err(SplitCodecError::InvalidLaunch)?;
        let total = SPLIT_REQUEST_PREFIX_BYTES
            .checked_add(launch_len)
            .ok_or(SplitCodecError::LengthOverflow)?;
        validate_total_len(total)?;
        Ok(total)
    }
}

impl fmt::Debug for SplitRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SplitRequest { <redacted> }")
    }
}

/// A decoded split request borrowing the embedded launch from the input.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitRequestView<'a> {
    key: OperationKey,
    target: TargetRef,
    expected_layout_revision: u64,
    direction: SplitDirection,
    launch: LaunchRequestView<'a>,
    launch_bytes: &'a [u8],
    encoded_len: usize,
}

impl<'a> SplitRequestView<'a> {
    /// Classify this request under the existing operation protocol.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::DurableMutation
    }

    /// Return the validated operation key, equal to the embedded launch key.
    pub const fn key(&self) -> OperationKey {
        self.key
    }

    /// Return the validated complete target identity.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Return the nonzero expected layout revision.
    pub const fn expected_layout_revision(&self) -> u64 {
        self.expected_layout_revision
    }

    /// Return the requested split direction.
    pub const fn direction(&self) -> SplitDirection {
        self.direction
    }

    /// Return the validated borrowed view of the embedded launch request.
    pub const fn launch(&self) -> LaunchRequestView<'a> {
        self.launch
    }

    /// Return the exact embedded schema-6 launch request bytes.
    pub const fn launch_bytes(&self) -> &'a [u8] {
        self.launch_bytes
    }

    /// Return the exact encoded length validated by the decoder.
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

impl fmt::Debug for SplitRequestView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SplitRequestView { <redacted> }")
    }
}

/// A status query for the original split operation key.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitStatusQuery {
    /// Original operation key whose result is observed.
    pub key: OperationKey,
}

impl SplitStatusQuery {
    /// Classify status as an observation that does not reserve a new operation.
    pub const fn operation_class(self) -> OperationClass {
        OperationClass::Query
    }

    /// Validate the operation key and return the fixed encoded size.
    pub fn encoded_len(self) -> Result<usize, SplitCodecError> {
        validate_key(self.key)?;
        Ok(SPLIT_STATUS_QUERY_BYTES)
    }
}

impl fmt::Debug for SplitStatusQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SplitStatusQuery { <redacted> }")
    }
}

/// Typed result for one pane-split operation.
///
/// `Failed` means no layout commit: the committed revision is zero and no new
/// target exists. Every other outcome means the split committed: the revision
/// is nonzero and the new target is present.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitResult {
    /// Key identifying the split operation.
    pub key: OperationKey,
    /// Operation outcome.
    pub outcome: Outcome,
    /// Committed layout revision, or zero when nothing was committed.
    pub committed_layout_revision: u64,
    /// Complete identity of the pane that was split.
    pub existing_target: TargetRef,
    /// Complete identity of the newly created pane, when committed.
    pub new_target: Option<TargetRef>,
}

impl SplitResult {
    /// Validate this result and return its fixed encoded size.
    pub fn encoded_len(&self) -> Result<usize, SplitCodecError> {
        validate_result(self)?;
        Ok(SPLIT_RESULT_BYTES)
    }
}

impl fmt::Debug for SplitResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SplitResult { <redacted> }")
    }
}

/// Fixed, redacted validation errors for pane-split payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitCodecError {
    /// The complete payload exceeds the operation limit.
    PayloadTooLarge,
    /// The fixed prefix or a fixed-size payload is truncated.
    Truncated,
    /// Bytes remain after a fixed-size payload.
    TrailingBytes,
    /// A checked byte-count calculation overflowed.
    LengthOverflow,
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
    /// The direction code is unknown.
    UnknownDirection,
    /// The embedded launch key differs from the split operation key.
    LaunchKeyMismatch,
    /// The embedded schema-6 launch request is invalid.
    InvalidLaunch(CodecError),
    /// An outcome code is unsupported by pane split.
    UnknownOutcome,
    /// The new-target presence flag is not zero or one.
    InvalidTargetPresence,
    /// The new target is not a distinct pane in the existing workspace.
    NewTargetMismatch,
    /// Outcome, committed revision and new-target presence disagree.
    OutcomeMismatch,
}

impl fmt::Display for SplitCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::PayloadTooLarge => "pane-split payload exceeds its limit",
            Self::Truncated => "pane-split payload is truncated",
            Self::TrailingBytes => "bytes follow the pane-split payload",
            Self::LengthOverflow => "pane-split length calculation overflowed",
            Self::UnsupportedSchema => "pane-split schema is unsupported",
            Self::UnknownKind => "pane-split kind is unknown",
            Self::NonzeroReserved => "pane-split reserved bytes must be zero",
            Self::ZeroKeyComponent => "pane-split key components must be nonzero",
            Self::InvalidTarget => "pane-split target components must be nonzero",
            Self::GenerationMismatch => "pane-split target generation does not match the key",
            Self::ZeroLayoutRevision => "pane-split layout revision must be nonzero",
            Self::UnknownDirection => "pane-split direction is unknown",
            Self::LaunchKeyMismatch => "pane-split launch key does not match the operation key",
            Self::InvalidLaunch(_) => "pane-split embedded launch request is invalid",
            Self::UnknownOutcome => "pane-split outcome is unsupported",
            Self::InvalidTargetPresence => "pane-split new-target flag is invalid",
            Self::NewTargetMismatch => "pane-split new target is not a distinct sibling pane",
            Self::OutcomeMismatch => "pane-split outcome, revision and new target do not match",
        };
        f.write_str(message)
    }
}

impl Error for SplitCodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidLaunch(error) => Some(error),
            _ => None,
        }
    }
}

/// Encode failures preserve a writer's original `io::Error` while keeping the
/// codec's own debug and display text free of caller-supplied split data.
pub enum SplitEncodeError {
    /// Validation failed before the encoder wrote any bytes.
    Invalid(SplitCodecError),
    /// The destination writer rejected some encoded bytes.
    Writer(io::Error),
}

impl fmt::Debug for SplitEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => f
                .debug_tuple("SplitEncodeError::Invalid")
                .field(error)
                .finish(),
            Self::Writer(_) => f.write_str("SplitEncodeError::Writer(<redacted>)"),
        }
    }
}

impl fmt::Display for SplitEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(_) => f.write_str("pane-split codec validation failed"),
            Self::Writer(_) => f.write_str("pane-split codec writer failed"),
        }
    }
}

impl Error for SplitEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Writer(error) => Some(error),
        }
    }
}

impl From<SplitCodecError> for SplitEncodeError {
    fn from(value: SplitCodecError) -> Self {
        Self::Invalid(value)
    }
}

/// Decode and validate exactly one complete schema-7 split request.
///
/// The embedded launch is decoded by
/// [`decode_launch_request`], which consumes every remaining byte, so no
/// trailing data can follow a valid request.
pub fn decode_split_request(bytes: &[u8]) -> Result<SplitRequestView<'_>, SplitCodecError> {
    validate_total_len(bytes.len())?;
    if bytes.len() < SPLIT_REQUEST_PREFIX_BYTES {
        return Err(SplitCodecError::Truncated);
    }
    validate_header(bytes, KIND_SPLIT)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    let target = read_target(bytes, 28);
    validate_target(key, target)?;
    let expected_layout_revision = read_u64(bytes, 68);
    validate_revision(expected_layout_revision)?;
    let direction = decode_direction(read_u16(bytes, 76))?;
    if bytes[78..SPLIT_REQUEST_PREFIX_BYTES]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(SplitCodecError::NonzeroReserved);
    }

    let launch_bytes = &bytes[SPLIT_REQUEST_PREFIX_BYTES..];
    let launch = decode_launch_request(launch_bytes).map_err(SplitCodecError::InvalidLaunch)?;
    if launch.key() != key {
        return Err(SplitCodecError::LaunchKeyMismatch);
    }
    Ok(SplitRequestView {
        key,
        target,
        expected_layout_revision,
        direction,
        launch,
        launch_bytes,
        encoded_len: bytes.len(),
    })
}

/// Validate a split request completely, then encode it.
///
/// Nothing is written when validation fails.
pub fn encode_split_request<W: Write>(
    request: &SplitRequest<'_>,
    writer: &mut W,
) -> Result<usize, SplitEncodeError> {
    let encoded_len = request.encoded_len()?;
    let mut prefix = [0_u8; SPLIT_REQUEST_PREFIX_BYTES];
    put_header(&mut prefix, KIND_SPLIT, request.key);
    put_target(&mut prefix, 28, request.target);
    put_u64(&mut prefix, 68, request.expected_layout_revision);
    put_u16(&mut prefix, 76, direction_code(request.direction));
    writer
        .write_all(&prefix)
        .map_err(SplitEncodeError::Writer)?;
    let launch_len =
        encode_launch_request(request.launch, writer).map_err(|error| match error {
            EncodeError::Invalid(error) => {
                SplitEncodeError::Invalid(SplitCodecError::InvalidLaunch(error))
            }
            EncodeError::Writer(error) => SplitEncodeError::Writer(error),
        })?;
    debug_assert_eq!(SPLIT_REQUEST_PREFIX_BYTES + launch_len, encoded_len);
    Ok(encoded_len)
}

/// Validate and encode a fixed-size query for an already-admitted split key.
pub fn encode_split_status_query<W: Write>(
    query: SplitStatusQuery,
    writer: &mut W,
) -> Result<usize, SplitEncodeError> {
    let encoded_len = query.encoded_len()?;
    let mut bytes = [0_u8; SPLIT_STATUS_QUERY_BYTES];
    put_header(&mut bytes, KIND_STATUS_QUERY, query.key);
    writer.write_all(&bytes).map_err(SplitEncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 32-byte split status query.
pub fn decode_split_status_query(bytes: &[u8]) -> Result<SplitStatusQuery, SplitCodecError> {
    validate_exact_size(bytes, SPLIT_STATUS_QUERY_BYTES)?;
    validate_header(bytes, KIND_STATUS_QUERY)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    if bytes[28..32].iter().any(|byte| *byte != 0) {
        return Err(SplitCodecError::NonzeroReserved);
    }
    Ok(SplitStatusQuery { key })
}

/// Validate and encode one fixed-size split result.
pub fn encode_split_result<W: Write>(
    result: &SplitResult,
    writer: &mut W,
) -> Result<usize, SplitEncodeError> {
    let encoded_len = result.encoded_len()?;
    let mut bytes = [0_u8; SPLIT_RESULT_BYTES];
    put_header(&mut bytes, KIND_RESULT, result.key);
    put_u16(&mut bytes, 28, outcome_code(result.outcome));
    put_u64(&mut bytes, 32, result.committed_layout_revision);
    put_target(&mut bytes, 40, result.existing_target);
    if let Some(new_target) = result.new_target {
        put_u16(&mut bytes, 30, 1);
        put_target(&mut bytes, 80, new_target);
    }
    writer.write_all(&bytes).map_err(SplitEncodeError::Writer)?;
    Ok(encoded_len)
}

/// Decode one exact 128-byte split result.
pub fn decode_split_result(bytes: &[u8]) -> Result<SplitResult, SplitCodecError> {
    validate_exact_size(bytes, SPLIT_RESULT_BYTES)?;
    validate_header(bytes, KIND_RESULT)?;
    let key = read_key(bytes, 4);
    validate_key(key)?;
    let outcome = decode_outcome(read_u16(bytes, 28))?;
    let new_target = match read_u16(bytes, 30) {
        0 if bytes[80..120].iter().all(|byte| *byte == 0) => None,
        0 => return Err(SplitCodecError::NonzeroReserved),
        1 => Some(read_target(bytes, 80)),
        _ => return Err(SplitCodecError::InvalidTargetPresence),
    };
    if bytes[120..SPLIT_RESULT_BYTES].iter().any(|byte| *byte != 0) {
        return Err(SplitCodecError::NonzeroReserved);
    }
    let result = SplitResult {
        key,
        outcome,
        committed_layout_revision: read_u64(bytes, 32),
        existing_target: read_target(bytes, 40),
        new_target,
    };
    validate_result(&result)?;
    Ok(result)
}

fn validate_result(result: &SplitResult) -> Result<(), SplitCodecError> {
    validate_key(result.key)?;
    validate_target(result.key, result.existing_target)?;
    if let Some(new_target) = result.new_target {
        validate_target(result.key, new_target)?;
        let existing = result.existing_target;
        if new_target.workspace != existing.workspace
            || new_target.pane == existing.pane
            || new_target.session == existing.session
        {
            return Err(SplitCodecError::NewTargetMismatch);
        }
    }
    let committed = result.committed_layout_revision != 0;
    let has_new_target = result.new_target.is_some();
    let consistent = match result.outcome {
        Outcome::Failed => !committed && !has_new_target,
        Outcome::Accepted | Outcome::Applied | Outcome::Partial | Outcome::Unknown => {
            committed && has_new_target
        }
    };
    if consistent {
        Ok(())
    } else {
        Err(SplitCodecError::OutcomeMismatch)
    }
}

fn validate_key(key: OperationKey) -> Result<(), SplitCodecError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        Err(SplitCodecError::ZeroKeyComponent)
    } else {
        Ok(())
    }
}

fn validate_target(key: OperationKey, target: TargetRef) -> Result<(), SplitCodecError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(SplitCodecError::InvalidTarget);
    }
    if target.generation != key.generation {
        return Err(SplitCodecError::GenerationMismatch);
    }
    Ok(())
}

fn validate_revision(revision: u64) -> Result<(), SplitCodecError> {
    if revision == 0 {
        Err(SplitCodecError::ZeroLayoutRevision)
    } else {
        Ok(())
    }
}

fn validate_total_len(length: usize) -> Result<(), SplitCodecError> {
    if length > MAX_SPLIT_REQUEST_BYTES {
        Err(SplitCodecError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

fn validate_exact_size(bytes: &[u8], expected: usize) -> Result<(), SplitCodecError> {
    if bytes.len() < expected {
        Err(SplitCodecError::Truncated)
    } else if bytes.len() > expected {
        Err(SplitCodecError::TrailingBytes)
    } else {
        Ok(())
    }
}

/// Callers guarantee at least four bytes are present.
fn validate_header(bytes: &[u8], expected_kind: u16) -> Result<(), SplitCodecError> {
    if read_u16(bytes, 0) != SPLIT_SCHEMA {
        return Err(SplitCodecError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != expected_kind {
        return Err(SplitCodecError::UnknownKind);
    }
    Ok(())
}

const fn direction_code(direction: SplitDirection) -> u16 {
    match direction {
        SplitDirection::Left => 1,
        SplitDirection::Right => 2,
        SplitDirection::Up => 3,
        SplitDirection::Down => 4,
    }
}

fn decode_direction(code: u16) -> Result<SplitDirection, SplitCodecError> {
    match code {
        1 => Ok(SplitDirection::Left),
        2 => Ok(SplitDirection::Right),
        3 => Ok(SplitDirection::Up),
        4 => Ok(SplitDirection::Down),
        _ => Err(SplitCodecError::UnknownDirection),
    }
}

/// Codes 1-4 match the schema-6 managed-launch result mapping; code 5 adds
/// `Partial`, which a committed split with a failed launch or resize reports.
const fn outcome_code(outcome: Outcome) -> u16 {
    match outcome {
        Outcome::Accepted => 1,
        Outcome::Applied => 2,
        Outcome::Failed => 3,
        Outcome::Unknown => 4,
        Outcome::Partial => 5,
    }
}

fn decode_outcome(code: u16) -> Result<Outcome, SplitCodecError> {
    match code {
        1 => Ok(Outcome::Accepted),
        2 => Ok(Outcome::Applied),
        3 => Ok(Outcome::Failed),
        4 => Ok(Outcome::Unknown),
        5 => Ok(Outcome::Partial),
        _ => Err(SplitCodecError::UnknownOutcome),
    }
}

// The fixed-offset readers below are only called after the caller has checked
// that the payload is at least as long as the fixed region being read.

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
    put_u16(bytes, 0, SPLIT_SCHEMA);
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
mod tests {
    use super::*;
    use crate::managed_launch_protocol::EnvironmentEntry;

    const KEY: OperationKey = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };

    const TARGET: TargetRef = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };

    const NEW_TARGET: TargetRef = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 8,
        session: 9,
        occupant_revision: 1,
    };

    const DIRECTIONS: [(SplitDirection, u16); 4] = [
        (SplitDirection::Left, 1),
        (SplitDirection::Right, 2),
        (SplitDirection::Up, 3),
        (SplitDirection::Down, 4),
    ];

    fn sample_launch(key: OperationKey) -> LaunchRequest<'static> {
        LaunchRequest {
            key,
            rows: 24,
            columns: 80,
            requested_executable: b"sh",
            requested_working_directory: b".",
            client_working_directory: b"/",
            arguments: &[],
            environment: &[],
        }
    }

    fn split<'a>(launch: &'a LaunchRequest<'a>) -> SplitRequest<'a> {
        SplitRequest {
            key: KEY,
            target: TARGET,
            expected_layout_revision: 9,
            direction: SplitDirection::Left,
            launch,
        }
    }

    fn encode(request: &SplitRequest<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let written = encode_split_request(request, &mut bytes).expect("request encodes");
        assert_eq!(written, bytes.len());
        bytes
    }

    fn sample_bytes() -> Vec<u8> {
        let launch = sample_launch(KEY);
        encode(&split(&launch))
    }

    fn applied_result() -> SplitResult {
        SplitResult {
            key: KEY,
            outcome: Outcome::Applied,
            committed_layout_revision: 10,
            existing_target: TARGET,
            new_target: Some(NEW_TARGET),
        }
    }

    fn encode_result(result: &SplitResult) -> Vec<u8> {
        let mut bytes = Vec::new();
        assert_eq!(
            encode_split_result(result, &mut bytes).expect("result encodes"),
            SPLIT_RESULT_BYTES
        );
        bytes
    }

    fn set_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn set_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    #[derive(Default)]
    struct CountingWriter {
        bytes: Vec<u8>,
        calls: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "writer-private-marker",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[rustfmt::skip]
    const GOLDEN_REQUEST: [u8; 164] = [
        // Split prefix: schema 7, kind 1.
        0, 7, 0, 1,
        // Operation key 1/2/3.
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 3,
        // Target 1/4/5/6/7.
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 4,
        0, 0, 0, 0, 0, 0, 0, 5,
        0, 0, 0, 0, 0, 0, 0, 6,
        0, 0, 0, 0, 0, 0, 0, 7,
        // Expected layout revision 9.
        0, 0, 0, 0, 0, 0, 0, 9,
        // Direction Left.
        0, 1,
        // Reserved 78..96.
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // Embedded launch: schema 6, kind 1, same key.
        0, 6, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 3,
        // rows 24, columns 80, zero arguments, zero environment entries.
        0, 24, 0, 80, 0, 0, 0, 0,
        // Path lengths 2, 1, 1.
        0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1,
        // Launch reserved 48..64.
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        // "sh", ".", "/".
        b's', b'h', b'.', b'/',
    ];

    #[test]
    fn request_matches_golden_bytes_and_decodes_to_borrowed_view() {
        let launch = sample_launch(KEY);
        let request = split(&launch);
        assert_eq!(request.operation_class(), OperationClass::DurableMutation);
        assert_eq!(request.encoded_len(), Ok(GOLDEN_REQUEST.len()));
        let bytes = encode(&request);
        assert_eq!(bytes, GOLDEN_REQUEST);

        let view = decode_split_request(&bytes).expect("golden request decodes");
        assert_eq!(view.operation_class(), OperationClass::DurableMutation);
        assert_eq!(view.key(), KEY);
        assert_eq!(view.target(), TARGET);
        assert_eq!(view.expected_layout_revision(), 9);
        assert_eq!(view.direction(), SplitDirection::Left);
        assert_eq!(view.encoded_len(), bytes.len());
        assert_eq!(view.launch_bytes(), &GOLDEN_REQUEST[96..]);
        assert!(std::ptr::eq(view.launch_bytes(), &bytes[96..]));
        assert_eq!(view.launch().key(), KEY);
        assert_eq!(view.launch().dimensions(), (24, 80));
        assert_eq!(view.launch().requested_executable(), b"sh");
        assert_eq!(
            decode_launch_request(view.launch_bytes()),
            Ok(view.launch())
        );
    }

    #[test]
    fn all_four_directions_round_trip_with_their_codes() {
        let launch = sample_launch(KEY);
        for (direction, code) in DIRECTIONS {
            let request = SplitRequest {
                direction,
                ..split(&launch)
            };
            let bytes = encode(&request);
            assert_eq!(read_u16(&bytes, 76), code);
            let view = decode_split_request(&bytes).expect("direction decodes");
            assert_eq!(view.direction(), direction);
            assert_eq!(view.target(), TARGET);
        }
        for code in [0, 5, u16::MAX] {
            let mut bytes = sample_bytes();
            set_u16(&mut bytes, 76, code);
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::UnknownDirection)
            );
        }
    }

    #[test]
    fn every_zero_identity_or_revision_field_is_rejected() {
        let original = sample_bytes();
        for offset in [4, 12, 20] {
            let mut bytes = original.clone();
            set_u64(&mut bytes, offset, 0);
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::ZeroKeyComponent)
            );
        }
        for offset in [28, 36, 44, 52, 60] {
            let mut bytes = original.clone();
            set_u64(&mut bytes, offset, 0);
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::InvalidTarget)
            );
        }
        let mut zero_revision = original.clone();
        set_u64(&mut zero_revision, 68, 0);
        assert_eq!(
            decode_split_request(&zero_revision),
            Err(SplitCodecError::ZeroLayoutRevision)
        );

        let launch = sample_launch(KEY);
        let base = split(&launch);
        let mut cases = Vec::new();
        for index in 0..5 {
            let mut target = TARGET;
            match index {
                0 => target.generation = 0,
                1 => target.workspace = 0,
                2 => target.pane = 0,
                3 => target.session = 0,
                _ => target.occupant_revision = 0,
            }
            cases.push((
                SplitRequest { target, ..base },
                SplitCodecError::InvalidTarget,
            ));
        }
        cases.push((
            SplitRequest {
                expected_layout_revision: 0,
                ..base
            },
            SplitCodecError::ZeroLayoutRevision,
        ));
        for (request, expected) in cases {
            assert_eq!(request.encoded_len(), Err(expected));
        }

        let zero_key = OperationKey { lease: 0, ..KEY };
        let zero_launch = sample_launch(zero_key);
        let request = SplitRequest {
            key: zero_key,
            ..split(&zero_launch)
        };
        assert_eq!(
            request.encoded_len(),
            Err(SplitCodecError::ZeroKeyComponent)
        );
    }

    #[test]
    fn generation_and_embedded_key_mismatches_are_rejected() {
        let mut generation = sample_bytes();
        set_u64(&mut generation, 28, 2);
        assert_eq!(
            decode_split_request(&generation),
            Err(SplitCodecError::GenerationMismatch)
        );
        let launch = sample_launch(KEY);
        let request = SplitRequest {
            target: TargetRef {
                generation: 2,
                ..TARGET
            },
            ..split(&launch)
        };
        assert_eq!(
            request.encoded_len(),
            Err(SplitCodecError::GenerationMismatch)
        );

        // Each embedded-key component differs in turn from the outer key.
        for offset in [100, 108, 116] {
            let mut bytes = sample_bytes();
            set_u64(&mut bytes, offset, 77);
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::LaunchKeyMismatch)
            );
        }
        let other_launch = sample_launch(OperationKey { sequence: 4, ..KEY });
        let request = split(&other_launch);
        assert_eq!(
            request.encoded_len(),
            Err(SplitCodecError::LaunchKeyMismatch)
        );
    }

    #[test]
    fn header_reserved_and_truncation_are_rejected() {
        let original = sample_bytes();
        for end in 0..SPLIT_REQUEST_PREFIX_BYTES {
            assert_eq!(
                decode_split_request(&original[..end]),
                Err(SplitCodecError::Truncated),
                "prefix truncation at {end}"
            );
        }
        for end in SPLIT_REQUEST_PREFIX_BYTES..original.len() {
            assert_eq!(
                decode_split_request(&original[..end]),
                Err(SplitCodecError::InvalidLaunch(CodecError::Truncated)),
                "launch truncation at {end}"
            );
        }
        let mut trailing = original.clone();
        trailing.push(0);
        assert_eq!(
            decode_split_request(&trailing),
            Err(SplitCodecError::InvalidLaunch(CodecError::TrailingBytes))
        );
        for offset in 78..SPLIT_REQUEST_PREFIX_BYTES {
            let mut bytes = original.clone();
            bytes[offset] = 1;
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::NonzeroReserved)
            );
        }
        let mut schema = original.clone();
        set_u16(&mut schema, 0, 6);
        assert_eq!(
            decode_split_request(&schema),
            Err(SplitCodecError::UnsupportedSchema)
        );
        for kind in [2, KIND_RESULT, 99] {
            let mut bytes = original.clone();
            set_u16(&mut bytes, 2, kind);
            assert_eq!(
                decode_split_request(&bytes),
                Err(SplitCodecError::UnknownKind)
            );
        }
    }

    #[test]
    fn invalid_embedded_launch_is_reported_as_invalid_launch() {
        let mut bytes = sample_bytes();
        set_u16(&mut bytes, 96 + 28, 4);
        assert_eq!(
            decode_split_request(&bytes),
            Err(SplitCodecError::InvalidLaunch(
                CodecError::InvalidDimensions
            ))
        );
        let mut bytes = sample_bytes();
        set_u16(&mut bytes, 96, 7);
        assert_eq!(
            decode_split_request(&bytes),
            Err(SplitCodecError::InvalidLaunch(
                CodecError::UnsupportedSchema
            ))
        );

        let mut bad = sample_launch(KEY);
        bad.columns = 19;
        let request = split(&bad);
        assert_eq!(
            request.encoded_len(),
            Err(SplitCodecError::InvalidLaunch(
                CodecError::InvalidDimensions
            ))
        );
        let error = request.encoded_len().unwrap_err();
        assert!(error.source().is_some());
    }

    fn sized_launch(value: &[u8]) -> (Vec<EnvironmentEntry<'_>>, usize) {
        // Fixed launch fields: 64 prefix + "/x" + "r" + "/c" + one entry with
        // a 6-byte header and a one-byte name.
        let environment = vec![EnvironmentEntry { name: b"N", value }];
        (environment, 64 + 5 + 6 + 1 + value.len())
    }

    #[test]
    fn total_size_cap_includes_prefix_and_embedded_launch() {
        let value_len = MAX_SPLIT_REQUEST_BYTES - SPLIT_REQUEST_PREFIX_BYTES - 76;
        let value = vec![b'v'; value_len];
        let (environment, launch_len) = sized_launch(&value);
        let largest = LaunchRequest {
            requested_executable: b"/x",
            requested_working_directory: b"r",
            client_working_directory: b"/c",
            environment: &environment,
            ..sample_launch(KEY)
        };
        assert_eq!(largest.encoded_len(), Ok(launch_len));
        assert_eq!(launch_len, MAX_EMBEDDED_LAUNCH_BYTES);
        let bytes = encode(&split(&largest));
        assert_eq!(bytes.len(), MAX_SPLIT_REQUEST_BYTES);
        let view = decode_split_request(&bytes).expect("largest request decodes");
        assert_eq!(view.encoded_len(), MAX_SPLIT_REQUEST_BYTES);
        assert_eq!(view.launch_bytes().len(), MAX_EMBEDDED_LAUNCH_BYTES);

        let bigger_value = vec![b'v'; value_len + 1];
        let (environment, _) = sized_launch(&bigger_value);
        let over = LaunchRequest {
            requested_executable: b"/x",
            requested_working_directory: b"r",
            client_working_directory: b"/c",
            environment: &environment,
            ..sample_launch(KEY)
        };
        // The launch alone is still a legal schema-6 request.
        assert!(over.encoded_len().is_ok());
        let over_request = split(&over);
        assert_eq!(
            over_request.encoded_len(),
            Err(SplitCodecError::PayloadTooLarge)
        );
        let mut writer = CountingWriter::default();
        assert!(matches!(
            encode_split_request(&over_request, &mut writer),
            Err(SplitEncodeError::Invalid(SplitCodecError::PayloadTooLarge))
        ));
        assert_eq!(writer.calls, 0);

        // A well-formed but oversized wire is rejected before any parsing.
        let mut wire = bytes[..SPLIT_REQUEST_PREFIX_BYTES].to_vec();
        encode_launch_request(&over, &mut wire).expect("launch alone encodes");
        assert_eq!(wire.len(), MAX_SPLIT_REQUEST_BYTES + 1);
        assert_eq!(
            decode_split_request(&wire),
            Err(SplitCodecError::PayloadTooLarge)
        );
    }

    #[test]
    fn encoder_writes_nothing_when_validation_fails() {
        let valid_launch = sample_launch(KEY);
        let mut bad_launch = sample_launch(KEY);
        bad_launch.rows = 81;
        let mismatched_launch = sample_launch(OperationKey { lease: 9, ..KEY });
        let base = split(&valid_launch);
        let invalid = [
            SplitRequest {
                expected_layout_revision: 0,
                ..base
            },
            SplitRequest {
                target: TargetRef { pane: 0, ..TARGET },
                ..base
            },
            SplitRequest {
                target: TargetRef {
                    generation: 3,
                    ..TARGET
                },
                ..base
            },
            split(&bad_launch),
            split(&mismatched_launch),
        ];
        for request in invalid {
            let mut writer = CountingWriter::default();
            assert!(matches!(
                encode_split_request(&request, &mut writer),
                Err(SplitEncodeError::Invalid(_))
            ));
            assert_eq!(writer.calls, 0);
            assert!(writer.bytes.is_empty());
        }

        let mut writer = CountingWriter::default();
        assert!(matches!(
            encode_split_status_query(
                SplitStatusQuery {
                    key: OperationKey { sequence: 0, ..KEY }
                },
                &mut writer
            ),
            Err(SplitEncodeError::Invalid(SplitCodecError::ZeroKeyComponent))
        ));
        let invalid_result = SplitResult {
            new_target: None,
            ..applied_result()
        };
        assert!(matches!(
            encode_split_result(&invalid_result, &mut writer),
            Err(SplitEncodeError::Invalid(SplitCodecError::OutcomeMismatch))
        ));
        assert_eq!(writer.calls, 0);

        let error = encode_split_request(&base, &mut FailingWriter).unwrap_err();
        match &error {
            SplitEncodeError::Writer(error) => {
                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe)
            }
            SplitEncodeError::Invalid(_) => panic!("valid request must reach the writer"),
        }
        assert!(!format!("{error}").contains("writer-private-marker"));
        assert!(!format!("{error:?}").contains("writer-private-marker"));
    }

    #[test]
    fn payload_bytes_cover_direction_revision_target_and_launch() {
        static ENV_A: [EnvironmentEntry<'static>; 1] = [EnvironmentEntry {
            name: b"NAME",
            value: b"a",
        }];
        static ENV_B: [EnvironmentEntry<'static>; 1] = [EnvironmentEntry {
            name: b"NAME",
            value: b"b",
        }];
        let launch_a = LaunchRequest {
            environment: &ENV_A,
            ..sample_launch(KEY)
        };
        let launch_b = LaunchRequest {
            environment: &ENV_B,
            ..sample_launch(KEY)
        };
        let base = split(&launch_a);
        let base_bytes = encode(&base);
        let variants = [
            SplitRequest {
                direction: SplitDirection::Right,
                ..base
            },
            SplitRequest {
                expected_layout_revision: 10,
                ..base
            },
            SplitRequest {
                target: TargetRef {
                    occupant_revision: 8,
                    ..TARGET
                },
                ..base
            },
            SplitRequest {
                launch: &launch_b,
                ..base
            },
        ];
        let mut all = vec![base_bytes.clone()];
        for variant in variants {
            let bytes = encode(&variant);
            assert_eq!(bytes.len(), base_bytes.len());
            assert_ne!(bytes, base_bytes);
            assert!(!all.contains(&bytes));
            all.push(bytes);
        }
        // Re-encoding identical values is byte-for-byte stable.
        assert_eq!(encode(&base), base_bytes);
    }

    #[test]
    fn status_query_matches_golden_bytes_and_rejects_malformed_input() {
        let query = SplitStatusQuery { key: KEY };
        assert_eq!(query.operation_class(), OperationClass::Query);
        let mut bytes = Vec::new();
        assert_eq!(
            encode_split_status_query(query, &mut bytes).unwrap(),
            SPLIT_STATUS_QUERY_BYTES
        );
        #[rustfmt::skip]
        let golden: [u8; 32] = [
            0, 7, 0, 2,
            0, 0, 0, 0, 0, 0, 0, 1,
            0, 0, 0, 0, 0, 0, 0, 2,
            0, 0, 0, 0, 0, 0, 0, 3,
            0, 0, 0, 0,
        ];
        assert_eq!(bytes, golden);
        assert_eq!(decode_split_status_query(&bytes), Ok(query));
        assert_eq!(
            decode_split_status_query(&bytes[..31]),
            Err(SplitCodecError::Truncated)
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            decode_split_status_query(&trailing),
            Err(SplitCodecError::TrailingBytes)
        );
        let mut schema = bytes.clone();
        set_u16(&mut schema, 0, 6);
        assert_eq!(
            decode_split_status_query(&schema),
            Err(SplitCodecError::UnsupportedSchema)
        );
        let mut kind = bytes.clone();
        set_u16(&mut kind, 2, 1);
        assert_eq!(
            decode_split_status_query(&kind),
            Err(SplitCodecError::UnknownKind)
        );
        for offset in 28..32 {
            let mut reserved = bytes.clone();
            reserved[offset] = 1;
            assert_eq!(
                decode_split_status_query(&reserved),
                Err(SplitCodecError::NonzeroReserved)
            );
        }
        for offset in [4, 12, 20] {
            let mut zero = bytes.clone();
            set_u64(&mut zero, offset, 0);
            assert_eq!(
                decode_split_status_query(&zero),
                Err(SplitCodecError::ZeroKeyComponent)
            );
        }
    }

    #[test]
    fn result_matches_golden_bytes() {
        let bytes = encode_result(&applied_result());
        #[rustfmt::skip]
        let golden: [u8; 128] = [
            0, 7, 0x80, 0x01,
            0, 0, 0, 0, 0, 0, 0, 1,
            0, 0, 0, 0, 0, 0, 0, 2,
            0, 0, 0, 0, 0, 0, 0, 3,
            // Outcome Applied, new target present.
            0, 2, 0, 1,
            // Committed layout revision 10.
            0, 0, 0, 0, 0, 0, 0, 10,
            // Existing target 1/4/5/6/7.
            0, 0, 0, 0, 0, 0, 0, 1,
            0, 0, 0, 0, 0, 0, 0, 4,
            0, 0, 0, 0, 0, 0, 0, 5,
            0, 0, 0, 0, 0, 0, 0, 6,
            0, 0, 0, 0, 0, 0, 0, 7,
            // New target 1/4/8/9/1.
            0, 0, 0, 0, 0, 0, 0, 1,
            0, 0, 0, 0, 0, 0, 0, 4,
            0, 0, 0, 0, 0, 0, 0, 8,
            0, 0, 0, 0, 0, 0, 0, 9,
            0, 0, 0, 0, 0, 0, 0, 1,
            // Reserved.
            0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(bytes, golden);
        assert_eq!(decode_split_result(&bytes), Ok(applied_result()));
    }

    #[test]
    fn result_outcome_revision_and_new_target_contract_is_enforced() {
        let failed = SplitResult {
            outcome: Outcome::Failed,
            committed_layout_revision: 0,
            new_target: None,
            ..applied_result()
        };
        let valid = [
            applied_result(),
            SplitResult {
                outcome: Outcome::Accepted,
                ..applied_result()
            },
            SplitResult {
                outcome: Outcome::Partial,
                ..applied_result()
            },
            SplitResult {
                outcome: Outcome::Unknown,
                ..applied_result()
            },
            failed,
        ];
        for (result, code) in valid.into_iter().zip([2, 1, 5, 4, 3]) {
            assert_eq!(result.encoded_len(), Ok(SPLIT_RESULT_BYTES));
            let bytes = encode_result(&result);
            assert_eq!(read_u16(&bytes, 28), code);
            assert_eq!(decode_split_result(&bytes), Ok(result));
        }
        let failed_bytes = encode_result(&failed);
        assert!(failed_bytes[30..32].iter().all(|byte| *byte == 0));
        assert!(failed_bytes[32..40].iter().all(|byte| *byte == 0));
        assert!(failed_bytes[80..128].iter().all(|byte| *byte == 0));

        let mismatches = [
            SplitResult {
                new_target: None,
                ..applied_result()
            },
            SplitResult {
                committed_layout_revision: 0,
                ..applied_result()
            },
            SplitResult {
                outcome: Outcome::Unknown,
                new_target: None,
                ..applied_result()
            },
            SplitResult {
                committed_layout_revision: 3,
                ..failed
            },
            SplitResult {
                new_target: Some(NEW_TARGET),
                ..failed
            },
        ];
        for result in mismatches {
            assert_eq!(result.encoded_len(), Err(SplitCodecError::OutcomeMismatch));
        }

        let sibling_mismatches = [
            TargetRef {
                workspace: 5,
                ..NEW_TARGET
            },
            TargetRef {
                pane: TARGET.pane,
                ..NEW_TARGET
            },
            TargetRef {
                session: TARGET.session,
                ..NEW_TARGET
            },
        ];
        for new_target in sibling_mismatches {
            let result = SplitResult {
                new_target: Some(new_target),
                ..applied_result()
            };
            assert_eq!(
                result.encoded_len(),
                Err(SplitCodecError::NewTargetMismatch)
            );
        }
        let wrong_generation = SplitResult {
            new_target: Some(TargetRef {
                generation: 2,
                ..NEW_TARGET
            }),
            ..applied_result()
        };
        assert_eq!(
            wrong_generation.encoded_len(),
            Err(SplitCodecError::GenerationMismatch)
        );
    }

    #[test]
    fn result_decoder_rejects_malformed_wire_fields() {
        let original = encode_result(&applied_result());
        assert_eq!(
            decode_split_result(&original[..127]),
            Err(SplitCodecError::Truncated)
        );
        let mut trailing = original.clone();
        trailing.push(0);
        assert_eq!(
            decode_split_result(&trailing),
            Err(SplitCodecError::TrailingBytes)
        );
        let mut kind = original.clone();
        set_u16(&mut kind, 2, 1);
        assert_eq!(
            decode_split_result(&kind),
            Err(SplitCodecError::UnknownKind)
        );
        for code in [0, 6] {
            let mut outcome = original.clone();
            set_u16(&mut outcome, 28, code);
            assert_eq!(
                decode_split_result(&outcome),
                Err(SplitCodecError::UnknownOutcome)
            );
        }
        let mut flag = original.clone();
        set_u16(&mut flag, 30, 2);
        assert_eq!(
            decode_split_result(&flag),
            Err(SplitCodecError::InvalidTargetPresence)
        );
        let mut absent_with_bytes = original.clone();
        set_u16(&mut absent_with_bytes, 30, 0);
        assert_eq!(
            decode_split_result(&absent_with_bytes),
            Err(SplitCodecError::NonzeroReserved)
        );
        for offset in 120..128 {
            let mut reserved = original.clone();
            reserved[offset] = 1;
            assert_eq!(
                decode_split_result(&reserved),
                Err(SplitCodecError::NonzeroReserved)
            );
        }
        for offset in [4, 12, 20] {
            let mut zero = original.clone();
            set_u64(&mut zero, offset, 0);
            assert_eq!(
                decode_split_result(&zero),
                Err(SplitCodecError::ZeroKeyComponent)
            );
        }
        for offset in [40, 48, 56, 64, 72, 80, 88, 96, 104, 112] {
            let mut zero = original.clone();
            set_u64(&mut zero, offset, 0);
            assert_eq!(
                decode_split_result(&zero),
                Err(SplitCodecError::InvalidTarget)
            );
        }
        for offset in [40, 80] {
            let mut generation = original.clone();
            set_u64(&mut generation, offset, 2);
            assert_eq!(
                decode_split_result(&generation),
                Err(SplitCodecError::GenerationMismatch)
            );
        }
        let mut same_pane = original.clone();
        set_u64(&mut same_pane, 96, TARGET.pane);
        assert_eq!(
            decode_split_result(&same_pane),
            Err(SplitCodecError::NewTargetMismatch)
        );
        let mut zero_revision = original.clone();
        set_u64(&mut zero_revision, 32, 0);
        assert_eq!(
            decode_split_result(&zero_revision),
            Err(SplitCodecError::OutcomeMismatch)
        );
    }

    #[test]
    fn debug_and_error_text_disclose_no_identifiers_or_launch_data() {
        let key = OperationKey {
            generation: 0x1122_3344_5566_7788,
            lease: 0x2233_4455_6677_8899,
            sequence: 987_654_321,
        };
        let target = TargetRef {
            generation: key.generation,
            workspace: 123_456_789,
            pane: 234_567_891,
            session: 345_678_912,
            occupant_revision: 456_789_123,
        };
        static ENV: [EnvironmentEntry<'static>; 1] = [EnvironmentEntry {
            name: b"SECRET_NAME",
            value: b"secret-value",
        }];
        let launch = LaunchRequest {
            key,
            requested_executable: b"/private/launch/path",
            environment: &ENV,
            ..sample_launch(key)
        };
        let request = SplitRequest {
            key,
            target,
            expected_layout_revision: 567_891_234,
            direction: SplitDirection::Down,
            launch: &launch,
        };
        let bytes = encode(&request);
        let view = decode_split_request(&bytes).unwrap();
        let result = SplitResult {
            key,
            outcome: Outcome::Applied,
            committed_layout_revision: 678_912_345,
            existing_target: target,
            new_target: Some(TargetRef {
                pane: 789_123_456,
                session: 891_234_567,
                ..target
            }),
        };
        let text = format!(
            "{request:?} {view:?} {result:?} {:?}",
            SplitStatusQuery { key }
        );
        for private in [
            key.generation.to_string(),
            key.lease.to_string(),
            key.sequence.to_string(),
            target.workspace.to_string(),
            target.pane.to_string(),
            target.session.to_string(),
            target.occupant_revision.to_string(),
            "567891234".to_string(),
            "678912345".to_string(),
            "789123456".to_string(),
            "891234567".to_string(),
            "/private/launch/path".to_string(),
            "SECRET_NAME".to_string(),
            "secret-value".to_string(),
        ] {
            assert!(!text.contains(&private), "debug leaked {private}");
        }
        assert!(!text.chars().any(|c| c.is_ascii_digit()));

        let errors = [
            SplitCodecError::InvalidLaunch(CodecError::NulByte),
            SplitCodecError::LaunchKeyMismatch,
            SplitCodecError::GenerationMismatch,
        ];
        for error in errors {
            let rendered = format!("{error} {error:?}");
            assert!(!rendered.chars().any(|c| c.is_ascii_digit()));
        }
    }
}
