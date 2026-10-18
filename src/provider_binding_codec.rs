//! Bounded network-endian codec for metadata-only provider binding records.
//!
//! The codec preserves the exact validated identity bytes. It does not provide
//! storage, atomic replacement, provenance, or recovery guarantees.

use crate::control_protocol::TargetRef;
use crate::provider_binding::{
    Binding, BindingError, ProjectIdentity, SupportedVersion, VerificationSource,
    MAX_PROJECT_IDENTITY_BYTES, MAX_PROVIDER_SESSION_ID_BYTES, MAX_PROVIDER_VERSION_BYTES,
};
use std::error::Error;
use std::fmt;
use std::str;

/// Fixed-size record header, including sixteen zeroed reserved bytes.
pub const BINDING_RECORD_HEADER_BYTES: usize = 96;

/// Maximum record size for the fixed header and all bounded identity fields.
pub const MAX_BINDING_RECORD_BYTES: usize = BINDING_RECORD_HEADER_BYTES
    + MAX_PROVIDER_VERSION_BYTES
    + MAX_PROJECT_IDENTITY_BYTES
    + MAX_PROVIDER_SESSION_ID_BYTES;

const MAGIC: &[u8; 8] = b"PLBIND01";
const TARGET_OFFSET: usize = 8;
const BRIDGE_OFFSET: usize = 48;
const REVISION_OFFSET: usize = 56;
const VERIFIED_AT_OFFSET: usize = 64;
const SOURCE_OFFSET: usize = 72;
const VERSION_LENGTH_OFFSET: usize = 74;
const PROJECT_LENGTH_OFFSET: usize = 76;
const SESSION_LENGTH_OFFSET: usize = 78;
const RESERVED_OFFSET: usize = 80;
const RESERVED_BYTES: usize = 16;

/// Fixed codec errors. No variant contains record bytes or identity values.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum BindingCodecError {
    /// The input is shorter than the fixed header.
    RecordTooShort,
    /// The input exceeds the maximum bounded record size.
    RecordTooLarge,
    /// The record header, fields, lengths, or validated binding are malformed.
    InvalidRecord,
}

impl fmt::Debug for BindingCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingCodecError { <redacted> }")
    }
}

impl fmt::Display for BindingCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::RecordTooShort => "provider binding record is too short",
            Self::RecordTooLarge => "provider binding record is too large",
            Self::InvalidRecord => "invalid provider binding record",
        };
        f.write_str(message)
    }
}

impl Error for BindingCodecError {}

/// Encode one validated binding into its bounded metadata record.
///
/// The complete binding is revalidated before allocating the output vector.
/// The record contains target IDs, bridge/revision/time, source, provider
/// version, opaque project identity, and top-level session ID only.
pub fn encode(binding: &Binding) -> Result<Vec<u8>, BindingCodecError> {
    let target = binding.target();
    let target_fields = [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ];
    let version = binding.supported_version().as_bytes();
    let project = binding.project_identity();
    let session = binding.session_id().as_bytes();

    if target_fields.contains(&0)
        || binding.bridge_instance() == 0
        || binding.revision() == 0
        || binding.verified_at_unix_ms() == 0
        || !valid_version(version)
        || !valid_project(project)
        || !valid_session(session)
    {
        return Err(BindingCodecError::InvalidRecord);
    }

    let version_len = u16::try_from(version.len()).map_err(|_| BindingCodecError::InvalidRecord)?;
    let project_len = u16::try_from(project.len()).map_err(|_| BindingCodecError::InvalidRecord)?;
    let session_len = u16::try_from(session.len()).map_err(|_| BindingCodecError::InvalidRecord)?;
    let total_len = BINDING_RECORD_HEADER_BYTES
        .checked_add(version.len())
        .and_then(|length| length.checked_add(project.len()))
        .and_then(|length| length.checked_add(session.len()))
        .filter(|length| *length <= MAX_BINDING_RECORD_BYTES)
        .ok_or(BindingCodecError::InvalidRecord)?;

    // Every input field and the exact total size are validated before allocation.
    let mut record = Vec::with_capacity(total_len);
    record.extend_from_slice(MAGIC);
    for field in target_fields {
        record.extend_from_slice(&field.to_be_bytes());
    }
    record.extend_from_slice(&binding.bridge_instance().to_be_bytes());
    record.extend_from_slice(&binding.revision().to_be_bytes());
    record.extend_from_slice(&binding.verified_at_unix_ms().to_be_bytes());
    record.extend_from_slice(&source_tag(binding.source()).to_be_bytes());
    record.extend_from_slice(&version_len.to_be_bytes());
    record.extend_from_slice(&project_len.to_be_bytes());
    record.extend_from_slice(&session_len.to_be_bytes());
    record.extend_from_slice(&[0; RESERVED_BYTES]);
    record.extend_from_slice(version);
    record.extend_from_slice(project);
    record.extend_from_slice(session);
    debug_assert_eq!(record.len(), total_len);
    Ok(record)
}

