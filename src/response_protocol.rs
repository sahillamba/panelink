//! Borrowed, bounded response payloads for the initial server protocol.
//!
//! This module validates and encodes response payloads only. It does not
//! truncate read output, authenticate callers, check targets, dispatch work, or
//! provide socket I/O.

use crate::control_protocol::{ReadSource, TargetRef};
use crate::protocol::{OperationKey, OperationStatus, Outcome, MAX_OPERATION_PAYLOAD_BYTES};
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::str;

/// Version of the response payload schema.
pub const RESPONSE_SCHEMA_VERSION: u16 = 1;

/// Maximum complete response payload size, including the 36-byte prefix.
pub const MAX_RESPONSE_PAYLOAD_BYTES: usize = MAX_OPERATION_PAYLOAD_BYTES;

const PREFIX_BYTES: usize = 36;
const READ_BODY_PREFIX_BYTES: usize = 54;
const STATUS_KIND: u16 = 1;
const READ_KIND: u16 = 2;
const REJECTED_KIND: u16 = 3;

/// One borrowed typed response payload.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Response<'a> {
    /// Operation key associated with this observation or response.
    pub key: OperationKey,
    /// Typed response body.
    pub body: ResponseBody<'a>,
}

impl Response<'_> {
    /// Validate identifiers, body fields, and the complete payload budget.
    pub fn validate(&self) -> Result<(), ResponseError> {
        validate_key(self.key)?;
        if let ResponseBody::Read { target, .. } = self.body {
            if target.generation != self.key.generation {
                return Err(ResponseError::GenerationMismatch);
            }
        }
        let body_len = body_len(self.body)?;
        let payload_len = PREFIX_BYTES
            .checked_add(body_len)
            .ok_or(ResponseError::LengthOverflow)?;
        if payload_len > MAX_RESPONSE_PAYLOAD_BYTES {
            return Err(ResponseError::PayloadTooLarge);
        }
        Ok(())
    }

    /// Return the complete encoded payload size, including the prefix.
    ///
    /// A producer serving a [`crate::control_protocol::Command::Read`] must
    /// compare this value with that request's `max_bytes` before encoding. The
    /// request limit applies to this complete response payload, not just its
    /// display text.
    pub fn encoded_len(&self) -> Result<usize, ResponseError> {
        self.validate()?;
        PREFIX_BYTES
            .checked_add(body_len(self.body)?)
            .ok_or(ResponseError::LengthOverflow)
    }
}

impl fmt::Debug for Response<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("key", &"<redacted>")
            .field("body", &self.body)
            .finish()
    }
}

/// Typed response body. Read text remains borrowed from the input or producer.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ResponseBody<'a> {
    /// Read-only status of one operation key.
    Status(OperationStatus),
    /// Sanitized display text from a screen or live scrollback view.
    Read {
        /// Full immutable target identity associated with this read.
        target: TargetRef,
        /// Source of the returned display text.
        source: ReadSource,
        /// Nonzero revision of the screen state used for this result.
        screen_revision: u64,
        /// Whether the producer omitted additional content to fit its limit.
        truncated: bool,
        /// Borrowed UTF-8 display text. The codec never clips it.
        text: &'a str,
    },
    /// Fixed rejection code for a request that was not executed.
    Rejected(RejectionCode),
}

impl fmt::Debug for ResponseBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => f.debug_tuple("Status").field(status).finish(),
            Self::Read {
                source,
                truncated,
                text,
                ..
            } => f
                .debug_struct("Read")
                .field("source", source)
                .field("truncated", truncated)
                .field("text_len", &text.len())
                .finish(),
            Self::Rejected(code) => f.debug_tuple("Rejected").field(code).finish(),
        }
    }
}

