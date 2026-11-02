//! Property-based tests for the pure wire-framing and typed control codecs.
//!
//! Packet 2 of `docs/design/TRACING-PROPTEST-PLAN.md`. These are synthetic,
//! bounded strategies (256 cases, explicit shrink-iteration bound, on-disk
//! failure persistence). They supplement, and do not replace, the existing
//! deterministic golden, boundary, and writer-failure unit tests.
//!
//! No socket, file, terminal, or maximum-size allocation is performed here:
//! maximum payload boundaries stay in the unit tests so a random trial never
//! allocates megabytes, and every property works on in-memory byte vectors.
//!
//! Failures persist to `proptest-regressions/property_protocol.txt` (a reviewed,
//! versioned file). Re-running this test target replays every persisted case
//! before generating new ones. `PROPTEST_CASES` may raise the case count for an
//! opt-in local stress run; the committed default stays at 256.

use std::io;

use panelink::control_protocol::{
    decode_request_payload, encode_request_payload, Command, ControlError, ControlRequest,
    ReadSource, TargetRef, CONTROL_SCHEMA_VERSION, MAX_INPUT_TEXT_BYTES, MAX_READ_BYTES,
    MAX_READ_LINES,
};
use panelink::protocol::{OperationKey, MAX_OPERATION_PAYLOAD_BYTES};
use panelink::protocol_wire::{
    decode_frame, decode_header, encode_frame, Frame, MessageKind, WireError, HEADER_BYTES,
    PROTOCOL_MAJOR, PROTOCOL_MINOR,
};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Largest random opaque payload used by a trial. Deliberately far below every
/// kind limit; exact kind limits are covered by deterministic unit tests.
const MAX_RANDOM_PAYLOAD: usize = 256;

/// Largest random control text used by a trial, in Unicode scalars.
const MAX_RANDOM_TEXT_CHARS: usize = 32;

/// Fixed layout offsets of the typed control payload, re-verified by
/// `control_prefix_offsets_are_fixed` so a schema change fails loudly instead
/// of silently mutating the wrong field.
const CONTROL_PREFIX_BYTES: usize = 68;
const CONTROL_TEXT_BODY_PREFIX_BYTES: usize = 12;
const CONTROL_TEXT_OFFSET: usize = CONTROL_PREFIX_BYTES + CONTROL_TEXT_BODY_PREFIX_BYTES;
const CONTROL_READ_BODY_BYTES: usize = 7;
const CONTROL_INTERRUPT_BODY_BYTES: usize = 8;

/// Frame header layout: 4 length, 4 magic, 2+2 version, 2 kind, 2 flags, 8 ID.
const FRAME_BODY_HEADER_BYTES: u32 = 20;
const WIRE_MAGIC: &[u8; 4] = b"PLNK";

fn config() -> ProptestConfig {
    ProptestConfig {
        // Default is 256; honor PROPTEST_CASES for opt-in larger runs.
        max_shrink_iters: 4_096,
        // Failures persist to a reviewed, versioned file under the crate-root
        // `proptest-regressions/` directory. Replay: re-run the same test target;
        // proptest re-executes every persisted case before new random cases.
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/property_protocol.txt",
        ))),
        ..ProptestConfig::default()
    }
}

const KINDS: &[MessageKind] = &[
    MessageKind::Hello,
    MessageKind::Welcome,
    MessageKind::Request,
    MessageKind::Response,
    MessageKind::Snapshot,
    MessageKind::Error,
    MessageKind::Event,
];

const UNKNOWN_KIND_CODES: &[u16] = &[0, 8, 9, 100, u16::MAX];

fn arb_kind() -> impl Strategy<Value = MessageKind> {
    prop::sample::select(KINDS)
}

/// One valid frame description: kind, request identifier, opaque payload.
fn arb_frame() -> impl Strategy<Value = (MessageKind, u64, Vec<u8>)> {
    arb_kind().prop_flat_map(|kind| {
        let request_id: BoxedStrategy<u64> = if kind == MessageKind::Event {
            Just(0_u64).boxed()
        } else {
            any::<u64>().boxed()
        };
        (
            Just(kind),
            request_id,
            prop::collection::vec(any::<u8>(), 0..=MAX_RANDOM_PAYLOAD),
        )
    })
}

fn encode_valid_frame(kind: MessageKind, request_id: u64, payload: &[u8]) -> Vec<u8> {
    let frame = Frame::new(kind, request_id, payload).expect("bounded payload fits every kind");
    let mut bytes = Vec::new();
    encode_frame(&frame, &mut bytes).expect("in-memory writer accepts a valid frame");
    bytes
}

