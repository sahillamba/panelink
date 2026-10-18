//! Pure schema-5 watch payloads with an explicit per-session cursor namespace.
//!
//! The existing schema-4 codec remains unchanged. This wrapper reuses its
//! validation by making bounded temporary payload copies, changing only the
//! schema number and the newly assigned session field before delegation. The
//! largest temporary copy is one complete reconciliation payload, capped at
//! 65,536 bytes; returned reconciliation text borrows the original input.
//! This module does not negotiate schemas, access runtime state, or dispatch.

use crate::control_protocol::TargetRef;
use crate::event_journal::EventCursor;
use crate::session_protocol::SessionInfo;
use crate::watch_protocol::{
    self, Reconciliation as BaseReconciliation, WatchEnd as BaseWatchEnd,
    WatchRequest as BaseWatchRequest, WatchStatus,
};
use std::error::Error;
use std::fmt;
use std::str;

/// Schema version used by the session-scoped watch codec.
pub const WATCH_SCHEMA_VERSION: u16 = 5;

/// Maximum complete reconciliation payload, including the prefix.
pub const MAX_RECONCILIATION_BYTES: usize = watch_protocol::MAX_RECONCILIATION_BYTES;

/// Fixed prefix size for a schema-5 reconciliation response.
pub const RECONCILIATION_PREFIX_BYTES: usize = watch_protocol::RECONCILIATION_PREFIX_BYTES;

/// Exact encoded size of a schema-5 finite-watch terminal response.
pub const WATCH_END_BYTES: usize = watch_protocol::WATCH_END_BYTES;

const RECONCILE_REQUEST_BYTES: usize = 56;
const WATCH_REQUEST_BYTES: usize = 72;
const REQUEST_WATCH: u16 = 2;
const RESPONSE_RECONCILIATION: u16 = 1;
const RECONCILIATION_SESSION_OFFSET: usize = 108;
const RECONCILIATION_RESERVED_START: usize = 116;
const WATCH_SESSION_OFFSET: usize = 64;
const WATCH_END_SESSION_OFFSET: usize = 24;

/// Cursor namespace for one server generation and one managed session.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SessionCursor {
    /// Server-start generation containing the session.
    pub generation: u64,
    /// Never-reused server-issued managed-session identity.
    pub session: u64,
    /// Event sequence within this session; zero is the initial cursor.
    pub sequence: u64,
}

impl fmt::Debug for SessionCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionCursor { <redacted> }")
    }
}

/// One bounded schema-5 reconciliation or finite-watch request.
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
        /// Last event cursor already observed for this exact session.
        cursor: SessionCursor,
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

/// Borrowed authoritative snapshot payload before schema-5 encoding.
#[derive(Clone, Eq, PartialEq)]
pub struct Reconciliation<'a> {
    /// Current target, ownership, and lifecycle metadata.
    pub info: SessionInfo,
    /// Cursor captured atomically with this session's snapshot.
    pub cursor: SessionCursor,
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

/// Terminal metadata for a finite session-scoped watch response.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WatchEnd {
    /// Whether events changed, the finite wait expired, or state needs resync.
    pub status: WatchStatus,
    /// Cursor associated with this status, including its session namespace.
    pub cursor: SessionCursor,
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

/// Fixed, redacted validation failures for schema-5 watch payloads.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum WatchError {
    /// Validation failed in the existing schema-4 watch codec.
    Base(watch_protocol::WatchError),
    /// A cursor session identifier is zero.
    ZeroSession,
    /// A cursor session differs from the immutable target session.
    SessionMismatch,
}

impl fmt::Debug for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base(error) => f.debug_tuple("WatchError::Base").field(error).finish(),
            Self::ZeroSession => f.write_str("WatchError::ZeroSession"),
            Self::SessionMismatch => f.write_str("WatchError::SessionMismatch"),
        }
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base(error) => error.fmt(f),
            Self::ZeroSession => f.write_str("watch cursor session must be nonzero"),
            Self::SessionMismatch => f.write_str("watch cursor session does not match target"),
        }
    }
}

impl Error for WatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Base(error) => Some(error),
            Self::ZeroSession | Self::SessionMismatch => None,
        }
    }
}

