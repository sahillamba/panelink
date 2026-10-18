//! Borrowed typed request payloads for the initial agent-control operations.
//!
//! This module only validates and encodes request payloads. It does not
//! authenticate callers, resolve targets, check capabilities or input
//! ownership, deduplicate operations, or dispatch effects.

use crate::protocol::{OperationClass, OperationKey, MAX_OPERATION_PAYLOAD_BYTES};
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::str;

/// Version of the typed control-request payload schema.
pub const CONTROL_SCHEMA_VERSION: u16 = 1;

/// Maximum UTF-8 byte length accepted for one input command.
pub const MAX_INPUT_TEXT_BYTES: usize = 32_768;

/// Maximum byte count requested from one read operation.
pub const MAX_READ_BYTES: u32 = 65_536;

/// Maximum line count requested from one read operation.
pub const MAX_READ_LINES: u16 = 1_024;

const PREFIX_BYTES: usize = 68;
const READ_BODY_BYTES: usize = 7;
const TEXT_BODY_PREFIX_BYTES: usize = 12;
const TEXT_BYTES_OFFSET: usize = PREFIX_BYTES + TEXT_BODY_PREFIX_BYTES;
const INTERRUPT_BODY_BYTES: usize = 8;

const OPCODE_READ: u16 = 1;
const OPCODE_TEXT: u16 = 2;
const OPCODE_PASTE: u16 = 3;
const OPCODE_SUBMIT: u16 = 4;
const OPCODE_INTERRUPT: u16 = 5;

/// Immutable identity of one pane occupant within one server generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetRef {
    /// Server-start generation containing this target.
    pub generation: u64,
    /// Workspace identity.
    pub workspace: u64,
    /// Pane-view identity.
    pub pane: u64,
    /// Managed-session identity.
    pub session: u64,
    /// Revision identifying the current pane occupant.
    pub occupant_revision: u64,
}

/// Source requested by a bounded terminal read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadSource {
    /// Current visible screen contents.
    Screen,
    /// Bounded live scrollback contents.
    LiveScrollback,
}

/// One typed request command. Text remains borrowed from the caller.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Command<'a> {
    /// Read a bounded screen or live scrollback snapshot.
    Read {
        /// Requested read source.
        source: ReadSource,
        /// Maximum encoded output bytes requested.
        max_bytes: u32,
        /// Maximum output lines requested.
        max_lines: u16,
    },
    /// Send literal printable text to the current input owner.
    Text {
        /// Expected input-owner revision.
        owner_revision: u64,
        /// Borrowed UTF-8 text, with no implicit newline.
        text: &'a str,
    },
    /// Paste caller-supplied UTF-8 contents to the current input owner.
    Paste {
        /// Expected input-owner revision.
        owner_revision: u64,
        /// Borrowed paste contents. The codec does not reinterpret them.
        text: &'a str,
    },
    /// Submit printable text as an explicit submission operation.
    Submit {
        /// Expected input-owner revision.
        owner_revision: u64,
        /// Borrowed UTF-8 text, with no implicit newline addition.
        text: &'a str,
    },
    /// Request an interrupt signal for the targeted session.
    Interrupt {
        /// Expected input-owner revision.
        owner_revision: u64,
    },
}

impl Command<'_> {
    /// Return the operation class used by the existing admission protocol.
    pub const fn class(self) -> OperationClass {
        match self {
            Self::Read { .. } => OperationClass::Query,
            Self::Text { .. } | Self::Paste { .. } | Self::Submit { .. } => OperationClass::Input,
            Self::Interrupt { .. } => OperationClass::Signal,
        }
    }
}

impl fmt::Debug for Command<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read {
                source,
                max_bytes,
                max_lines,
            } => f
                .debug_struct("Command")
                .field("kind", &"Read")
                .field("source", source)
                .field("max_bytes", max_bytes)
                .field("max_lines", max_lines)
                .finish(),
            Self::Text {
                owner_revision,
                text,
            } => fmt_text_debug(f, "Text", *owner_revision, text.len()),
            Self::Paste {
                owner_revision,
                text,
            } => fmt_text_debug(f, "Paste", *owner_revision, text.len()),
            Self::Submit {
                owner_revision,
                text,
            } => fmt_text_debug(f, "Submit", *owner_revision, text.len()),
            Self::Interrupt { owner_revision } => f
                .debug_struct("Command")
                .field("kind", &"Interrupt")
                .field("owner_revision", owner_revision)
                .finish(),
        }
    }
}