/// A single structured header mutation. Each variant has an unconditionally
/// specified rejection class, so expectations never depend on random bytes.
#[derive(Clone, Copy, Debug)]
enum HeaderMutation {
    /// Corrupt one magic byte at `offset` by a nonzero delta.
    Magic { offset: u8, delta: u8 },
    /// Unsupported major version.
    MajorVersion(u16),
    /// Unsupported minor version.
    MinorVersion(u16),
    /// Unrecognized message kind code.
    UnknownKind(u16),
    /// Nonzero reserved flags.
    Flags(u16),
    /// Declared frame length below the 20-byte body header.
    SmallDeclaredLength(u32),
    /// Drop `count` bytes from the end.
    Truncate { count: usize },
    /// Append bytes after the declared frame.
    AppendTrailing(u8),
}

fn arb_header_mutation() -> impl Strategy<Value = HeaderMutation> {
    prop_oneof![
        (0u8..4, 1u8..=u8::MAX).prop_map(|(offset, delta)| HeaderMutation::Magic { offset, delta }),
        (2u16..=u16::MAX).prop_map(HeaderMutation::MajorVersion),
        (1u16..=u16::MAX).prop_map(HeaderMutation::MinorVersion),
        prop::sample::select(UNKNOWN_KIND_CODES).prop_map(HeaderMutation::UnknownKind),
        (1u16..=u16::MAX).prop_map(HeaderMutation::Flags),
        (0u32..FRAME_BODY_HEADER_BYTES).prop_map(HeaderMutation::SmallDeclaredLength),
        (1usize..=(HEADER_BYTES + MAX_RANDOM_PAYLOAD + 1))
            .prop_map(|count| HeaderMutation::Truncate { count }),
        any::<u8>().prop_map(HeaderMutation::AppendTrailing),
    ]
}

/// Apply one mutation in place and return the exact expected rejection.
fn apply(mutation: HeaderMutation, bytes: &mut Vec<u8>) -> WireError {
    match mutation {
        HeaderMutation::Magic { offset, delta } => {
            let slot = 4 + usize::from(offset);
            bytes[slot] = WIRE_MAGIC[usize::from(offset)].wrapping_add(delta);
            WireError::BadMagic
        }
        HeaderMutation::MajorVersion(version) => {
            bytes[8..10].copy_from_slice(&version.to_be_bytes());
            WireError::UnsupportedVersion
        }
        HeaderMutation::MinorVersion(version) => {
            bytes[10..12].copy_from_slice(&version.to_be_bytes());
            WireError::UnsupportedVersion
        }
        HeaderMutation::UnknownKind(kind) => {
            bytes[12..14].copy_from_slice(&kind.to_be_bytes());
            WireError::UnknownMessageKind
        }
        HeaderMutation::Flags(flags) => {
            bytes[14..16].copy_from_slice(&flags.to_be_bytes());
            WireError::UnsupportedFlags
        }
        HeaderMutation::SmallDeclaredLength(declared) => {
            bytes[0..4].copy_from_slice(&declared.to_be_bytes());
            WireError::FrameLengthTooSmall
        }
        HeaderMutation::Truncate { count } => {
            let remaining = bytes.len() - count.min(bytes.len());
            bytes.truncate(remaining);
            if remaining < HEADER_BYTES {
                WireError::TruncatedHeader
            } else {
                WireError::TruncatedFrame
            }
        }
        HeaderMutation::AppendTrailing(byte) => {
            bytes.push(byte);
            WireError::TrailingBytes
        }
    }
}

fn assert_documented_wire_error(error: WireError) {
    // Exhaustive on purpose: adding a rejection class must be a deliberate
    // update to the property-test evidence.
    match error {
        WireError::TruncatedHeader
        | WireError::FrameLengthTooSmall
        | WireError::LengthOverflow
        | WireError::BadMagic
        | WireError::UnsupportedVersion
        | WireError::UnknownMessageKind
        | WireError::UnsupportedFlags
        | WireError::PayloadTooLarge
        | WireError::EventRequestIdNotZero
        | WireError::TruncatedFrame
        | WireError::TrailingBytes
        | WireError::PayloadLengthMismatch => {}
    }
}

// ---------------------------------------------------------------------------
// Typed control requests
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum OwnedCommand {
    Read {
        source: ReadSource,
        max_bytes: u32,
        max_lines: u16,
    },
    Text {
        owner_revision: u64,
        text: String,
    },
    Paste {
        owner_revision: u64,
        text: String,
    },
    Submit {
        owner_revision: u64,
        text: String,
    },
    Interrupt {
        owner_revision: u64,
    },
}

