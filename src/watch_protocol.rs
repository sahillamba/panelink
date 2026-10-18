//! Pure schema-4 reconciliation and finite-watch payloads.
//!
//! This module validates and encodes bounded metadata only. It does not read
//! terminal state, deliver events, wait, authenticate peers, or perform I/O.

use crate::control_protocol::TargetRef;
use crate::event_journal::EventCursor;
use crate::session_protocol::{
    decode_session_info, encode_session_info, SessionInfo, SESSION_INFO_BYTES,
};
use std::error::Error;
use std::fmt;
use std::str;

/// Version of the reconciliation and watch request/response schema.
pub const WATCH_SCHEMA_VERSION: u16 = 4;

/// Fixed prefix size for a reconciliation response, including session info.
pub const RECONCILIATION_PREFIX_BYTES: usize = 128;

/// Exact encoded size of a finite-watch terminal response.
pub const WATCH_END_BYTES: usize = 32;

/// Maximum number of metadata events in one watch response.
pub const MAX_WATCH_EVENTS: u16 = 64;

/// Maximum finite wait duration requested by one watch, in milliseconds.
pub const MAX_WATCH_WAIT_MS: u16 = 1_000;

/// Maximum complete reconciliation response size, including its prefix.
pub const MAX_RECONCILIATION_BYTES: usize = 65_536;

/// Maximum number of display lines in a reconciliation response.
pub const MAX_RECONCILIATION_LINES: u16 = 1_024;

const RECONCILE_REQUEST_BYTES: usize = 56;
const WATCH_REQUEST_BYTES: usize = 72;
const REQUEST_RECONCILE: u16 = 1;
const REQUEST_WATCH: u16 = 2;
const RESPONSE_RECONCILIATION: u16 = 1;
const RESPONSE_WATCH_END: u16 = 2;

/// One bounded schema-4 reconciliation or finite-watch request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum WatchRequest {
    /// Read one bounded authoritative screen and metadata snapshot.
    Reconcile {
        /// Full immutable target identity.
        target: TargetRef,
        /// Maximum complete response size, including the 128-byte prefix.
        max_bytes: u32,
        /// Maximum number of display lines.
        max_lines: u16,
    },
    /// Read a finite ordered page of metadata events.
    Watch {
        /// Full immutable target identity.
        target: TargetRef,
        /// Last event cursor already observed by the caller.
        cursor: EventCursor,
        /// Maximum number of events to return.
        limit: u16,
        /// Finite wait in milliseconds; zero requests an immediate poll.
        wait_ms: u16,
    },
}

impl fmt::Debug for WatchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Reconcile { .. } => "Reconcile",
            Self::Watch { .. } => "Watch",
        };
        f.debug_struct("WatchRequest")
            .field("kind", &kind)
            .field("fields", &"<redacted>")
            .finish()
    }
}

/// Borrowed authoritative snapshot payload before encoding.
#[derive(Clone, Eq, PartialEq)]
pub struct Reconciliation<'a> {
    /// Current target, ownership, and lifecycle metadata.
    pub info: SessionInfo,
    /// Journal cursor captured with this snapshot.
    pub cursor: EventCursor,
    /// Nonzero revision of the screen represented by `text`.
    pub screen_revision: u64,
    /// Whether the requested snapshot was truncated.
    pub truncated: bool,
    /// Sanitized display text, not raw terminal bytes.
    pub text: &'a str,
}

impl fmt::Debug for Reconciliation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reconciliation")
            .field("info", &"<redacted>")
            .field("cursor", &"<redacted>")
            .field("screen_revision", &"<redacted>")
            .field("truncated", &self.truncated)
            .field("text", &"<redacted>")
            .finish()
    }
}

/// Outcome of one finite metadata watch.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum WatchStatus {
    /// One or more complete metadata events follow.
    Changed,
    /// The wait expired without an event.
    TimedOut,
    /// The caller must reconcile before treating state as current.
    ResyncRequired,
}

impl fmt::Debug for WatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Changed => "Changed",
            Self::TimedOut => "TimedOut",
            Self::ResyncRequired => "ResyncRequired",
        })
    }
}

/// Terminal metadata for a finite watch response.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WatchEnd {
    /// Whether events changed, the finite wait expired, or state needs resync.
    pub status: WatchStatus,
    /// Cursor associated with the status; a resync cursor is only a hint.
    pub cursor: EventCursor,
    /// Number of complete Event frames sent before this terminal response.
    pub event_count: u16,
}

impl fmt::Debug for WatchEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchEnd")
            .field("status", &self.status)
            .field("cursor", &"<redacted>")
            .field("event_count", &"<redacted>")
            .finish()
    }
}

