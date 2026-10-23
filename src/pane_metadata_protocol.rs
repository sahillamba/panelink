//! Strict, bounded codec for live managed-child pane metadata (schema 12).
//!
//! This module does no process I/O or authorization. Paths are sensitive and
//! are deliberately redacted from `Debug` output.

use crate::control_protocol::TargetRef;
use std::error::Error;
use std::fmt;

/// Metadata wire schema.
pub const PANE_METADATA_SCHEMA: u16 = 12;
/// Optional capability bit advertised by supporting multi-session servers.
pub use crate::handshake_protocol::CAPABILITY_PANE_METADATA;
/// Exact request size.
pub const PANE_METADATA_QUERY_BYTES: usize = 48;
/// Reply prefix before the two path byte strings.
pub const PANE_METADATA_REPLY_PREFIX_BYTES: usize = 72;
/// Maximum UTF-8 bytes in either path.
pub const MAX_PATH_BYTES: usize = 4095;
/// Maximum complete observation reply.
pub const MAX_PANE_METADATA_REPLY_BYTES: usize =
    PANE_METADATA_REPLY_PREFIX_BYTES + MAX_PATH_BYTES * 2;

const KIND_QUERY: u16 = 1;
const KIND_REPLY: u16 = 0x8001;
const STATUS_OFFSET: usize = 56;

/// Data-free codec errors; no error includes path or target contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataError {
    /// Payload is shorter than its fixed or declared size.
    Truncated,
    /// Payload contains bytes after its declared fields.
    TrailingBytes,
    /// Payload exceeds the protocol's maximum size.
    PayloadTooLarge,
    /// Schema value is unsupported.
    UnsupportedSchema,
    /// Request/reply kind is incorrect.
    InvalidKind,
    /// A reserved byte is nonzero.
    NonzeroReserved,
    /// A target identity component is zero.
    InvalidTarget,
    /// A status code is unknown or inconsistent with its path length.
    InvalidStatus,
    /// Available path is empty, non-absolute, contains forbidden characters,
    /// or exceeds the byte bound.
    InvalidPath,
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "pane metadata payload is truncated",
            Self::TrailingBytes => "pane metadata payload has trailing bytes",
            Self::PayloadTooLarge => "pane metadata payload exceeds its limit",
            Self::UnsupportedSchema => "pane metadata schema is unsupported",
            Self::InvalidKind => "pane metadata message kind is invalid",
            Self::NonzeroReserved => "pane metadata reserved bytes are nonzero",
            Self::InvalidTarget => "pane metadata target is invalid",
            Self::InvalidStatus => "pane metadata status is invalid",
            Self::InvalidPath => "pane metadata path is invalid",
        })
    }
}

impl Error for MetadataError {}

/// Path observation state. Values are intentionally excluded from Debug.
#[derive(Clone, Eq, PartialEq)]
pub struct PathObservation {
    /// Whether the query supplied a usable path.
    pub status: PathStatus,
    /// Absolute UTF-8 path when status is [`PathStatus::Available`].
    pub value: Option<String>,
}