/// Stable, fixed response code for a rejected request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum RejectionCode {
    /// The caller lacks the required authorization.
    Unauthorized = 1,
    /// The referenced target is stale.
    StaleTarget = 2,
    /// Another input operation currently holds the input slot.
    InputBusy = 3,
    /// The expected input-owner revision is stale.
    StaleOwner = 4,
    /// The input-owner lease has expired.
    ExpiredOwner = 5,
    /// A bounded resource has no remaining capacity.
    Capacity = 6,
    /// The operation is unsupported by this protocol/runtime version.
    Unsupported = 7,
    /// The request fields are invalid.
    InvalidRequest = 8,
    /// The caller's operation context is unknown.
    UnknownContext = 9,
}

impl RejectionCode {
    const fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Unauthorized),
            2 => Some(Self::StaleTarget),
            3 => Some(Self::InputBusy),
            4 => Some(Self::StaleOwner),
            5 => Some(Self::ExpiredOwner),
            6 => Some(Self::Capacity),
            7 => Some(Self::Unsupported),
            8 => Some(Self::InvalidRequest),
            9 => Some(Self::UnknownContext),
            _ => None,
        }
    }
}

/// Fixed, redacted validation errors for response payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseError {
    /// The fixed response prefix ended before all fields were present.
    TruncatedPrefix,
    /// The declared response body ended before its fields were present.
    TruncatedBody,
    /// Bytes remain after the declared response or body contents.
    TrailingBytes,
    /// The complete payload exceeds its maximum size.
    PayloadTooLarge,
    /// A checked response length calculation overflowed.
    LengthOverflow,
    /// The response payload schema version is unsupported.
    UnsupportedSchema,
    /// The response kind is not one of the three defined kinds.
    UnknownKind,
    /// The reserved prefix field is nonzero.
    UnsupportedReserved,
    /// A key, target, or screen revision identifier is zero.
    ZeroIdentifier,
    /// The operation key and read target use different generations.
    GenerationMismatch,
    /// The status code is invalid or represents nonterminal Accepted.
    InvalidStatus,
    /// A read-source code is unsupported.
    UnsupportedReadSource,
    /// The truncated field is not zero or one.
    InvalidTruncatedFlag,
    /// A rejected-response code is not one of the defined fixed values.
    UnknownRejectionCode,
    /// The read display text is not valid UTF-8.
    InvalidUtf8,
    /// The read display text contains a disallowed Unicode control scalar.
    InvalidDisplayText,
    /// The display text exceeds the supported line count.
    TooManyLines,
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TruncatedPrefix => "response prefix is truncated",
            Self::TruncatedBody => "response body is truncated",
            Self::TrailingBytes => "bytes follow the response body",
            Self::PayloadTooLarge => "response payload exceeds its limit",
            Self::LengthOverflow => "response payload length overflowed",
            Self::UnsupportedSchema => "response schema is unsupported",
            Self::UnknownKind => "response kind is unknown",
            Self::UnsupportedReserved => "response reserved field is nonzero",
            Self::ZeroIdentifier => "response identifiers must be nonzero",
            Self::GenerationMismatch => "response key and target generations do not match",
            Self::InvalidStatus => "response status is invalid",
            Self::UnsupportedReadSource => "response read source is unsupported",
            Self::InvalidTruncatedFlag => "response truncated flag is invalid",
            Self::UnknownRejectionCode => "response rejection code is unknown",
            Self::InvalidUtf8 => "response display text is not valid UTF-8",
            Self::InvalidDisplayText => "response display text contains a control scalar",
            Self::TooManyLines => "response display text exceeds its line limit",
        };
        f.write_str(message)
    }
}

impl Error for ResponseError {}

