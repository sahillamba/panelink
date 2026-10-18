//! Bounded, versioned wire envelopes for communication with a Panelink server.
//!
//! This module only frames opaque payload bytes. It does not define operation
//! schemas, perform socket I/O, or make delivery and recovery guarantees.

use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};

/// Number of bytes in a complete wire header, including its length prefix.
pub const HEADER_BYTES: usize = 24;

/// Maximum validated payload size for a control message or event, in bytes.
///
/// This is an admission limit; it does not reserve memory or define a queue.
pub const MAX_CONTROL_PAYLOAD_BYTES: usize = 65_536;

/// Maximum validated payload size for a screen snapshot, in bytes.
///
/// This is an admission limit; it does not reserve memory or define a queue.
pub const MAX_SNAPSHOT_PAYLOAD_BYTES: usize = 2_097_152;

/// Maximum encoded size of a control frame, including its header.
pub const MAX_CONTROL_FRAME_BYTES: usize = HEADER_BYTES + MAX_CONTROL_PAYLOAD_BYTES;

/// Maximum encoded size of a snapshot frame, including its header.
pub const MAX_SNAPSHOT_FRAME_BYTES: usize = HEADER_BYTES + MAX_SNAPSHOT_PAYLOAD_BYTES;

/// Major version accepted by this wire decoder.
pub const PROTOCOL_MAJOR: u16 = 1;

/// Minor version accepted by this wire decoder.
pub const PROTOCOL_MINOR: u16 = 0;

const MAGIC: &[u8; 4] = b"PLNK";
const FRAME_HEADER_BYTES: u32 = 20;
const FLAGS_NONE: u16 = 0;

/// A recognized v1.0 wire-message kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum MessageKind {
    /// Client handshake request.
    Hello = 1,
    /// Server handshake response.
    Welcome = 2,
    /// Opaque client request.
    Request = 3,
    /// Opaque server response.
    Response = 4,
    /// Opaque screen snapshot.
    Snapshot = 5,
    /// Opaque error response.
    Error = 6,
    /// Unsolicited opaque event with a zero request ID.
    ///
    /// Runtime use requires an explicitly negotiated event capability; this
    /// framing module does not implement capability negotiation.
    Event = 7,
}

impl MessageKind {
    const fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Hello),
            2 => Some(Self::Welcome),
            3 => Some(Self::Request),
            4 => Some(Self::Response),
            5 => Some(Self::Snapshot),
            6 => Some(Self::Error),
            7 => Some(Self::Event),
            _ => None,
        }
    }

    const fn wire_value(self) -> u16 {
        self as u16
    }

    /// Return the maximum validated opaque payload size for this message kind.
    ///
    /// The returned limit describes frame admission only. It does not reserve
    /// memory or imply a queue capacity.
    pub const fn max_payload_bytes(self) -> usize {
        match self {
            Self::Hello | Self::Welcome | Self::Error => 4_096,
            Self::Request | Self::Response | Self::Event => MAX_CONTROL_PAYLOAD_BYTES,
            Self::Snapshot => MAX_SNAPSHOT_PAYLOAD_BYTES,
        }
    }
}

/// Fixed validation errors for v1.0 wire envelopes.
///
/// Variants carry no received bytes or payload content, so formatting an error
/// cannot disclose frame contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    /// The input ended before a complete 24-byte header was available.
    TruncatedHeader,
    /// The declared frame length is shorter than its 20-byte body header.
    FrameLengthTooSmall,
    /// A checked frame-length conversion or addition overflowed.
    LengthOverflow,
    /// The four-byte protocol marker is not `PLNK`.
    BadMagic,
    /// The major or minor protocol version is unsupported.
    UnsupportedVersion,
    /// The numeric message kind is not recognized.
    UnknownMessageKind,
    /// The header contains nonzero reserved flags.
    UnsupportedFlags,
    /// The payload exceeds the limit for its message kind.
    PayloadTooLarge,
    /// An event frame has a nonzero request identifier.
    EventRequestIdNotZero,
    /// The complete header was present, but the declared payload was truncated.
    TruncatedFrame,
    /// Bytes remain after the one declared frame.
    TrailingBytes,
    /// A frame's stored payload length does not match its borrowed payload.
    PayloadLengthMismatch,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TruncatedHeader => "wire header is truncated",
            Self::FrameLengthTooSmall => "declared frame is shorter than its header",
            Self::LengthOverflow => "wire frame length overflowed",
            Self::BadMagic => "wire magic is invalid",
            Self::UnsupportedVersion => "wire protocol version is unsupported",
            Self::UnknownMessageKind => "wire message kind is unknown",
            Self::UnsupportedFlags => "wire flags are unsupported",
            Self::PayloadTooLarge => "wire payload exceeds its kind limit",
            Self::EventRequestIdNotZero => "event request ID must be zero",
            Self::TruncatedFrame => "wire payload is truncated",
            Self::TrailingBytes => "bytes follow the declared wire frame",
            Self::PayloadLengthMismatch => "frame payload length is inconsistent",
        };
        f.write_str(message)
    }
}