impl OwnedCommand {
    fn as_command(&self) -> Command<'_> {
        match self {
            Self::Read {
                source,
                max_bytes,
                max_lines,
            } => Command::Read {
                source: *source,
                max_bytes: *max_bytes,
                max_lines: *max_lines,
            },
            Self::Text {
                owner_revision,
                text,
            } => Command::Text {
                owner_revision: *owner_revision,
                text: text.as_str(),
            },
            Self::Paste {
                owner_revision,
                text,
            } => Command::Paste {
                owner_revision: *owner_revision,
                text: text.as_str(),
            },
            Self::Submit {
                owner_revision,
                text,
            } => Command::Submit {
                owner_revision: *owner_revision,
                text: text.as_str(),
            },
            Self::Interrupt { owner_revision } => Command::Interrupt {
                owner_revision: *owner_revision,
            },
        }
    }

    fn opcode(&self) -> u16 {
        match self {
            Self::Read { .. } => 1,
            Self::Text { .. } => 2,
            Self::Paste { .. } => 3,
            Self::Submit { .. } => 4,
            Self::Interrupt { .. } => 5,
        }
    }

    fn owner_revision(&self) -> Option<u64> {
        match self {
            Self::Read { .. } => None,
            Self::Text { owner_revision, .. }
            | Self::Paste { owner_revision, .. }
            | Self::Submit { owner_revision, .. }
            | Self::Interrupt { owner_revision } => Some(*owner_revision),
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } | Self::Paste { text, .. } | Self::Submit { text, .. } => {
                Some(text.as_str())
            }
            Self::Read { .. } | Self::Interrupt { .. } => None,
        }
    }
}

const READ_SOURCES: &[ReadSource] = &[ReadSource::Screen, ReadSource::LiveScrollback];

fn arb_read_source() -> impl Strategy<Value = ReadSource> {
    prop::sample::select(READ_SOURCES)
}

/// Any nonzero identifier, including the maximum value.
fn arb_nonzero_id() -> impl Strategy<Value = u64> {
    any::<u64>().prop_map(|value| value.max(1))
}

/// Valid text for `Text`/`Submit`: no Unicode control scalars.
fn arb_printable_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=MAX_RANDOM_TEXT_CHARS).prop_map(|chars| {
        chars
            .into_iter()
            .filter(|character| !character.is_control())
            .collect()
    })
}

/// Any UTF-8 text, including control scalars, as `Paste` permits.
fn arb_any_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=MAX_RANDOM_TEXT_CHARS)
        .prop_map(|chars| chars.into_iter().collect())
}

fn arb_nonempty_printable_text() -> impl Strategy<Value = String> {
    arb_printable_text().prop_map(|text| {
        if text.is_empty() {
            String::from("x")
        } else {
            text
        }
    })
}

fn arb_command() -> impl Strategy<Value = OwnedCommand> {
    prop_oneof![
        (
            arb_read_source(),
            1u32..=MAX_READ_BYTES,
            1u16..=MAX_READ_LINES
        )
            .prop_map(|(source, max_bytes, max_lines)| OwnedCommand::Read {
                source,
                max_bytes,
                max_lines,
            }),
        (arb_nonzero_id(), arb_printable_text()).prop_map(|(owner_revision, text)| {
            OwnedCommand::Text {
                owner_revision,
                text,
            }
        }),
        (arb_nonzero_id(), arb_any_text()).prop_map(|(owner_revision, text)| OwnedCommand::Paste {
            owner_revision,
            text,
        }),
        (arb_nonzero_id(), arb_printable_text()).prop_map(|(owner_revision, text)| {
            OwnedCommand::Submit {
                owner_revision,
                text,
            }
        }),
        arb_nonzero_id().prop_map(|owner_revision| OwnedCommand::Interrupt { owner_revision }),
    ]
}

fn arb_request() -> impl Strategy<Value = (OperationKey, TargetRef, OwnedCommand)> {
    (
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        arb_command(),
    )
        .prop_map(
            |(
                generation,
                lease,
                sequence,
                workspace,
                pane,
                session,
                occupant_revision,
                command,
            )| {
                let generation = generation.max(1);
                (
                    OperationKey {
                        generation,
                        lease: lease.max(1),
                        sequence: sequence.max(1),
                    },
                    TargetRef {
                        generation,
                        workspace: workspace.max(1),
                        pane: pane.max(1),
                        session: session.max(1),
                        occupant_revision: occupant_revision.max(1),
                    },
                    command,
                )
            },
        )
}

/// A coherent, nonzero identity pair whose key and target share a generation.
fn arb_identity() -> impl Strategy<Value = (OperationKey, TargetRef)> {
    (any::<u64>(), any::<u64>(), any::<u64>()).prop_map(|(generation, workspace, pane)| {
        let generation = generation.max(1);
        (
            OperationKey {
                generation,
                lease: 2,
                sequence: 3,
            },
            TargetRef {
                generation,
                workspace: workspace.max(1),
                pane: pane.max(1),
                session: 5,
                occupant_revision: 6,
            },
        )
    })
}

fn encode_control(key: OperationKey, target: TargetRef, command: &OwnedCommand) -> Vec<u8> {
    let request = ControlRequest {
        key,
        target,
        command: command.as_command(),
    };
    let mut bytes = Vec::new();
    encode_request_payload(&request, &mut bytes).expect("valid control request encodes");
    bytes
}

fn arb_control_char() -> impl Strategy<Value = char> {
    prop::sample::select(vec![
        '\u{0}', '\u{1}', '\n', '\r', '\t', '\u{1b}', '\u{7f}', '\u{85}', '\u{9f}',
    ])
}