fn fmt_text_debug(
    f: &mut fmt::Formatter<'_>,
    kind: &'static str,
    owner_revision: u64,
    text_len: usize,
) -> fmt::Result {
    f.debug_struct("Command")
        .field("kind", &kind)
        .field("owner_revision", &owner_revision)
        .field("text_len", &text_len)
        .finish()
}

/// A borrowed typed request payload.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ControlRequest<'a> {
    /// Existing ledger key used to identify and deduplicate the operation.
    pub key: OperationKey,
    /// Full immutable target identity supplied by the caller.
    pub target: TargetRef,
    /// Typed operation body.
    pub command: Command<'a>,
}

impl ControlRequest<'_> {
    /// Validate identifiers, generation consistency, and command bounds.
    pub fn validate(&self) -> Result<(), ControlError> {
        validate_identifiers(self.key, self.target)?;
        validate_command(self.command)
    }

    /// Return this request's exact encoded payload size after validation.
    pub fn encoded_len(&self) -> Result<usize, ControlError> {
        self.validate()?;
        match self.command {
            Command::Read { .. } => Ok(PREFIX_BYTES + READ_BODY_BYTES),
            Command::Text { text, .. }
            | Command::Paste { text, .. }
            | Command::Submit { text, .. } => Ok(TEXT_BYTES_OFFSET + text.len()),
            Command::Interrupt { .. } => Ok(PREFIX_BYTES + INTERRUPT_BODY_BYTES),
        }
    }
}

impl fmt::Debug for ControlRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlRequest")
            .field("key", &self.key)
            .field("target", &self.target)
            .field("command", &self.command)
            .finish()
    }
}

/// Fixed, redacted validation errors for typed control-request payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    /// The request prefix or operation body ended before all fields were present.
    Truncated,
    /// Bytes remain after the declared request body.
    TrailingBytes,
    /// The payload exceeds the outer request-payload ceiling.
    PayloadTooLarge,
    /// The typed payload schema version is unsupported.
    UnsupportedSchema,
    /// The command opcode is not one of the five defined operations.
    UnknownOpcode,
    /// The read-source discriminator is unsupported.
    UnsupportedReadSource,
    /// A key, target, or owner-revision identifier is zero.
    ZeroIdentifier,
    /// The operation key and target name different server generations.
    GenerationMismatch,
    /// A requested read byte or line limit is outside its supported range.
    InvalidReadLimit,
    /// Input text exceeds the UTF-8 byte ceiling.
    InputTextTooLarge,
    /// A text field is not valid UTF-8.
    InvalidUtf8,
    /// Literal text or submit text contains a Unicode control scalar.
    InvalidText,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "control request payload is truncated",
            Self::TrailingBytes => "bytes follow the control request body",
            Self::PayloadTooLarge => "control request payload exceeds its limit",
            Self::UnsupportedSchema => "control request schema is unsupported",
            Self::UnknownOpcode => "control request opcode is unknown",
            Self::UnsupportedReadSource => "control read source is unsupported",
            Self::ZeroIdentifier => "control request identifiers must be nonzero",
            Self::GenerationMismatch => "control request generations do not match",
            Self::InvalidReadLimit => "control read limit is outside its supported range",
            Self::InputTextTooLarge => "control input text exceeds its byte limit",
            Self::InvalidUtf8 => "control input text is not valid UTF-8",
            Self::InvalidText => "control text contains a disallowed control scalar",
        };
        f.write_str(message)
    }
}

impl Error for ControlError {}

/// Decode exactly one typed control request, borrowing any text from the input slice.
pub fn decode_request_payload(bytes: &[u8]) -> Result<ControlRequest<'_>, ControlError> {
    if bytes.len() > MAX_OPERATION_PAYLOAD_BYTES {
        return Err(ControlError::PayloadTooLarge);
    }
    if bytes.len() < PREFIX_BYTES {
        return Err(ControlError::Truncated);
    }

    let schema = read_u16(bytes, 0)?;
    if schema != CONTROL_SCHEMA_VERSION {
        return Err(ControlError::UnsupportedSchema);
    }
    let opcode = read_u16(bytes, 2)?;
    let key = OperationKey {
        generation: read_u64(bytes, 4)?,
        lease: read_u64(bytes, 12)?,
        sequence: read_u64(bytes, 20)?,
    };
    let target = TargetRef {
        generation: read_u64(bytes, 28)?,
        workspace: read_u64(bytes, 36)?,
        pane: read_u64(bytes, 44)?,
        session: read_u64(bytes, 52)?,
        occupant_revision: read_u64(bytes, 60)?,
    };
    validate_identifiers(key, target)?;

    let command = match opcode {
        OPCODE_READ => decode_read(bytes)?,
        OPCODE_TEXT => decode_text(bytes, TextKind::Text)?,
        OPCODE_PASTE => decode_text(bytes, TextKind::Paste)?,
        OPCODE_SUBMIT => decode_text(bytes, TextKind::Submit)?,
        OPCODE_INTERRUPT => decode_interrupt(bytes)?,
        _ => return Err(ControlError::UnknownOpcode),
    };
    Ok(ControlRequest {
        key,
        target,
        command,
    })
}