/// Decode exactly one response payload while borrowing any read text.
pub fn decode_response_payload(bytes: &[u8]) -> Result<Response<'_>, ResponseError> {
    if bytes.len() > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err(ResponseError::PayloadTooLarge);
    }
    if bytes.len() < PREFIX_BYTES {
        return Err(ResponseError::TruncatedPrefix);
    }

    if read_u16(bytes, 0) != RESPONSE_SCHEMA_VERSION {
        return Err(ResponseError::UnsupportedSchema);
    }
    let kind = read_u16(bytes, 2);
    if !matches!(kind, STATUS_KIND | READ_KIND | REJECTED_KIND) {
        return Err(ResponseError::UnknownKind);
    }

    let key = OperationKey {
        generation: read_u64(bytes, 4),
        lease: read_u64(bytes, 12),
        sequence: read_u64(bytes, 20),
    };
    validate_key(key)?;

    let body_len =
        usize::try_from(read_u32(bytes, 28)).map_err(|_| ResponseError::LengthOverflow)?;
    if read_u32(bytes, 32) != 0 {
        return Err(ResponseError::UnsupportedReserved);
    }
    let expected_len = PREFIX_BYTES
        .checked_add(body_len)
        .ok_or(ResponseError::LengthOverflow)?;
    if bytes.len() < expected_len {
        return Err(ResponseError::TruncatedBody);
    }
    if bytes.len() > expected_len {
        return Err(ResponseError::TrailingBytes);
    }
    let body_bytes = &bytes[PREFIX_BYTES..expected_len];
    let body = match kind {
        STATUS_KIND => decode_status(body_bytes)?,
        READ_KIND => decode_read(key, body_bytes)?,
        REJECTED_KIND => decode_rejected(body_bytes)?,
        _ => return Err(ResponseError::UnknownKind),
    };
    let response = Response { key, body };
    response.validate()?;
    Ok(response)
}

/// Encode one response payload after validating every field before writing.
///
/// If the writer fails after accepting a prefix, those bytes cannot be rolled
/// back; the caller must treat the response write as incomplete.
pub fn encode_response_payload<W: Write>(
    response: &Response<'_>,
    writer: &mut W,
) -> io::Result<()> {
    response
        .encoded_len()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    let (kind, body_bytes) = kind_and_body_len(response.body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let body_len = u32::try_from(body_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, ResponseError::LengthOverflow))?;

    write_u16(writer, RESPONSE_SCHEMA_VERSION)?;
    write_u16(writer, kind)?;
    write_u64(writer, response.key.generation)?;
    write_u64(writer, response.key.lease)?;
    write_u64(writer, response.key.sequence)?;
    write_u32(writer, body_len)?;
    write_u32(writer, 0)?;

    match response.body {
        ResponseBody::Status(status) => {
            writer.write_all(&[status_code(status).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, ResponseError::InvalidStatus)
            })?])?
        }
        ResponseBody::Read {
            target,
            source,
            screen_revision,
            truncated,
            text,
        } => {
            write_u64(writer, target.generation)?;
            write_u64(writer, target.workspace)?;
            write_u64(writer, target.pane)?;
            write_u64(writer, target.session)?;
            write_u64(writer, target.occupant_revision)?;
            writer.write_all(&[read_source_code(source)])?;
            write_u64(writer, screen_revision)?;
            writer.write_all(&[u8::from(truncated)])?;
            let text_len = u32::try_from(text.len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, ResponseError::LengthOverflow)
            })?;
            write_u32(writer, text_len)?;
            writer.write_all(text.as_bytes())?;
        }
        ResponseBody::Rejected(code) => write_u16(writer, code as u16)?,
    }
    Ok(())
}

fn body_len(body: ResponseBody<'_>) -> Result<usize, ResponseError> {
    match body {
        ResponseBody::Status(status) => {
            status_code(status).ok_or(ResponseError::InvalidStatus)?;
            Ok(1)
        }
        ResponseBody::Read {
            target,
            screen_revision,
            truncated: _,
            text,
            source: _,
        } => {
            validate_target(target, screen_revision)?;
            validate_display_text(text)?;
            let body_len = READ_BODY_PREFIX_BYTES
                .checked_add(text.len())
                .ok_or(ResponseError::LengthOverflow)?;
            let payload_len = PREFIX_BYTES
                .checked_add(body_len)
                .ok_or(ResponseError::LengthOverflow)?;
            if payload_len > MAX_RESPONSE_PAYLOAD_BYTES {
                return Err(ResponseError::PayloadTooLarge);
            }
            u32::try_from(text.len()).map_err(|_| ResponseError::LengthOverflow)?;
            Ok(body_len)
        }
        ResponseBody::Rejected(_) => Ok(2),
    }
}