proptest! {
    #![proptest_config(config())]

    /// Valid frames round trip with exact announced lengths and a payload that
    /// still borrows the decoded input buffer.
    #[test]
    fn valid_frames_round_trip_with_exact_lengths_and_borrowed_payload(
        (kind, request_id, payload) in arb_frame()
    ) {
        let frame = Frame::new(kind, request_id, &payload).expect("bounded payload fits");
        let header = frame.header();
        prop_assert_eq!(header.kind(), kind);
        prop_assert_eq!(header.request_id(), request_id);
        prop_assert_eq!(header.payload_len(), payload.len());

        let mut encoded = Vec::new();
        encode_frame(&frame, &mut encoded).expect("vector writer accepts the frame");
        prop_assert_eq!(encoded.len(), HEADER_BYTES + payload.len());
        prop_assert_eq!(
            u32::from_be_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]),
            FRAME_BODY_HEADER_BYTES + payload.len() as u32
        );
        prop_assert_eq!(&encoded[4..8], &WIRE_MAGIC[..]);
        prop_assert_eq!(u16::from_be_bytes([encoded[8], encoded[9]]), PROTOCOL_MAJOR);
        prop_assert_eq!(u16::from_be_bytes([encoded[10], encoded[11]]), PROTOCOL_MINOR);
        prop_assert_eq!(
            u16::from_be_bytes([encoded[12], encoded[13]]),
            kind_wire_code(kind)
        );
        prop_assert_eq!(u16::from_be_bytes([encoded[14], encoded[15]]), 0);
        prop_assert_eq!(
            u64::from_be_bytes([
                encoded[16], encoded[17], encoded[18], encoded[19], encoded[20], encoded[21],
                encoded[22], encoded[23],
            ]),
            request_id
        );

        let decoded = decode_frame(&encoded).expect("encoded frame decodes");
        prop_assert_eq!(decoded.header(), header);
        prop_assert_eq!(decoded.payload(), &payload[..]);
        prop_assert_eq!(
            decoded.payload().as_ptr(),
            encoded[HEADER_BYTES..].as_ptr(),
            "decoded payload must borrow the input buffer, not copy it"
        );
    }

    /// Every proper prefix of a valid frame is rejected with the exact class
    /// implied by how many header bytes survived.
    #[test]
    fn every_proper_prefix_is_rejected_with_the_truncation_class(
        (kind, request_id, payload) in arb_frame()
    ) {
        let encoded = encode_valid_frame(kind, request_id, &payload);
        for prefix_len in 0..encoded.len() {
            let expected = if prefix_len < HEADER_BYTES {
                WireError::TruncatedHeader
            } else {
                WireError::TruncatedFrame
            };
            prop_assert_eq!(
                decode_frame(&encoded[..prefix_len]),
                Err(expected),
                "prefix length {} of {}",
                prefix_len,
                encoded.len()
            );
        }
    }

    /// One structured near-valid header mutation always produces exactly the
    /// specified rejection class, and never a frame.
    #[test]
    fn structured_header_mutations_have_exact_rejection_classes(
        (kind, request_id, payload) in arb_frame(),
        mutation in arb_header_mutation()
    ) {
        let mut encoded = encode_valid_frame(kind, request_id, &payload);
        let expected = apply(mutation, &mut encoded);
        prop_assert_eq!(
            decode_frame(&encoded),
            Err(expected),
            "mutation {:?} was not classified as expected",
            mutation
        );
        assert_documented_wire_error(expected);
    }

    /// Declared-length bytes that disagree with the supplied body are rejected
    /// by the independent classification: kind limit, trailing, or truncation.
    #[test]
    fn declared_length_disagreements_are_classified_exactly(
        (kind, request_id, payload) in arb_frame(),
        declared_payload in 0u32..=8_192,
    ) {
        let mut encoded = encode_valid_frame(kind, request_id, &payload);
        encoded[0..4].copy_from_slice(&(FRAME_BODY_HEADER_BYTES + declared_payload).to_be_bytes());

        let declared_len = HEADER_BYTES + declared_payload as usize;
        let expected = if declared_payload as usize > kind.max_payload_bytes() {
            Err(WireError::PayloadTooLarge)
        } else if declared_len < encoded.len() {
            Err(WireError::TrailingBytes)
        } else if declared_len > encoded.len() {
            Err(WireError::TruncatedFrame)
        } else {
            Ok(())
        };

        match expected {
            Ok(()) => {
                let decoded = decode_frame(&encoded).expect("matching declared length decodes");
                prop_assert_eq!(decoded.payload(), &payload[..]);
            }
            Err(error) => prop_assert_eq!(decode_frame(&encoded), Err(error)),
        }
    }

    /// Appending any nonempty suffix after one complete frame is trailing data.
    #[test]
    fn trailing_bytes_after_a_frame_are_rejected(
        (kind, request_id, payload) in arb_frame(),
        suffix in prop::collection::vec(any::<u8>(), 1..=4),
    ) {
        let mut encoded = encode_valid_frame(kind, request_id, &payload);
        encoded.extend_from_slice(&suffix);
        prop_assert_eq!(decode_frame(&encoded), Err(WireError::TrailingBytes));
    }

    /// Arbitrary bounded bytes either fail closed with a documented class or
    /// decode to exactly the frame they encode; they never panic.
    #[test]
    fn arbitrary_bytes_fail_closed_or_round_trip(
        bytes in prop::collection::vec(any::<u8>(), 0..=MAX_RANDOM_PAYLOAD + HEADER_BYTES)
    ) {
        match decode_frame(&bytes) {
            Ok(frame) => {
                prop_assert_eq!(bytes.len(), HEADER_BYTES + frame.header().payload_len());
                let mut reencoded = Vec::new();
                encode_frame(&frame, &mut reencoded).expect("decoded frame re-encodes");
                prop_assert_eq!(reencoded, bytes);
            }
            Err(error) => assert_documented_wire_error(error),
        }
    }

    /// An arbitrary fixed header either fails closed or reports a payload that
    /// matches its declared length and its kind limit. A 24-byte input can
    /// therefore only succeed for an empty payload.
    #[test]
    fn arbitrary_headers_fail_closed_or_report_consistent_lengths(
        bytes in prop::array::uniform::<_, HEADER_BYTES>(any::<u8>())
    ) {
        match decode_header(&bytes) {
            Err(error) => assert_documented_wire_error(error),
            Ok(header) => {
                let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                prop_assert!(declared >= FRAME_BODY_HEADER_BYTES);
                prop_assert_eq!(
                    header.payload_len(),
                    (declared - FRAME_BODY_HEADER_BYTES) as usize
                );
                prop_assert!(header.payload_len() <= header.kind().max_payload_bytes());
                if header.payload_len() == 0 {
                    prop_assert_eq!(decode_frame(&bytes), Ok(Frame::new(
                        header.kind(),
                        header.request_id(),
                        &[],
                    ).expect("empty payload fits")));
                } else {
                    prop_assert_eq!(
                        decode_frame(&bytes).map(|_| ()),
                        Err(WireError::TruncatedFrame)
                    );
                }
            }
        }
    }

    /// A nonzero request identifier is only invalid for event frames.
    #[test]
    fn nonzero_request_identifiers_are_accepted_except_for_events(request_id in arb_nonzero_id()) {
        for kind in KINDS {
            let payload = [0x11, 0x22];
            let encoded = if *kind == MessageKind::Event {
                let frame = Frame::new(*kind, 0, &payload).expect("zero-ID event frame");
                let mut encoded = Vec::new();
                encode_frame(&frame, &mut encoded).expect("event encodes");
                encoded[16..24].copy_from_slice(&request_id.to_be_bytes());
                encoded
            } else {
                encode_valid_frame(*kind, request_id, &payload)
            };

            if *kind == MessageKind::Event {
                prop_assert_eq!(
                    decode_frame(&encoded),
                    Err(WireError::EventRequestIdNotZero),
                    "event must reject a nonzero request ID"
                );
            } else {
                let decoded = decode_frame(&encoded).expect("non-event IDs may be nonzero");
                prop_assert_eq!(decoded.header().request_id(), request_id);
                prop_assert_eq!(decoded.payload(), &payload[..]);
            }
        }
    }

    /// Valid control requests round trip, keep exact payload lengths, and keep
    /// text borrowed from the encoded payload bytes.
    #[test]
    fn valid_control_requests_round_trip_exactly(
        (key, target, owned) in arb_request()
    ) {
        let command = owned.as_command();
        let request = ControlRequest { key, target, command };
        prop_assert_eq!(request.validate(), Ok(()));

        let mut bytes = Vec::new();
        encode_request_payload(&request, &mut bytes).expect("valid request encodes");
        prop_assert_eq!(request.encoded_len(), Ok(bytes.len()));
        prop_assert_eq!(&bytes[0..2], &CONTROL_SCHEMA_VERSION.to_be_bytes()[..]);
        prop_assert_eq!(&bytes[2..4], &owned.opcode().to_be_bytes()[..]);

        let decoded = decode_request_payload(&bytes).expect("encoded request decodes");
        prop_assert_eq!(decoded, request);
        prop_assert_eq!(decoded.key, key);
        prop_assert_eq!(decoded.target, target);

        if let Some(expected_text) = owned.text() {
            prop_assert_eq!(
                bytes.len(),
                CONTROL_TEXT_OFFSET + expected_text.len(),
                "text body must be the exact declared payload suffix"
            );
            match decoded.command {
                Command::Text { text, .. }
                | Command::Paste { text, .. }
                | Command::Submit { text, .. } => {
                    prop_assert_eq!(text, expected_text);
                    prop_assert_eq!(
                        text.as_ptr(),
                        bytes[CONTROL_TEXT_OFFSET..].as_ptr(),
                        "decoded text must borrow the input buffer, not copy it"
                    );
                }
                Command::Read { .. } | Command::Interrupt { .. } => {
                    prop_assert!(false, "text command decoded as another variant");
                }
            }
        } else if let Some(owner_revision) = owned.owner_revision() {
            prop_assert_eq!(
                u64::from_be_bytes([
                    bytes[CONTROL_PREFIX_BYTES],
                    bytes[CONTROL_PREFIX_BYTES + 1],
                    bytes[CONTROL_PREFIX_BYTES + 2],
                    bytes[CONTROL_PREFIX_BYTES + 3],
                    bytes[CONTROL_PREFIX_BYTES + 4],
                    bytes[CONTROL_PREFIX_BYTES + 5],
                    bytes[CONTROL_PREFIX_BYTES + 6],
                    bytes[CONTROL_PREFIX_BYTES + 7],
                ]),
                owner_revision
            );
        }
    }

    /// Zeroing any prefix identifier rejects with the zero-identifier class.
    #[test]
    fn zeroed_prefix_identifiers_are_rejected_exactly(
        (key, target, owned) in arb_request(),
        field in 0usize..8,
    ) {
        let mut bytes = encode_control(key, target, &owned);
        let offset = [4usize, 12, 20, 28, 36, 44, 52, 60][field];
        bytes[offset..offset + 8].fill(0);
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::ZeroIdentifier),
            "zeroed identifier at offset {}",
            offset
        );
    }

    /// Zeroing any owner revision rejects with the zero-identifier class.
    #[test]
    fn zeroed_owner_revisions_are_rejected_exactly((key, target, owned) in arb_request()) {
        if owned.owner_revision().is_some() {
            let mut bytes = encode_control(key, target, &owned);
            bytes[CONTROL_PREFIX_BYTES..CONTROL_PREFIX_BYTES + 8].fill(0);
            prop_assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::ZeroIdentifier)
            );
        }
    }

    /// A target generation that differs from the key generation is rejected;
    /// wrapping to zero is reported as a zero identifier instead.
    #[test]
    fn generation_mismatches_are_rejected_exactly(
        (key, target, owned) in arb_request(),
        delta in arb_nonzero_id(),
    ) {
        let mut bytes = encode_control(key, target, &owned);
        let mismatched = key.generation.wrapping_add(delta);
        bytes[28..36].copy_from_slice(&mismatched.to_be_bytes());
        let expected = if mismatched == 0 {
            ControlError::ZeroIdentifier
        } else {
            ControlError::GenerationMismatch
        };
        prop_assert_eq!(decode_request_payload(&bytes), Err(expected));
    }

    /// Unsupported schema versions and unknown opcodes fail closed before any
    /// body parsing.
    #[test]
    fn schema_and_opcode_mutations_fail_closed(
        (key, target, owned) in arb_request(),
        schema in any::<u16>(),
        opcode in any::<u16>(),
    ) {
        let schema = if schema == CONTROL_SCHEMA_VERSION {
            CONTROL_SCHEMA_VERSION + 1
        } else {
            schema
        };
        let mut bytes = encode_control(key, target, &owned);
        bytes[0..2].copy_from_slice(&schema.to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::UnsupportedSchema)
        );

        let opcode = if (1..=5).contains(&opcode) {
            6
        } else {
            opcode
        };
        let mut bytes = encode_control(key, target, &owned);
        bytes[2..4].copy_from_slice(&opcode.to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::UnknownOpcode)
        );
    }

    /// Every proper prefix of a valid typed request is truncated.
    #[test]
    fn every_proper_control_prefix_is_rejected_as_truncated(
        (key, target, owned) in arb_request()
    ) {
        let bytes = encode_control(key, target, &owned);
        for prefix_len in 0..bytes.len() {
            prop_assert_eq!(
                decode_request_payload(&bytes[..prefix_len]),
                Err(ControlError::Truncated),
                "prefix length {} of {}",
                prefix_len,
                bytes.len()
            );
        }
    }

    /// Bytes after the exact declared body are trailing data.
    #[test]
    fn trailing_control_bytes_are_rejected(
        (key, target, owned) in arb_request(),
        suffix in prop::collection::vec(any::<u8>(), 1..=4),
    ) {
        let mut bytes = encode_control(key, target, &owned);
        bytes.extend_from_slice(&suffix);
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::TrailingBytes)
        );
    }

    /// Read source and read-limit fields outside their exact ranges reject with
    /// their specific classes.
    #[test]
    fn read_source_and_limit_mutations_are_rejected_exactly(
        (key, target) in arb_identity(),
        max_bytes in 1u32..=MAX_READ_BYTES,
        max_lines in 1u16..=MAX_READ_LINES,
        source in any::<u8>(),
    ) {
        let owned = OwnedCommand::Read {
            source: ReadSource::Screen,
            max_bytes,
            max_lines,
        };
        let mut bytes = encode_control(key, target, &owned);
        let source = if (1..=2).contains(&source) { 3 } else { source };
        bytes[CONTROL_PREFIX_BYTES] = source;
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::UnsupportedReadSource)
        );

        let mut bytes = encode_control(key, target, &owned);
        bytes[CONTROL_PREFIX_BYTES + 1..CONTROL_PREFIX_BYTES + 5]
            .copy_from_slice(&(MAX_READ_BYTES + 1).to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InvalidReadLimit)
        );

        let mut bytes = encode_control(key, target, &owned);
        bytes[CONTROL_PREFIX_BYTES + 1..CONTROL_PREFIX_BYTES + 5].copy_from_slice(&0u32.to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InvalidReadLimit)
        );

        let mut bytes = encode_control(key, target, &owned);
        bytes[CONTROL_PREFIX_BYTES + 5..CONTROL_PREFIX_BYTES + 7]
            .copy_from_slice(&(MAX_READ_LINES + 1).to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InvalidReadLimit)
        );

        let mut bytes = encode_control(key, target, &owned);
        bytes[CONTROL_PREFIX_BYTES + 5..CONTROL_PREFIX_BYTES + 7].copy_from_slice(&0u16.to_be_bytes());
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InvalidReadLimit)
        );

        prop_assert_eq!(CONTROL_READ_BODY_BYTES, 7);
        prop_assert_eq!(CONTROL_INTERRUPT_BODY_BYTES, 8);
    }

    /// An arbitrary declared text length is classified exactly: over the byte
    /// ceiling, shorter than the body, longer than the body, or exact.
    #[test]
    fn declared_text_lengths_are_classified_exactly(
        (key, target) in arb_identity(),
        text in arb_any_text(),
        declared in 0u32..=(MAX_INPUT_TEXT_BYTES as u32 + 2),
    ) {
        let owned = OwnedCommand::Paste {
            owner_revision: 7,
            text,
        };
        let mut bytes = encode_control(key, target, &owned);
        bytes[CONTROL_PREFIX_BYTES + 8..CONTROL_PREFIX_BYTES + 12]
            .copy_from_slice(&declared.to_be_bytes());

        let declared_end = CONTROL_TEXT_OFFSET + declared as usize;
        let expected = if declared as usize > MAX_INPUT_TEXT_BYTES {
            Err(ControlError::InputTextTooLarge)
        } else if declared_end < bytes.len() {
            Err(ControlError::TrailingBytes)
        } else if declared_end > bytes.len() {
            Err(ControlError::Truncated)
        } else {
            Ok(())
        };

        match expected {
            Ok(()) => {
                let decoded = decode_request_payload(&bytes).expect("exact length decodes");
                prop_assert_eq!(decoded, request_for(key, target, &owned));
            }
            Err(error) => prop_assert_eq!(decode_request_payload(&bytes), Err(error)),
        }
    }

    /// Any invalid UTF-8 byte inside a text payload is rejected as invalid UTF-8.
    #[test]
    fn invalid_utf8_inside_text_bodies_is_rejected_exactly(
        (key, target) in arb_identity(),
        text in arb_nonempty_printable_text(),
    ) {
        let owned = OwnedCommand::Paste {
            owner_revision: 7,
            text,
        };
        let mut bytes = encode_control(key, target, &owned);
        let last = bytes.len() - 1;
        bytes[last] = 0xff;
        prop_assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InvalidUtf8)
        );
    }

    /// Literal and submit text reject control scalars, while paste preserves the
    /// identical bytes; encoding refuses invalid text before any write.
    #[test]
    fn control_scalars_split_text_from_paste_exactly(
        (key, target) in arb_identity(),
        base in arb_printable_text(),
        control in arb_control_char(),
    ) {
        let text = format!("{base}{control}");

        for command in [
            Command::Text { owner_revision: 7, text: &text },
            Command::Submit { owner_revision: 7, text: &text },
        ] {
            let request = ControlRequest { key, target, command };
            prop_assert_eq!(request.validate(), Err(ControlError::InvalidText));
            let mut writer = Vec::new();
            let error = encode_request_payload(&request, &mut writer)
                .expect_err("invalid text must not encode");
            prop_assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            prop_assert!(writer.is_empty(), "no bytes may be written before validation");
        }

        let paste = ControlRequest {
            key,
            target,
            command: Command::Paste {
                owner_revision: 7,
                text: &text,
            },
        };
        let mut bytes = Vec::new();
        encode_request_payload(&paste, &mut bytes).expect("paste preserves control scalars");
        let decoded = decode_request_payload(&bytes).expect("paste payload decodes");
        prop_assert_eq!(decoded, paste);

        for opcode in [2u16, 4] {
            let mut patched = bytes.clone();
            patched[2..4].copy_from_slice(&opcode.to_be_bytes());
            prop_assert_eq!(
                decode_request_payload(&patched),
                Err(ControlError::InvalidText)
            );
        }
    }
}