impl fmt::Debug for PathObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathObservation")
            .field("status", &self.status)
            .field("value", &self.value.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Fixed status codes for one independently sampled path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PathStatus {
    /// Path is present and valid.
    Available = 1,
    /// Managed child has exited.
    Exited = 2,
    /// OS denied process information.
    PermissionDenied = 3,
    /// Process disappeared during the query.
    ProcessGone = 4,
    /// Path was invalid, non-UTF-8, or outside the contract.
    InvalidPath = 5,
    /// Native query failed.
    QueryFailed = 6,
    /// Owner-side query deadline elapsed.
    TimedOut = 7,
    /// Platform does not implement this query.
    Unsupported = 8,
    /// PID/start identity did not match the managed child.
    IdentityMismatch = 9,
    /// A prior native query still occupies the bounded query slot.
    Busy = 10,
}

impl PathStatus {
    fn from_byte(value: u8) -> Result<Self, MetadataError> {
        match value {
            1 => Ok(Self::Available),
            2 => Ok(Self::Exited),
            3 => Ok(Self::PermissionDenied),
            4 => Ok(Self::ProcessGone),
            5 => Ok(Self::InvalidPath),
            6 => Ok(Self::QueryFailed),
            7 => Ok(Self::TimedOut),
            8 => Ok(Self::Unsupported),
            9 => Ok(Self::IdentityMismatch),
            10 => Ok(Self::Busy),
            _ => Err(MetadataError::InvalidStatus),
        }
    }
}

impl PathObservation {
    /// Construct a state without a path; `Available` is not permitted.
    pub fn unavailable(status: PathStatus) -> Self {
        assert!(status != PathStatus::Available, "available requires a path");
        Self {
            status,
            value: None,
        }
    }

    /// Validate and construct an available absolute path.
    pub fn available(value: String) -> Result<Self, MetadataError> {
        validate_path(&value)?;
        Ok(Self {
            status: PathStatus::Available,
            value: Some(value),
        })
    }

    fn validate(&self) -> Result<(), MetadataError> {
        match (self.status, self.value.as_deref()) {
            (PathStatus::Available, Some(path)) => validate_path(path),
            (PathStatus::Available, None) | (_, Some(_)) => Err(MetadataError::InvalidStatus),
            (_, None) => Ok(()),
        }
    }
}

/// One bounded, point-in-time observation of the managed child.
#[derive(Clone, Eq, PartialEq)]
pub struct Observation {
    /// Exact requested pane occupant.
    pub target: TargetRef,
    /// Server-generation-relative monotonic milliseconds.
    pub sampled_at_mono_ms: u64,
    /// Managed child's cwd.
    pub cwd: PathObservation,
    /// Managed child's current executable image path.
    pub executable: PathObservation,
}

impl fmt::Debug for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observation")
            .field("target", &"<redacted>")
            .field("sampled_at_mono_ms", &self.sampled_at_mono_ms)
            .field("cwd", &self.cwd)
            .field("executable", &self.executable)
            .finish()
    }
}

impl Observation {
    /// Construct an observation where both paths share an unavailable state.
    pub fn unavailable(target: TargetRef, sampled_at_mono_ms: u64, status: PathStatus) -> Self {
        Self {
            target,
            sampled_at_mono_ms,
            cwd: PathObservation::unavailable(status),
            executable: PathObservation::unavailable(status),
        }
    }

    fn validate(&self) -> Result<(), MetadataError> {
        validate_target(self.target)?;
        self.cwd.validate()?;
        self.executable.validate()?;
        Ok(())
    }
}

/// Encode a schema-12 query naming one complete target.
pub fn encode_query(target: TargetRef) -> Result<Vec<u8>, MetadataError> {
    validate_target(target)?;
    let mut bytes = vec![0; PANE_METADATA_QUERY_BYTES];
    put_u16(&mut bytes, 0, PANE_METADATA_SCHEMA);
    put_u16(&mut bytes, 2, KIND_QUERY);
    put_target(&mut bytes, 8, target);
    Ok(bytes)
}

/// Decode one exact schema-12 query and return its complete target.
pub fn decode_query(bytes: &[u8]) -> Result<TargetRef, MetadataError> {
    if bytes.len() < PANE_METADATA_QUERY_BYTES {
        return Err(MetadataError::Truncated);
    }
    if bytes.len() > PANE_METADATA_QUERY_BYTES {
        return Err(MetadataError::TrailingBytes);
    }
    validate_header(bytes, KIND_QUERY)?;
    validate_zero(&bytes[4..8])?;
    let target = read_target(bytes, 8);
    validate_target(target)?;
    Ok(target)
}

/// Validate and encode one schema-12 observation.
pub fn encode_observation(observation: &Observation) -> Result<Vec<u8>, MetadataError> {
    observation.validate()?;
    let cwd = observation.cwd.value.as_deref().unwrap_or("").as_bytes();
    let executable = observation
        .executable
        .value
        .as_deref()
        .unwrap_or("")
        .as_bytes();
    let total = PANE_METADATA_REPLY_PREFIX_BYTES + cwd.len() + executable.len();
    if total > MAX_PANE_METADATA_REPLY_BYTES {
        return Err(MetadataError::PayloadTooLarge);
    }
    let mut bytes = vec![0; total];
    put_u16(&mut bytes, 0, PANE_METADATA_SCHEMA);
    put_u16(&mut bytes, 2, KIND_REPLY);
    put_target(&mut bytes, 8, observation.target);
    put_u64(&mut bytes, 48, observation.sampled_at_mono_ms);
    bytes[STATUS_OFFSET] = observation.cwd.status as u8;
    bytes[STATUS_OFFSET + 1] = observation.executable.status as u8;
    put_u16(&mut bytes, 60, cwd.len() as u16);
    put_u16(&mut bytes, 62, executable.len() as u16);
    bytes[PANE_METADATA_REPLY_PREFIX_BYTES..PANE_METADATA_REPLY_PREFIX_BYTES + cwd.len()]
        .copy_from_slice(cwd);
    bytes[PANE_METADATA_REPLY_PREFIX_BYTES + cwd.len()..].copy_from_slice(executable);
    Ok(bytes)
}