impl From<watch_protocol::WatchError> for WatchError {
    fn from(error: watch_protocol::WatchError) -> Self {
        Self::Base(error)
    }
}

/// Encode a validated schema-5 reconciliation or watch request.
pub fn encode_watch_request(request: &WatchRequest) -> Result<Vec<u8>, WatchError> {
    let base = match request {
        WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        } => BaseWatchRequest::Reconcile {
            target: *target,
            max_bytes: *max_bytes,
            max_lines: *max_lines,
        },
        WatchRequest::Watch {
            target,
            cursor,
            limit,
            wait_ms,
        } => {
            validate_cursor_scope(*target, *cursor)?;
            BaseWatchRequest::Watch {
                target: *target,
                cursor: EventCursor {
                    generation: cursor.generation,
                    sequence: cursor.sequence,
                },
                limit: *limit,
                wait_ms: *wait_ms,
            }
        }
    };

    let mut bytes = watch_protocol::encode_watch_request(&base)?;
    write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
    if let WatchRequest::Watch { cursor, .. } = request {
        write_u64(&mut bytes, WATCH_SESSION_OFFSET, cursor.session);
    }
    Ok(bytes)
}

/// Decode one exact schema-5 reconciliation or watch request.
pub fn decode_watch_request(bytes: &[u8]) -> Result<WatchRequest, WatchError> {
    if bytes.len() != RECONCILE_REQUEST_BYTES && bytes.len() != WATCH_REQUEST_BYTES {
        return Err(watch_protocol::WatchError::InvalidLength.into());
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(watch_protocol::WatchError::UnsupportedSchema.into());
    }

    let kind = read_u16(bytes, 2);
    let cursor_session = if bytes.len() == WATCH_REQUEST_BYTES && kind == REQUEST_WATCH {
        Some(read_u64(bytes, WATCH_SESSION_OFFSET))
    } else {
        None
    };
    let mut base_bytes = bounded_base_copy(bytes);
    if cursor_session.is_some() {
        base_bytes[WATCH_SESSION_OFFSET..WATCH_REQUEST_BYTES].fill(0);
    }
    let request = watch_protocol::decode_watch_request(&base_bytes)?;

    match request {
        BaseWatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        } => Ok(WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        }),
        BaseWatchRequest::Watch {
            target,
            cursor,
            limit,
            wait_ms,
        } => {
            let session = cursor_session.ok_or(watch_protocol::WatchError::InvalidLength)?;
            let cursor = SessionCursor {
                generation: cursor.generation,
                session,
                sequence: cursor.sequence,
            };
            validate_cursor_scope(target, cursor)?;
            Ok(WatchRequest::Watch {
                target,
                cursor,
                limit,
                wait_ms,
            })
        }
    }
}

/// Encode one bounded schema-5 reconciliation response.
pub fn encode_reconciliation(reconciliation: &Reconciliation<'_>) -> Result<Vec<u8>, WatchError> {
    validate_cursor_scope(reconciliation.info.target, reconciliation.cursor)?;
    let base = BaseReconciliation {
        info: reconciliation.info,
        cursor: EventCursor {
            generation: reconciliation.cursor.generation,
            sequence: reconciliation.cursor.sequence,
        },
        screen_revision: reconciliation.screen_revision,
        truncated: reconciliation.truncated,
        text: reconciliation.text,
    };
    let mut bytes = watch_protocol::encode_reconciliation(&base)?;
    write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
    write_u64(
        &mut bytes,
        RECONCILIATION_SESSION_OFFSET,
        reconciliation.cursor.session,
    );
    Ok(bytes)
}

