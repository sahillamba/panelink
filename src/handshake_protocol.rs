//! Pure v1.0 Hello and Welcome payloads for the local protocol.
//!
//! This module validates and encodes handshake payload bytes only. A runtime
//! must authenticate its peer before passing only implemented and authorized
//! capabilities to [`negotiate`], and must supply a server-issued lease and a
//! generation unique to that server start. This codec does not authenticate,
//! issue or renew leases, create generations, or perform I/O. Envelope version
//! validation belongs to `protocol_wire`; this schema has no version fallback.

use std::error::Error;
use std::fmt;

/// Schema version used by both handshake payloads.
pub const HANDSHAKE_SCHEMA_VERSION: u16 = 1;

/// Lease duration advertised by a valid Welcome payload.
pub const HANDSHAKE_LEASE_TTL_SECONDS: u32 = 1_800;

/// Exact encoded Hello payload size.
pub const HELLO_PAYLOAD_BYTES: usize = 28;

/// Exact encoded Welcome payload size.
pub const WELCOME_PAYLOAD_BYTES: usize = 40;

/// Capability bit for bounded reads.
pub const CAPABILITY_READ: u64 = 1;

/// Capability bit for input operations.
pub const CAPABILITY_INPUT: u64 = 2;

/// Capability bit for event delivery.
pub const CAPABILITY_EVENTS: u64 = 4;

/// Capability bit for workspace-scoped layout reconciliation and watches.
pub const CAPABILITY_WORKSPACE_EVENTS: u64 = 8;

/// Capability bit for live managed-child cwd and executable observations.
/// Only the multi-session server may offer this capability.
pub const CAPABILITY_PANE_METADATA: u64 = 16;

/// Capability for authoritative visible-row soft-wrap metadata in UI snapshots.
/// Only the multi-session UI path offers this capability.
pub const CAPABILITY_SCREEN_ROW_WRAPS: u64 = 32;

/// Capability for the current normal-history cursor in UI screen/history replies.
/// Only the multi-session UI path offers this capability.
pub const CAPABILITY_HISTORY_CURSOR: u64 = 512;

/// Capability vocabulary for authoritative pane swap, divider and zoom operations.
/// Runtime paths must not offer it until those operations are implemented.
pub const CAPABILITY_PANE_OPERATIONS: u64 = 64;

/// Live foreground-agent process observations and guarded submissions.
pub const CAPABILITY_AGENT_OBSERVATION: u64 = 128;

/// Read-only server build identity query after handshake negotiation.
pub const CAPABILITY_BUILD_IDENTITY: u64 = 256;

/// Optional capability for vacancy-scoped mouse-wheel input.
pub const CAPABILITY_SCOPED_MOUSE_WHEEL: u64 = 1024;

/// Stable names for every capability bit supported by this protocol revision.
pub const CAPABILITY_NAMES: &[(u64, &str)] = &[
    (CAPABILITY_READ, "read"),
    (CAPABILITY_INPUT, "input"),
    (CAPABILITY_EVENTS, "events"),
    (CAPABILITY_WORKSPACE_EVENTS, "workspace_events"),
    (CAPABILITY_PANE_METADATA, "pane_metadata"),
    (CAPABILITY_SCREEN_ROW_WRAPS, "screen_row_wraps"),
    (CAPABILITY_HISTORY_CURSOR, "history_cursor"),
    (CAPABILITY_PANE_OPERATIONS, "pane_operations"),
    (CAPABILITY_AGENT_OBSERVATION, "agent_observation"),
    (CAPABILITY_BUILD_IDENTITY, "build_identity"),
    (CAPABILITY_SCOPED_MOUSE_WHEEL, "scoped_mouse_wheel"),
];

/// All capability bits currently defined by this schema.
pub const KNOWN_CAPABILITIES: u64 = CAPABILITY_READ
    | CAPABILITY_INPUT
    | CAPABILITY_EVENTS
    | CAPABILITY_WORKSPACE_EVENTS
    | CAPABILITY_PANE_METADATA
    | CAPABILITY_SCREEN_ROW_WRAPS
    | CAPABILITY_HISTORY_CURSOR
    | CAPABILITY_PANE_OPERATIONS
    | CAPABILITY_AGENT_OBSERVATION
    | CAPABILITY_BUILD_IDENTITY
    | CAPABILITY_SCOPED_MOUSE_WHEEL;