/// Fixed, data-free validation failures for schema-4 watch payloads.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum WatchError {
    /// The payload length is incomplete, inconsistent, or has trailing bytes.
    InvalidLength,
    /// The schema version is unsupported.
    UnsupportedSchema,
    /// The request or response kind is unknown.
    UnknownKind,
    /// A reserved field is nonzero.
    NonzeroReserved,
    /// A required target, cursor generation, or screen revision is zero.
    ZeroIdentifier,
    /// A watch cursor generation differs from its target generation.
    GenerationMismatch,
    /// Reconciliation byte or line budget is outside its supported range.
    InvalidBudget,
    /// Event limit is outside the range one through sixty-four.
    InvalidLimit,
    /// Wait duration exceeds one thousand milliseconds.
    InvalidWait,
    /// A boolean or status code is not recognized.
    InvalidStatus,
    /// A reconciliation text field contains invalid UTF-8 or forbidden controls.
    InvalidText,
    /// Display text exceeds the maximum supported line count.
    TooManyLines,
    /// The complete reconciliation payload exceeds 65,536 bytes.
    PayloadTooLarge,
    /// Embedded session information failed its existing schema validation.
    InvalidSessionInfo,
    /// Event count and watch status are inconsistent.
    InvalidEventCount,
}

impl fmt::Debug for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLength => "InvalidLength",
            Self::UnsupportedSchema => "UnsupportedSchema",
            Self::UnknownKind => "UnknownKind",
            Self::NonzeroReserved => "NonzeroReserved",
            Self::ZeroIdentifier => "ZeroIdentifier",
            Self::GenerationMismatch => "GenerationMismatch",
            Self::InvalidBudget => "InvalidBudget",
            Self::InvalidLimit => "InvalidLimit",
            Self::InvalidWait => "InvalidWait",
            Self::InvalidStatus => "InvalidStatus",
            Self::InvalidText => "InvalidText",
            Self::TooManyLines => "TooManyLines",
            Self::PayloadTooLarge => "PayloadTooLarge",
            Self::InvalidSessionInfo => "InvalidSessionInfo",
            Self::InvalidEventCount => "InvalidEventCount",
        })
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLength => "watch payload length is invalid",
            Self::UnsupportedSchema => "watch payload schema is unsupported",
            Self::UnknownKind => "watch payload kind is unknown",
            Self::NonzeroReserved => "watch payload reserved fields must be zero",
            Self::ZeroIdentifier => {
                "watch target, cursor, and revision identifiers must be nonzero"
            }
            Self::GenerationMismatch => "watch cursor and target generations do not match",
            Self::InvalidBudget => "reconciliation budget is outside its supported range",
            Self::InvalidLimit => "watch event limit is outside its supported range",
            Self::InvalidWait => "watch wait exceeds its supported duration",
            Self::InvalidStatus => "watch status code is invalid",
            Self::InvalidText => "reconciliation display text is invalid",
            Self::TooManyLines => "reconciliation display text exceeds its line limit",
            Self::PayloadTooLarge => "reconciliation payload exceeds its size limit",
            Self::InvalidSessionInfo => "embedded session information is invalid",
            Self::InvalidEventCount => "watch event count is inconsistent with its status",
        };
        f.write_str(message)
    }
}

impl Error for WatchError {}

/// Encode a validated reconciliation or watch request.
pub fn encode_watch_request(request: &WatchRequest) -> Result<Vec<u8>, WatchError> {
    match request {
        WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        } => {
            validate_target(*target)?;
            validate_reconciliation_budget(*max_bytes, *max_lines)?;
            let mut bytes = vec![0; RECONCILE_REQUEST_BYTES];
            write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
            write_u16(&mut bytes, 2, REQUEST_RECONCILE);
            write_target(&mut bytes, 4, *target);
            write_u32(&mut bytes, 44, *max_bytes);
            write_u16(&mut bytes, 48, *max_lines);
            Ok(bytes)
        }
        WatchRequest::Watch {
            target,
            cursor,
            limit,
            wait_ms,
        } => {
            validate_target(*target)?;
            validate_cursor(*target, *cursor)?;
            validate_watch_limit(*limit)?;
            validate_wait(*wait_ms)?;
            let mut bytes = vec![0; WATCH_REQUEST_BYTES];
            write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
            write_u16(&mut bytes, 2, REQUEST_WATCH);
            write_target(&mut bytes, 4, *target);
            write_u64(&mut bytes, 44, cursor.generation);
            write_u64(&mut bytes, 52, cursor.sequence);
            write_u16(&mut bytes, 60, *limit);
            write_u16(&mut bytes, 62, *wait_ms);
            Ok(bytes)
        }
    }
}