/// Decode and strictly validate one complete schema-12 observation.
pub fn decode_observation(bytes: &[u8]) -> Result<Observation, MetadataError> {
    if bytes.len() < PANE_METADATA_REPLY_PREFIX_BYTES {
        return Err(MetadataError::Truncated);
    }
    if bytes.len() > MAX_PANE_METADATA_REPLY_BYTES {
        return Err(MetadataError::PayloadTooLarge);
    }
    validate_header(bytes, KIND_REPLY)?;
    validate_zero(&bytes[4..8])?;
    validate_zero(&bytes[58..60])?;
    validate_zero(&bytes[64..72])?;
    let target = read_target(bytes, 8);
    validate_target(target)?;
    let cwd_status = PathStatus::from_byte(bytes[56])?;
    let executable_status = PathStatus::from_byte(bytes[57])?;
    let cwd_len = usize::from(read_u16(bytes, 60));
    let executable_len = usize::from(read_u16(bytes, 62));
    if cwd_len > MAX_PATH_BYTES || executable_len > MAX_PATH_BYTES {
        return Err(MetadataError::PayloadTooLarge);
    }
    let expected = PANE_METADATA_REPLY_PREFIX_BYTES + cwd_len + executable_len;
    if bytes.len() < expected {
        return Err(MetadataError::Truncated);
    }
    if bytes.len() > expected {
        return Err(MetadataError::TrailingBytes);
    }
    let cwd = decode_path(cwd_status, &bytes[72..72 + cwd_len])?;
    let executable = decode_path(executable_status, &bytes[72 + cwd_len..])?;
    let observation = Observation {
        target,
        sampled_at_mono_ms: read_u64(bytes, 48),
        cwd,
        executable,
    };
    observation.validate()?;
    Ok(observation)
}

fn decode_path(status: PathStatus, bytes: &[u8]) -> Result<PathObservation, MetadataError> {
    match status {
        PathStatus::Available => {
            if bytes.is_empty() {
                return Err(MetadataError::InvalidStatus);
            }
            let value = std::str::from_utf8(bytes).map_err(|_| MetadataError::InvalidPath)?;
            PathObservation::available(value.to_owned())
        }
        _ if bytes.is_empty() => Ok(PathObservation::unavailable(status)),
        _ => Err(MetadataError::InvalidStatus),
    }
}

fn validate_path(value: &str) -> Result<(), MetadataError> {
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || !value.starts_with('/')
        || value.as_bytes().contains(&0)
        || value.chars().any(char::is_control)
    {
        return Err(MetadataError::InvalidPath);
    }
    Ok(())
}

fn validate_target(target: TargetRef) -> Result<(), MetadataError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        Err(MetadataError::InvalidTarget)
    } else {
        Ok(())
    }
}