fn kind_and_body_len(body: ResponseBody<'_>) -> Result<(u16, usize), ResponseError> {
    let body_len = body_len(body)?;
    let kind = match body {
        ResponseBody::Status(_) => STATUS_KIND,
        ResponseBody::Read { .. } => READ_KIND,
        ResponseBody::Rejected(_) => REJECTED_KIND,
    };
    Ok((kind, body_len))
}

fn validate_key(key: OperationKey) -> Result<(), ResponseError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        return Err(ResponseError::ZeroIdentifier);
    }
    Ok(())
}

fn validate_target(target: TargetRef, screen_revision: u64) -> Result<(), ResponseError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
        || screen_revision == 0
    {
        return Err(ResponseError::ZeroIdentifier);
    }
    Ok(())
}

fn validate_display_text(text: &str) -> Result<(), ResponseError> {
    let line_count = if text.is_empty() {
        0
    } else {
        1 + text.bytes().filter(|byte| *byte == b'\n').count()
    };
    if line_count > 1_024 {
        return Err(ResponseError::TooManyLines);
    }
    if text
        .chars()
        .any(|scalar| scalar.is_control() && scalar != '\n')
    {
        return Err(ResponseError::InvalidDisplayText);
    }
    Ok(())
}

fn decode_status(bytes: &[u8]) -> Result<ResponseBody<'_>, ResponseError> {
    check_body_length(bytes, 1)?;
    let status = match bytes[0] {
        0 => OperationStatus::NotAdmitted,
        1 => OperationStatus::InProgress,
        2 => OperationStatus::Completed(Outcome::Applied),
        3 => OperationStatus::Completed(Outcome::Failed),
        4 => OperationStatus::Completed(Outcome::Partial),
        5 => OperationStatus::Completed(Outcome::Unknown),
        _ => return Err(ResponseError::InvalidStatus),
    };
    Ok(ResponseBody::Status(status))
}

fn decode_read<'a>(key: OperationKey, bytes: &'a [u8]) -> Result<ResponseBody<'a>, ResponseError> {
    if bytes.len() < READ_BODY_PREFIX_BYTES {
        return Err(ResponseError::TruncatedBody);
    }
    let target = TargetRef {
        generation: read_u64(bytes, 0),
        workspace: read_u64(bytes, 8),
        pane: read_u64(bytes, 16),
        session: read_u64(bytes, 24),
        occupant_revision: read_u64(bytes, 32),
    };
    validate_target(target, read_u64(bytes, 41))?;
    if target.generation != key.generation {
        return Err(ResponseError::GenerationMismatch);
    }
    let source = match bytes[40] {
        1 => ReadSource::Screen,
        2 => ReadSource::LiveScrollback,
        _ => return Err(ResponseError::UnsupportedReadSource),
    };
    let truncated = match bytes[49] {
        0 => false,
        1 => true,
        _ => return Err(ResponseError::InvalidTruncatedFlag),
    };
    let text_len =
        usize::try_from(read_u32(bytes, 50)).map_err(|_| ResponseError::LengthOverflow)?;
    let expected_len = READ_BODY_PREFIX_BYTES
        .checked_add(text_len)
        .ok_or(ResponseError::LengthOverflow)?;
    check_body_length(bytes, expected_len)?;
    let text_bytes = &bytes[READ_BODY_PREFIX_BYTES..expected_len];
    let text = str::from_utf8(text_bytes).map_err(|_| ResponseError::InvalidUtf8)?;
    Ok(ResponseBody::Read {
        target,
        source,
        screen_revision: read_u64(bytes, 41),
        truncated,
        text,
    })
}