/// Decode exactly one validated schema-4 reconciliation or watch request.
pub fn decode_watch_request(bytes: &[u8]) -> Result<WatchRequest, WatchError> {
    if bytes.len() != RECONCILE_REQUEST_BYTES && bytes.len() != WATCH_REQUEST_BYTES {
        return Err(WatchError::InvalidLength);
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(WatchError::UnsupportedSchema);
    }

    match read_u16(bytes, 2) {
        REQUEST_RECONCILE => {
            if bytes.len() != RECONCILE_REQUEST_BYTES {
                return Err(WatchError::InvalidLength);
            }
            if bytes[50..56] != [0; 6] {
                return Err(WatchError::NonzeroReserved);
            }
            let target = read_target(bytes, 4);
            let max_bytes = read_u32(bytes, 44);
            let max_lines = read_u16(bytes, 48);
            validate_target(target)?;
            validate_reconciliation_budget(max_bytes, max_lines)?;
            Ok(WatchRequest::Reconcile {
                target,
                max_bytes,
                max_lines,
            })
        }
        REQUEST_WATCH => {
            if bytes.len() != WATCH_REQUEST_BYTES {
                return Err(WatchError::InvalidLength);
            }
            if bytes[64..72] != [0; 8] {
                return Err(WatchError::NonzeroReserved);
            }
            let target = read_target(bytes, 4);
            let cursor = EventCursor {
                generation: read_u64(bytes, 44),
                sequence: read_u64(bytes, 52),
            };
            let limit = read_u16(bytes, 60);
            let wait_ms = read_u16(bytes, 62);
            validate_target(target)?;
            validate_cursor(target, cursor)?;
            validate_watch_limit(limit)?;
            validate_wait(wait_ms)?;
            Ok(WatchRequest::Watch {
                target,
                cursor,
                limit,
                wait_ms,
            })
        }
        _ => Err(WatchError::UnknownKind),
    }
}

/// Encode one complete bounded reconciliation response.
pub fn encode_reconciliation(reconciliation: &Reconciliation<'_>) -> Result<Vec<u8>, WatchError> {
    let session_info =
        encode_session_info(&reconciliation.info).map_err(|_| WatchError::InvalidSessionInfo)?;
    validate_cursor(reconciliation.info.target, reconciliation.cursor)?;
    if reconciliation.screen_revision == 0 {
        return Err(WatchError::ZeroIdentifier);
    }
    validate_display_text(reconciliation.text)?;

    let text_len = reconciliation.text.len();
    let complete_len = RECONCILIATION_PREFIX_BYTES
        .checked_add(text_len)
        .ok_or(WatchError::PayloadTooLarge)?;
    if complete_len > MAX_RECONCILIATION_BYTES {
        return Err(WatchError::PayloadTooLarge);
    }
    let text_len_u32 = u32::try_from(text_len).map_err(|_| WatchError::PayloadTooLarge)?;

    let mut bytes = vec![0; complete_len];
    write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
    write_u16(&mut bytes, 2, RESPONSE_RECONCILIATION);
    bytes[4..4 + SESSION_INFO_BYTES].copy_from_slice(&session_info);
    write_u64(&mut bytes, 76, reconciliation.cursor.generation);
    write_u64(&mut bytes, 84, reconciliation.cursor.sequence);
    write_u64(&mut bytes, 92, reconciliation.screen_revision);
    bytes[100] = u8::from(reconciliation.truncated);
    write_u32(&mut bytes, 104, text_len_u32);
    bytes[RECONCILIATION_PREFIX_BYTES..].copy_from_slice(reconciliation.text.as_bytes());
    Ok(bytes)
}

