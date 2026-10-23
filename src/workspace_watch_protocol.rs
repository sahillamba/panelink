//! Pure schema-11 workspace layout reconciliation and finite-watch payloads.
//!
//! Workspace events are revision invalidations. This codec carries no terminal
//! contents, performs no I/O, and does not dispatch or retry mutations.

use crate::ui_protocol::{self, LayoutReply};
use crate::watch_protocol::WatchStatus;
use std::error::Error;
use std::fmt;

/// Version of the workspace reconciliation and watch payloads.
pub const WORKSPACE_WATCH_SCHEMA: u16 = 11;
/// Exact request size.
pub const REQUEST_BYTES: usize = 40;
/// Exact workspace reconciliation size.
pub const RECONCILIATION_BYTES: usize = 472;
/// Header size of one workspace watch response.
pub const WATCH_REPLY_HEADER_BYTES: usize = 40;
/// Exact metadata size of one layout event.
pub const LAYOUT_EVENT_BYTES: usize = 24;
/// Maximum events in a watch page.
pub const MAX_WORKSPACE_WATCH_EVENTS: usize = 64;
/// Maximum encoded watch response size.
pub const MAX_WATCH_REPLY_BYTES: usize =
    WATCH_REPLY_HEADER_BYTES + MAX_WORKSPACE_WATCH_EVENTS * LAYOUT_EVENT_BYTES;
/// Maximum finite wait in milliseconds.
pub const MAX_WORKSPACE_WAIT_MS: u16 = 1_000;

const REQUEST_RECONCILE: u16 = 1;
const REQUEST_WATCH: u16 = 2;
const RESPONSE_RECONCILIATION: u16 = 0x8001;
const RESPONSE_WATCH: u16 = 0x8002;
const LAYOUT_OFFSET: usize = 48;

/// Cursor namespace for one workspace in one server generation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WorkspaceCursor {
    /// Server-start generation.
    pub generation: u64,
    /// Stable workspace identifier.
    pub workspace: u64,
    /// Event sequence; zero is the initial cursor.
    pub sequence: u64,
}

impl fmt::Debug for WorkspaceCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkspaceCursor { <redacted> }")
    }
}

/// One bounded workspace reconciliation or finite-watch request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Request {
    /// Obtain the current layout and its atomically captured workspace cursor.
    Reconcile { generation: u64, workspace: u64 },
    /// Read ordered layout revision invalidations after a cursor.
    Watch {
        /// Cursor previously returned by reconciliation or a watch.
        cursor: WorkspaceCursor,
        /// Maximum number of events to return.
        limit: u16,
        /// Finite wait duration in milliseconds.
        wait_ms: u16,
    },
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Reconcile { .. } => "Reconcile",
            Self::Watch { .. } => "Watch",
        };
        f.debug_struct("Request")
            .field("kind", &kind)
            .field("fields", &"<redacted>")
            .finish()
    }
}

/// Authoritative layout snapshot and cursor captured in one owner turn.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Reconciliation {
    /// Cursor paired with this snapshot.
    pub cursor: WorkspaceCursor,
    /// Current revision, or last revision when `layout` is empty.
    pub revision: u64,
    /// Current validated layout; absent means no positively live layout remains.
    pub layout: Option<LayoutReply>,
}

impl fmt::Debug for Reconciliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reconciliation")
            .field("cursor", &"<redacted>")
            .field("revision", &"<redacted>")
            .field("has_layout", &self.layout.is_some())
            .finish()
    }
}

/// One workspace topology invalidation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutEvent {
    /// Consecutive event sequence in the workspace journal.
    pub sequence: u64,
    /// Nonzero committed layout revision.
    pub revision: u64,
    /// Whether no positively live layout remains.
    pub empty: bool,
}

impl fmt::Debug for LayoutEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutEvent")
            .field("sequence", &"<redacted>")
            .field("revision", &"<redacted>")
            .field("empty", &self.empty)
            .finish()
    }
}

/// One complete finite workspace-watch response.
#[derive(Clone, Eq, PartialEq)]
pub struct WatchReply {
    /// Watch outcome.
    pub status: WatchStatus,
    /// Cursor associated with the response; on resync it is only a hint.
    pub cursor: WorkspaceCursor,
    /// Complete ordered events, empty unless status is `Changed`.
    pub events: Vec<LayoutEvent>,
}