/// Client capability request for the v1.0 handshake.
///
/// Capability fields are protocol vocabulary. Their presence does not prove
/// that a running server implements or authorizes those features. Unknown
/// capability bits are preserved in a decoded Hello so the server can reject
/// unknown required bits while ignoring unknown optional bits.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Hello {
    /// Client-generated, nonzero value echoed by Welcome.
    pub nonce: u64,
    /// Features the client requires for this connection.
    pub required_capabilities: u64,
    /// Features the client can use if the server selects them.
    pub optional_capabilities: u64,
}

impl Hello {
    /// Validate the nonce and ensure required and optional bits do not overlap.
    ///
    /// Unknown capability bits are valid here; negotiation decides whether
    /// required bits are available and ignores unknown optional bits.
    pub fn validate(&self) -> Result<(), HandshakeError> {
        validate_hello(*self)
    }
}

impl fmt::Debug for Hello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hello")
            .field("nonce", &"<redacted>")
            .field("required_capabilities", &self.required_capabilities)
            .field("optional_capabilities", &self.optional_capabilities)
            .finish()
    }
}

/// Server response selecting a subset of the client's requested capabilities.
///
/// Generation and lease values are caller-supplied to this pure codec. A
/// runtime is responsible for making the generation unique per server start
/// and issuing the lease after peer authentication.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Welcome {
    /// Nonzero client nonce copied from Hello.
    pub nonce: u64,
    /// Nonzero server-start generation supplied by the runtime.
    pub generation: u64,
    /// Nonzero server-issued client lease supplied by the runtime.
    pub lease: u64,
    /// Capabilities selected by the server.
    pub selected_capabilities: u64,
    /// Lease duration in seconds; v1.0 requires 1800.
    pub ttl_seconds: u32,
}

impl Welcome {
    /// Validate identifiers, selected capability bits, and the v1.0 lease TTL.
    pub fn validate(&self) -> Result<(), HandshakeError> {
        validate_welcome_payload(*self)
    }
}

impl fmt::Debug for Welcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Welcome")
            .field("nonce", &"<redacted>")
            .field("generation", &"<redacted>")
            .field("lease", &"<redacted>")
            .field("selected_capabilities", &self.selected_capabilities)
            .field("ttl_seconds", &self.ttl_seconds)
            .finish()
    }
}

/// Fixed validation and negotiation errors for handshake payloads.
///
/// Variants carry no received bytes, nonces, generations, or lease values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeError {
    /// Payload size is not the exact size required by its message kind.
    InvalidLength,
    /// The payload schema version is unsupported.
    UnsupportedSchema,
    /// A reserved field is nonzero.
    NonzeroReserved,
    /// A required identifier is zero.
    ZeroIdentifier,
    /// Required and optional client capability masks overlap.
    OverlappingCapabilities,
    /// The server supplied capability bits unknown to this schema.
    UnknownAvailableCapabilities,
    /// The server selected capability bits unknown to this schema.
    UnknownSelectedCapabilities,
    /// One or more required client capabilities are unavailable.
    RequiredCapabilitiesUnavailable,
    /// The response did not select every required client capability.
    MissingRequiredCapabilities,
    /// The response selected a capability the client did not offer.
    UnsolicitedCapabilities,
    /// The response nonce does not match the Hello nonce.
    NonceMismatch,
    /// The response lease TTL is not the v1.0 value of 1800 seconds.
    InvalidTtl,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLength => "handshake payload has an invalid length",
            Self::UnsupportedSchema => "handshake schema is unsupported",
            Self::NonzeroReserved => "handshake reserved field must be zero",
            Self::ZeroIdentifier => "handshake identifiers must be nonzero",
            Self::OverlappingCapabilities => "required and optional capabilities overlap",
            Self::UnknownAvailableCapabilities => {
                "server availability contains unknown capabilities"
            }
            Self::UnknownSelectedCapabilities => "server selected unknown capabilities",
            Self::RequiredCapabilitiesUnavailable => "required capabilities are unavailable",
            Self::MissingRequiredCapabilities => "response omitted required capabilities",
            Self::UnsolicitedCapabilities => "response selected unoffered capabilities",
            Self::NonceMismatch => "response nonce does not match Hello",
            Self::InvalidTtl => "handshake lease TTL is invalid",
        };
        f.write_str(message)
    }
}