impl Error for WireError {}

/// Validated header metadata for one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    kind: MessageKind,
    request_id: u64,
    payload_len: usize,
}

impl FrameHeader {
    /// Return the validated message kind.
    pub const fn kind(self) -> MessageKind {
        self.kind
    }

    /// Return the opaque request identifier; zero is valid.
    pub const fn request_id(self) -> u64 {
        self.request_id
    }

    /// Return the validated payload length in bytes.
    pub const fn payload_len(self) -> usize {
        self.payload_len
    }
}

/// A frame borrowing its opaque payload from the caller.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Frame<'a> {
    header: FrameHeader,
    payload: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Construct a frame after checking the kind-specific payload limit.
    pub fn new(kind: MessageKind, request_id: u64, payload: &'a [u8]) -> Result<Self, WireError> {
        validate_request_id(kind, request_id)?;
        validate_payload_len(kind, payload.len())?;
        Ok(Self {
            header: FrameHeader {
                kind,
                request_id,
                payload_len: payload.len(),
            },
            payload,
        })
    }

    /// Return the validated header metadata.
    pub const fn header(&self) -> FrameHeader {
        self.header
    }

    /// Return the borrowed opaque payload.
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("kind_code", &self.header.kind.wire_value())
            .field("request_id", &self.header.request_id)
            .field("payload_len", &self.header.payload_len)
            .finish()
    }
}

/// Decode and validate one fixed-size header without allocating.
pub fn decode_header(bytes: &[u8; HEADER_BYTES]) -> Result<FrameHeader, WireError> {
    let declared_frame_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if declared_frame_len < FRAME_HEADER_BYTES {
        return Err(WireError::FrameLengthTooSmall);
    }
    if &bytes[4..8] != MAGIC {
        return Err(WireError::BadMagic);
    }

    let major = read_u16(bytes, 8);
    let minor = read_u16(bytes, 10);
    if major != PROTOCOL_MAJOR || minor != PROTOCOL_MINOR {
        return Err(WireError::UnsupportedVersion);
    }

    let kind = MessageKind::from_wire(read_u16(bytes, 12)).ok_or(WireError::UnknownMessageKind)?;
    if read_u16(bytes, 14) != FLAGS_NONE {
        return Err(WireError::UnsupportedFlags);
    }

    let payload_len_u32 = declared_frame_len - FRAME_HEADER_BYTES;
    let payload_len = usize::try_from(payload_len_u32).map_err(|_| WireError::LengthOverflow)?;
    validate_payload_len(kind, payload_len)?;
    let request_id = u64::from_be_bytes([
        bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
    ]);
    validate_request_id(kind, request_id)?;

    Ok(FrameHeader {
        kind,
        request_id,
        payload_len,
    })
}

/// Decode exactly one complete frame while borrowing its payload from `bytes`.
pub fn decode_frame(bytes: &[u8]) -> Result<Frame<'_>, WireError> {
    let header_bytes: &[u8; HEADER_BYTES] = bytes
        .get(..HEADER_BYTES)
        .ok_or(WireError::TruncatedHeader)?
        .try_into()
        .map_err(|_| WireError::TruncatedHeader)?;
    let header = decode_header(header_bytes)?;
    let expected_len = HEADER_BYTES
        .checked_add(header.payload_len)
        .ok_or(WireError::LengthOverflow)?;
    if bytes.len() < expected_len {
        return Err(WireError::TruncatedFrame);
    }
    if bytes.len() > expected_len {
        return Err(WireError::TrailingBytes);
    }

    let payload = bytes
        .get(HEADER_BYTES..expected_len)
        .ok_or(WireError::TruncatedFrame)?;
    Ok(Frame { header, payload })
}