/// Decode one complete borrowed reconciliation response without clipping.
pub fn decode_reconciliation(bytes: &[u8]) -> Result<Reconciliation<'_>, WatchError> {
    if bytes.len() < RECONCILIATION_PREFIX_BYTES {
        return Err(WatchError::InvalidLength);
    }
    if bytes.len() > MAX_RECONCILIATION_BYTES {
        return Err(WatchError::PayloadTooLarge);
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(WatchError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != RESPONSE_RECONCILIATION {
        return Err(WatchError::UnknownKind);
    }
    if bytes[101..104] != [0; 3] || bytes[108..RECONCILIATION_PREFIX_BYTES] != [0; 20] {
        return Err(WatchError::NonzeroReserved);
    }

    let info = decode_session_info(&bytes[4..4 + SESSION_INFO_BYTES])
        .map_err(|_| WatchError::InvalidSessionInfo)?;
    let cursor = EventCursor {
        generation: read_u64(bytes, 76),
        sequence: read_u64(bytes, 84),
    };
    validate_cursor(info.target, cursor)?;
    let screen_revision = read_u64(bytes, 92);
    if screen_revision == 0 {
        return Err(WatchError::ZeroIdentifier);
    }
    let truncated = match bytes[100] {
        0 => false,
        1 => true,
        _ => return Err(WatchError::InvalidStatus),
    };
    let text_len = usize::try_from(read_u32(bytes, 104)).map_err(|_| WatchError::InvalidLength)?;
    let expected_len = RECONCILIATION_PREFIX_BYTES
        .checked_add(text_len)
        .ok_or(WatchError::InvalidLength)?;
    if bytes.len() != expected_len {
        return Err(WatchError::InvalidLength);
    }
    let text = str::from_utf8(&bytes[RECONCILIATION_PREFIX_BYTES..])
        .map_err(|_| WatchError::InvalidText)?;
    validate_display_text(text)?;

    Ok(Reconciliation {
        info,
        cursor,
        screen_revision,
        truncated,
        text,
    })
}

/// Encode one validated fixed-size finite-watch terminal response.
pub fn encode_watch_end(end: &WatchEnd) -> Result<[u8; WATCH_END_BYTES], WatchError> {
    validate_watch_end(end)?;
    let mut bytes = [0; WATCH_END_BYTES];
    write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
    write_u16(&mut bytes, 2, RESPONSE_WATCH_END);
    write_u16(
        &mut bytes,
        4,
        match end.status {
            WatchStatus::Changed => 1,
            WatchStatus::TimedOut => 2,
            WatchStatus::ResyncRequired => 3,
        },
    );
    write_u16(&mut bytes, 6, end.event_count);
    write_u64(&mut bytes, 8, end.cursor.generation);
    write_u64(&mut bytes, 16, end.cursor.sequence);
    Ok(bytes)
}

/// Decode exactly one fixed-size finite-watch terminal response.
pub fn decode_watch_end(bytes: &[u8]) -> Result<WatchEnd, WatchError> {
    if bytes.len() != WATCH_END_BYTES {
        return Err(WatchError::InvalidLength);
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(WatchError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != RESPONSE_WATCH_END {
        return Err(WatchError::UnknownKind);
    }
    if bytes[24..32] != [0; 8] {
        return Err(WatchError::NonzeroReserved);
    }
    let status = match read_u16(bytes, 4) {
        1 => WatchStatus::Changed,
        2 => WatchStatus::TimedOut,
        3 => WatchStatus::ResyncRequired,
        _ => return Err(WatchError::InvalidStatus),
    };
    let end = WatchEnd {
        status,
        event_count: read_u16(bytes, 6),
        cursor: EventCursor {
            generation: read_u64(bytes, 8),
            sequence: read_u64(bytes, 16),
        },
    };
    validate_watch_end(&end)?;
    Ok(end)
}

fn validate_target(target: TargetRef) -> Result<(), WatchError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(WatchError::ZeroIdentifier);
    }
    Ok(())
}

fn validate_cursor(target: TargetRef, cursor: EventCursor) -> Result<(), WatchError> {
    if cursor.generation == 0 {
        return Err(WatchError::ZeroIdentifier);
    }
    if cursor.generation != target.generation {
        return Err(WatchError::GenerationMismatch);
    }
    Ok(())
}

fn validate_reconciliation_budget(max_bytes: u32, max_lines: u16) -> Result<(), WatchError> {
    if !(RECONCILIATION_PREFIX_BYTES as u32..=MAX_RECONCILIATION_BYTES as u32).contains(&max_bytes)
        || !(1..=MAX_RECONCILIATION_LINES).contains(&max_lines)
    {
        return Err(WatchError::InvalidBudget);
    }
    Ok(())
}

fn validate_watch_limit(limit: u16) -> Result<(), WatchError> {
    if !(1..=MAX_WATCH_EVENTS).contains(&limit) {
        return Err(WatchError::InvalidLimit);
    }
    Ok(())
}

fn validate_wait(wait_ms: u16) -> Result<(), WatchError> {
    if wait_ms > MAX_WATCH_WAIT_MS {
        return Err(WatchError::InvalidWait);
    }
    Ok(())
}

fn validate_display_text(text: &str) -> Result<(), WatchError> {
    if text
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(WatchError::InvalidText);
    }
    let lines = if text.is_empty() {
        0
    } else {
        1 + text.bytes().filter(|byte| *byte == b'\n').count()
    };
    if lines > usize::from(MAX_RECONCILIATION_LINES) {
        return Err(WatchError::TooManyLines);
    }
    Ok(())
}

fn validate_watch_end(end: &WatchEnd) -> Result<(), WatchError> {
    if end.cursor.generation == 0 {
        return Err(WatchError::ZeroIdentifier);
    }
    match end.status {
        WatchStatus::Changed => {
            if !(1..=MAX_WATCH_EVENTS).contains(&end.event_count) {
                return Err(WatchError::InvalidEventCount);
            }
            if end.cursor.sequence == 0 {
                return Err(WatchError::ZeroIdentifier);
            }
        }
        WatchStatus::TimedOut | WatchStatus::ResyncRequired => {
            if end.event_count != 0 {
                return Err(WatchError::InvalidEventCount);
            }
        }
    }
    Ok(())
}