/// Encode only the typed request payload, validating it before the first write.
///
/// If the writer fails after accepting a prefix, that prefix cannot be rolled
/// back. The caller must treat the incomplete payload as a failed write.
pub fn encode_request_payload<W: Write>(
    request: &ControlRequest<'_>,
    writer: &mut W,
) -> io::Result<()> {
    request
        .encoded_len()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    write_u16(writer, CONTROL_SCHEMA_VERSION)?;
    write_u16(writer, command_opcode(request.command))?;
    write_u64(writer, request.key.generation)?;
    write_u64(writer, request.key.lease)?;
    write_u64(writer, request.key.sequence)?;
    write_u64(writer, request.target.generation)?;
    write_u64(writer, request.target.workspace)?;
    write_u64(writer, request.target.pane)?;
    write_u64(writer, request.target.session)?;
    write_u64(writer, request.target.occupant_revision)?;

    match request.command {
        Command::Read {
            source,
            max_bytes,
            max_lines,
        } => {
            writer.write_all(&[read_source_code(source)])?;
            write_u32(writer, max_bytes)?;
            write_u16(writer, max_lines)?;
        }
        Command::Text {
            owner_revision,
            text,
        }
        | Command::Paste {
            owner_revision,
            text,
        }
        | Command::Submit {
            owner_revision,
            text,
        } => {
            write_u64(writer, owner_revision)?;
            let text_len = u32::try_from(text.len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, ControlError::InputTextTooLarge)
            })?;
            write_u32(writer, text_len)?;
            writer.write_all(text.as_bytes())?;
        }
        Command::Interrupt { owner_revision } => write_u64(writer, owner_revision)?,
    }
    Ok(())
}

fn validate_identifiers(key: OperationKey, target: TargetRef) -> Result<(), ControlError> {
    if key.generation == 0
        || key.lease == 0
        || key.sequence == 0
        || target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
    {
        return Err(ControlError::ZeroIdentifier);
    }
    if key.generation != target.generation {
        return Err(ControlError::GenerationMismatch);
    }
    Ok(())
}

fn validate_command(command: Command<'_>) -> Result<(), ControlError> {
    match command {
        Command::Read {
            source: _,
            max_bytes,
            max_lines,
        } => validate_read_limits(max_bytes, max_lines),
        Command::Text {
            owner_revision,
            text,
        }
        | Command::Submit {
            owner_revision,
            text,
        } => {
            validate_owner_revision(owner_revision)?;
            validate_text(text, true)
        }
        Command::Paste {
            owner_revision,
            text,
        } => {
            validate_owner_revision(owner_revision)?;
            validate_text(text, false)
        }
        Command::Interrupt { owner_revision } => validate_owner_revision(owner_revision),
    }
}

fn validate_owner_revision(owner_revision: u64) -> Result<(), ControlError> {
    if owner_revision == 0 {
        Err(ControlError::ZeroIdentifier)
    } else {
        Ok(())
    }
}

fn validate_read_limits(max_bytes: u32, max_lines: u16) -> Result<(), ControlError> {
    if !(1..=MAX_READ_BYTES).contains(&max_bytes) || !(1..=MAX_READ_LINES).contains(&max_lines) {
        return Err(ControlError::InvalidReadLimit);
    }
    Ok(())
}

fn validate_text(text: &str, reject_controls: bool) -> Result<(), ControlError> {
    if text.len() > MAX_INPUT_TEXT_BYTES {
        return Err(ControlError::InputTextTooLarge);
    }
    if reject_controls && text.chars().any(char::is_control) {
        return Err(ControlError::InvalidText);
    }
    Ok(())
}