fn decode_rejected(bytes: &[u8]) -> Result<ResponseBody<'_>, ResponseError> {
    check_body_length(bytes, 2)?;
    let code =
        RejectionCode::from_wire(read_u16(bytes, 0)).ok_or(ResponseError::UnknownRejectionCode)?;
    Ok(ResponseBody::Rejected(code))
}

fn check_body_length(bytes: &[u8], expected_len: usize) -> Result<(), ResponseError> {
    if bytes.len() < expected_len {
        Err(ResponseError::TruncatedBody)
    } else if bytes.len() > expected_len {
        Err(ResponseError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn status_code(status: OperationStatus) -> Option<u8> {
    match status {
        OperationStatus::NotAdmitted => Some(0),
        OperationStatus::InProgress => Some(1),
        OperationStatus::Completed(Outcome::Applied) => Some(2),
        OperationStatus::Completed(Outcome::Failed) => Some(3),
        OperationStatus::Completed(Outcome::Partial) => Some(4),
        OperationStatus::Completed(Outcome::Unknown) => Some(5),
        OperationStatus::Completed(Outcome::Accepted) => None,
    }
}

const fn read_source_code(source: ReadSource) -> u8 {
    match source {
        ReadSource::Screen => 1,
        ReadSource::LiveScrollback => 2,
    }
}

fn write_u16<W: Write>(writer: &mut W, value: u16) -> io::Result<()> {
    writer.write_all(&value.to_be_bytes())
}

fn write_u32<W: Write>(writer: &mut W, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_be_bytes())
}

fn write_u64<W: Write>(writer: &mut W, value: u64) -> io::Result<()> {
    writer.write_all(&value.to_be_bytes())
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
    use std::io;

    // Independent literal examples for each response kind, with key 1/2/3.
    const STATUS_GOLDEN: &[u8] = &[
        0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0,
        0, 1, 0, 0, 0, 0, 0,
    ];
    const READ_GOLDEN: &[u8] = &[
        // Schema and kind.
        0, 1, 0, 2, // Key: generation 1, lease 2, sequence 3.
        0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3,
        // Body length 56 and reserved zero.
        0, 0, 0, 56, 0, 0, 0, 0,
        // Target generation 1, workspace 4, pane 5, session 6, occupant revision 7.
        0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0,
        6, 0, 0, 0, 0, 0, 0, 0, 7,
        // Screen source, revision 8, not truncated, two bytes of UTF-8 display text.
        1, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 2, b'o', b'k',
    ];
    const REJECTED_GOLDEN: &[u8] = &[
        0, 1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0,
        0, 2, 0, 0, 0, 0, 0, 1,
    ];

    fn key() -> OperationKey {
        OperationKey {
            generation: 1,
            lease: 2,
            sequence: 3,
        }
    }

    fn target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 4,
            pane: 5,
            session: 6,
            occupant_revision: 7,
        }
    }

    fn read_response(text: &str) -> Response<'_> {
        Response {
            key: key(),
            body: ResponseBody::Read {
                target: target(),
                source: ReadSource::Screen,
                screen_revision: 8,
                truncated: false,
                text,
            },
        }
    }

    fn encode(response: &Response<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_response_payload(response, &mut bytes).expect("valid response should encode");
        bytes
    }

    fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn literal_golden_examples_round_trip_all_response_kinds() {
        let status = decode_response_payload(STATUS_GOLDEN).expect("status golden should decode");
        assert_eq!(
            status,
            Response {
                key: key(),
                body: ResponseBody::Status(OperationStatus::NotAdmitted),
            }
        );
        assert_eq!(encode(&status), STATUS_GOLDEN);

        let read = decode_response_payload(READ_GOLDEN).expect("read golden should decode");
        assert_eq!(
            read,
            Response {
                key: key(),
                body: ResponseBody::Read {
                    target: target(),
                    source: ReadSource::Screen,
                    screen_revision: 8,
                    truncated: false,
                    text: "ok",
                },
            }
        );
        assert_eq!(encode(&read), READ_GOLDEN);

        let rejected =
            decode_response_payload(REJECTED_GOLDEN).expect("rejected golden should decode");
        assert_eq!(
            rejected,
            Response {
                key: key(),
                body: ResponseBody::Rejected(RejectionCode::Unauthorized),
            }
        );
        assert_eq!(encode(&rejected), REJECTED_GOLDEN);
    }

    #[test]
    fn status_codes_map_to_existing_ledger_types_and_reject_accepted() {
        let cases = [
            (OperationStatus::NotAdmitted, 0),
            (OperationStatus::InProgress, 1),
            (OperationStatus::Completed(Outcome::Applied), 2),
            (OperationStatus::Completed(Outcome::Failed), 3),
            (OperationStatus::Completed(Outcome::Partial), 4),
            (OperationStatus::Completed(Outcome::Unknown), 5),
        ];
        for (status, code) in cases {
            let response = Response {
                key: key(),
                body: ResponseBody::Status(status),
            };
            let bytes = encode(&response);
            assert_eq!(bytes[36], code);
            assert_eq!(decode_response_payload(&bytes), Ok(response));
        }

        let invalid = Response {
            key: key(),
            body: ResponseBody::Status(OperationStatus::Completed(Outcome::Accepted)),
        };
        assert_eq!(invalid.validate(), Err(ResponseError::InvalidStatus));
    }

    #[test]
    fn every_rejection_code_has_a_stable_wire_value() {
        let codes = [
            RejectionCode::Unauthorized,
            RejectionCode::StaleTarget,
            RejectionCode::InputBusy,
            RejectionCode::StaleOwner,
            RejectionCode::ExpiredOwner,
            RejectionCode::Capacity,
            RejectionCode::Unsupported,
            RejectionCode::InvalidRequest,
            RejectionCode::UnknownContext,
        ];
        for (index, code) in codes.into_iter().enumerate() {
            let response = Response {
                key: key(),
                body: ResponseBody::Rejected(code),
            };
            let bytes = encode(&response);
            assert_eq!(u16::from_be_bytes([bytes[36], bytes[37]]), index as u16 + 1);
            assert_eq!(decode_response_payload(&bytes), Ok(response));
        }
    }

    #[test]
    fn rejects_every_truncated_golden_prefix_and_trailing_byte() {
        for golden in [STATUS_GOLDEN, READ_GOLDEN, REJECTED_GOLDEN] {
            for end in 0..golden.len() {
                let expected = if end < PREFIX_BYTES {
                    ResponseError::TruncatedPrefix
                } else {
                    ResponseError::TruncatedBody
                };
                assert_eq!(decode_response_payload(&golden[..end]), Err(expected));
            }
            let mut trailing = golden.to_vec();
            trailing.push(0xee);
            assert_eq!(
                decode_response_payload(&trailing),
                Err(ResponseError::TrailingBytes)
            );
        }
    }

    #[test]
    fn validates_prefix_kind_reserved_and_operation_key_before_body() {
        let mut bytes = STATUS_GOLDEN.to_vec();
        bytes[1] = 2;
        assert_eq!(
            decode_response_payload(&bytes),
            Err(ResponseError::UnsupportedSchema)
        );

        let mut bytes = STATUS_GOLDEN.to_vec();
        bytes[3] = 4;
        assert_eq!(
            decode_response_payload(&bytes),
            Err(ResponseError::UnknownKind)
        );

        let mut bytes = STATUS_GOLDEN.to_vec();
        bytes[35] = 1;
        assert_eq!(
            decode_response_payload(&bytes),
            Err(ResponseError::UnsupportedReserved)
        );

        for offset in [4, 12, 20] {
            let mut bytes = STATUS_GOLDEN.to_vec();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_response_payload(&bytes),
                Err(ResponseError::ZeroIdentifier)
            );
        }
    }

    #[test]
    fn validates_read_target_generation_and_identifiers() {
        for offset in [36, 44, 52, 60, 68] {
            let mut bytes = READ_GOLDEN.to_vec();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_response_payload(&bytes),
                Err(ResponseError::ZeroIdentifier)
            );
        }

        let mut mismatch = READ_GOLDEN.to_vec();
        mismatch[43] = 2;
        assert_eq!(
            decode_response_payload(&mismatch),
            Err(ResponseError::GenerationMismatch)
        );

        let mut revision_zero = READ_GOLDEN.to_vec();
        revision_zero[84] = 0;
        assert_eq!(
            decode_response_payload(&revision_zero),
            Err(ResponseError::ZeroIdentifier)
        );
    }

    #[test]
    fn rejects_invalid_status_read_fields_and_rejection_codes() {
        let mut status = STATUS_GOLDEN.to_vec();
        status[36] = 6;
        assert_eq!(
            decode_response_payload(&status),
            Err(ResponseError::InvalidStatus)
        );

        let mut source = READ_GOLDEN.to_vec();
        source[76] = 3;
        assert_eq!(
            decode_response_payload(&source),
            Err(ResponseError::UnsupportedReadSource)
        );

        let mut truncated = READ_GOLDEN.to_vec();
        truncated[85] = 2;
        assert_eq!(
            decode_response_payload(&truncated),
            Err(ResponseError::InvalidTruncatedFlag)
        );

        let mut rejected = REJECTED_GOLDEN.to_vec();
        rejected[37] = 10;
        assert_eq!(
            decode_response_payload(&rejected),
            Err(ResponseError::UnknownRejectionCode)
        );
    }

    #[test]
    fn rejects_malformed_body_lengths_without_allocating_text() {
        let mut short = STATUS_GOLDEN.to_vec();
        set_u32(&mut short, 28, 0);
        assert_eq!(
            decode_response_payload(&short),
            Err(ResponseError::TrailingBytes)
        );

        let mut long = STATUS_GOLDEN.to_vec();
        set_u32(&mut long, 28, 2);
        assert_eq!(
            decode_response_payload(&long),
            Err(ResponseError::TruncatedBody)
        );

        let mut forged_text_length = READ_GOLDEN.to_vec();
        set_u32(&mut forged_text_length, 86, u32::MAX);
        assert_eq!(
            decode_response_payload(&forged_text_length),
            Err(ResponseError::TruncatedBody)
        );
    }

    #[test]
    fn read_text_is_validated_borrowed_and_bounded_by_complete_payload_size() {
        let response = read_response("ok");
        assert_eq!(response.encoded_len(), Ok(READ_GOLDEN.len()));
        let bytes = encode(&response);
        let decoded = decode_response_payload(&bytes).expect("encoded read should decode");
        let ResponseBody::Read { text, .. } = decoded.body else {
            panic!("expected read response");
        };
        assert_eq!(text.as_ptr(), bytes[90..].as_ptr());

        let maximum_text =
            "x".repeat(MAX_RESPONSE_PAYLOAD_BYTES - PREFIX_BYTES - READ_BODY_PREFIX_BYTES);
        assert_eq!(
            read_response(&maximum_text).encoded_len(),
            Ok(MAX_RESPONSE_PAYLOAD_BYTES)
        );
        let oversized_text = format!("{maximum_text}x");
        assert_eq!(
            read_response(&oversized_text).validate(),
            Err(ResponseError::PayloadTooLarge)
        );

        let exactly_max_lines = "\n".repeat(1_023);
        assert!(read_response(&exactly_max_lines).validate().is_ok());
        let too_many_lines = "\n".repeat(1_024);
        assert_eq!(
            read_response(&too_many_lines).validate(),
            Err(ResponseError::TooManyLines)
        );
        assert!(read_response("").validate().is_ok());
    }

    #[test]
    fn rejects_invalid_utf8_and_disallowed_controls_but_allows_lf() {
        let mut invalid_utf8 = READ_GOLDEN.to_vec();
        invalid_utf8[90] = 0xff;
        assert_eq!(
            decode_response_payload(&invalid_utf8),
            Err(ResponseError::InvalidUtf8)
        );

        for text in ["\u{1b}", "\0", "\u{85}", "\t", "\r", "\u{7f}"] {
            assert_eq!(
                read_response(text).validate(),
                Err(ResponseError::InvalidDisplayText)
            );
        }
        assert!(read_response("first\nsecond").validate().is_ok());
    }

    #[test]
    fn oversized_payload_is_rejected_before_parsing() {
        let oversized = vec![0; MAX_RESPONSE_PAYLOAD_BYTES + 1];
        assert_eq!(
            decode_response_payload(&oversized),
            Err(ResponseError::PayloadTooLarge)
        );
    }

    #[test]
    fn forged_invalid_response_writes_nothing() {
        let mut invalid = Response {
            key: key(),
            body: ResponseBody::Status(OperationStatus::NotAdmitted),
        };
        invalid.key.sequence = 0;
        let mut output = Vec::new();
        let error = encode_response_payload(&invalid, &mut output)
            .expect_err("invalid identifiers must fail before writing");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(output.is_empty());
    }

    #[test]
    fn writer_errors_preserve_only_the_accepted_prefix() {
        let response = Response {
            key: key(),
            body: ResponseBody::Status(OperationStatus::NotAdmitted),
        };
        let mut before = FailAfter::new(0);
        assert!(encode_response_payload(&response, &mut before).is_err());
        assert!(before.bytes.is_empty());

        let mut after_prefix = FailAfter::new(PREFIX_BYTES);
        assert!(encode_response_payload(&response, &mut after_prefix).is_err());
        assert_eq!(after_prefix.bytes, STATUS_GOLDEN[..PREFIX_BYTES]);
    }

    #[test]
    fn debug_and_errors_do_not_disclose_read_text_or_ids() {
        let response = Response {
            key: OperationKey {
                generation: 9_876_543_210,
                lease: 88_777,
                sequence: 66_555,
            },
            body: ResponseBody::Read {
                target: TargetRef {
                    generation: 9_876_543_210,
                    workspace: 55_444,
                    pane: 33_222,
                    session: 11_000,
                    occupant_revision: 77_888,
                },
                source: ReadSource::LiveScrollback,
                screen_revision: 44_333,
                truncated: false,
                text: "sensitive-panelink-canary",
            },
        };
        let debug = format!("{response:?}");
        for secret in [
            "sensitive-panelink-canary",
            "9876543210",
            "88777",
            "66555",
            "55444",
            "44333",
        ] {
            assert!(!debug.contains(secret));
        }
        assert_eq!(
            ResponseError::InvalidUtf8.to_string(),
            "response display text is not valid UTF-8"
        );
    }

    #[test]
    fn encoded_read_must_match_key_generation_before_any_write() {
        let response = Response {
            key: OperationKey {
                generation: 1,
                lease: 1,
                sequence: 1,
            },
            body: ResponseBody::Read {
                target: TargetRef {
                    generation: 2,
                    workspace: 1,
                    pane: 1,
                    session: 1,
                    occupant_revision: 1,
                },
                source: ReadSource::Screen,
                screen_revision: 1,
                truncated: false,
                text: "visible",
            },
        };
        assert_eq!(response.validate(), Err(ResponseError::GenerationMismatch));
        let mut writer = Vec::new();
        assert!(encode_response_payload(&response, &mut writer).is_err());
        assert!(writer.is_empty());
    }

    struct FailAfter {
        limit: usize,
        bytes: Vec<u8>,
    }

    impl FailAfter {
        fn new(limit: usize) -> Self {
            Self {
                limit,
                bytes: Vec::new(),
            }
        }
    }

    impl Write for FailAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.bytes.len() >= self.limit {
                return Err(io::Error::other("injected failure"));
            }
            let accepted = bytes.len().min(self.limit - self.bytes.len());
            self.bytes.extend_from_slice(&bytes[..accepted]);
            Ok(accepted)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