impl fmt::Debug for WatchReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchReply")
            .field("status", &self.status)
            .field("cursor", &"<redacted>")
            .field("event_count", &self.events.len())
            .finish()
    }
}

/// Fixed, data-free errors for workspace watch payloads.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CodecError {
    /// Payload has an invalid size or trailing bytes.
    InvalidLength,
    /// Schema number is unsupported.
    UnsupportedSchema,
    /// Request or response kind is unknown.
    UnknownKind,
    /// Reserved bytes are nonzero.
    NonzeroReserved,
    /// Generation, workspace, or required revision is zero.
    ZeroIdentifier,
    /// Request fields do not match their operation.
    InvalidRequest,
    /// Watch limit or wait is outside the supported bounds.
    InvalidLimitOrWait,
    /// Embedded layout failed the existing layout codec or scope checks.
    InvalidLayout,
    /// Reconciliation layout and revision are inconsistent.
    InvalidReconciliation,
    /// Event flag is not zero or one.
    InvalidEvent,
    /// Status and event count are inconsistent.
    InvalidStatusOrCount,
    /// Event sequences are not positive, consecutive, and ending at the cursor.
    InvalidEventSequence,
}

impl fmt::Debug for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLength => "InvalidLength",
            Self::UnsupportedSchema => "UnsupportedSchema",
            Self::UnknownKind => "UnknownKind",
            Self::NonzeroReserved => "NonzeroReserved",
            Self::ZeroIdentifier => "ZeroIdentifier",
            Self::InvalidRequest => "InvalidRequest",
            Self::InvalidLimitOrWait => "InvalidLimitOrWait",
            Self::InvalidLayout => "InvalidLayout",
            Self::InvalidReconciliation => "InvalidReconciliation",
            Self::InvalidEvent => "InvalidEvent",
            Self::InvalidStatusOrCount => "InvalidStatusOrCount",
            Self::InvalidEventSequence => "InvalidEventSequence",
        })
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("workspace watch payload is invalid")
    }
}

impl Error for CodecError {}

/// Encode one validated workspace request.
pub fn encode_request(request: &Request) -> Result<Vec<u8>, CodecError> {
    let mut bytes = vec![0; REQUEST_BYTES];
    put_u16(&mut bytes, 0, WORKSPACE_WATCH_SCHEMA);
    match *request {
        Request::Reconcile {
            generation,
            workspace,
        } => {
            validate_namespace(generation, workspace)?;
            put_u16(&mut bytes, 2, REQUEST_RECONCILE);
            put_u64(&mut bytes, 8, generation);
            put_u64(&mut bytes, 16, workspace);
        }
        Request::Watch {
            cursor,
            limit,
            wait_ms,
        } => {
            validate_cursor(cursor)?;
            validate_watch_bounds(limit, wait_ms)?;
            put_u16(&mut bytes, 2, REQUEST_WATCH);
            put_u64(&mut bytes, 8, cursor.generation);
            put_u64(&mut bytes, 16, cursor.workspace);
            put_u64(&mut bytes, 24, cursor.sequence);
            put_u16(&mut bytes, 32, limit);
            put_u16(&mut bytes, 34, wait_ms);
        }
    }
    Ok(bytes)
}

/// Decode one exact, validated workspace request.
pub fn decode_request(bytes: &[u8]) -> Result<Request, CodecError> {
    exact_size(bytes, REQUEST_BYTES)?;
    check_schema(bytes)?;
    zero(&bytes[4..8])?;
    zero(&bytes[36..40])?;
    let generation = read_u64(bytes, 8);
    let workspace = read_u64(bytes, 16);
    match read_u16(bytes, 2) {
        REQUEST_RECONCILE => {
            validate_namespace(generation, workspace)?;
            if read_u64(bytes, 24) != 0 || read_u16(bytes, 32) != 0 || read_u16(bytes, 34) != 0 {
                return Err(CodecError::InvalidRequest);
            }
            Ok(Request::Reconcile {
                generation,
                workspace,
            })
        }
        REQUEST_WATCH => {
            let cursor = WorkspaceCursor {
                generation,
                workspace,
                sequence: read_u64(bytes, 24),
            };
            let limit = read_u16(bytes, 32);
            let wait_ms = read_u16(bytes, 34);
            validate_cursor(cursor)?;
            validate_watch_bounds(limit, wait_ms)?;
            Ok(Request::Watch {
                cursor,
                limit,
                wait_ms,
            })
        }
        _ => Err(CodecError::UnknownKind),
    }
}