/// Decode one complete schema-5 reconciliation, borrowing display text from
/// `bytes` after validating a bounded temporary schema-4 copy.
pub fn decode_reconciliation(bytes: &[u8]) -> Result<Reconciliation<'_>, WatchError> {
    if bytes.len() < RECONCILIATION_PREFIX_BYTES {
        return Err(watch_protocol::WatchError::InvalidLength.into());
    }
    if bytes.len() > MAX_RECONCILIATION_BYTES {
        return Err(watch_protocol::WatchError::PayloadTooLarge.into());
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(watch_protocol::WatchError::UnsupportedSchema.into());
    }

    let kind = read_u16(bytes, 2);
    let cursor_session = if kind == RESPONSE_RECONCILIATION {
        read_u64(bytes, RECONCILIATION_SESSION_OFFSET)
    } else {
        0
    };
    let mut base_bytes = bounded_base_copy(bytes);
    if kind == RESPONSE_RECONCILIATION {
        base_bytes[RECONCILIATION_SESSION_OFFSET..RECONCILIATION_RESERVED_START].fill(0);
    }

    let (info, cursor, screen_revision, truncated, text_length) = {
        let base = watch_protocol::decode_reconciliation(&base_bytes)?;
        let cursor = SessionCursor {
            generation: base.cursor.generation,
            session: cursor_session,
            sequence: base.cursor.sequence,
        };
        validate_cursor_scope(base.info.target, cursor)?;
        (
            base.info,
            cursor,
            base.screen_revision,
            base.truncated,
            base.text.len(),
        )
    };
    let text_end = RECONCILIATION_PREFIX_BYTES
        .checked_add(text_length)
        .ok_or(watch_protocol::WatchError::InvalidLength)?;
    let text = str::from_utf8(&bytes[RECONCILIATION_PREFIX_BYTES..text_end])
        .map_err(|_| watch_protocol::WatchError::InvalidText)?;

    Ok(Reconciliation {
        info,
        cursor,
        screen_revision,
        truncated,
        text,
    })
}

/// Encode a schema-5 WatchEnd for the exact request target.
pub fn encode_watch_end(
    end: &WatchEnd,
    target: TargetRef,
) -> Result<[u8; WATCH_END_BYTES], WatchError> {
    validate_cursor_scope(target, end.cursor)?;
    let base = BaseWatchEnd {
        status: end.status,
        cursor: EventCursor {
            generation: end.cursor.generation,
            sequence: end.cursor.sequence,
        },
        event_count: end.event_count,
    };
    let mut bytes = watch_protocol::encode_watch_end(&base)?;
    write_u16(&mut bytes, 0, WATCH_SCHEMA_VERSION);
    write_u64(&mut bytes, WATCH_END_SESSION_OFFSET, end.cursor.session);
    Ok(bytes)
}

/// Decode a schema-5 WatchEnd and require it to match the request target.
pub fn decode_watch_end(bytes: &[u8], target: TargetRef) -> Result<WatchEnd, WatchError> {
    if bytes.len() != WATCH_END_BYTES {
        return Err(watch_protocol::WatchError::InvalidLength.into());
    }
    if read_u16(bytes, 0) != WATCH_SCHEMA_VERSION {
        return Err(watch_protocol::WatchError::UnsupportedSchema.into());
    }
    let session = read_u64(bytes, WATCH_END_SESSION_OFFSET);
    let mut base_bytes = [0; WATCH_END_BYTES];
    base_bytes.copy_from_slice(bytes);
    write_u16(&mut base_bytes, 0, watch_protocol::WATCH_SCHEMA_VERSION);
    base_bytes[WATCH_END_SESSION_OFFSET..WATCH_END_BYTES].fill(0);
    let base = watch_protocol::decode_watch_end(&base_bytes)?;
    let end = WatchEnd {
        status: base.status,
        cursor: SessionCursor {
            generation: base.cursor.generation,
            session,
            sequence: base.cursor.sequence,
        },
        event_count: base.event_count,
    };
    validate_cursor_scope(target, end.cursor)?;
    Ok(end)
}

fn validate_cursor_scope(target: TargetRef, cursor: SessionCursor) -> Result<(), WatchError> {
    validate_target(target)?;
    if cursor.generation == 0 {
        return Err(watch_protocol::WatchError::ZeroIdentifier.into());
    }
    if cursor.generation != target.generation {
        return Err(watch_protocol::WatchError::GenerationMismatch.into());
    }
    if cursor.session == 0 {
        return Err(WatchError::ZeroSession);
    }
    if cursor.session != target.session {
        return Err(WatchError::SessionMismatch);
    }
    Ok(())
}

fn validate_target(target: TargetRef) -> Result<(), WatchError> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(watch_protocol::WatchError::ZeroIdentifier.into());
    }
    Ok(())
}