fn decode_read(bytes: &[u8]) -> Result<Command<'_>, ControlError> {
    let end = PREFIX_BYTES + READ_BODY_BYTES;
    check_exact_length(bytes, end)?;
    let source = match read_u8(bytes, PREFIX_BYTES)? {
        1 => ReadSource::Screen,
        2 => ReadSource::LiveScrollback,
        _ => return Err(ControlError::UnsupportedReadSource),
    };
    let max_bytes = read_u32(bytes, PREFIX_BYTES + 1)?;
    let max_lines = read_u16(bytes, PREFIX_BYTES + 5)?;
    validate_read_limits(max_bytes, max_lines)?;
    Ok(Command::Read {
        source,
        max_bytes,
        max_lines,
    })
}

#[derive(Clone, Copy)]
enum TextKind {
    Text,
    Paste,
    Submit,
}

fn decode_text(bytes: &[u8], kind: TextKind) -> Result<Command<'_>, ControlError> {
    if bytes.len() < TEXT_BYTES_OFFSET {
        return Err(ControlError::Truncated);
    }
    let owner_revision = read_u64(bytes, PREFIX_BYTES)?;
    validate_owner_revision(owner_revision)?;
    let text_len = read_u32(bytes, PREFIX_BYTES + 8)?;
    let text_len = usize::try_from(text_len).map_err(|_| ControlError::InputTextTooLarge)?;
    if text_len > MAX_INPUT_TEXT_BYTES {
        return Err(ControlError::InputTextTooLarge);
    }
    let end = TEXT_BYTES_OFFSET
        .checked_add(text_len)
        .ok_or(ControlError::InputTextTooLarge)?;
    check_exact_length(bytes, end)?;
    let text_bytes = bytes
        .get(TEXT_BYTES_OFFSET..end)
        .ok_or(ControlError::Truncated)?;
    let text = str::from_utf8(text_bytes).map_err(|_| ControlError::InvalidUtf8)?;
    let reject_controls = !matches!(kind, TextKind::Paste);
    validate_text(text, reject_controls)?;
    let command = match kind {
        TextKind::Text => Command::Text {
            owner_revision,
            text,
        },
        TextKind::Paste => Command::Paste {
            owner_revision,
            text,
        },
        TextKind::Submit => Command::Submit {
            owner_revision,
            text,
        },
    };
    Ok(command)
}

fn decode_interrupt(bytes: &[u8]) -> Result<Command<'_>, ControlError> {
    let end = PREFIX_BYTES + INTERRUPT_BODY_BYTES;
    check_exact_length(bytes, end)?;
    let owner_revision = read_u64(bytes, PREFIX_BYTES)?;
    validate_owner_revision(owner_revision)?;
    Ok(Command::Interrupt { owner_revision })
}