/// Encode one frame as a header followed by its opaque payload.
///
/// Validation happens before the first write. I/O errors are returned as-is;
/// bytes accepted by the writer before an error are not rolled back or resent.
pub fn encode_frame<W: Write>(frame: &Frame<'_>, writer: &mut W) -> io::Result<()> {
    let header_bytes =
        encode_header(frame).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    writer.write_all(&header_bytes)?;
    writer.write_all(frame.payload)
}

fn encode_header(frame: &Frame<'_>) -> Result<[u8; HEADER_BYTES], WireError> {
    validate_request_id(frame.header.kind, frame.header.request_id)?;
    validate_payload_len(frame.header.kind, frame.payload.len())?;
    if frame.header.payload_len != frame.payload.len() {
        return Err(WireError::PayloadLengthMismatch);
    }
    let frame_len = FRAME_HEADER_BYTES
        .checked_add(u32::try_from(frame.payload.len()).map_err(|_| WireError::LengthOverflow)?)
        .ok_or(WireError::LengthOverflow)?;

    let mut bytes = [0_u8; HEADER_BYTES];
    bytes[0..4].copy_from_slice(&frame_len.to_be_bytes());
    bytes[4..8].copy_from_slice(MAGIC);
    bytes[8..10].copy_from_slice(&PROTOCOL_MAJOR.to_be_bytes());
    bytes[10..12].copy_from_slice(&PROTOCOL_MINOR.to_be_bytes());
    bytes[12..14].copy_from_slice(&frame.header.kind.wire_value().to_be_bytes());
    bytes[14..16].copy_from_slice(&FLAGS_NONE.to_be_bytes());
    bytes[16..24].copy_from_slice(&frame.header.request_id.to_be_bytes());
    Ok(bytes)
}

fn validate_payload_len(kind: MessageKind, payload_len: usize) -> Result<(), WireError> {
    if payload_len > kind.max_payload_bytes() {
        return Err(WireError::PayloadTooLarge);
    }
    Ok(())
}

fn validate_request_id(kind: MessageKind, request_id: u64) -> Result<(), WireError> {
    if kind == MessageKind::Event && request_id != 0 {
        return Err(WireError::EventRequestIdNotZero);
    }
    Ok(())
}