fn kind_wire_code(kind: MessageKind) -> u16 {
    match kind {
        MessageKind::Hello => 1,
        MessageKind::Welcome => 2,
        MessageKind::Request => 3,
        MessageKind::Response => 4,
        MessageKind::Snapshot => 5,
        MessageKind::Error => 6,
        MessageKind::Event => 7,
    }
}

fn request_for<'a>(
    key: OperationKey,
    target: TargetRef,
    owned: &'a OwnedCommand,
) -> ControlRequest<'a> {
    ControlRequest {
        key,
        target,
        command: owned.as_command(),
    }
}

// ---------------------------------------------------------------------------
// Deterministic companions
// ---------------------------------------------------------------------------

#[test]
fn control_prefix_offsets_are_fixed() {
    let key = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };
    let target = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };
    let read = ControlRequest {
        key,
        target,
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 1,
            max_lines: 1,
        },
    };
    let mut bytes = Vec::new();
    encode_request_payload(&read, &mut bytes).expect("read encodes");
    assert_eq!(bytes.len(), CONTROL_PREFIX_BYTES + CONTROL_READ_BODY_BYTES);

    let interrupt = ControlRequest {
        key,
        target,
        command: Command::Interrupt { owner_revision: 8 },
    };
    let mut bytes = Vec::new();
    encode_request_payload(&interrupt, &mut bytes).expect("interrupt encodes");
    assert_eq!(
        bytes.len(),
        CONTROL_PREFIX_BYTES + CONTROL_INTERRUPT_BODY_BYTES
    );

    for text in ["", "a", "é"] {
        let text_request = ControlRequest {
            key,
            target,
            command: Command::Text {
                owner_revision: 8,
                text,
            },
        };
        let mut bytes = Vec::new();
        encode_request_payload(&text_request, &mut bytes).expect("text encodes");
        assert_eq!(bytes.len(), CONTROL_TEXT_OFFSET + text.len());
    }
}