/// Encode one bounded workspace reconciliation.
pub fn encode_reconciliation(value: &Reconciliation) -> Result<Vec<u8>, CodecError> {
    validate_reconciliation(value)?;
    let mut bytes = vec![0; RECONCILIATION_BYTES];
    put_u16(&mut bytes, 0, WORKSPACE_WATCH_SCHEMA);
    put_u16(&mut bytes, 2, RESPONSE_RECONCILIATION);
    bytes[4] = u8::from(value.layout.is_some());
    put_cursor(&mut bytes, 8, value.cursor);
    put_u64(&mut bytes, 32, value.revision);
    if let Some(layout) = &value.layout {
        let encoded =
            ui_protocol::encode_layout_reply(layout).map_err(|_| CodecError::InvalidLayout)?;
        bytes[LAYOUT_OFFSET..].copy_from_slice(&encoded);
    }
    Ok(bytes)
}

/// Decode one exact workspace reconciliation.
pub fn decode_reconciliation(bytes: &[u8]) -> Result<Reconciliation, CodecError> {
    exact_size(bytes, RECONCILIATION_BYTES)?;
    check_schema(bytes)?;
    if read_u16(bytes, 2) != RESPONSE_RECONCILIATION {
        return Err(CodecError::UnknownKind);
    }
    zero(&bytes[5..8])?;
    zero(&bytes[40..48])?;
    let cursor = read_cursor(bytes, 8);
    let revision = read_u64(bytes, 32);
    let layout = match bytes[4] {
        0 => {
            zero(&bytes[LAYOUT_OFFSET..])?;
            None
        }
        1 => Some(
            ui_protocol::decode_layout_reply(&bytes[LAYOUT_OFFSET..])
                .map_err(|_| CodecError::InvalidLayout)?,
        ),
        _ => return Err(CodecError::InvalidEvent),
    };
    let value = Reconciliation {
        cursor,
        revision,
        layout,
    };
    validate_reconciliation(&value)?;
    Ok(value)
}

/// Encode a complete bounded finite-watch response.
pub fn encode_watch_reply(reply: &WatchReply) -> Result<Vec<u8>, CodecError> {
    validate_watch_reply(reply)?;
    let mut bytes = vec![0; WATCH_REPLY_HEADER_BYTES + reply.events.len() * LAYOUT_EVENT_BYTES];
    put_u16(&mut bytes, 0, WORKSPACE_WATCH_SCHEMA);
    put_u16(&mut bytes, 2, RESPONSE_WATCH);
    put_u16(&mut bytes, 4, status_code(reply.status));
    put_u16(&mut bytes, 6, reply.events.len() as u16);
    put_cursor(&mut bytes, 8, reply.cursor);
    for (index, event) in reply.events.iter().enumerate() {
        let offset = WATCH_REPLY_HEADER_BYTES + index * LAYOUT_EVENT_BYTES;
        put_u64(&mut bytes, offset, event.sequence);
        put_u64(&mut bytes, offset + 8, event.revision);
        bytes[offset + 16] = u8::from(event.empty);
    }
    Ok(bytes)
}

/// Decode and validate an entire bounded finite-watch response.
pub fn decode_watch_reply(bytes: &[u8]) -> Result<WatchReply, CodecError> {
    if bytes.len() < WATCH_REPLY_HEADER_BYTES || bytes.len() > MAX_WATCH_REPLY_BYTES {
        return Err(CodecError::InvalidLength);
    }
    check_schema(bytes)?;
    if read_u16(bytes, 2) != RESPONSE_WATCH {
        return Err(CodecError::UnknownKind);
    }
    zero(&bytes[32..40])?;
    let count = usize::from(read_u16(bytes, 6));
    if count > MAX_WORKSPACE_WATCH_EVENTS
        || bytes.len() != WATCH_REPLY_HEADER_BYTES + count * LAYOUT_EVENT_BYTES
    {
        return Err(CodecError::InvalidLength);
    }
    let status = decode_status(read_u16(bytes, 4))?;
    let cursor = read_cursor(bytes, 8);
    let mut events = Vec::with_capacity(count);
    for index in 0..count {
        let offset = WATCH_REPLY_HEADER_BYTES + index * LAYOUT_EVENT_BYTES;
        zero(&bytes[offset + 17..offset + 24])?;
        let empty = match bytes[offset + 16] {
            0 => false,
            1 => true,
            _ => return Err(CodecError::InvalidEvent),
        };
        events.push(LayoutEvent {
            sequence: read_u64(bytes, offset),
            revision: read_u64(bytes, offset + 8),
            empty,
        });
    }
    let reply = WatchReply {
        status,
        cursor,
        events,
    };
    validate_watch_reply(&reply)?;
    Ok(reply)
}