fn validate_header(bytes: &[u8], kind: u16) -> Result<(), MetadataError> {
    if read_u16(bytes, 0) != PANE_METADATA_SCHEMA {
        return Err(MetadataError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != kind {
        return Err(MetadataError::InvalidKind);
    }
    Ok(())
}

fn validate_zero(bytes: &[u8]) -> Result<(), MetadataError> {
    if bytes.iter().any(|byte| *byte != 0) {
        Err(MetadataError::NonzeroReserved)
    } else {
        Ok(())
    }
}

fn put_target(bytes: &mut [u8], offset: usize, target: TargetRef) {
    for (index, value) in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .into_iter()
    .enumerate()
    {
        put_u64(bytes, offset + index * 8, value);
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
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}
fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("fixed field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        }
    }

    fn sample() -> Observation {
        Observation {
            target: target(),
            sampled_at_mono_ms: 42,
            cwd: PathObservation::available("/private/work".into()).unwrap(),
            executable: PathObservation::available("/bin/zsh".into()).unwrap(),
        }
    }

    #[test]
    fn query_round_trip_and_strict_header_and_target_validation() {
        let encoded = encode_query(target()).unwrap();
        assert_eq!(encoded.len(), PANE_METADATA_QUERY_BYTES);
        assert_eq!(decode_query(&encoded), Ok(target()));
        assert_eq!(decode_query(&encoded[..47]), Err(MetadataError::Truncated));
        let mut bad = encoded.clone();
        bad.push(0);
        assert_eq!(decode_query(&bad), Err(MetadataError::TrailingBytes));
        let mut bad = encoded.clone();
        bad[4] = 1;
        assert_eq!(decode_query(&bad), Err(MetadataError::NonzeroReserved));
        let mut bad = encoded;
        bad[8..16].fill(0);
        assert_eq!(decode_query(&bad), Err(MetadataError::InvalidTarget));
    }

    #[test]
    fn observation_round_trip_and_unavailable_contract() {
        let value = sample();
        let encoded = encode_observation(&value).unwrap();
        assert_eq!(encoded.len(), 72 + "/private/work".len() + "/bin/zsh".len());
        assert_eq!(decode_observation(&encoded), Ok(value));
        let unavailable = Observation::unavailable(target(), 0, PathStatus::Busy);
        assert_eq!(
            decode_observation(&encode_observation(&unavailable).unwrap()),
            Ok(unavailable)
        );
        assert_eq!(
            encode_observation(&Observation::unavailable(target(), 1, PathStatus::Busy))
                .unwrap()
                .len(),
            72
        );
    }

    #[test]
    fn malformed_reply_lengths_reserved_status_utf8_and_status_value_fail_closed() {
        let encoded = encode_observation(&sample()).unwrap();
        assert_eq!(
            decode_observation(&encoded[..71]),
            Err(MetadataError::Truncated)
        );
        let mut bad = encoded.clone();
        bad[58] = 1;
        assert_eq!(
            decode_observation(&bad),
            Err(MetadataError::NonzeroReserved)
        );
        let mut bad = encoded.clone();
        bad[56] = 99;
        assert_eq!(decode_observation(&bad), Err(MetadataError::InvalidStatus));
        let mut bad = encoded.clone();
        bad[60..62].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(decode_observation(&bad), Err(MetadataError::TrailingBytes));
        bad.truncate(72 + "/bin/zsh".len());
        assert_eq!(decode_observation(&bad), Err(MetadataError::InvalidStatus));
        let mut bad = encoded.clone();
        bad[72] = 0xff;
        assert_eq!(decode_observation(&bad), Err(MetadataError::InvalidPath));
        let mut bad = encoded;
        bad.push(0);
        assert_eq!(decode_observation(&bad), Err(MetadataError::TrailingBytes));
    }

    #[test]
    fn path_constructor_rejects_relative_empty_nul_controls_and_oversize() {
        for path in [
            "",
            "relative",
            "/has\nnewline",
            "/has\u{7f}del",
            "/has\0nul",
        ] {
            assert_eq!(
                PathObservation::available(path.into()),
                Err(MetadataError::InvalidPath)
            );
        }
        assert_eq!(
            PathObservation::available(format!("/{}", "x".repeat(MAX_PATH_BYTES))),
            Err(MetadataError::InvalidPath)
        );
        assert_eq!(
            PathObservation::available(format!("/{}", "x".repeat(MAX_PATH_BYTES - 1)))
                .unwrap()
                .value
                .unwrap()
                .len(),
            MAX_PATH_BYTES
        );
    }

    #[test]
    fn inconsistent_status_lengths_and_sensitive_debug_are_rejected_or_redacted() {
        let mut observation = sample();
        observation.cwd.status = PathStatus::Busy;
        assert_eq!(
            encode_observation(&observation),
            Err(MetadataError::InvalidStatus)
        );
        let debug = format!("{:?}", sample());
        assert!(!debug.contains("/private/work"));
        assert!(!debug.contains("/bin/zsh"));
        assert!(debug.contains("<redacted>"));
        assert!(!format!("{:?}", MetadataError::InvalidPath).contains("/private"));
    }
}