fn check_exact_length(bytes: &[u8], expected: usize) -> Result<(), ControlError> {
    if bytes.len() < expected {
        Err(ControlError::Truncated)
    } else if bytes.len() > expected {
        Err(ControlError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn command_opcode(command: Command<'_>) -> u16 {
    match command {
        Command::Read { .. } => OPCODE_READ,
        Command::Text { .. } => OPCODE_TEXT,
        Command::Paste { .. } => OPCODE_PASTE,
        Command::Submit { .. } => OPCODE_SUBMIT,
        Command::Interrupt { .. } => OPCODE_INTERRUPT,
    }
}

fn read_source_code(source: ReadSource) -> u8 {
    match source {
        ReadSource::Screen => 1,
        ReadSource::LiveScrollback => 2,
    }
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, ControlError> {
    bytes.get(offset).copied().ok_or(ControlError::Truncated)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ControlError> {
    let end = offset.checked_add(2).ok_or(ControlError::Truncated)?;
    let field = bytes.get(offset..end).ok_or(ControlError::Truncated)?;
    Ok(u16::from_be_bytes([field[0], field[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ControlError> {
    let end = offset.checked_add(4).ok_or(ControlError::Truncated)?;
    let field = bytes.get(offset..end).ok_or(ControlError::Truncated)?;
    Ok(u32::from_be_bytes([field[0], field[1], field[2], field[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ControlError> {
    let end = offset.checked_add(8).ok_or(ControlError::Truncated)?;
    let field = bytes.get(offset..end).ok_or(ControlError::Truncated)?;
    Ok(u64::from_be_bytes([
        field[0], field[1], field[2], field[3], field[4], field[5], field[6], field[7],
    ]))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::RetryDisposition;
    use crate::protocol_wire::{decode_frame, encode_frame, Frame, MessageKind, HEADER_BYTES};
    use std::io;

    const GOLDEN_READ: [u8; 75] = [
        0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0,
        0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 6,
        0, 0, 0, 0, 0, 0, 0, 7, 1, 0, 0, 0, 1, 0, 1,
    ];

    const GOLDEN_TEXT: [u8; 82] = [
        0, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0,
        0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 6,
        0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 2, 0xc3, 0xa9,
    ];

    fn request(command: Command<'_>) -> ControlRequest<'_> {
        ControlRequest {
            key: OperationKey {
                generation: 1,
                lease: 2,
                sequence: 3,
            },
            target: TargetRef {
                generation: 1,
                workspace: 4,
                pane: 5,
                session: 6,
                occupant_revision: 7,
            },
            command,
        }
    }

    fn encoded(value: &ControlRequest<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_request_payload(value, &mut bytes).expect("valid test request encodes");
        bytes
    }

    fn sample_requests() -> [ControlRequest<'static>; 5] {
        [
            request(Command::Read {
                source: ReadSource::Screen,
                max_bytes: 1,
                max_lines: 1,
            }),
            request(Command::Text {
                owner_revision: 8,
                text: "hello",
            }),
            request(Command::Paste {
                owner_revision: 8,
                text: "paste",
            }),
            request(Command::Submit {
                owner_revision: 8,
                text: "submit",
            }),
            request(Command::Interrupt { owner_revision: 8 }),
        ]
    }

    #[test]
    fn independent_golden_bytes_fix_prefix_offsets_and_body_lengths() {
        let read = request(Command::Read {
            source: ReadSource::Screen,
            max_bytes: 1,
            max_lines: 1,
        });
        assert_eq!(encoded(&read), GOLDEN_READ);
        assert_eq!(read.encoded_len(), Ok(75));

        let text = request(Command::Text {
            owner_revision: 8,
            text: "é",
        });
        assert_eq!(encoded(&text), GOLDEN_TEXT);
        assert_eq!(text.encoded_len(), Ok(82));
        assert_eq!(u16::from_be_bytes([GOLDEN_TEXT[0], GOLDEN_TEXT[1]]), 1);
        assert_eq!(u16::from_be_bytes([GOLDEN_TEXT[2], GOLDEN_TEXT[3]]), 2);
        assert_eq!(&GOLDEN_TEXT[76..80], &[0, 0, 0, 2]);
        assert_eq!(&GOLDEN_TEXT[80..], "é".as_bytes());
    }

    #[test]
    fn all_operations_round_trip_and_text_borrows_payload_storage() {
        for expected in sample_requests() {
            let bytes = encoded(&expected);
            let decoded = decode_request_payload(&bytes).expect("encoded request decodes");
            assert_eq!(decoded, expected);
            if let Command::Text { text, .. }
            | Command::Paste { text, .. }
            | Command::Submit { text, .. } = decoded.command
            {
                assert!(std::ptr::eq(
                    text.as_ptr(),
                    bytes[TEXT_BYTES_OFFSET..].as_ptr()
                ));
            }
        }
    }

    #[test]
    fn request_payload_round_trips_inside_existing_request_frame() {
        let expected = request(Command::Text {
            owner_revision: 8,
            text: "frame text",
        });
        let payload = encoded(&expected);
        let frame = Frame::new(MessageKind::Request, 99, &payload).expect("bounded request frame");
        let mut frame_bytes = Vec::new();
        encode_frame(&frame, &mut frame_bytes).expect("frame encodes");
        let decoded_frame = decode_frame(&frame_bytes).expect("frame decodes");
        assert_eq!(decoded_frame.header().kind(), MessageKind::Request);
        assert_eq!(decoded_frame.header().request_id(), 99);
        assert_eq!(
            decode_request_payload(decoded_frame.payload()),
            Ok(expected)
        );
        assert_eq!(frame_bytes.len(), HEADER_BYTES + payload.len());
    }

    #[test]
    fn encoded_target_owner_and_action_are_part_of_ledger_payload_identity() {
        use crate::protocol::{Admission, Ledger, Outcome, ProtocolError};

        let original = request(Command::Text {
            owner_revision: 8,
            text: "explicit input",
        });
        let original_bytes = encoded(&original);
        let mut ledger = Ledger::new(1).unwrap();
        ledger.open_lease(2, 100).unwrap();
        assert_eq!(
            ledger.admit(original.key, &original_bytes, 101),
            Ok(Admission::Execute)
        );
        // Duplicate admission is a server defense, not permission for a client
        // to resend nonreplayable input after an uncertain acknowledgment.
        assert_eq!(
            ledger.admit(original.key, &original_bytes, 101),
            Ok(Admission::InProgress)
        );

        let mut changed_target = original;
        changed_target.target.session = 9;
        let changed_owner = ControlRequest {
            command: Command::Text {
                owner_revision: 9,
                text: "explicit input",
            },
            ..original
        };
        let changed_action = ControlRequest {
            command: Command::Submit {
                owner_revision: 8,
                text: "explicit input",
            },
            ..original
        };
        for changed in [changed_target, changed_owner, changed_action] {
            assert_eq!(
                ledger.admit(changed.key, &encoded(&changed), 101),
                Err(ProtocolError::OperationConflict)
            );
        }
        ledger.complete(original.key, Outcome::Applied).unwrap();
        ledger.receipt(2, original.key.sequence, 102).unwrap();
        assert_eq!(
            ledger.admit(original.key, &original_bytes, 103),
            Err(ProtocolError::ResultExpired)
        );
        assert_eq!(
            original.command.class().retry_disposition(),
            RetryDisposition::Never
        );
    }

    #[test]
    fn every_truncated_prefix_of_each_operation_is_rejected() {
        for request in sample_requests() {
            let bytes = encoded(&request);
            for prefix_len in 0..bytes.len() {
                assert_eq!(
                    decode_request_payload(&bytes[..prefix_len]),
                    Err(ControlError::Truncated),
                    "accepted truncated prefix length {prefix_len} of {}",
                    bytes.len()
                );
            }
        }
    }

    #[test]
    fn unsupported_schema_opcodes_and_read_sources_fail_closed() {
        let mut bytes = GOLDEN_READ.to_vec();
        bytes[1] = 2;
        assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::UnsupportedSchema)
        );

        for opcode in [0_u16, 6, u16::MAX] {
            let mut bytes = GOLDEN_READ;
            bytes[2..4].copy_from_slice(&opcode.to_be_bytes());
            assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::UnknownOpcode)
            );
        }

        for source in [0_u8, 3, u8::MAX] {
            let mut bytes = GOLDEN_READ;
            bytes[PREFIX_BYTES] = source;
            assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::UnsupportedReadSource)
            );
        }
    }

    #[test]
    fn zero_key_target_and_owner_identifiers_are_rejected() {
        for offset in [4_usize, 12, 20, 28, 36, 44, 52, 60] {
            let mut bytes = GOLDEN_READ;
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::ZeroIdentifier)
            );
        }

        for request in [
            request(Command::Text {
                owner_revision: 0,
                text: "ok",
            }),
            request(Command::Paste {
                owner_revision: 0,
                text: "ok",
            }),
            request(Command::Submit {
                owner_revision: 0,
                text: "ok",
            }),
            request(Command::Interrupt { owner_revision: 0 }),
        ] {
            assert_eq!(request.validate(), Err(ControlError::ZeroIdentifier));
            assert_eq!(
                decode_request_payload(&encoded_unchecked(request)),
                Err(ControlError::ZeroIdentifier)
            );
        }
    }

    fn encoded_unchecked(request: ControlRequest<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&CONTROL_SCHEMA_VERSION.to_be_bytes());
        bytes.extend_from_slice(&command_opcode(request.command).to_be_bytes());
        for value in [
            request.key.generation,
            request.key.lease,
            request.key.sequence,
            request.target.generation,
            request.target.workspace,
            request.target.pane,
            request.target.session,
            request.target.occupant_revision,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        match request.command {
            Command::Text {
                owner_revision,
                text,
            }
            | Command::Paste {
                owner_revision,
                text,
            }
            | Command::Submit {
                owner_revision,
                text,
            } => {
                bytes.extend_from_slice(&owner_revision.to_be_bytes());
                bytes.extend_from_slice(&(text.len() as u32).to_be_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
            Command::Interrupt { owner_revision } => {
                bytes.extend_from_slice(&owner_revision.to_be_bytes());
            }
            Command::Read { .. } => unreachable!("test helper is only for owner revision cases"),
        }
        bytes
    }

    #[test]
    fn mismatched_generations_and_invalid_read_boundaries_are_rejected() {
        let mut bytes = GOLDEN_READ;
        bytes[28..36].copy_from_slice(&9_u64.to_be_bytes());
        assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::GenerationMismatch)
        );

        for max_bytes in [0_u32, MAX_READ_BYTES + 1] {
            let mut bytes = GOLDEN_READ;
            bytes[69..73].copy_from_slice(&max_bytes.to_be_bytes());
            assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::InvalidReadLimit)
            );
        }
        for max_lines in [0_u16, MAX_READ_LINES + 1] {
            let mut bytes = GOLDEN_READ;
            bytes[73..75].copy_from_slice(&max_lines.to_be_bytes());
            assert_eq!(
                decode_request_payload(&bytes),
                Err(ControlError::InvalidReadLimit)
            );
        }

        for max_bytes in [1, MAX_READ_BYTES] {
            for max_lines in [1, MAX_READ_LINES] {
                let request = request(Command::Read {
                    source: ReadSource::LiveScrollback,
                    max_bytes,
                    max_lines,
                });
                assert_eq!(decode_request_payload(&encoded(&request)), Ok(request));
            }
        }
    }

    #[test]
    fn utf8_byte_limits_and_malformed_text_are_checked_without_copying() {
        let at_limit = format!("{}é", "€".repeat(10_922));
        assert_eq!(at_limit.len(), MAX_INPUT_TEXT_BYTES);
        let at_limit_request = request(Command::Text {
            owner_revision: 8,
            text: &at_limit,
        });
        let bytes = encoded(&at_limit_request);
        assert_eq!(bytes.len(), TEXT_BYTES_OFFSET + MAX_INPUT_TEXT_BYTES);
        assert_eq!(decode_request_payload(&bytes), Ok(at_limit_request));

        let over_limit = "€".repeat(10_923);
        let over_limit_request = request(Command::Text {
            owner_revision: 8,
            text: &over_limit,
        });
        assert_eq!(
            over_limit_request.validate(),
            Err(ControlError::InputTextTooLarge)
        );

        let mut bytes = encoded_unchecked(request(Command::Text {
            owner_revision: 8,
            text: "x",
        }));
        bytes[76..80].copy_from_slice(&u32::MAX.to_be_bytes());
        bytes.truncate(TEXT_BYTES_OFFSET);
        assert_eq!(
            decode_request_payload(&bytes),
            Err(ControlError::InputTextTooLarge)
        );

        let mut malformed = encoded_unchecked(request(Command::Text {
            owner_revision: 8,
            text: "x",
        }));
        malformed[76..80].copy_from_slice(&1_u32.to_be_bytes());
        malformed[TEXT_BYTES_OFFSET] = 0xff;
        assert_eq!(
            decode_request_payload(&malformed),
            Err(ControlError::InvalidUtf8)
        );

        let mut one_over = encoded_unchecked(request(Command::Text {
            owner_revision: 8,
            text: "",
        }));
        one_over[76..80].copy_from_slice(&((MAX_INPUT_TEXT_BYTES + 1) as u32).to_be_bytes());
        one_over.resize(TEXT_BYTES_OFFSET + MAX_INPUT_TEXT_BYTES + 1, b'a');
        assert_eq!(
            decode_request_payload(&one_over),
            Err(ControlError::InputTextTooLarge)
        );
    }

    #[test]
    fn literal_text_and_submit_reject_controls_while_paste_preserves_them() {
        for text in ["line\nbreak", "tab\there", "escape\u{1b}"] {
            let text_request = request(Command::Text {
                owner_revision: 8,
                text,
            });
            assert_eq!(text_request.validate(), Err(ControlError::InvalidText));
            let submit_request = request(Command::Submit {
                owner_revision: 8,
                text,
            });
            assert_eq!(submit_request.validate(), Err(ControlError::InvalidText));
        }

        let pasted = "first\nsecond\t\u{1b}[31m";
        let paste = request(Command::Paste {
            owner_revision: 8,
            text: pasted,
        });
        assert_eq!(decode_request_payload(&encoded(&paste)), Ok(paste));
    }

    #[test]
    fn empty_text_paste_and_submit_keep_distinct_opcodes_and_meanings() {
        let requests = [
            request(Command::Text {
                owner_revision: 8,
                text: "",
            }),
            request(Command::Paste {
                owner_revision: 8,
                text: "",
            }),
            request(Command::Submit {
                owner_revision: 8,
                text: "",
            }),
        ];
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request.encoded_len(), Ok(TEXT_BYTES_OFFSET));
            let bytes = encoded(request);
            assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]) as usize, index + 2);
            assert_eq!(decode_request_payload(&bytes), Ok(*request));
        }
    }

    #[test]
    fn payload_ceiling_trailing_bytes_and_control_classification_are_exact() {
        let mut trailing_read = GOLDEN_READ.to_vec();
        trailing_read.push(0);
        assert_eq!(
            decode_request_payload(&trailing_read),
            Err(ControlError::TrailingBytes)
        );
        assert_eq!(
            decode_request_payload(&vec![0; MAX_OPERATION_PAYLOAD_BYTES + 1]),
            Err(ControlError::PayloadTooLarge)
        );

        assert_eq!(
            request(Command::Read {
                source: ReadSource::Screen,
                max_bytes: 1,
                max_lines: 1,
            })
            .command
            .class(),
            OperationClass::Query
        );
        for command in [
            Command::Text {
                owner_revision: 8,
                text: "x",
            },
            Command::Paste {
                owner_revision: 8,
                text: "x",
            },
            Command::Submit {
                owner_revision: 8,
                text: "x",
            },
        ] {
            assert_eq!(command.class(), OperationClass::Input);
            assert_eq!(command.class().retry_disposition(), RetryDisposition::Never);
        }
        let interrupt = Command::Interrupt { owner_revision: 8 };
        assert_eq!(interrupt.class(), OperationClass::Signal);
        assert_eq!(
            interrupt.class().retry_disposition(),
            RetryDisposition::Never
        );
    }

    #[test]
    fn invalid_requests_are_rejected_before_the_writer_receives_bytes() {
        let valid_read = request(Command::Read {
            source: ReadSource::Screen,
            max_bytes: 1,
            max_lines: 1,
        });
        let mut invalid_id = valid_read;
        invalid_id.target.pane = 0;
        let mut mismatched_generation = valid_read;
        mismatched_generation.target.generation = 2;
        let invalid_read_limit = request(Command::Read {
            source: ReadSource::Screen,
            max_bytes: 0,
            max_lines: 1,
        });
        let mut writer = Vec::new();
        for invalid_request in [invalid_id, mismatched_generation, invalid_read_limit] {
            assert_eq!(
                encode_request_payload(&invalid_request, &mut writer)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(writer.is_empty());
        }

        let invalid_text = request(Command::Submit {
            owner_revision: 8,
            text: "bad\ntext",
        });
        assert!(encode_request_payload(&invalid_text, &mut writer).is_err());
        assert!(writer.is_empty());

        let too_long = "x".repeat(MAX_INPUT_TEXT_BYTES + 1);
        let invalid_length = request(Command::Paste {
            owner_revision: 8,
            text: &too_long,
        });
        assert!(encode_request_payload(&invalid_length, &mut writer).is_err());
        assert!(writer.is_empty());
    }

    struct FailAfter {
        remaining: usize,
        accepted: Vec<u8>,
    }

    impl Write for FailAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected failure",
                ));
            }
            let count = bytes.len().min(self.remaining).min(3);
            self.accepted.extend_from_slice(&bytes[..count]);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_failure_keeps_only_the_exact_accepted_prefix() {
        let request = request(Command::Text {
            owner_revision: 8,
            text: "writer canary",
        });
        let expected = encoded(&request);
        let mut writer = FailAfter {
            remaining: 71,
            accepted: Vec::new(),
        };
        let error = encode_request_payload(&request, &mut writer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(writer.accepted, expected[..71]);
    }

    #[test]
    fn request_debug_and_errors_never_disclose_text() {
        let canary = "private-input-canary";
        let canary_request = request(Command::Paste {
            owner_revision: 8,
            text: canary,
        });
        assert!(!format!("{canary_request:?}").contains(canary));
        assert!(!format!("{:?}", canary_request.command).contains(canary));

        let invalid_request = request(Command::Text {
            owner_revision: 8,
            text: "private-input-canary\n",
        });
        let error = invalid_request.validate().unwrap_err();
        assert!(!format!("{error:?} {error}").contains(canary));
    }

    #[test]
    fn read_and_interrupt_source_payload_sizes_match_the_contract() {
        let read = request(Command::Read {
            source: ReadSource::LiveScrollback,
            max_bytes: MAX_READ_BYTES,
            max_lines: MAX_READ_LINES,
        });
        assert_eq!(read.encoded_len(), Ok(75));
        assert_eq!(encoded(&read).len(), 75);

        let interrupt = request(Command::Interrupt { owner_revision: 8 });
        assert_eq!(interrupt.encoded_len(), Ok(76));
        assert_eq!(encoded(&interrupt).len(), 76);
    }
}