fn bounded_base_copy(bytes: &[u8]) -> Vec<u8> {
    let mut copy = bytes.to_vec();
    write_u16(&mut copy, 0, watch_protocol::WATCH_SCHEMA_VERSION);
    copy
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
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

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_protocol::SessionInfo;
    use crate::watch_protocol::{
        self, WatchStatus, MAX_RECONCILIATION_LINES, MAX_WATCH_EVENTS, MAX_WATCH_WAIT_MS,
    };

    const TARGET: TargetRef = TargetRef {
        generation: 11,
        workspace: 2,
        pane: 3,
        session: 41,
        occupant_revision: 5,
    };
    const OTHER_SESSION_TARGET: TargetRef = TargetRef {
        session: 42,
        ..TARGET
    };
    const CURSOR_ZERO: SessionCursor = SessionCursor {
        generation: TARGET.generation,
        session: TARGET.session,
        sequence: 0,
    };
    const CURSOR_ONE: SessionCursor = SessionCursor {
        sequence: 1,
        ..CURSOR_ZERO
    };

    fn session_info(target: TargetRef) -> SessionInfo {
        SessionInfo {
            target,
            owner_revision: 6,
            owner_lease: 0,
            owner_expires_secs: 0,
            child_exited: false,
        }
    }

    fn sample_reconciliation() -> Reconciliation<'static> {
        Reconciliation {
            info: session_info(TARGET),
            cursor: CURSOR_ZERO,
            screen_revision: 7,
            truncated: false,
            text: "screen\ntext",
        }
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    fn request_watch(cursor: SessionCursor) -> WatchRequest {
        WatchRequest::Watch {
            target: TARGET,
            cursor,
            limit: 4,
            wait_ms: 25,
        }
    }

    #[test]
    fn schema5_requests_round_trip_with_unchanged_shapes_and_scoped_cursor() {
        let reconcile = WatchRequest::Reconcile {
            target: TARGET,
            max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
            max_lines: 1,
        };
        let bytes = encode_watch_request(&reconcile).unwrap();
        assert_eq!(bytes.len(), 56);
        assert_eq!(&bytes[..4], &[0, 5, 0, 1]);
        assert_eq!(decode_watch_request(&bytes), Ok(reconcile));

        let watch = request_watch(CURSOR_ZERO);
        let bytes = encode_watch_request(&watch).unwrap();
        assert_eq!(bytes.len(), 72);
        assert_eq!(&bytes[..4], &[0, 5, 0, 2]);
        assert_eq!(&bytes[64..72], &TARGET.session.to_be_bytes());
        assert_eq!(decode_watch_request(&bytes), Ok(watch));
        assert_eq!(
            decode_watch_request(&bytes).unwrap(),
            request_watch(CURSOR_ZERO)
        );

        let maximum_target = TargetRef {
            generation: u64::MAX,
            session: u64::MAX,
            ..TARGET
        };
        let maximum = WatchRequest::Watch {
            target: maximum_target,
            cursor: SessionCursor {
                generation: u64::MAX,
                session: u64::MAX,
                sequence: u64::MAX,
            },
            limit: MAX_WATCH_EVENTS,
            wait_ms: MAX_WATCH_WAIT_MS,
        };
        let maximum_bytes = encode_watch_request(&maximum).unwrap();
        assert_eq!(decode_watch_request(&maximum_bytes), Ok(maximum));
    }

    #[test]
    fn watch_cursor_requires_nonzero_generation_and_session_and_exact_target_scope() {
        assert_eq!(
            encode_watch_request(&request_watch(SessionCursor {
                session: 0,
                ..CURSOR_ZERO
            })),
            Err(WatchError::ZeroSession)
        );
        assert_eq!(
            encode_watch_request(&request_watch(SessionCursor {
                generation: 0,
                ..CURSOR_ZERO
            })),
            Err(WatchError::Base(watch_protocol::WatchError::ZeroIdentifier))
        );
        assert_eq!(
            encode_watch_request(&request_watch(SessionCursor {
                generation: TARGET.generation + 1,
                ..CURSOR_ZERO
            })),
            Err(WatchError::Base(
                watch_protocol::WatchError::GenerationMismatch
            ))
        );
        assert_eq!(
            encode_watch_request(&request_watch(SessionCursor {
                session: TARGET.session + 1,
                ..CURSOR_ZERO
            })),
            Err(WatchError::SessionMismatch)
        );

        let encoded = encode_watch_request(&request_watch(CURSOR_ZERO)).unwrap();
        let mut zero_session = encoded.clone();
        put_u64(&mut zero_session, WATCH_SESSION_OFFSET, 0);
        assert_eq!(
            decode_watch_request(&zero_session),
            Err(WatchError::ZeroSession)
        );

        let mut other_session = encoded.clone();
        put_u64(
            &mut other_session,
            WATCH_SESSION_OFFSET,
            OTHER_SESSION_TARGET.session,
        );
        assert_eq!(
            decode_watch_request(&other_session),
            Err(WatchError::SessionMismatch)
        );

        let mut wrong_generation = encoded;
        put_u64(&mut wrong_generation, 44, TARGET.generation + 1);
        assert_eq!(
            decode_watch_request(&wrong_generation),
            Err(WatchError::Base(
                watch_protocol::WatchError::GenerationMismatch
            ))
        );
    }

    #[test]
    fn reconciliation_round_trip_binds_cursor_generation_and_session() {
        let value = sample_reconciliation();
        let bytes = encode_reconciliation(&value).unwrap();
        assert_eq!(bytes.len(), RECONCILIATION_PREFIX_BYTES + value.text.len());
        assert_eq!(&bytes[..4], &[0, 5, 0, 1]);
        assert_eq!(
            &bytes[RECONCILIATION_SESSION_OFFSET..RECONCILIATION_RESERVED_START],
            &TARGET.session.to_be_bytes()
        );
        assert!(
            bytes[RECONCILIATION_RESERVED_START..RECONCILIATION_PREFIX_BYTES]
                .iter()
                .all(|byte| *byte == 0)
        );
        let decoded = decode_reconciliation(&bytes).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(
            decoded.text.as_ptr(),
            bytes[RECONCILIATION_PREFIX_BYTES..].as_ptr()
        );

        let mut zero_session = bytes.clone();
        put_u64(&mut zero_session, RECONCILIATION_SESSION_OFFSET, 0);
        assert_eq!(
            decode_reconciliation(&zero_session),
            Err(WatchError::ZeroSession)
        );
        let mut other_session = bytes.clone();
        put_u64(
            &mut other_session,
            RECONCILIATION_SESSION_OFFSET,
            OTHER_SESSION_TARGET.session,
        );
        assert_eq!(
            decode_reconciliation(&other_session),
            Err(WatchError::SessionMismatch)
        );
        let mut wrong_generation = bytes.clone();
        put_u64(&mut wrong_generation, 76, TARGET.generation + 1);
        assert_eq!(
            decode_reconciliation(&wrong_generation),
            Err(WatchError::Base(
                watch_protocol::WatchError::GenerationMismatch
            ))
        );

        let wrong_scope = Reconciliation {
            cursor: SessionCursor {
                session: TARGET.session + 1,
                ..CURSOR_ZERO
            },
            ..value
        };
        assert_eq!(
            encode_reconciliation(&wrong_scope),
            Err(WatchError::SessionMismatch)
        );
    }

    #[test]
    fn watch_end_round_trips_all_statuses_and_requires_the_requested_target() {
        let changed = WatchEnd {
            status: WatchStatus::Changed,
            cursor: CURSOR_ONE,
            event_count: 1,
        };
        let timed_out = WatchEnd {
            status: WatchStatus::TimedOut,
            cursor: CURSOR_ZERO,
            event_count: 0,
        };
        let resync = WatchEnd {
            status: WatchStatus::ResyncRequired,
            cursor: CURSOR_ZERO,
            event_count: 0,
        };
        for end in [changed, timed_out, resync] {
            let bytes = encode_watch_end(&end, TARGET).unwrap();
            assert_eq!(bytes.len(), WATCH_END_BYTES);
            assert_eq!(&bytes[..4], &[0, 5, 0, 2]);
            assert_eq!(
                &bytes[WATCH_END_SESSION_OFFSET..],
                &TARGET.session.to_be_bytes()
            );
            assert_eq!(decode_watch_end(&bytes, TARGET), Ok(end));
        }

        let bytes = encode_watch_end(&timed_out, TARGET).unwrap();
        assert_eq!(
            decode_watch_end(&bytes, OTHER_SESSION_TARGET),
            Err(WatchError::SessionMismatch)
        );
        let other_generation = TargetRef {
            generation: TARGET.generation + 1,
            ..TARGET
        };
        assert_eq!(
            decode_watch_end(&bytes, other_generation),
            Err(WatchError::Base(
                watch_protocol::WatchError::GenerationMismatch
            ))
        );

        let mut zero_session = bytes;
        put_u64(&mut zero_session, WATCH_END_SESSION_OFFSET, 0);
        assert_eq!(
            decode_watch_end(&zero_session, TARGET),
            Err(WatchError::ZeroSession)
        );

        for invalid in [
            WatchEnd {
                status: WatchStatus::Changed,
                cursor: CURSOR_ONE,
                event_count: 0,
            },
            WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: CURSOR_ZERO,
                event_count: 1,
            },
        ] {
            assert_eq!(
                encode_watch_end(&invalid, TARGET),
                Err(WatchError::Base(
                    watch_protocol::WatchError::InvalidEventCount
                ))
            );
        }
    }

    #[test]
    fn schema4_and_schema5_reject_each_others_payloads() {
        let base_request = watch_protocol::encode_watch_request(&BaseWatchRequest::Reconcile {
            target: TARGET,
            max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
            max_lines: 1,
        })
        .unwrap();
        let scoped_request = encode_watch_request(&WatchRequest::Reconcile {
            target: TARGET,
            max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
            max_lines: 1,
        })
        .unwrap();
        assert_eq!(
            decode_watch_request(&base_request),
            Err(WatchError::Base(
                watch_protocol::WatchError::UnsupportedSchema
            ))
        );
        assert_eq!(
            watch_protocol::decode_watch_request(&scoped_request),
            Err(watch_protocol::WatchError::UnsupportedSchema)
        );

        let base_reconciliation = watch_protocol::encode_reconciliation(&BaseReconciliation {
            info: session_info(TARGET),
            cursor: EventCursor {
                generation: TARGET.generation,
                sequence: 0,
            },
            screen_revision: 7,
            truncated: false,
            text: "text",
        })
        .unwrap();
        let scoped_reconciliation = encode_reconciliation(&sample_reconciliation()).unwrap();
        assert_eq!(
            decode_reconciliation(&base_reconciliation),
            Err(WatchError::Base(
                watch_protocol::WatchError::UnsupportedSchema
            ))
        );
        assert_eq!(
            watch_protocol::decode_reconciliation(&scoped_reconciliation),
            Err(watch_protocol::WatchError::UnsupportedSchema)
        );

        let base_end = watch_protocol::encode_watch_end(&BaseWatchEnd {
            status: WatchStatus::TimedOut,
            cursor: EventCursor {
                generation: TARGET.generation,
                sequence: 0,
            },
            event_count: 0,
        })
        .unwrap();
        let scoped_end = encode_watch_end(
            &WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: CURSOR_ZERO,
                event_count: 0,
            },
            TARGET,
        )
        .unwrap();
        assert_eq!(
            decode_watch_end(&base_end, TARGET),
            Err(WatchError::Base(
                watch_protocol::WatchError::UnsupportedSchema
            ))
        );
        assert_eq!(
            watch_protocol::decode_watch_end(&scoped_end),
            Err(watch_protocol::WatchError::UnsupportedSchema)
        );
    }

    #[test]
    fn every_reserved_byte_unknown_kind_and_schema_is_rejected() {
        let base_reconcile = watch_protocol::encode_watch_request(&BaseWatchRequest::Reconcile {
            target: TARGET,
            max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
            max_lines: 1,
        })
        .unwrap();
        for offset in 50..56 {
            let mut bytes = encode_watch_request(&WatchRequest::Reconcile {
                target: TARGET,
                max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
                max_lines: 1,
            })
            .unwrap();
            bytes[offset] = 1;
            assert_eq!(
                decode_watch_request(&bytes),
                Err(WatchError::Base(
                    watch_protocol::WatchError::NonzeroReserved
                )),
                "reconcile reserved byte {offset}"
            );
        }
        let mut unknown_request_kind = base_reconcile;
        put_u16(&mut unknown_request_kind, 0, WATCH_SCHEMA_VERSION);
        put_u16(&mut unknown_request_kind, 2, 99);
        assert_eq!(
            decode_watch_request(&unknown_request_kind),
            Err(WatchError::Base(watch_protocol::WatchError::UnknownKind))
        );

        let original = encode_reconciliation(&sample_reconciliation()).unwrap();
        for offset in (101..104).chain(RECONCILIATION_RESERVED_START..RECONCILIATION_PREFIX_BYTES) {
            let mut bytes = original.clone();
            bytes[offset] = 1;
            assert_eq!(
                decode_reconciliation(&bytes),
                Err(WatchError::Base(
                    watch_protocol::WatchError::NonzeroReserved
                )),
                "reconciliation reserved byte {offset}"
            );
        }
        let mut unknown_reconciliation_kind = original.clone();
        put_u16(&mut unknown_reconciliation_kind, 2, 99);
        assert_eq!(
            decode_reconciliation(&unknown_reconciliation_kind),
            Err(WatchError::Base(watch_protocol::WatchError::UnknownKind))
        );
        let mut unknown_reconciliation_schema = original;
        put_u16(
            &mut unknown_reconciliation_schema,
            0,
            WATCH_SCHEMA_VERSION + 1,
        );
        assert_eq!(
            decode_reconciliation(&unknown_reconciliation_schema),
            Err(WatchError::Base(
                watch_protocol::WatchError::UnsupportedSchema
            ))
        );

        let end = encode_watch_end(
            &WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: CURSOR_ZERO,
                event_count: 0,
            },
            TARGET,
        )
        .unwrap();
        let mut unknown_end_kind = end;
        put_u16(&mut unknown_end_kind, 2, 99);
        assert_eq!(
            decode_watch_end(&unknown_end_kind, TARGET),
            Err(WatchError::Base(watch_protocol::WatchError::UnknownKind))
        );
    }

    #[test]
    fn fixed_and_bounded_payload_lengths_reject_truncation_trailing_and_overflow() {
        for request in [
            WatchRequest::Reconcile {
                target: TARGET,
                max_bytes: RECONCILIATION_PREFIX_BYTES as u32,
                max_lines: 1,
            },
            request_watch(CURSOR_ZERO),
        ] {
            let bytes = encode_watch_request(&request).unwrap();
            for end in 0..bytes.len() {
                assert_eq!(
                    decode_watch_request(&bytes[..end]),
                    Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
                );
            }
            let mut trailing = bytes;
            trailing.push(0);
            assert_eq!(
                decode_watch_request(&trailing),
                Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
            );
        }

        let valid = encode_reconciliation(&sample_reconciliation()).unwrap();
        for end in 0..valid.len() {
            assert_eq!(
                decode_reconciliation(&valid[..end]),
                Err(WatchError::Base(watch_protocol::WatchError::InvalidLength)),
                "reconciliation truncation at {end}"
            );
        }
        let mut trailing = valid;
        trailing.push(0);
        assert_eq!(
            decode_reconciliation(&trailing),
            Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
        );
        assert_eq!(
            decode_reconciliation(&vec![0; MAX_RECONCILIATION_BYTES + 1]),
            Err(WatchError::Base(
                watch_protocol::WatchError::PayloadTooLarge
            ))
        );

        let end = encode_watch_end(
            &WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: CURSOR_ZERO,
                event_count: 0,
            },
            TARGET,
        )
        .unwrap();
        for length in 0..end.len() {
            assert_eq!(
                decode_watch_end(&end[..length], TARGET),
                Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
            );
        }
        let mut trailing_end = end.to_vec();
        trailing_end.push(0);
        assert_eq!(
            decode_watch_end(&trailing_end, TARGET),
            Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
        );
    }

    #[test]
    fn schema4_validation_preserves_budgets_text_and_complete_payload_bound() {
        for (max_bytes, max_lines) in [
            (0, 1),
            ((RECONCILIATION_PREFIX_BYTES - 1) as u32, 1),
            ((MAX_RECONCILIATION_BYTES + 1) as u32, 1),
            (RECONCILIATION_PREFIX_BYTES as u32, 0),
            (
                RECONCILIATION_PREFIX_BYTES as u32,
                MAX_RECONCILIATION_LINES + 1,
            ),
        ] {
            assert!(matches!(
                encode_watch_request(&WatchRequest::Reconcile {
                    target: TARGET,
                    max_bytes,
                    max_lines,
                }),
                Err(WatchError::Base(watch_protocol::WatchError::InvalidBudget))
            ));
        }

        for (limit, wait_ms) in [
            (0, 0),
            (MAX_WATCH_EVENTS + 1, 0),
            (1, MAX_WATCH_WAIT_MS + 1),
        ] {
            assert!(matches!(
                encode_watch_request(&WatchRequest::Watch {
                    target: TARGET,
                    cursor: CURSOR_ZERO,
                    limit,
                    wait_ms,
                }),
                Err(WatchError::Base(_))
            ));
        }

        let max_text = "x".repeat(MAX_RECONCILIATION_BYTES - RECONCILIATION_PREFIX_BYTES);
        let maximum = Reconciliation {
            text: &max_text,
            ..sample_reconciliation()
        };
        let bytes = encode_reconciliation(&maximum).unwrap();
        assert_eq!(bytes.len(), MAX_RECONCILIATION_BYTES);
        assert_eq!(
            decode_reconciliation(&bytes).unwrap().text.len(),
            max_text.len()
        );

        let one_over_text = "x".repeat(max_text.len() + 1);
        let one_over = Reconciliation {
            text: &one_over_text,
            ..sample_reconciliation()
        };
        assert_eq!(
            encode_reconciliation(&one_over),
            Err(WatchError::Base(
                watch_protocol::WatchError::PayloadTooLarge
            ))
        );

        let mut invalid_utf8 = encode_reconciliation(&sample_reconciliation()).unwrap();
        let text_offset = RECONCILIATION_PREFIX_BYTES;
        invalid_utf8[text_offset] = 0xff;
        assert_eq!(
            decode_reconciliation(&invalid_utf8),
            Err(WatchError::Base(watch_protocol::WatchError::InvalidText))
        );
        let mut control_text = encode_reconciliation(&sample_reconciliation()).unwrap();
        control_text[text_offset] = 0x01;
        assert_eq!(
            decode_reconciliation(&control_text),
            Err(WatchError::Base(watch_protocol::WatchError::InvalidText))
        );

        let mut invalid_session_info = encode_reconciliation(&sample_reconciliation()).unwrap();
        put_u64(&mut invalid_session_info, 4, 0);
        assert_eq!(
            decode_reconciliation(&invalid_session_info),
            Err(WatchError::Base(
                watch_protocol::WatchError::InvalidSessionInfo
            ))
        );
        let mut invalid_text_length = encode_reconciliation(&sample_reconciliation()).unwrap();
        put_u32(&mut invalid_text_length, 104, u32::MAX);
        assert_eq!(
            decode_reconciliation(&invalid_text_length),
            Err(WatchError::Base(watch_protocol::WatchError::InvalidLength))
        );

        let mut too_many_lines = encode_reconciliation(&sample_reconciliation()).unwrap();
        too_many_lines.truncate(RECONCILIATION_PREFIX_BYTES);
        let many_lines = "\n".repeat(usize::from(MAX_RECONCILIATION_LINES) + 1);
        put_u32(&mut too_many_lines, 104, many_lines.len() as u32);
        too_many_lines.extend_from_slice(many_lines.as_bytes());
        assert_eq!(
            decode_reconciliation(&too_many_lines),
            Err(WatchError::Base(watch_protocol::WatchError::TooManyLines))
        );
    }

    #[test]
    fn debug_and_error_text_redact_target_cursor_and_snapshot_text() {
        let request = request_watch(CURSOR_ONE);
        let reconciliation = Reconciliation {
            text: "private-watch-text",
            ..sample_reconciliation()
        };
        let end = WatchEnd {
            status: WatchStatus::TimedOut,
            cursor: CURSOR_ZERO,
            event_count: 0,
        };
        let debug = format!("{request:?} {reconciliation:?} {end:?} {:?}", CURSOR_ONE);
        for private in ["private-watch-text", "41", "11", "2", "3", "5"] {
            assert!(!debug.contains(private));
        }
        for error in [WatchError::ZeroSession, WatchError::SessionMismatch] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(!display.contains("41"));
            assert!(!debug.contains("41"));
            assert!(!display.contains("private-watch-text"));
            assert!(!debug.contains("private-watch-text"));
        }
    }
}