#[test]
fn oversized_control_payload_is_rejected_before_parsing() {
    let oversized = vec![0_u8; MAX_OPERATION_PAYLOAD_BYTES + 1];
    assert_eq!(
        decode_request_payload(&oversized),
        Err(ControlError::PayloadTooLarge)
    );

    // Exactly at the ceiling, the length check passes and parsing rejects the
    // zero schema instead, which proves the boundary is not off by one.
    let at_ceiling = vec![0_u8; MAX_OPERATION_PAYLOAD_BYTES];
    assert_eq!(
        decode_request_payload(&at_ceiling),
        Err(ControlError::UnsupportedSchema)
    );
}

#[test]
fn maximum_input_text_ceiling_is_exact() {
    let key = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };
    let target = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };
    let at_limit = "é".repeat(MAX_INPUT_TEXT_BYTES / 2);
    assert_eq!(at_limit.len(), MAX_INPUT_TEXT_BYTES);
    let request = ControlRequest {
        key,
        target,
        command: Command::Text {
            owner_revision: 8,
            text: &at_limit,
        },
    };
    let mut bytes = Vec::new();
    encode_request_payload(&request, &mut bytes).expect("text at the ceiling encodes");
    assert_eq!(bytes.len(), CONTROL_TEXT_OFFSET + MAX_INPUT_TEXT_BYTES);
    assert_eq!(decode_request_payload(&bytes), Ok(request));

    let over_limit = format!("{at_limit}a");
    let over = ControlRequest {
        key,
        target,
        command: Command::Text {
            owner_revision: 8,
            text: &over_limit,
        },
    };
    assert_eq!(over.validate(), Err(ControlError::InputTextTooLarge));
}

