//! Explicit build identity shared by local diagnostics and the server runtime.
//!
//! The fingerprint is a deterministic FNV-1a source-tree fingerprint, not a
//! cryptographic checksum. It covers the current contents of Rust sources and
//! Cargo manifests/lockfile, vendored sources and embedded assets, including
//! relevant untracked files, without
//! exposing checkout paths.

use std::fmt::Write as _;

pub const PACKAGE: &str = env!("CARGO_PKG_NAME");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REVISION: &str = env!("PANELINK_BUILD_REVISION");
pub const SOURCE_FINGERPRINT: &str = env!("PANELINK_SOURCE_FINGERPRINT");
pub const DIRTY: bool = env!("PANELINK_BUILD_DIRTY").as_bytes()[0] == b't';

/// Build identity of this executable or library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildIdentity {
    pub package: &'static str,
    pub version: &'static str,
    pub revision: &'static str,
    pub source_fingerprint: &'static str,
    pub dirty: bool,
}

pub const fn current() -> BuildIdentity {
    BuildIdentity {
        package: PACKAGE,
        version: VERSION,
        revision: REVISION,
        source_fingerprint: SOURCE_FINGERPRINT,
        dirty: DIRTY,
    }
}

impl BuildIdentity {
    /// Return a compact JSON object containing only build identity fields.
    pub fn to_json(self) -> String {
        let mut json = String::with_capacity(192);
        let _ = write!(
            json,
            "{{\"package\":\"{}\",\"version\":\"{}\",\"revision\":\"{}\",\"source_fingerprint\":\"{}\",\"dirty\":{}}}",
            self.package,
            self.version,
            self.revision,
            self.source_fingerprint,
            self.dirty
        );
        json
    }
}

/// Runtime identity returned only after the connection negotiates identity
/// query support. `selected_capabilities` is connection-specific.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerIdentity {
    pub build: BuildIdentity,
    pub generation: u64,
    pub selected_capabilities: u64,
}

impl ServerIdentity {
    pub fn to_json(self) -> String {
        let mut json = self.build.to_json();
        let _ = json.pop();
        let _ = write!(
            json,
            ",\"generation\":{},\"selected_capability_mask\":{},\"selected_capabilities\":[",
            self.generation, self.selected_capabilities
        );
        let mut first = true;
        for &(bit, name) in crate::handshake_protocol::CAPABILITY_NAMES {
            if self.selected_capabilities & bit != 0 {
                if !first {
                    json.push(',');
                }
                let _ = write!(json, "\"{name}\"");
                first = false;
            }
        }
        json.push_str("]}");
        json
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_contains_explicit_identity_without_checkout_path() {
        let json = current().to_json();
        assert!(json.contains("\"package\":\"panelink\""));
        assert!(json.contains("\"revision\":"));
        assert!(json.contains("\"source_fingerprint\":"));
        assert!(json.contains("\"dirty\":"));
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(!json.contains("/Users/"));
    }
}

/// Schema 15 is read-only and gated by capability 256. No lease mutation or
/// durable operation key is involved.
pub const QUERY: [u8; 4] = [0, 15, 1, 0];

pub fn encode_server_identity(value: ServerIdentity) -> Vec<u8> {
    let mut bytes = vec![0, 15, 2, u8::from(value.build.dirty)];
    bytes.extend_from_slice(&value.generation.to_be_bytes());
    bytes.extend_from_slice(&value.selected_capabilities.to_be_bytes());
    for field in [
        value.build.package,
        value.build.version,
        value.build.revision,
        value.build.source_fingerprint,
    ] {
        bytes.push(u8::try_from(field.len()).expect("bounded build identifier"));
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes
}

/// Validated owned identity received from a server, never inferred from the client.
#[derive(Debug)]
pub struct IdentityReply {
    pub generation: u64,
    pub selected_capabilities: u64,
    pub dirty: bool,
    pub fields: [String; 4],
}
pub fn decode_server_identity(bytes: &[u8]) -> Option<IdentityReply> {
    if bytes.len() < 24 || bytes.len() > 256 || bytes[..3] != [0, 15, 2] || bytes[3] > 1 {
        return None;
    }
    let generation = u64::from_be_bytes(bytes[4..12].try_into().ok()?);
    let selected_capabilities = u64::from_be_bytes(bytes[12..20].try_into().ok()?);
    if generation == 0
        || selected_capabilities & crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY == 0
        || selected_capabilities & !crate::handshake_protocol::KNOWN_CAPABILITIES != 0
    {
        return None;
    }
    let mut cursor = 20;
    let mut fields = std::array::from_fn(|_| String::new());
    for field in &mut fields {
        let len = usize::from(*bytes.get(cursor)?);
        cursor += 1;
        if !(1..=64).contains(&len) {
            return None;
        }
        let value = bytes.get(cursor..cursor + len)?;
        if !value
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(c))
        {
            return None;
        }
        *field = String::from_utf8(value.to_vec()).ok()?;
        cursor += len;
    }
    if cursor != bytes.len()
        || fields[3].len() != 16
        || !fields[3].bytes().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(IdentityReply {
        generation,
        selected_capabilities,
        dirty: bytes[3] == 1,
        fields,
    })
}
impl IdentityReply {
    pub fn to_json(&self) -> String {
        let names = crate::handshake_protocol::CAPABILITY_NAMES
            .iter()
            .filter(|(bit, _)| self.selected_capabilities & bit != 0)
            .map(|(_, name)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"package\":\"{}\",\"version\":\"{}\",\"revision\":\"{}\",\"source_fingerprint\":\"{}\",\"dirty\":{},\"generation\":{},\"selected_capability_mask\":{},\"selected_capabilities\":[{}]}}", self.fields[0], self.fields[1], self.fields[2], self.fields[3], self.dirty, self.generation, self.selected_capabilities, names)
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    #[test]
    fn identity_reply_is_bounded_strict_and_round_trips() {
        let bytes = encode_server_identity(ServerIdentity {
            build: current(),
            generation: 1,
            selected_capabilities: crate::handshake_protocol::CAPABILITY_READ
                | crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY,
        });
        let decoded = decode_server_identity(&bytes).unwrap();
        assert_eq!(decoded.fields[2], REVISION);
        assert_eq!(decoded.fields[3], SOURCE_FINGERPRINT);
        for len in 0..bytes.len() {
            assert!(decode_server_identity(&bytes[..len]).is_none());
        }
        let mut malformed = bytes.clone();
        malformed.push(0);
        assert!(decode_server_identity(&malformed).is_none());
        let mut malformed = bytes;
        malformed[3] = 2;
        assert!(decode_server_identity(&malformed).is_none());
    }
}