fn write_target(bytes: &mut [u8], offset: usize, target: TargetRef) {
    write_u64(bytes, offset, target.generation);
    write_u64(bytes, offset + 8, target.workspace);
    write_u64(bytes, offset + 16, target.pane);
    write_u64(bytes, offset + 24, target.session);
    write_u64(bytes, offset + 32, target.occupant_revision);
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

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
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

    const GENERATION: u64 = 0x0102_0304_0506_0708;

    fn target() -> TargetRef {
        TargetRef {
            generation: GENERATION,
            workspace: 0x1112_1314_1516_1718,
            pane: 0x2122_2324_2526_2728,
            session: 0x3132_3334_3536_3738,
            occupant_revision: 0x4142_4344_4546_4748,
        }
    }

    fn session_info() -> SessionInfo {
        SessionInfo {
            target: target(),
            owner_revision: 0x5152_5354_5556_5758,
            owner_lease: 0x6162_6364_6566_6768,
            owner_expires_secs: 0x7172_7374_7576_7778,
            child_exited: true,
        }
    }

    fn write_golden_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    fn write_golden_target(bytes: &mut [u8], offset: usize, value: TargetRef) {
        write_golden_u64(bytes, offset, value.generation);
        write_golden_u64(bytes, offset + 8, value.workspace);
        write_golden_u64(bytes, offset + 16, value.pane);
        write_golden_u64(bytes, offset + 24, value.session);
        write_golden_u64(bytes, offset + 32, value.occupant_revision);
    }

    #[test]
    fn reconcile_request_matches_golden_layout_and_round_trips() {
        let request = WatchRequest::Reconcile {
            target: target(),
            max_bytes: 65_536,
            max_lines: 1_024,
        };
        let encoded = encode_watch_request(&request).expect("valid reconcile request");
        let mut golden = [0_u8; RECONCILE_REQUEST_BYTES];
        golden[0..2].copy_from_slice(&4_u16.to_be_bytes());
        golden[2..4].copy_from_slice(&1_u16.to_be_bytes());
        write_golden_target(&mut golden, 4, target());
        golden[44..48].copy_from_slice(&65_536_u32.to_be_bytes());
        golden[48..50].copy_from_slice(&1_024_u16.to_be_bytes());
        assert_eq!(encoded, golden);
        assert_eq!(decode_watch_request(&golden), Ok(request));
    }

    #[test]
    fn watch_request_matches_golden_layout_and_round_trips() {
        let request = WatchRequest::Watch {
            target: target(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0x5152_5354_5556_5758,
            },
            limit: 64,
            wait_ms: 1_000,
        };
        let encoded = encode_watch_request(&request).expect("valid watch request");
        let mut golden = [0_u8; WATCH_REQUEST_BYTES];
        golden[0..2].copy_from_slice(&4_u16.to_be_bytes());
        golden[2..4].copy_from_slice(&2_u16.to_be_bytes());
        write_golden_target(&mut golden, 4, target());
        write_golden_u64(&mut golden, 44, GENERATION);
        write_golden_u64(&mut golden, 52, 0x5152_5354_5556_5758);
        golden[60..62].copy_from_slice(&64_u16.to_be_bytes());
        golden[62..64].copy_from_slice(&1_000_u16.to_be_bytes());
        assert_eq!(encoded, golden);
        assert_eq!(decode_watch_request(&golden), Ok(request));
        assert_eq!(golden[64..], [0; 8]);
    }

    #[test]
    fn reconciliation_matches_golden_prefix_and_borrows_round_trip_text() {
        let text = "A\nΩ";
        let value = Reconciliation {
            info: session_info(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0,
            },
            screen_revision: 0x8182_8384_8586_8788,
            truncated: true,
            text,
        };
        let encoded = encode_reconciliation(&value).expect("valid snapshot");

        let mut golden = vec![0_u8; RECONCILIATION_PREFIX_BYTES + text.len()];
        golden[0..2].copy_from_slice(&4_u16.to_be_bytes());
        golden[2..4].copy_from_slice(&1_u16.to_be_bytes());
        golden[4..6].copy_from_slice(&2_u16.to_be_bytes());
        golden[6..8].copy_from_slice(&1_u16.to_be_bytes());
        write_golden_target(&mut golden, 8, target());
        write_golden_u64(&mut golden, 48, 0x5152_5354_5556_5758);
        write_golden_u64(&mut golden, 56, 0x6162_6364_6566_6768);
        write_golden_u64(&mut golden, 64, 0x7172_7374_7576_7778);
        golden[72] = 1;
        write_golden_u64(&mut golden, 76, GENERATION);
        write_golden_u64(&mut golden, 84, 0);
        write_golden_u64(&mut golden, 92, 0x8182_8384_8586_8788);
        golden[100] = 1;
        golden[104..108].copy_from_slice(&(text.len() as u32).to_be_bytes());
        golden[RECONCILIATION_PREFIX_BYTES..].copy_from_slice(text.as_bytes());

        assert_eq!(encoded, golden);
        assert_eq!(encoded.len(), 128 + 4);
        assert_eq!(decode_reconciliation(&golden), Ok(value.clone()));
        let decoded = decode_reconciliation(&encoded).expect("encoded snapshot decodes");
        assert_eq!(decoded.text.as_ptr(), encoded[128..].as_ptr());
        assert_eq!(decoded.info, value.info);
    }

    #[test]
    fn watch_end_matches_golden_layout_and_all_statuses_round_trip() {
        let changed = WatchEnd {
            status: WatchStatus::Changed,
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0x5152_5354_5556_5758,
            },
            event_count: 2,
        };
        let encoded = encode_watch_end(&changed).expect("valid changed response");
        let mut golden = [0_u8; WATCH_END_BYTES];
        golden[0..2].copy_from_slice(&4_u16.to_be_bytes());
        golden[2..4].copy_from_slice(&2_u16.to_be_bytes());
        golden[4..6].copy_from_slice(&1_u16.to_be_bytes());
        golden[6..8].copy_from_slice(&2_u16.to_be_bytes());
        write_golden_u64(&mut golden, 8, GENERATION);
        write_golden_u64(&mut golden, 16, 0x5152_5354_5556_5758);
        assert_eq!(encoded, golden);
        assert_eq!(decode_watch_end(&golden), Ok(changed));

        for (status, sequence) in [
            (WatchStatus::TimedOut, 0),
            (WatchStatus::TimedOut, 2),
            (WatchStatus::ResyncRequired, 0),
            (WatchStatus::ResyncRequired, 3),
        ] {
            let end = WatchEnd {
                status,
                cursor: EventCursor {
                    generation: GENERATION,
                    sequence,
                },
                event_count: 0,
            };
            assert_eq!(
                decode_watch_end(&encode_watch_end(&end).expect("valid empty end")),
                Ok(end)
            );
        }
        assert_eq!(encoded[24..], [0; 8]);
    }

    #[test]
    fn requests_reject_bad_lengths_reserved_fields_and_unknown_discriminants() {
        let request = encode_watch_request(&WatchRequest::Reconcile {
            target: target(),
            max_bytes: 128,
            max_lines: 1,
        })
        .expect("valid request");
        assert_eq!(
            decode_watch_request(&request[..request.len() - 1]),
            Err(WatchError::InvalidLength)
        );
        let mut trailing = request.clone();
        trailing.push(0);
        assert_eq!(
            decode_watch_request(&trailing),
            Err(WatchError::InvalidLength)
        );

        let mut bad_version = request.clone();
        bad_version[1] = 5;
        assert_eq!(
            decode_watch_request(&bad_version),
            Err(WatchError::UnsupportedSchema)
        );
        let mut bad_kind = request.clone();
        bad_kind[3] = 3;
        assert_eq!(
            decode_watch_request(&bad_kind),
            Err(WatchError::UnknownKind)
        );
        let mut reserved = request;
        reserved[55] = 1;
        assert_eq!(
            decode_watch_request(&reserved),
            Err(WatchError::NonzeroReserved)
        );

        let mut watch = encode_watch_request(&WatchRequest::Watch {
            target: target(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0,
            },
            limit: 1,
            wait_ms: 0,
        })
        .expect("valid watch");
        watch[64] = 1;
        assert_eq!(
            decode_watch_request(&watch),
            Err(WatchError::NonzeroReserved)
        );
    }

    #[test]
    fn requests_enforce_target_budgets_cursor_limits_and_wait() {
        let mut zero_target = target();
        zero_target.pane = 0;
        assert_eq!(
            encode_watch_request(&WatchRequest::Reconcile {
                target: zero_target,
                max_bytes: 128,
                max_lines: 1,
            }),
            Err(WatchError::ZeroIdentifier)
        );
        for max_bytes in [127, 65_537] {
            assert_eq!(
                encode_watch_request(&WatchRequest::Reconcile {
                    target: target(),
                    max_bytes,
                    max_lines: 1,
                }),
                Err(WatchError::InvalidBudget)
            );
        }
        for max_lines in [0, 1_025] {
            assert_eq!(
                encode_watch_request(&WatchRequest::Reconcile {
                    target: target(),
                    max_bytes: 128,
                    max_lines,
                }),
                Err(WatchError::InvalidBudget)
            );
        }
        for limit in [0, 65] {
            assert_eq!(
                encode_watch_request(&WatchRequest::Watch {
                    target: target(),
                    cursor: EventCursor {
                        generation: GENERATION,
                        sequence: 0
                    },
                    limit,
                    wait_ms: 0,
                }),
                Err(WatchError::InvalidLimit)
            );
        }
        assert_eq!(
            encode_watch_request(&WatchRequest::Watch {
                target: target(),
                cursor: EventCursor {
                    generation: GENERATION,
                    sequence: 0
                },
                limit: 1,
                wait_ms: 1_001,
            }),
            Err(WatchError::InvalidWait)
        );
        assert_eq!(
            encode_watch_request(&WatchRequest::Watch {
                target: target(),
                cursor: EventCursor {
                    generation: GENERATION + 1,
                    sequence: 0
                },
                limit: 1,
                wait_ms: 0,
            }),
            Err(WatchError::GenerationMismatch)
        );
        assert_eq!(
            encode_watch_request(&WatchRequest::Watch {
                target: target(),
                cursor: EventCursor {
                    generation: 0,
                    sequence: 0
                },
                limit: 1,
                wait_ms: 0,
            }),
            Err(WatchError::ZeroIdentifier)
        );
    }

    #[test]
    fn reconciliation_rejects_session_cursor_revision_and_text_invariants() {
        let mut invalid_info = session_info();
        invalid_info.owner_revision = 0;
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                info: invalid_info,
                cursor: EventCursor {
                    generation: GENERATION,
                    sequence: 0
                },
                screen_revision: 1,
                truncated: false,
                text: "",
            }),
            Err(WatchError::InvalidSessionInfo)
        );
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                info: session_info(),
                cursor: EventCursor {
                    generation: GENERATION + 1,
                    sequence: 0
                },
                screen_revision: 1,
                truncated: false,
                text: "",
            }),
            Err(WatchError::GenerationMismatch)
        );
        for screen_revision in [0, 1] {
            if screen_revision == 0 {
                assert_eq!(
                    encode_reconciliation(&Reconciliation {
                        info: session_info(),
                        cursor: EventCursor {
                            generation: GENERATION,
                            sequence: 0
                        },
                        screen_revision,
                        truncated: false,
                        text: "",
                    }),
                    Err(WatchError::ZeroIdentifier)
                );
            }
        }
        for text in ["bad\0text", "bad\rtext", "bad\ttext", "bad\u{7f}text"] {
            assert_eq!(
                encode_reconciliation(&Reconciliation {
                    info: session_info(),
                    cursor: EventCursor {
                        generation: GENERATION,
                        sequence: 0
                    },
                    screen_revision: 1,
                    truncated: false,
                    text,
                }),
                Err(WatchError::InvalidText)
            );
        }
        let too_many_lines = "x\n".repeat(1_024);
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                info: session_info(),
                cursor: EventCursor {
                    generation: GENERATION,
                    sequence: 0
                },
                screen_revision: 1,
                truncated: false,
                text: &too_many_lines,
            }),
            Err(WatchError::TooManyLines)
        );
        let exactly_max_lines = format!("{}x", "x\n".repeat(1_023));
        assert!(encode_reconciliation(&Reconciliation {
            info: session_info(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0
            },
            screen_revision: 1,
            truncated: false,
            text: &exactly_max_lines,
        })
        .is_ok());
    }

    #[test]
    fn reconciliation_accepts_complete_size_boundary_and_rejects_one_over() {
        let max_text = "x".repeat(MAX_RECONCILIATION_BYTES - RECONCILIATION_PREFIX_BYTES);
        let exact = encode_reconciliation(&Reconciliation {
            info: session_info(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0,
            },
            screen_revision: 1,
            truncated: false,
            text: &max_text,
        })
        .expect("complete response at the size limit");
        assert_eq!(exact.len(), MAX_RECONCILIATION_BYTES);
        assert_eq!(
            decode_reconciliation(&exact)
                .expect("boundary response decodes")
                .text
                .len(),
            max_text.len()
        );

        let one_over = format!("{max_text}x");
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                info: session_info(),
                cursor: EventCursor {
                    generation: GENERATION,
                    sequence: 0
                },
                screen_revision: 1,
                truncated: false,
                text: &one_over,
            }),
            Err(WatchError::PayloadTooLarge)
        );
    }

    #[test]
    fn reconciliation_decode_rejects_length_reserved_boolean_utf8_and_controls() {
        let encoded = encode_reconciliation(&Reconciliation {
            info: session_info(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 0,
            },
            screen_revision: 1,
            truncated: false,
            text: "ok",
        })
        .expect("valid response");
        assert_eq!(
            decode_reconciliation(&encoded[..127]),
            Err(WatchError::InvalidLength)
        );
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            decode_reconciliation(&trailing),
            Err(WatchError::InvalidLength)
        );
        let mut reserved = encoded.clone();
        reserved[101] = 1;
        assert_eq!(
            decode_reconciliation(&reserved),
            Err(WatchError::NonzeroReserved)
        );
        let mut bad_boolean = encoded.clone();
        bad_boolean[100] = 2;
        assert_eq!(
            decode_reconciliation(&bad_boolean),
            Err(WatchError::InvalidStatus)
        );
        let mut invalid_utf8 = encoded.clone();
        invalid_utf8[128] = 0xff;
        assert_eq!(
            decode_reconciliation(&invalid_utf8),
            Err(WatchError::InvalidText)
        );
        let mut forbidden_control = encoded;
        forbidden_control[128] = b'\r';
        assert_eq!(
            decode_reconciliation(&forbidden_control),
            Err(WatchError::InvalidText)
        );
    }

    #[test]
    fn watch_end_rejects_malformed_status_count_cursor_and_reserved_bytes() {
        let valid = encode_watch_end(&WatchEnd {
            status: WatchStatus::Changed,
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 1,
            },
            event_count: 1,
        })
        .expect("valid changed response");
        assert_eq!(
            decode_watch_end(&valid[..31]),
            Err(WatchError::InvalidLength)
        );
        let mut bad_version = valid;
        bad_version[0] = 5;
        assert_eq!(
            decode_watch_end(&bad_version),
            Err(WatchError::UnsupportedSchema)
        );
        let mut bad_kind = valid;
        bad_kind[3] = 3;
        assert_eq!(decode_watch_end(&bad_kind), Err(WatchError::UnknownKind));
        let mut bad_status = valid;
        bad_status[5] = 4;
        assert_eq!(
            decode_watch_end(&bad_status),
            Err(WatchError::InvalidStatus)
        );
        let mut reserved = valid;
        reserved[31] = 1;
        assert_eq!(
            decode_watch_end(&reserved),
            Err(WatchError::NonzeroReserved)
        );
        let mut zero_generation = valid;
        zero_generation[8..16].fill(0);
        assert_eq!(
            decode_watch_end(&zero_generation),
            Err(WatchError::ZeroIdentifier)
        );
        let mut zero_sequence = valid;
        zero_sequence[16..24].fill(0);
        assert_eq!(
            decode_watch_end(&zero_sequence),
            Err(WatchError::ZeroIdentifier)
        );
        let mut too_many = valid;
        too_many[6..8].copy_from_slice(&65_u16.to_be_bytes());
        assert_eq!(
            decode_watch_end(&too_many),
            Err(WatchError::InvalidEventCount)
        );
        let mut empty_changed = valid;
        empty_changed[6..8].fill(0);
        assert_eq!(
            decode_watch_end(&empty_changed),
            Err(WatchError::InvalidEventCount)
        );
        let mut timed_out_with_events = [0; WATCH_END_BYTES];
        timed_out_with_events[0..2].copy_from_slice(&4_u16.to_be_bytes());
        timed_out_with_events[2..4].copy_from_slice(&2_u16.to_be_bytes());
        timed_out_with_events[4..6].copy_from_slice(&2_u16.to_be_bytes());
        timed_out_with_events[6..8].copy_from_slice(&1_u16.to_be_bytes());
        write_golden_u64(&mut timed_out_with_events, 8, GENERATION);
        assert_eq!(
            decode_watch_end(&timed_out_with_events),
            Err(WatchError::InvalidEventCount)
        );
    }

    #[test]
    fn debug_output_redacts_targets_cursors_revisions_counts_and_text() {
        let canaries = [
            format!("{GENERATION}"),
            format!("{}", target().workspace),
            format!("{}", target().pane),
            format!("{}", target().session),
            format!("{}", target().occupant_revision),
            "private-screen-canary".to_owned(),
        ];
        let request = WatchRequest::Watch {
            target: target(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 90_001,
            },
            limit: 64,
            wait_ms: 1_000,
        };
        let reconciliation = Reconciliation {
            info: session_info(),
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 90_001,
            },
            screen_revision: 80_002,
            truncated: false,
            text: "private-screen-canary",
        };
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: EventCursor {
                generation: GENERATION,
                sequence: 90_001,
            },
            event_count: 64,
        };
        for debug in [
            format!("{request:?}"),
            format!("{reconciliation:?}"),
            format!("{end:?}"),
        ] {
            for canary in canaries
                .iter()
                .chain([&"90001".to_owned(), &"80002".to_owned()])
            {
                assert!(!debug.contains(canary), "debug leaked {canary}: {debug}");
            }
        }
        assert!(!format!("{reconciliation:?}").contains("private-screen-canary"));
    }
}