/// Decode exactly one validated binding record.
///
/// The total input bound and exact length are checked before allocating or
/// copying any field. The result is a `Binding`, not a last-confirmed restore
/// record; storage integration must wrap only its durable last-confirmed slot
/// and restoration still begins stale.
pub fn decode(record: &[u8]) -> Result<Binding, BindingCodecError> {
    if record.len() > MAX_BINDING_RECORD_BYTES {
        return Err(BindingCodecError::RecordTooLarge);
    }
    if record.len() < BINDING_RECORD_HEADER_BYTES {
        return Err(BindingCodecError::RecordTooShort);
    }
    if &record[..MAGIC.len()] != MAGIC
        || record[RESERVED_OFFSET..RESERVED_OFFSET + RESERVED_BYTES]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(BindingCodecError::InvalidRecord);
    }

    let version_len = read_u16(record, VERSION_LENGTH_OFFSET)? as usize;
    let project_len = read_u16(record, PROJECT_LENGTH_OFFSET)? as usize;
    let session_len = read_u16(record, SESSION_LENGTH_OFFSET)? as usize;
    if version_len == 0
        || version_len > MAX_PROVIDER_VERSION_BYTES
        || project_len == 0
        || project_len > MAX_PROJECT_IDENTITY_BYTES
        || session_len == 0
        || session_len > MAX_PROVIDER_SESSION_ID_BYTES
    {
        return Err(BindingCodecError::InvalidRecord);
    }

    let expected_len = BINDING_RECORD_HEADER_BYTES
        .checked_add(version_len)
        .and_then(|length| length.checked_add(project_len))
        .and_then(|length| length.checked_add(session_len))
        .ok_or(BindingCodecError::InvalidRecord)?;
    if expected_len != record.len() {
        return Err(BindingCodecError::InvalidRecord);
    }

    let target = TargetRef {
        generation: read_u64(record, TARGET_OFFSET)?,
        workspace: read_u64(record, TARGET_OFFSET + 8)?,
        pane: read_u64(record, TARGET_OFFSET + 16)?,
        session: read_u64(record, TARGET_OFFSET + 24)?,
        occupant_revision: read_u64(record, TARGET_OFFSET + 32)?,
    };
    let bridge_instance = read_u64(record, BRIDGE_OFFSET)?;
    let revision = read_u64(record, REVISION_OFFSET)?;
    let verified_at_unix_ms = read_u64(record, VERIFIED_AT_OFFSET)?;
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
        bridge_instance,
        revision,
        verified_at_unix_ms,
    ]
    .contains(&0)
    {
        return Err(BindingCodecError::InvalidRecord);
    }
    let source =
        parse_source(read_u16(record, SOURCE_OFFSET)?).ok_or(BindingCodecError::InvalidRecord)?;

    let version_start = BINDING_RECORD_HEADER_BYTES;
    let project_start = version_start + version_len;
    let session_start = project_start + project_len;
    let version_bytes = &record[version_start..project_start];
    let project_bytes = &record[project_start..session_start];
    let session_bytes = &record[session_start..expected_len];
    if !valid_version(version_bytes)
        || !valid_project(project_bytes)
        || !valid_session(session_bytes)
    {
        return Err(BindingCodecError::InvalidRecord);
    }

    // All framing, bounds, and field syntax checks above are allocation-free.
    let version = str::from_utf8(version_bytes).map_err(|_| BindingCodecError::InvalidRecord)?;
    let session = str::from_utf8(session_bytes).map_err(|_| BindingCodecError::InvalidRecord)?;
    let project = ProjectIdentity::new(project_bytes).map_err(map_binding_error)?;
    let version = SupportedVersion::new(version).map_err(map_binding_error)?;
    Binding::new(
        target,
        &project,
        &version,
        bridge_instance,
        revision,
        session,
        verified_at_unix_ms,
        source,
    )
    .map_err(map_binding_error)
}