impl Error for HandshakeError {}

/// Encode a validated Hello payload using the exact v1.0 big-endian layout.
pub fn encode_hello(hello: &Hello) -> Result<[u8; HELLO_PAYLOAD_BYTES], HandshakeError> {
    hello.validate()?;

    let mut bytes = [0_u8; HELLO_PAYLOAD_BYTES];
    write_u16(&mut bytes, 0, HANDSHAKE_SCHEMA_VERSION);
    write_u16(&mut bytes, 2, 0);
    write_u64(&mut bytes, 4, hello.nonce);
    write_u64(&mut bytes, 12, hello.required_capabilities);
    write_u64(&mut bytes, 20, hello.optional_capabilities);
    Ok(bytes)
}

/// Decode exactly one Hello payload, preserving unknown capability bits.
pub fn decode_hello(bytes: &[u8]) -> Result<Hello, HandshakeError> {
    if bytes.len() != HELLO_PAYLOAD_BYTES {
        return Err(HandshakeError::InvalidLength);
    }
    if read_u16(bytes, 0) != HANDSHAKE_SCHEMA_VERSION {
        return Err(HandshakeError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != 0 {
        return Err(HandshakeError::NonzeroReserved);
    }

    let hello = Hello {
        nonce: read_u64(bytes, 4),
        required_capabilities: read_u64(bytes, 12),
        optional_capabilities: read_u64(bytes, 20),
    };
    hello.validate()?;
    Ok(hello)
}

/// Encode a validated Welcome payload using the exact v1.0 big-endian layout.
pub fn encode_welcome(welcome: &Welcome) -> Result<[u8; WELCOME_PAYLOAD_BYTES], HandshakeError> {
    welcome.validate()?;

    let mut bytes = [0_u8; WELCOME_PAYLOAD_BYTES];
    write_u16(&mut bytes, 0, HANDSHAKE_SCHEMA_VERSION);
    write_u16(&mut bytes, 2, 0);
    write_u64(&mut bytes, 4, welcome.nonce);
    write_u64(&mut bytes, 12, welcome.generation);
    write_u64(&mut bytes, 20, welcome.lease);
    write_u64(&mut bytes, 28, welcome.selected_capabilities);
    write_u32(&mut bytes, 36, welcome.ttl_seconds);
    Ok(bytes)
}

/// Decode exactly one Welcome payload and reject unknown selected bits.
pub fn decode_welcome(bytes: &[u8]) -> Result<Welcome, HandshakeError> {
    if bytes.len() != WELCOME_PAYLOAD_BYTES {
        return Err(HandshakeError::InvalidLength);
    }
    if read_u16(bytes, 0) != HANDSHAKE_SCHEMA_VERSION {
        return Err(HandshakeError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != 0 {
        return Err(HandshakeError::NonzeroReserved);
    }

    let welcome = Welcome {
        nonce: read_u64(bytes, 4),
        generation: read_u64(bytes, 12),
        lease: read_u64(bytes, 20),
        selected_capabilities: read_u64(bytes, 28),
        ttl_seconds: read_u32(bytes, 36),
    };
    welcome.validate()?;
    Ok(welcome)
}

/// Select capabilities for a Hello from caller-supplied server availability.
///
/// This function is pure: `available` must already represent capabilities the
/// authenticated runtime actually implements and authorizes. The supplied
/// generation and lease are copied into the response; this function does not
/// create, issue, authenticate, or renew either identifier.
pub fn negotiate(
    hello: &Hello,
    available: u64,
    generation: u64,
    lease: u64,
) -> Result<Welcome, HandshakeError> {
    hello.validate()?;
    if available & !KNOWN_CAPABILITIES != 0 {
        return Err(HandshakeError::UnknownAvailableCapabilities);
    }
    if hello.required_capabilities & !available != 0 {
        return Err(HandshakeError::RequiredCapabilitiesUnavailable);
    }

    let welcome = Welcome {
        nonce: hello.nonce,
        generation,
        lease,
        selected_capabilities: hello.required_capabilities
            | (hello.optional_capabilities & available),
        ttl_seconds: HANDSHAKE_LEASE_TTL_SECONDS,
    };
    welcome.validate()?;
    Ok(welcome)
}

/// Check that a Welcome is valid for the Hello that elicited it.
pub fn validate_welcome(hello: &Hello, welcome: &Welcome) -> Result<(), HandshakeError> {
    hello.validate()?;
    welcome.validate()?;
    if welcome.nonce != hello.nonce {
        return Err(HandshakeError::NonceMismatch);
    }
    if welcome.selected_capabilities & hello.required_capabilities != hello.required_capabilities {
        return Err(HandshakeError::MissingRequiredCapabilities);
    }
    let offered = hello.required_capabilities | hello.optional_capabilities;
    if welcome.selected_capabilities & !offered != 0 {
        return Err(HandshakeError::UnsolicitedCapabilities);
    }
    Ok(())
}

fn validate_hello(hello: Hello) -> Result<(), HandshakeError> {
    if hello.nonce == 0 {
        return Err(HandshakeError::ZeroIdentifier);
    }
    if hello.required_capabilities & hello.optional_capabilities != 0 {
        return Err(HandshakeError::OverlappingCapabilities);
    }
    Ok(())
}

fn validate_welcome_payload(welcome: Welcome) -> Result<(), HandshakeError> {
    if welcome.nonce == 0 || welcome.generation == 0 || welcome.lease == 0 {
        return Err(HandshakeError::ZeroIdentifier);
    }
    if welcome.selected_capabilities & !KNOWN_CAPABILITIES != 0 {
        return Err(HandshakeError::UnknownSelectedCapabilities);
    }
    if welcome.ttl_seconds != HANDSHAKE_LEASE_TTL_SECONDS {
        return Err(HandshakeError::InvalidTtl);
    }
    Ok(())
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

fn write_u16<const N: usize>(bytes: &mut [u8; N], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32<const N: usize>(bytes: &mut [u8; N], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn write_u64<const N: usize>(bytes: &mut [u8; N], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol_wire::{decode_frame, encode_frame, Frame, MessageKind};

    const HELLO_GOLDEN: [u8; HELLO_PAYLOAD_BYTES] = [
        0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06,
    ];

    const WELCOME_GOLDEN: [u8; WELCOME_PAYLOAD_BYTES] = [
        0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x11, 0x12, 0x13,
        0x14, 0x15, 0x16, 0x17, 0x18, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x07, 0x08,
    ];

    fn sample_hello() -> Hello {
        Hello {
            nonce: 0x0102_0304_0506_0708,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT | CAPABILITY_EVENTS,
        }
    }

    fn sample_welcome() -> Welcome {
        Welcome {
            nonce: 0x0102_0304_0506_0708,
            generation: 0x1112_1314_1516_1718,
            lease: 0x2122_2324_2526_2728,
            selected_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
            ttl_seconds: HANDSHAKE_LEASE_TTL_SECONDS,
        }
    }

    #[test]
    fn hello_golden_bytes_are_stable_and_independently_decodable() {
        let hello = sample_hello();
        assert_eq!(encode_hello(&hello), Ok(HELLO_GOLDEN));
        assert_eq!(decode_hello(&HELLO_GOLDEN), Ok(hello));
    }

    #[test]
    fn welcome_golden_bytes_are_stable_and_independently_decodable() {
        let welcome = sample_welcome();
        assert_eq!(encode_welcome(&welcome), Ok(WELCOME_GOLDEN));
        assert_eq!(decode_welcome(&WELCOME_GOLDEN), Ok(welcome));
    }

    #[test]
    fn valid_payloads_round_trip() {
        let hello = Hello {
            nonce: 0x8877_6655_4433_2211,
            required_capabilities: CAPABILITY_INPUT,
            optional_capabilities: CAPABILITY_READ,
        };
        assert_eq!(decode_hello(&encode_hello(&hello).unwrap()), Ok(hello));

        let welcome = Welcome {
            nonce: hello.nonce,
            generation: 42,
            lease: 9,
            selected_capabilities: CAPABILITY_INPUT,
            ttl_seconds: HANDSHAKE_LEASE_TTL_SECONDS,
        };
        assert_eq!(
            decode_welcome(&encode_welcome(&welcome).unwrap()),
            Ok(welcome)
        );
        assert_eq!(validate_welcome(&hello, &welcome), Ok(()));
    }

    #[test]
    fn rejects_every_truncation_and_any_trailing_byte() {
        for length in 0..HELLO_PAYLOAD_BYTES {
            assert_eq!(
                decode_hello(&HELLO_GOLDEN[..length]),
                Err(HandshakeError::InvalidLength)
            );
        }
        let mut long_hello = [0_u8; HELLO_PAYLOAD_BYTES + 1];
        long_hello[..HELLO_PAYLOAD_BYTES].copy_from_slice(&HELLO_GOLDEN);
        assert_eq!(
            decode_hello(&long_hello),
            Err(HandshakeError::InvalidLength)
        );

        for length in 0..WELCOME_PAYLOAD_BYTES {
            assert_eq!(
                decode_welcome(&WELCOME_GOLDEN[..length]),
                Err(HandshakeError::InvalidLength)
            );
        }
        let mut long_welcome = [0_u8; WELCOME_PAYLOAD_BYTES + 1];
        long_welcome[..WELCOME_PAYLOAD_BYTES].copy_from_slice(&WELCOME_GOLDEN);
        assert_eq!(
            decode_welcome(&long_welcome),
            Err(HandshakeError::InvalidLength)
        );
    }

    #[test]
    fn rejects_unsupported_schema_and_nonzero_reserved_fields() {
        let mut hello = HELLO_GOLDEN;
        hello[1] = 2;
        assert_eq!(decode_hello(&hello), Err(HandshakeError::UnsupportedSchema));
        hello = HELLO_GOLDEN;
        hello[3] = 1;
        assert_eq!(decode_hello(&hello), Err(HandshakeError::NonzeroReserved));

        let mut welcome = WELCOME_GOLDEN;
        welcome[1] = 2;
        assert_eq!(
            decode_welcome(&welcome),
            Err(HandshakeError::UnsupportedSchema)
        );
        welcome = WELCOME_GOLDEN;
        welcome[3] = 1;
        assert_eq!(
            decode_welcome(&welcome),
            Err(HandshakeError::NonzeroReserved)
        );
    }

    #[test]
    fn rejects_zero_identifiers_invalid_ttl_and_overlapping_masks() {
        let mut hello = sample_hello();
        hello.nonce = 0;
        assert_eq!(encode_hello(&hello), Err(HandshakeError::ZeroIdentifier));

        hello = sample_hello();
        hello.required_capabilities = CAPABILITY_READ;
        hello.optional_capabilities = CAPABILITY_READ;
        assert_eq!(
            hello.validate(),
            Err(HandshakeError::OverlappingCapabilities)
        );

        let mut welcome = sample_welcome();
        welcome.nonce = 0;
        assert_eq!(welcome.validate(), Err(HandshakeError::ZeroIdentifier));
        welcome = sample_welcome();
        welcome.generation = 0;
        assert_eq!(welcome.validate(), Err(HandshakeError::ZeroIdentifier));
        welcome = sample_welcome();
        welcome.lease = 0;
        assert_eq!(welcome.validate(), Err(HandshakeError::ZeroIdentifier));
        welcome = sample_welcome();
        welcome.ttl_seconds = HANDSHAKE_LEASE_TTL_SECONDS - 1;
        assert_eq!(welcome.validate(), Err(HandshakeError::InvalidTtl));
    }

    #[test]
    fn negotiates_known_available_bits_and_ignores_unknown_optional_bits() {
        let hello = Hello {
            nonce: 1,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT | (1 << 40),
        };
        assert_eq!(decode_hello(&encode_hello(&hello).unwrap()), Ok(hello));
        let welcome = negotiate(&hello, CAPABILITY_READ | CAPABILITY_INPUT, 2, 3)
            .expect("known available capability bits should negotiate");
        assert_eq!(
            welcome.selected_capabilities,
            CAPABILITY_READ | CAPABILITY_INPUT
        );
        assert_eq!(validate_welcome(&hello, &welcome), Ok(()));
    }

    #[test]
    fn scoped_wheel_is_optional_and_does_not_imply_input() {
        let legacy = Hello {
            nonce: 1,
            required_capabilities: 0,
            optional_capabilities: CAPABILITY_SCOPED_MOUSE_WHEEL,
        };
        let selected = negotiate(&legacy, CAPABILITY_READ | CAPABILITY_INPUT, 2, 3).unwrap();
        assert_eq!(selected.selected_capabilities, 0);
        let independent = negotiate(&legacy, CAPABILITY_SCOPED_MOUSE_WHEEL, 2, 3).unwrap();
        assert_eq!(
            independent.selected_capabilities,
            CAPABILITY_SCOPED_MOUSE_WHEEL
        );
        assert_eq!(
            CAPABILITY_NAMES
                .iter()
                .find(|(bit, _)| *bit == CAPABILITY_SCOPED_MOUSE_WHEEL)
                .map(|(_, name)| *name),
            Some("scoped_mouse_wheel")
        );
        assert_ne!(KNOWN_CAPABILITIES & CAPABILITY_SCOPED_MOUSE_WHEEL, 0);
    }

    #[test]
    fn rejects_unknown_server_availability_and_unavailable_required_bits() {
        let hello = Hello {
            nonce: 1,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: 0,
        };
        assert_eq!(
            negotiate(&hello, CAPABILITY_READ | (1 << 40), 2, 3),
            Err(HandshakeError::UnknownAvailableCapabilities)
        );
        assert_eq!(
            negotiate(&hello, 0, 2, 3),
            Err(HandshakeError::RequiredCapabilitiesUnavailable)
        );

        let unknown_required = Hello {
            nonce: 1,
            required_capabilities: 1 << 40,
            optional_capabilities: 0,
        };
        assert_eq!(unknown_required.validate(), Ok(()));
        assert_eq!(
            negotiate(&unknown_required, KNOWN_CAPABILITIES, 2, 3),
            Err(HandshakeError::RequiredCapabilitiesUnavailable)
        );
    }

    #[test]
    fn rejects_unknown_selected_capabilities_and_allows_zero_selection() {
        let mut welcome = sample_welcome();
        welcome.selected_capabilities = 1 << 40;
        assert_eq!(
            welcome.validate(),
            Err(HandshakeError::UnknownSelectedCapabilities)
        );

        let mut hello = sample_hello();
        hello.optional_capabilities = 0;
        let zero = Welcome {
            selected_capabilities: 0,
            ..sample_welcome()
        };
        assert_eq!(
            validate_welcome(&hello, &zero),
            Err(HandshakeError::MissingRequiredCapabilities)
        );

        hello.required_capabilities = 0;
        assert_eq!(validate_welcome(&hello, &zero), Ok(()));
    }

    #[test]
    fn validates_required_bits_offers_nonce_and_no_unsolicited_selection() {
        let hello = Hello {
            nonce: 10,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT,
        };
        let mut welcome = Welcome {
            nonce: 10,
            generation: 20,
            lease: 30,
            selected_capabilities: CAPABILITY_INPUT,
            ttl_seconds: HANDSHAKE_LEASE_TTL_SECONDS,
        };
        assert_eq!(
            validate_welcome(&hello, &welcome),
            Err(HandshakeError::MissingRequiredCapabilities)
        );

        welcome.selected_capabilities = CAPABILITY_READ | CAPABILITY_EVENTS;
        assert_eq!(
            validate_welcome(&hello, &welcome),
            Err(HandshakeError::UnsolicitedCapabilities)
        );

        welcome.selected_capabilities = CAPABILITY_READ;
        welcome.nonce = 11;
        assert_eq!(
            validate_welcome(&hello, &welcome),
            Err(HandshakeError::NonceMismatch)
        );
    }

    #[test]
    fn redacts_nonce_and_server_identifiers_from_debug_output() {
        let hello = Hello {
            nonce: 8_765_432_109,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: 0,
        };
        let welcome = Welcome {
            nonce: hello.nonce,
            generation: 7_654_321_098,
            lease: 6_543_210_987,
            selected_capabilities: CAPABILITY_READ,
            ttl_seconds: HANDSHAKE_LEASE_TTL_SECONDS,
        };
        let hello_debug = format!("{hello:?}");
        let welcome_debug = format!("{welcome:?}");
        assert!(!hello_debug.contains("8765432109"));
        assert!(!welcome_debug.contains("8765432109"));
        assert!(!welcome_debug.contains("7654321098"));
        assert!(!welcome_debug.contains("6543210987"));
    }

    #[test]
    fn handshake_payloads_use_the_existing_hello_and_welcome_envelope_kinds() {
        let hello_payload = encode_hello(&sample_hello()).unwrap();
        let hello_frame = Frame::new(MessageKind::Hello, 0, &hello_payload).unwrap();
        let mut hello_wire = Vec::new();
        encode_frame(&hello_frame, &mut hello_wire).unwrap();
        let decoded_hello_frame = decode_frame(&hello_wire).unwrap();
        assert_eq!(decoded_hello_frame.header().kind(), MessageKind::Hello);
        assert_eq!(
            decode_hello(decoded_hello_frame.payload()),
            Ok(sample_hello())
        );

        let welcome_payload = encode_welcome(&sample_welcome()).unwrap();
        let welcome_frame = Frame::new(MessageKind::Welcome, 0, &welcome_payload).unwrap();
        let mut welcome_wire = Vec::new();
        encode_frame(&welcome_frame, &mut welcome_wire).unwrap();
        let decoded_welcome_frame = decode_frame(&welcome_wire).unwrap();
        assert_eq!(decoded_welcome_frame.header().kind(), MessageKind::Welcome);
        assert_eq!(
            decode_welcome(decoded_welcome_frame.payload()),
            Ok(sample_welcome())
        );
    }

    #[test]
    fn decoded_fields_obey_semantic_constraints_before_becoming_usable() {
        let mut hello = HELLO_GOLDEN;
        hello[4..12].fill(0);
        assert_eq!(decode_hello(&hello), Err(HandshakeError::ZeroIdentifier));
        hello = HELLO_GOLDEN;
        hello[20..28].copy_from_slice(&CAPABILITY_READ.to_be_bytes());
        assert_eq!(
            decode_hello(&hello),
            Err(HandshakeError::OverlappingCapabilities)
        );
        for offset in [4, 12, 20] {
            let mut welcome = WELCOME_GOLDEN;
            welcome[offset..offset + 8].fill(0);
            assert_eq!(
                decode_welcome(&welcome),
                Err(HandshakeError::ZeroIdentifier)
            );
        }
        let mut welcome = WELCOME_GOLDEN;
        welcome[36..40].fill(0);
        assert_eq!(decode_welcome(&welcome), Err(HandshakeError::InvalidTtl));
        welcome = WELCOME_GOLDEN;
        welcome[28..36].copy_from_slice(&(1_u64 << 63).to_be_bytes());
        assert_eq!(
            decode_welcome(&welcome),
            Err(HandshakeError::UnknownSelectedCapabilities)
        );
    }

    #[test]
    fn advertised_lease_duration_matches_ledger_expiry_policy() {
        assert_eq!(
            u64::from(HANDSHAKE_LEASE_TTL_SECONDS),
            crate::protocol::LEASE_TTL_SECS
        );
    }
}