fn validate_reconciliation(value: &Reconciliation) -> Result<(), CodecError> {
    validate_cursor(value.cursor)?;
    if value.revision == 0 {
        return Err(CodecError::ZeroIdentifier);
    }
    match value.layout {
        None => Ok(()),
        Some(layout) => {
            if layout.revision != value.revision
                || ui_protocol::encode_layout_reply(&layout).is_err()
                || layout.panes.iter().flatten().any(|pane| {
                    pane.target.generation != value.cursor.generation
                        || pane.target.workspace != value.cursor.workspace
                })
            {
                return Err(CodecError::InvalidReconciliation);
            }
            Ok(())
        }
    }
}

fn validate_watch_reply(reply: &WatchReply) -> Result<(), CodecError> {
    validate_cursor(reply.cursor)?;
    match reply.status {
        WatchStatus::Changed => {
            if reply.events.is_empty() || reply.events.len() > MAX_WORKSPACE_WATCH_EVENTS {
                return Err(CodecError::InvalidStatusOrCount);
            }
            let first = reply.events[0].sequence;
            for (index, event) in reply.events.iter().enumerate() {
                if event.sequence == 0 || event.revision == 0 {
                    return Err(CodecError::ZeroIdentifier);
                }
                if event.sequence
                    != first
                        .checked_add(index as u64)
                        .ok_or(CodecError::InvalidEventSequence)?
                {
                    return Err(CodecError::InvalidEventSequence);
                }
            }
            if reply.events.last().map(|event| event.sequence) != Some(reply.cursor.sequence) {
                return Err(CodecError::InvalidEventSequence);
            }
        }
        WatchStatus::TimedOut | WatchStatus::ResyncRequired if !reply.events.is_empty() => {
            return Err(CodecError::InvalidStatusOrCount);
        }
        WatchStatus::TimedOut | WatchStatus::ResyncRequired => {}
    }
    Ok(())
}

fn validate_cursor(cursor: WorkspaceCursor) -> Result<(), CodecError> {
    validate_namespace(cursor.generation, cursor.workspace)
}

fn validate_namespace(generation: u64, workspace: u64) -> Result<(), CodecError> {
    if generation == 0 || workspace == 0 {
        Err(CodecError::ZeroIdentifier)
    } else {
        Ok(())
    }
}

fn validate_watch_bounds(limit: u16, wait_ms: u16) -> Result<(), CodecError> {
    if !(1..=MAX_WORKSPACE_WATCH_EVENTS as u16).contains(&limit) || wait_ms > MAX_WORKSPACE_WAIT_MS
    {
        return Err(CodecError::InvalidLimitOrWait);
    }
    Ok(())
}

fn status_code(status: WatchStatus) -> u16 {
    match status {
        WatchStatus::Changed => 1,
        WatchStatus::TimedOut => 2,
        WatchStatus::ResyncRequired => 3,
    }
}

fn decode_status(value: u16) -> Result<WatchStatus, CodecError> {
    match value {
        1 => Ok(WatchStatus::Changed),
        2 => Ok(WatchStatus::TimedOut),
        3 => Ok(WatchStatus::ResyncRequired),
        _ => Err(CodecError::InvalidStatusOrCount),
    }
}

fn exact_size(bytes: &[u8], expected: usize) -> Result<(), CodecError> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(CodecError::InvalidLength)
    }
}

fn check_schema(bytes: &[u8]) -> Result<(), CodecError> {
    if read_u16(bytes, 0) == WORKSPACE_WATCH_SCHEMA {
        Ok(())
    } else {
        Err(CodecError::UnsupportedSchema)
    }
}

fn zero(bytes: &[u8]) -> Result<(), CodecError> {
    if bytes.iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(CodecError::NonzeroReserved)
    }
}

fn put_cursor(bytes: &mut [u8], offset: usize, cursor: WorkspaceCursor) {
    put_u64(bytes, offset, cursor.generation);
    put_u64(bytes, offset + 8, cursor.workspace);
    put_u64(bytes, offset + 16, cursor.sequence);
}