fn read_u16(bytes: &[u8; HEADER_BYTES], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const GOLDEN: [u8; HEADER_BYTES + 2] = [
        0x00, 0x00, 0x00, 0x16, b'P', b'L', b'N', b'K', 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0xaa, 0xbb,
    ];

    const EVENT_GOLDEN: [u8; HEADER_BYTES + 2] = [
        0x00, 0x00, 0x00, 0x16, b'P', b'L', b'N', b'K', 0x00, 0x01, 0x00, 0x00, 0x00, 0x07, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xaa, 0xbb,
    ];

    fn header_for(kind: MessageKind, payload_len: u32) -> [u8; HEADER_BYTES] {
        let frame_len = FRAME_HEADER_BYTES + payload_len;
        let mut bytes = [0_u8; HEADER_BYTES];
        bytes[0..4].copy_from_slice(&frame_len.to_be_bytes());
        bytes[4..8].copy_from_slice(MAGIC);
        bytes[8..10].copy_from_slice(&PROTOCOL_MAJOR.to_be_bytes());
        bytes[10..12].copy_from_slice(&PROTOCOL_MINOR.to_be_bytes());
        bytes[12..14].copy_from_slice(&kind.wire_value().to_be_bytes());
        bytes[14..16].copy_from_slice(&FLAGS_NONE.to_be_bytes());
        bytes
    }

    fn assert_header_error(
        mut bytes: [u8; HEADER_BYTES],
        offset: usize,
        value: u8,
        error: WireError,
    ) {
        bytes[offset] = value;
        assert_eq!(decode_header(&bytes), Err(error));
    }

    #[test]
    fn golden_header_bytes_are_stable() {
        let frame = Frame::new(MessageKind::Hello, 0x0102_0304_0506_0708, &[0xaa, 0xbb])
            .expect("small hello frame should be valid");
        let mut encoded = Vec::new();
        encode_frame(&frame, &mut encoded).expect("vector writer should succeed");
        assert_eq!(encoded, GOLDEN);

        let decoded = decode_frame(&GOLDEN).expect("independent golden frame should decode");
        assert_eq!(decoded.header().kind(), MessageKind::Hello);
        assert_eq!(decoded.header().request_id(), 0x0102_0304_0506_0708);
        assert_eq!(decoded.payload(), &[0xaa, 0xbb]);
    }

    #[test]
    fn all_message_kinds_round_trip_binary_payloads_and_zero_request_id() {
        let payload = [0x00, 0xff, 0x80, 0x01];
        let kinds = [
            MessageKind::Hello,
            MessageKind::Welcome,
            MessageKind::Request,
            MessageKind::Response,
            MessageKind::Snapshot,
            MessageKind::Error,
            MessageKind::Event,
        ];

        for (index, kind) in kinds.into_iter().enumerate() {
            let request_id = if matches!(kind, MessageKind::Hello | MessageKind::Event) {
                0
            } else {
                index as u64
            };
            let frame = Frame::new(kind, request_id, &payload).expect("payload fits every kind");
            let mut encoded = Vec::new();
            encode_frame(&frame, &mut encoded).expect("vector writer should succeed");
            let decoded = decode_frame(&encoded).expect("encoded frame should decode");
            assert_eq!(decoded.header().kind(), kind);
            assert_eq!(decoded.header().request_id(), request_id);
            assert_eq!(decoded.payload(), payload);
        }
    }

    #[test]
    fn event_golden_header_uses_kind_seven_and_zero_request_id() {
        let decoded = decode_frame(&EVENT_GOLDEN).expect("independent event frame should decode");
        assert_eq!(decoded.header().kind(), MessageKind::Event);
        assert_eq!(decoded.header().request_id(), 0);
        assert_eq!(decoded.payload(), &[0xaa, 0xbb]);

        let frame = Frame::new(MessageKind::Event, 0, &[0xaa, 0xbb])
            .expect("zero-ID event frame should be valid");
        let mut encoded = Vec::new();
        encode_frame(&frame, &mut encoded).expect("vector writer should succeed");
        assert_eq!(encoded, EVENT_GOLDEN);
    }

    #[test]
    fn rejects_short_headers_and_truncated_bodies() {
        assert_eq!(decode_frame(&[]), Err(WireError::TruncatedHeader));
        assert_eq!(
            decode_frame(&GOLDEN[..HEADER_BYTES - 1]),
            Err(WireError::TruncatedHeader)
        );
        assert_eq!(
            decode_frame(&GOLDEN[..GOLDEN.len() - 1]),
            Err(WireError::TruncatedFrame)
        );

        let mut too_short = GOLDEN;
        too_short[0..4].copy_from_slice(&19_u32.to_be_bytes());
        assert_eq!(
            decode_frame(&too_short),
            Err(WireError::FrameLengthTooSmall)
        );

        let mut too_long = GOLDEN;
        too_long[0..4].copy_from_slice(&23_u32.to_be_bytes());
        assert_eq!(decode_frame(&too_long), Err(WireError::TruncatedFrame));
    }

    #[test]
    fn rejects_trailing_bytes_and_invalid_header_fields() {
        let mut trailing = GOLDEN.to_vec();
        trailing.push(0xcc);
        assert_eq!(decode_frame(&trailing), Err(WireError::TrailingBytes));

        let valid_header = header_for(MessageKind::Hello, 0);
        assert_header_error(valid_header, 4, b'X', WireError::BadMagic);
        assert_header_error(valid_header, 9, 2, WireError::UnsupportedVersion);
        assert_header_error(valid_header, 11, 1, WireError::UnsupportedVersion);
        assert_header_error(valid_header, 13, 8, WireError::UnknownMessageKind);
        assert_header_error(valid_header, 15, 1, WireError::UnsupportedFlags);
    }

    #[test]
    fn event_requires_zero_request_id_at_construction_decode_and_encode() {
        assert_eq!(
            Frame::new(MessageKind::Event, 1, &[]).unwrap_err(),
            WireError::EventRequestIdNotZero
        );

        let mut nonzero_id_header = header_for(MessageKind::Event, 0);
        nonzero_id_header[23] = 1;
        assert_eq!(
            decode_header(&nonzero_id_header),
            Err(WireError::EventRequestIdNotZero)
        );

        let forged = Frame {
            header: FrameHeader {
                kind: MessageKind::Event,
                request_id: 1,
                payload_len: 0,
            },
            payload: &[],
        };
        let mut writer = Vec::new();
        let error = encode_frame(&forged, &mut writer).expect_err("forged frame should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            error.to_string(),
            WireError::EventRequestIdNotZero.to_string()
        );
        assert!(writer.is_empty());
    }

    #[test]
    fn every_kind_obeys_its_exact_payload_boundary() {
        let kinds_and_limits = [
            (MessageKind::Hello, 4_096),
            (MessageKind::Welcome, 4_096),
            (MessageKind::Request, MAX_CONTROL_PAYLOAD_BYTES),
            (MessageKind::Response, MAX_CONTROL_PAYLOAD_BYTES),
            (MessageKind::Snapshot, MAX_SNAPSHOT_PAYLOAD_BYTES),
            (MessageKind::Error, 4_096),
            (MessageKind::Event, MAX_CONTROL_PAYLOAD_BYTES),
        ];

        for (kind, limit) in kinds_and_limits {
            let exact = header_for(kind, limit as u32);
            let decoded = decode_header(&exact).expect("exact kind limit should fit");
            assert_eq!(decoded.kind(), kind);
            assert_eq!(decoded.payload_len(), limit);
            assert!(Frame::new(kind, 0, &vec![0; limit]).is_ok());

            let one_over = header_for(kind, limit as u32 + 1);
            assert_eq!(decode_header(&one_over), Err(WireError::PayloadTooLarge));
            assert_eq!(
                Frame::new(kind, 0, &vec![0; limit + 1]).unwrap_err(),
                WireError::PayloadTooLarge
            );
        }
    }

    struct CountingWriter {
        written: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn maximum_control_and_snapshot_encodings_match_frame_limits() {
        assert_eq!(MAX_CONTROL_FRAME_BYTES, 65_560);
        assert_eq!(MAX_SNAPSHOT_FRAME_BYTES, 2_097_176);

        for (kind, payload_len, frame_limit) in [
            (
                MessageKind::Request,
                MAX_CONTROL_PAYLOAD_BYTES,
                MAX_CONTROL_FRAME_BYTES,
            ),
            (
                MessageKind::Event,
                MAX_CONTROL_PAYLOAD_BYTES,
                MAX_CONTROL_FRAME_BYTES,
            ),
            (
                MessageKind::Snapshot,
                MAX_SNAPSHOT_PAYLOAD_BYTES,
                MAX_SNAPSHOT_FRAME_BYTES,
            ),
        ] {
            let payload = vec![0; payload_len];
            let frame = Frame::new(kind, 0, &payload).expect("maximum payload should fit");
            let mut writer = CountingWriter { written: 0 };
            encode_frame(&frame, &mut writer).expect("counting writer should succeed");
            assert_eq!(writer.written, frame_limit);
        }
    }

    #[test]
    fn u32_max_length_is_rejected_without_a_body_allocation() {
        let mut bytes = header_for(MessageKind::Snapshot, 0);
        bytes[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode_header(&bytes), Err(WireError::PayloadTooLarge));
    }

    #[test]
    fn decoded_payload_borrows_the_input_buffer() {
        let decoded = decode_frame(&GOLDEN).expect("golden frame should decode");
        assert_eq!(decoded.payload().as_ptr(), GOLDEN[HEADER_BYTES..].as_ptr());
    }

    struct FailAfterPrefix {
        remaining: usize,
        accepted: Vec<u8>,
    }

    impl Write for FailAfterPrefix {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "test failure"));
            }
            let count = bytes.len().min(self.remaining);
            self.accepted.extend_from_slice(&bytes[..count]);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_failure_preserves_only_the_accepted_prefix() {
        let frame = Frame::new(MessageKind::Request, 44, &[1, 2, 3, 4, 5])
            .expect("request payload should fit");
        let mut complete = Vec::new();
        encode_frame(&frame, &mut complete).expect("vector writer should succeed");

        let accepted_len = HEADER_BYTES + 2;
        let mut writer = FailAfterPrefix {
            remaining: accepted_len,
            accepted: Vec::new(),
        };
        let error = encode_frame(&frame, &mut writer).expect_err("writer should fail");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(writer.accepted, complete[..accepted_len]);
    }

    #[test]
    fn encoder_rejects_invalid_internal_frame_before_first_write() {
        let frame = Frame {
            header: FrameHeader {
                kind: MessageKind::Hello,
                request_id: 0,
                payload_len: 4_097,
            },
            payload: &[0; 4_097],
        };
        let mut writer = Vec::new();
        let error = encode_frame(&frame, &mut writer).expect_err("invalid frame should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(writer.is_empty());
    }

    #[test]
    fn frame_debug_and_errors_do_not_disclose_payload_canary() {
        let canary = b"private-payload-canary";
        let frame = Frame::new(MessageKind::Request, 9, canary).expect("payload should fit");
        let diagnostic = format!("{frame:?} {}", WireError::BadMagic);
        assert!(!diagnostic.contains("private-payload-canary"));
        assert!(diagnostic.contains("payload_len"));
    }
}
