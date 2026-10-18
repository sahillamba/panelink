//! Fixed-size metadata event payloads for the experimental v1.0 wire envelope.
//!
//! This module only validates, encodes, and decodes event payloads. It does not
//! provide subscriptions, dispatch events, advertise capabilities, or perform
//! socket or PTY I/O.

use crate::control_protocol::TargetRef;
use std::error::Error;
use std::fmt;

/// Version of the metadata event payload schema.
pub const EVENT_SCHEMA_VERSION: u16 = 3;

/// Exact encoded size of one metadata event payload, in bytes.
pub const EVENT_PAYLOAD_BYTES: usize = 72;

const KIND_SCREEN_CHANGED: u16 = 1;
const KIND_INPUT_OWNER_CHANGED: u16 = 2;
const KIND_CHILD_EXITED: u16 = 3;

/// One metadata-only event for a target in a server generation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Event {
    /// Nonzero sequence number within the server generation.
    pub cursor: u64,
    /// Immutable pane-occupant identity associated with the event.
    pub target: TargetRef,
    /// Metadata body; terminal output and provider contents are excluded.
    pub body: EventBody,
}

impl Event {
    /// Validate the cursor, target identity, and body invariants.
    pub fn validate(&self) -> Result<(), EventError> {
        if self.cursor == 0
            || self.target.generation == 0
            || self.target.workspace == 0
            || self.target.pane == 0
            || self.target.session == 0
            || self.target.occupant_revision == 0
        {
            return Err(EventError::ZeroIdentifier);
        }

        match self.body {
            EventBody::ScreenChanged { revision }
            | EventBody::InputOwnerChanged { revision, .. }
                if revision == 0 =>
            {
                Err(EventError::ZeroIdentifier)
            }
            EventBody::ScreenChanged { .. }
            | EventBody::InputOwnerChanged { .. }
            | EventBody::ChildExited => Ok(()),
        }
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("cursor", &"<redacted>")
            .field("target", &"<redacted>")
            .field("body", &self.body)
            .finish()
    }
}

/// A bounded event body containing metadata only.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum EventBody {
    /// The visible screen revision changed.
    ScreenChanged {
        /// Nonzero screen revision for the target.
        revision: u64,
    },
    /// The session's input owner or vacant state changed.
    InputOwnerChanged {
        /// Nonzero revision of the owner state.
        revision: u64,
        /// Lease identity of the client owner, or zero when vacant.
        owner_lease: u64,
    },
    /// The child associated with the target exited.
    ChildExited,
}

impl fmt::Debug for EventBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScreenChanged { .. } => f
                .debug_struct("ScreenChanged")
                .field("revision", &"<redacted>")
                .finish(),
            Self::InputOwnerChanged { .. } => f
                .debug_struct("InputOwnerChanged")
                .field("revision", &"<redacted>")
                .field("owner_lease", &"<redacted>")
                .finish(),
            Self::ChildExited => f.write_str("ChildExited"),
        }
    }
}

/// Fixed, redacted errors for metadata event payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventError {
    /// The payload is not exactly 72 bytes long.
    InvalidLength,
    /// The payload schema version is unsupported.
    UnsupportedSchema,
    /// The event kind is unknown.
    UnknownKind,
    /// A reserved field is nonzero.
    NonzeroReserved,
    /// A required cursor, target identifier, or revision is zero.
    ZeroIdentifier,
    /// A body contains a field that is invalid for its event kind.
    InvalidBody,
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLength => "event payload must be exactly 72 bytes",
            Self::UnsupportedSchema => "event payload schema is unsupported",
            Self::UnknownKind => "event payload kind is unknown",
            Self::NonzeroReserved => "event payload reserved field must be zero",
            Self::ZeroIdentifier => "event identifiers and revisions must be nonzero",
            Self::InvalidBody => "event body fields are invalid for its kind",
        };
        f.write_str(message)
    }
}

impl Error for EventError {}