fn read_cursor(bytes: &[u8], offset: usize) -> WorkspaceCursor {
    WorkspaceCursor {
        generation: read_u64(bytes, offset),
        workspace: read_u64(bytes, offset + 8),
        sequence: read_u64(bytes, offset + 16),
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}
fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed validated slice"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::TargetRef;
    use crate::pane_layout::Rect;
    use crate::ui_protocol::UiPane;

    fn cursor(sequence: u64) -> WorkspaceCursor {
        WorkspaceCursor {
            generation: 4,
            workspace: 7,
            sequence,
        }
    }

    fn layout(revision: u64) -> LayoutReply {
        LayoutReply {
            revision,
            rows: 24,
            columns: 80,
            panes: [
                Some(UiPane {
                    target: TargetRef {
                        generation: 4,
                        workspace: 7,
                        pane: 9,
                        session: 10,
                        occupant_revision: 1,
                    },
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 80,
                    },
                    pending_confirmed: None,
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; crate::pane_layout::MAX_PANES - 1],
        }
    }

    #[test]
    fn request_reconcile_and_watch_round_trip_with_exact_sizes() {
        let reconcile = Request::Reconcile {
            generation: 4,
            workspace: 7,
        };
        let encoded = encode_request(&reconcile).unwrap();
        assert_eq!(encoded.len(), REQUEST_BYTES);
        assert_eq!(decode_request(&encoded), Ok(reconcile));
        let watch = Request::Watch {
            cursor: cursor(0),
            limit: 64,
            wait_ms: 1_000,
        };
        assert_eq!(decode_request(&encode_request(&watch).unwrap()), Ok(watch));
    }

    #[test]
    fn request_rejects_reserved_scope_kind_and_bound_corruption() {
        let valid = encode_request(&Request::Reconcile {
            generation: 4,
            workspace: 7,
        })
        .unwrap();
        for (offset, value, expected) in [
            (4, 1, CodecError::NonzeroReserved),
            (0, 10, CodecError::UnsupportedSchema),
            (2, 3, CodecError::UnknownKind),
        ] {
            let mut bad = valid.clone();
            bad[offset] = value;
            assert_eq!(decode_request(&bad), Err(expected));
        }
        let mut bad = valid.clone();
        bad[8..16].fill(0);
        assert_eq!(decode_request(&bad), Err(CodecError::ZeroIdentifier));
        assert_eq!(decode_request(&valid[..39]), Err(CodecError::InvalidLength));
        assert!(encode_request(&Request::Watch {
            cursor: cursor(0),
            limit: 0,
            wait_ms: 0
        })
        .is_err());
        assert!(encode_request(&Request::Watch {
            cursor: cursor(0),
            limit: 1,
            wait_ms: 1_001
        })
        .is_err());
    }

    #[test]
    fn reconciliation_round_trips_layout_and_empty_and_checks_scope() {
        let value = Reconciliation {
            cursor: cursor(12),
            revision: 3,
            layout: Some(layout(3)),
        };
        let bytes = encode_reconciliation(&value).unwrap();
        assert_eq!(bytes.len(), RECONCILIATION_BYTES);
        assert_eq!(bytes[4], 1, "one means layout present");
        assert_eq!(decode_reconciliation(&bytes), Ok(value));
        let empty = Reconciliation {
            cursor: cursor(13),
            revision: 3,
            layout: None,
        };
        let bytes = encode_reconciliation(&empty).unwrap();
        assert_eq!(bytes[4], 0, "zero means empty");
        assert_eq!(decode_reconciliation(&bytes), Ok(empty));
        let mut bad = bytes;
        bad[40] = 1;
        assert_eq!(
            decode_reconciliation(&bad),
            Err(CodecError::NonzeroReserved)
        );
        let mut mismatch = layout(3);
        mismatch.panes[0].as_mut().unwrap().target.workspace = 8;
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                layout: Some(mismatch),
                ..value
            }),
            Err(CodecError::InvalidReconciliation)
        );
        assert_eq!(
            encode_reconciliation(&Reconciliation {
                revision: 0,
                ..empty
            }),
            Err(CodecError::ZeroIdentifier)
        );
    }

    #[test]
    fn watch_reply_round_trips_and_rejects_gap_count_reserved_and_bool() {
        let reply = WatchReply {
            status: WatchStatus::Changed,
            cursor: cursor(3),
            events: vec![
                LayoutEvent {
                    sequence: 2,
                    revision: 4,
                    empty: false,
                },
                LayoutEvent {
                    sequence: 3,
                    revision: 5,
                    empty: true,
                },
            ],
        };
        let bytes = encode_watch_reply(&reply).unwrap();
        assert_eq!(decode_watch_reply(&bytes), Ok(reply));
        assert_eq!(
            bytes.len(),
            WATCH_REPLY_HEADER_BYTES + 2 * LAYOUT_EVENT_BYTES
        );
        let mut bad = bytes.clone();
        let second_sequence = (WATCH_REPLY_HEADER_BYTES + LAYOUT_EVENT_BYTES)
            ..(WATCH_REPLY_HEADER_BYTES + LAYOUT_EVENT_BYTES + 8);
        bad[second_sequence].copy_from_slice(&4_u64.to_be_bytes());
        assert_eq!(
            decode_watch_reply(&bad),
            Err(CodecError::InvalidEventSequence)
        );
        let mut bad = bytes.clone();
        bad[WATCH_REPLY_HEADER_BYTES + 17] = 1;
        assert_eq!(decode_watch_reply(&bad), Err(CodecError::NonzeroReserved));
        let mut bad = bytes.clone();
        bad[WATCH_REPLY_HEADER_BYTES + 16] = 2;
        assert_eq!(decode_watch_reply(&bad), Err(CodecError::InvalidEvent));
        let timed_out = WatchReply {
            status: WatchStatus::TimedOut,
            cursor: cursor(3),
            events: vec![],
        };
        assert_eq!(
            decode_watch_reply(&encode_watch_reply(&timed_out).unwrap()),
            Ok(timed_out)
        );
        assert_eq!(
            encode_watch_reply(&WatchReply {
                status: WatchStatus::Changed,
                cursor: cursor(0),
                events: vec![]
            }),
            Err(CodecError::InvalidStatusOrCount)
        );
    }

    #[test]
    fn watch_reply_rejects_zero_identifiers_bad_counts_and_sequence_overflow() {
        let zero_cursor = WatchReply {
            status: WatchStatus::TimedOut,
            cursor: WorkspaceCursor {
                generation: 0,
                workspace: 7,
                sequence: 0,
            },
            events: vec![],
        };
        assert_eq!(
            encode_watch_reply(&zero_cursor),
            Err(CodecError::ZeroIdentifier)
        );

        let reply = WatchReply {
            status: WatchStatus::Changed,
            cursor: cursor(1),
            events: vec![LayoutEvent {
                sequence: 1,
                revision: 1,
                empty: false,
            }],
        };
        let bytes = encode_watch_reply(&reply).unwrap();
        let mut wrong_count = bytes.clone();
        wrong_count[6..8].copy_from_slice(&0_u16.to_be_bytes());
        assert_eq!(
            decode_watch_reply(&wrong_count),
            Err(CodecError::InvalidLength)
        );

        let overflow = WatchReply {
            status: WatchStatus::Changed,
            cursor: cursor(u64::MAX),
            events: vec![
                LayoutEvent {
                    sequence: u64::MAX,
                    revision: 1,
                    empty: false,
                },
                LayoutEvent {
                    sequence: u64::MAX,
                    revision: 2,
                    empty: true,
                },
            ],
        };
        assert_eq!(
            encode_watch_reply(&overflow),
            Err(CodecError::InvalidEventSequence)
        );
        let zero_event = WatchReply {
            status: WatchStatus::Changed,
            cursor: cursor(1),
            events: vec![LayoutEvent {
                sequence: 0,
                revision: 1,
                empty: false,
            }],
        };
        assert_eq!(
            encode_watch_reply(&zero_event),
            Err(CodecError::ZeroIdentifier)
        );
    }

    #[test]
    fn debug_and_errors_do_not_disclose_identity_or_revision() {
        let text = format!(
            "{:?}",
            Reconciliation {
                cursor: cursor(91),
                revision: 73,
                layout: Some(layout(73))
            }
        );
        assert!(!text.contains("91"));
        assert!(!text.contains("73"));
        assert!(!text.contains("generation"));
        assert_eq!(
            CodecError::InvalidLayout.to_string(),
            "workspace watch payload is invalid"
        );
    }
}