#[test]
fn declared_text_length_of_u32_max_is_rejected_without_allocating() {
    let key = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };
    let target = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };
    let request = ControlRequest {
        key,
        target,
        command: Command::Paste {
            owner_revision: 8,
            text: "canary",
        },
    };
    let mut bytes = Vec::new();
    encode_request_payload(&request, &mut bytes).expect("paste encodes");
    bytes[CONTROL_PREFIX_BYTES + 8..CONTROL_PREFIX_BYTES + 12]
        .copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        decode_request_payload(&bytes),
        Err(ControlError::InputTextTooLarge)
    );
}

#[test]
fn control_debug_output_never_discloses_payload_text() {
    let canary = "distinctive-property-canary";
    let key = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };
    let target = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };
    let owned = OwnedCommand::Paste {
        owner_revision: 8,
        text: String::from(canary),
    };
    let request = request_for(key, target, &owned);
    let mut bytes = Vec::new();
    encode_request_payload(&request, &mut bytes).expect("paste encodes");
    let frame = Frame::new(MessageKind::Request, 9, &bytes).expect("bounded request frame");
    let mut frame_bytes = Vec::new();
    encode_frame(&frame, &mut frame_bytes).expect("frame encodes");
    let diagnostic = format!(
        "{request:?} {frame:?} {} {}",
        ControlError::InvalidText,
        WireError::BadMagic
    );
    assert!(!diagnostic.contains(canary));
    assert!(diagnostic.contains("text_len"));
}