/// Encode one validated metadata event into its exact 72-byte representation.
pub fn encode_event(event: &Event) -> Result<[u8; EVENT_PAYLOAD_BYTES], EventError> {
    event.validate()?;

    let (kind, revision, owner_lease) = match event.body {
        EventBody::ScreenChanged { revision } => (KIND_SCREEN_CHANGED, revision, 0),
        EventBody::InputOwnerChanged {
            revision,
            owner_lease,
        } => (KIND_INPUT_OWNER_CHANGED, revision, owner_lease),
        EventBody::ChildExited => (KIND_CHILD_EXITED, 0, 0),
    };

    let mut bytes = [0; EVENT_PAYLOAD_BYTES];
    bytes[0..2].copy_from_slice(&EVENT_SCHEMA_VERSION.to_be_bytes());
    bytes[2..4].copy_from_slice(&kind.to_be_bytes());
    // bytes[4..8] are reserved and remain zero.
    bytes[8..16].copy_from_slice(&event.cursor.to_be_bytes());

    for (offset, value) in [
        (16, event.target.generation),
        (24, event.target.workspace),
        (32, event.target.pane),
        (40, event.target.session),
        (48, event.target.occupant_revision),
        (56, revision),
        (64, owner_lease),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    Ok(bytes)
}

/// Decode and validate exactly one metadata event payload.
pub fn decode_event(bytes: &[u8]) -> Result<Event, EventError> {
    if bytes.len() != EVENT_PAYLOAD_BYTES {
        return Err(EventError::InvalidLength);
    }

    let schema = u16::from_be_bytes([bytes[0], bytes[1]]);
    if schema != EVENT_SCHEMA_VERSION {
        return Err(EventError::UnsupportedSchema);
    }

    let kind = u16::from_be_bytes([bytes[2], bytes[3]]);
    if bytes[4..8] != [0, 0, 0, 0] {
        return Err(EventError::NonzeroReserved);
    }

    let cursor = read_u64(bytes, 8);
    let target = TargetRef {
        generation: read_u64(bytes, 16),
        workspace: read_u64(bytes, 24),
        pane: read_u64(bytes, 32),
        session: read_u64(bytes, 40),
        occupant_revision: read_u64(bytes, 48),
    };
    let revision = read_u64(bytes, 56);
    let owner_lease = read_u64(bytes, 64);

    let body = match kind {
        KIND_SCREEN_CHANGED if owner_lease == 0 => EventBody::ScreenChanged { revision },
        KIND_SCREEN_CHANGED => return Err(EventError::InvalidBody),
        KIND_INPUT_OWNER_CHANGED => EventBody::InputOwnerChanged {
            revision,
            owner_lease,
        },
        KIND_CHILD_EXITED if revision == 0 && owner_lease == 0 => EventBody::ChildExited,
        KIND_CHILD_EXITED => return Err(EventError::InvalidBody),
        _ => return Err(EventError::UnknownKind),
    };

    let event = Event {
        cursor,
        target,
        body,
    };
    event.validate()?;
    Ok(event)
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
    use super::{decode_event, encode_event, Event, EventBody, EventError, EVENT_PAYLOAD_BYTES};
    use crate::control_protocol::TargetRef;

    fn target() -> TargetRef {
        TargetRef {
            generation: 0x1112_1314_1516_1718,
            workspace: 0x2122_2324_2526_2728,
            pane: 0x3132_3334_3536_3738,
            session: 0x4142_4344_4546_4748,
            occupant_revision: 0x5152_5354_5556_5758,
        }
    }

    fn event(body: EventBody) -> Event {
        Event {
            cursor: 0x0102_0304_0506_0708,
            target: target(),
            body,
        }
    }

    fn encoded(body: EventBody) -> [u8; EVENT_PAYLOAD_BYTES] {
        encode_event(&event(body)).expect("fixture event is valid")
    }

    #[test]
    fn screen_changed_matches_the_golden_v3_layout() {
        let golden = [
            0x00, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x21, 0x22, 0x23, 0x24,
            0x25, 0x26, 0x27, 0x28, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x41, 0x42,
            0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
            0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let screen = event(EventBody::ScreenChanged {
            revision: 0x6162_6364_6566_6768,
        });

        assert_eq!(encode_event(&screen), Ok(golden));
        assert_eq!(decode_event(&golden), Ok(screen));
    }

    #[test]
    fn every_event_body_round_trips() {
        let events = [
            event(EventBody::ScreenChanged { revision: 7 }),
            event(EventBody::InputOwnerChanged {
                revision: 8,
                owner_lease: 0,
            }),
            event(EventBody::InputOwnerChanged {
                revision: 9,
                owner_lease: 10,
            }),
            event(EventBody::ChildExited),
        ];

        for original in events {
            let payload = encode_event(&original).expect("fixture event is valid");
            assert_eq!(payload.len(), EVENT_PAYLOAD_BYTES);
            assert_eq!(decode_event(&payload), Ok(original));
        }
    }

    #[test]
    fn rejects_zero_cursor_target_fields_and_revisions() {
        let valid = event(EventBody::ScreenChanged { revision: 1 });

        let mut invalid = valid;
        invalid.cursor = 0;
        assert_eq!(encode_event(&invalid), Err(EventError::ZeroIdentifier));

        let valid_payload = encoded(EventBody::ScreenChanged { revision: 1 });
        let mut zero_cursor = valid_payload;
        zero_cursor[8..16].fill(0);
        assert_eq!(decode_event(&zero_cursor), Err(EventError::ZeroIdentifier));

        for field in 0..5 {
            let mut invalid_target = target();
            match field {
                0 => invalid_target.generation = 0,
                1 => invalid_target.workspace = 0,
                2 => invalid_target.pane = 0,
                3 => invalid_target.session = 0,
                4 => invalid_target.occupant_revision = 0,
                _ => unreachable!(),
            }
            let invalid = Event {
                target: invalid_target,
                ..valid
            };
            assert_eq!(invalid.validate(), Err(EventError::ZeroIdentifier));
            assert_eq!(encode_event(&invalid), Err(EventError::ZeroIdentifier));

            let mut zero_target_field = valid_payload;
            let offset = 16 + field * 8;
            zero_target_field[offset..offset + 8].fill(0);
            assert_eq!(
                decode_event(&zero_target_field),
                Err(EventError::ZeroIdentifier)
            );
        }

        for invalid_body in [
            EventBody::ScreenChanged { revision: 0 },
            EventBody::InputOwnerChanged {
                revision: 0,
                owner_lease: 4,
            },
        ] {
            assert_eq!(
                event(invalid_body).validate(),
                Err(EventError::ZeroIdentifier)
            );
            assert_eq!(
                encode_event(&event(invalid_body)),
                Err(EventError::ZeroIdentifier)
            );
        }

        let mut zero_screen_revision = valid_payload;
        zero_screen_revision[56..64].fill(0);
        assert_eq!(
            decode_event(&zero_screen_revision),
            Err(EventError::ZeroIdentifier)
        );

        let mut zero_owner_revision = encoded(EventBody::InputOwnerChanged {
            revision: 1,
            owner_lease: 0,
        });
        zero_owner_revision[56..64].fill(0);
        assert_eq!(
            decode_event(&zero_owner_revision),
            Err(EventError::ZeroIdentifier)
        );
    }

    #[test]
    fn rejects_invalid_lengths_schema_kind_and_reserved_bytes() {
        let valid = encoded(EventBody::ScreenChanged { revision: 1 });
        assert_eq!(
            decode_event(&valid[..valid.len() - 1]),
            Err(EventError::InvalidLength)
        );

        let mut trailing = valid.to_vec();
        trailing.push(0);
        assert_eq!(decode_event(&trailing), Err(EventError::InvalidLength));

        let mut unsupported_schema = valid;
        unsupported_schema[1] = 4;
        assert_eq!(
            decode_event(&unsupported_schema),
            Err(EventError::UnsupportedSchema)
        );

        let mut unknown_kind = valid;
        unknown_kind[3] = 4;
        assert_eq!(decode_event(&unknown_kind), Err(EventError::UnknownKind));

        let mut nonzero_reserved = valid;
        nonzero_reserved[7] = 1;
        assert_eq!(
            decode_event(&nonzero_reserved),
            Err(EventError::NonzeroReserved)
        );
    }

    #[test]
    fn rejects_body_fields_that_do_not_match_the_kind() {
        let mut screen_owner = encoded(EventBody::ScreenChanged { revision: 1 });
        screen_owner[71] = 1;
        assert_eq!(decode_event(&screen_owner), Err(EventError::InvalidBody));

        let mut exited_revision = encoded(EventBody::ChildExited);
        exited_revision[63] = 1;
        assert_eq!(decode_event(&exited_revision), Err(EventError::InvalidBody));

        let mut exited_owner = encoded(EventBody::ChildExited);
        exited_owner[71] = 1;
        assert_eq!(decode_event(&exited_owner), Err(EventError::InvalidBody));
    }

    #[test]
    fn debug_redacts_cursor_target_and_body_identifiers() {
        let secret = Event {
            cursor: 7_777_777_777_777_777_777,
            target: TargetRef {
                generation: 1_111_111_111_111_111_111,
                workspace: 2_222_222_222_222_222_222,
                pane: 3_333_333_333_333_333_333,
                session: 4_444_444_444_444_444_444,
                occupant_revision: 5_555_555_555_555_555_555,
            },
            body: EventBody::InputOwnerChanged {
                revision: 6_666_666_666_666_666_666,
                owner_lease: 8_888_888_888_888_888_888,
            },
        };
        let debug = format!("{secret:?}");

        for identifier in [
            secret.cursor,
            secret.target.generation,
            secret.target.workspace,
            secret.target.pane,
            secret.target.session,
            secret.target.occupant_revision,
            6_666_666_666_666_666_666,
            8_888_888_888_888_888_888,
        ] {
            assert!(!debug.contains(&identifier.to_string()));
        }
        assert!(debug.contains("<redacted>"));
    }
}