fn valid_version(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= MAX_PROVIDER_VERSION_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'+' | b'-'))
}

fn valid_project(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= MAX_PROJECT_IDENTITY_BYTES && !bytes.contains(&0)
}

fn valid_session(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= MAX_PROVIDER_SESSION_ID_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.' | b'-'))
}

const fn source_tag(source: VerificationSource) -> u16 {
    match source {
        VerificationSource::TuiRouteSnapshot => 1,
        VerificationSource::OrderedSelection => 2,
    }
}

const fn parse_source(tag: u16) -> Option<VerificationSource> {
    match tag {
        1 => Some(VerificationSource::TuiRouteSnapshot),
        2 => Some(VerificationSource::OrderedSelection),
        _ => None,
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BindingCodecError> {
    let field = bytes
        .get(offset..offset + 2)
        .ok_or(BindingCodecError::InvalidRecord)?;
    Ok(u16::from_be_bytes(
        field
            .try_into()
            .map_err(|_| BindingCodecError::InvalidRecord)?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, BindingCodecError> {
    let field = bytes
        .get(offset..offset + 8)
        .ok_or(BindingCodecError::InvalidRecord)?;
    Ok(u64::from_be_bytes(
        field
            .try_into()
            .map_err(|_| BindingCodecError::InvalidRecord)?,
    ))
}

fn map_binding_error(_: BindingError) -> BindingCodecError {
    BindingCodecError::InvalidRecord
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::LastConfirmedRecord;

    fn minimal_binding() -> Binding {
        let project = ProjectIdentity::new(b"p").unwrap();
        let version = SupportedVersion::new("v").unwrap();
        Binding::new(
            TargetRef {
                generation: 1,
                workspace: 2,
                pane: 3,
                session: 4,
                occupant_revision: 5,
            },
            &project,
            &version,
            6,
            7,
            "s",
            8,
            VerificationSource::TuiRouteSnapshot,
        )
        .unwrap()
    }

    fn golden_record() -> Vec<u8> {
        let mut record = vec![
            // Magic.
            0x50, 0x4c, 0x42, 0x49, 0x4e, 0x44, 0x30, 0x31,
            // TargetRef: generation 1, workspace 2, pane 3, session 4,
            // occupant revision 5.
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0,
            0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5, // Bridge 6, revision 7, verification time 8.
            0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 8,
            // OrderedSelection, then version/project/session lengths 6/4/4.
            0, 2, 0, 6, 0, 4, 0, 4, // Sixteen reserved bytes.
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(record.len(), 96);
        record.extend_from_slice(b"v1.2.3");
        record.extend_from_slice(b"\xfffoo");
        record.extend_from_slice(b"s_01");
        record
    }

    fn set_u16(record: &mut [u8], offset: usize, value: u16) {
        record[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn golden_record_proves_network_order_offsets_and_body_order() {
        let golden = golden_record();
        assert_eq!(&golden[0..8], b"PLBIND01");
        assert_eq!(&golden[8..16], &[0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&golden[16..24], &[0, 0, 0, 0, 0, 0, 0, 2]);
        assert_eq!(&golden[24..32], &[0, 0, 0, 0, 0, 0, 0, 3]);
        assert_eq!(&golden[32..40], &[0, 0, 0, 0, 0, 0, 0, 4]);
        assert_eq!(&golden[40..48], &[0, 0, 0, 0, 0, 0, 0, 5]);
        assert_eq!(&golden[48..56], &[0, 0, 0, 0, 0, 0, 0, 6]);
        assert_eq!(&golden[56..64], &[0, 0, 0, 0, 0, 0, 0, 7]);
        assert_eq!(&golden[64..72], &[0, 0, 0, 0, 0, 0, 0, 8]);
        assert_eq!(&golden[72..80], &[0, 2, 0, 6, 0, 4, 0, 4]);
        assert!(golden[80..96].iter().all(|byte| *byte == 0));
        assert_eq!(&golden[96..102], b"v1.2.3");
        assert_eq!(&golden[102..106], b"\xfffoo");
        assert_eq!(&golden[106..110], b"s_01");

        let decoded = decode(&golden).unwrap();
        assert_eq!(decoded.target().generation, 1);
        assert_eq!(decoded.target().workspace, 2);
        assert_eq!(decoded.target().pane, 3);
        assert_eq!(decoded.target().session, 4);
        assert_eq!(decoded.target().occupant_revision, 5);
        assert_eq!(decoded.bridge_instance(), 6);
        assert_eq!(decoded.revision(), 7);
        assert_eq!(decoded.verified_at_unix_ms(), 8);
        assert_eq!(decoded.source(), VerificationSource::OrderedSelection);
        assert_eq!(decoded.supported_version(), "v1.2.3");
        assert_eq!(decoded.project_identity(), b"\xfffoo");
        assert_eq!(decoded.session_id(), "s_01");
        assert_eq!(encode(&decoded).unwrap(), golden);
    }

    #[test]
    fn minimum_and_maximum_field_lengths_are_supported() {
        let min = minimal_binding();
        let encoded_min = encode(&min).unwrap();
        assert_eq!(encoded_min.len(), BINDING_RECORD_HEADER_BYTES + 3);
        assert_eq!(decode(&encoded_min).unwrap(), min);

        let project = ProjectIdentity::new(&vec![0xfe; MAX_PROJECT_IDENTITY_BYTES]).unwrap();
        let version = SupportedVersion::new(&"v".repeat(MAX_PROVIDER_VERSION_BYTES)).unwrap();
        let session_id = "s".repeat(MAX_PROVIDER_SESSION_ID_BYTES);
        let max = Binding::new(
            TargetRef {
                generation: u64::MAX,
                workspace: u64::MAX,
                pane: u64::MAX,
                session: u64::MAX,
                occupant_revision: u64::MAX,
            },
            &project,
            &version,
            u64::MAX,
            u64::MAX,
            &session_id,
            u64::MAX,
            VerificationSource::TuiRouteSnapshot,
        )
        .unwrap();
        let encoded_max = encode(&max).unwrap();
        assert_eq!(encoded_max.len(), MAX_BINDING_RECORD_BYTES);
        assert_eq!(decode(&encoded_max).unwrap(), max);
        assert_eq!(u16::from_be_bytes([encoded_max[72], encoded_max[73]]), 1);
    }

    #[test]
    fn every_truncation_and_trailing_bytes_are_rejected() {
        let record = golden_record();
        for length in 0..record.len() {
            assert!(
                decode(&record[..length]).is_err(),
                "accepted length {length}"
            );
        }
        let mut trailing = record;
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(BindingCodecError::InvalidRecord));
    }

    #[test]
    fn every_reserved_byte_must_be_zero() {
        for offset in RESERVED_OFFSET..RESERVED_OFFSET + RESERVED_BYTES {
            let mut record = golden_record();
            record[offset] = 1;
            assert_eq!(decode(&record), Err(BindingCodecError::InvalidRecord));
        }
    }

    #[test]
    fn rejects_bad_magic_source_zero_ids_and_invalid_identity_bytes() {
        let mut bad_magic = golden_record();
        bad_magic[0] ^= 1;
        assert_eq!(decode(&bad_magic), Err(BindingCodecError::InvalidRecord));

        for source in [0, 3, u16::MAX] {
            let mut bad_source = golden_record();
            set_u16(&mut bad_source, SOURCE_OFFSET, source);
            assert_eq!(decode(&bad_source), Err(BindingCodecError::InvalidRecord));
        }

        for offset in [8, 16, 24, 32, 40, 48, 56, 64] {
            let mut zero_id = golden_record();
            zero_id[offset..offset + 8].fill(0);
            assert_eq!(decode(&zero_id), Err(BindingCodecError::InvalidRecord));
        }

        let mut bad_version = golden_record();
        bad_version[96] = b'\n';
        assert_eq!(decode(&bad_version), Err(BindingCodecError::InvalidRecord));

        let mut nul_project = golden_record();
        nul_project[102] = 0;
        assert_eq!(decode(&nul_project), Err(BindingCodecError::InvalidRecord));

        let mut bad_session = golden_record();
        bad_session[109] = 0x80;
        assert_eq!(decode(&bad_session), Err(BindingCodecError::InvalidRecord));
    }

    #[test]
    fn rejects_zero_and_oversized_lengths_and_preflights_total_input_bound() {
        for (offset, invalid_length) in [
            (VERSION_LENGTH_OFFSET, 0),
            (
                VERSION_LENGTH_OFFSET,
                (MAX_PROVIDER_VERSION_BYTES + 1) as u16,
            ),
            (PROJECT_LENGTH_OFFSET, 0),
            (
                PROJECT_LENGTH_OFFSET,
                (MAX_PROJECT_IDENTITY_BYTES + 1) as u16,
            ),
            (SESSION_LENGTH_OFFSET, 0),
            (
                SESSION_LENGTH_OFFSET,
                (MAX_PROVIDER_SESSION_ID_BYTES + 1) as u16,
            ),
        ] {
            let mut record = golden_record();
            set_u16(&mut record, offset, invalid_length);
            assert_eq!(decode(&record), Err(BindingCodecError::InvalidRecord));
        }

        let oversized = vec![0xff; MAX_BINDING_RECORD_BYTES + 1];
        assert_eq!(decode(&oversized), Err(BindingCodecError::RecordTooLarge));
        assert_eq!(
            decode(&[0; BINDING_RECORD_HEADER_BYTES - 1]),
            Err(BindingCodecError::RecordTooShort)
        );
    }

    #[test]
    fn codec_diagnostics_are_fixed_and_restore_remains_a_separate_step() {
        let secret_bytes = b"private-session-id";
        let project = ProjectIdentity::new(secret_bytes).unwrap();
        let version = SupportedVersion::new("secret-version").unwrap();
        let binding = Binding::new(
            TargetRef {
                generation: 1,
                workspace: 2,
                pane: 3,
                session: 4,
                occupant_revision: 5,
            },
            &project,
            &version,
            6,
            7,
            "private-session-id",
            8,
            VerificationSource::OrderedSelection,
        )
        .unwrap();
        assert!(!format!("{binding:?}").contains("private-session-id"));
        assert!(!format!("{:?}", BindingCodecError::InvalidRecord).contains("private-session-id"));
        assert_eq!(
            format!("{:?}", BindingCodecError::InvalidRecord),
            "BindingCodecError { <redacted> }"
        );

        let decoded = decode(&encode(&binding).unwrap()).unwrap();
        let last_confirmed = LastConfirmedRecord::from_validated_binding(decoded);
        assert_eq!(last_confirmed.binding(), &binding);
        // The codec produces a validated Binding only; storage must decide
        // whether it came from a durable last-confirmed slot before wrapping.
    }
}
