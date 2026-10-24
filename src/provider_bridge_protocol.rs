//! Bounded binary wire codec for the independent provider bridge channel.
//!
//! Wire messages carry provider identity metadata and a handshake capability.
//! Callers must never log or persist the encoded bytes.

use crate::control_protocol::TargetRef;
use crate::provider_binding::{
    SupportedVersion, VerificationSource, MAX_PROJECT_IDENTITY_BYTES,
    MAX_PROVIDER_SESSION_ID_BYTES, MAX_PROVIDER_VERSION_BYTES,
};
use crate::provider_bridge::{Frame, FrameScope, Selection, BRIDGE_PROTOCOL_VERSION};
use std::fmt;
use std::str;

pub const HEADER_BYTES: usize = 32;
pub const MAX_BODY_BYTES: usize = 4_356;
pub const MAX_MESSAGE_BYTES: usize = HEADER_BYTES + MAX_BODY_BYTES;
const MAGIC: &[u8; 8] = b"PLBRDG01";
const HELLO_MIN: usize = 88;
const HELLO_MAX: usize = 88 + MAX_PROVIDER_VERSION_BYTES + MAX_PROJECT_IDENTITY_BYTES;
const OBSERVATION_MIN: usize = 68;
const OBSERVATION_MAX: usize = OBSERVATION_MIN
    + MAX_PROVIDER_VERSION_BYTES
    + MAX_PROJECT_IDENTITY_BYTES
    + MAX_PROVIDER_SESSION_ID_BYTES;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Kind {
    Hello,
    Welcome,
    Observation,
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hello => "Hello",
            Self::Welcome => "Welcome",
            Self::Observation => "Observation",
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Header {
    body_len: usize,
    kind: Kind,
}

impl Header {
    pub const fn body_len(self) -> usize {
        self.body_len
    }
    pub const fn kind(self) -> Kind {
        self.kind
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Header { <redacted> }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Hello {
    target: TargetRef,
    project_identity: Vec<u8>,
    provider_version: String,
    bridge_instance: u64,
    secret: [u8; 32],
}

impl Hello {
    pub fn new(
        target: TargetRef,
        project: &[u8],
        version: &str,
        instance: u64,
        secret: [u8; 32],
    ) -> Result<Self, WireError> {
        validate_target(target)?;
        validate_project(project)?;
        validate_version(version)?;
        if instance == 0 || secret.iter().all(|b| *b == 0) {
            return Err(WireError);
        }
        Ok(Self {
            target,
            project_identity: project.to_vec(),
            provider_version: version.to_owned(),
            bridge_instance: instance,
            secret,
        })
    }
    pub const fn target(&self) -> TargetRef {
        self.target
    }
    pub fn project_identity(&self) -> &[u8] {
        &self.project_identity
    }
    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }
    pub const fn bridge_instance(&self) -> u64 {
        self.bridge_instance
    }
    pub(crate) const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl fmt::Debug for Hello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Hello { <redacted> }")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum Message {
    Hello(Hello),
    Welcome { bridge_instance: u64 },
    Observation(Frame),
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Message { <redacted> }")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct WireError;

impl fmt::Debug for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireError { invalid bridge message }")
    }
}
impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid provider bridge message")
    }
}
impl std::error::Error for WireError {}

pub fn decode_header(bytes: &[u8; HEADER_BYTES]) -> Result<Header, WireError> {
    if &bytes[..8] != MAGIC
        || u16::from_be_bytes([bytes[8], bytes[9]]) != 1
        || bytes[16..].iter().any(|b| *b != 0)
    {
        return Err(WireError);
    }
    let kind = match u16::from_be_bytes([bytes[10], bytes[11]]) {
        1 => Kind::Hello,
        2 => Kind::Welcome,
        3 => Kind::Observation,
        _ => return Err(WireError),
    };
    let body_len = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let (min, max) = match kind {
        Kind::Hello => (HELLO_MIN, HELLO_MAX),
        Kind::Welcome => (8, 8),
        Kind::Observation => (OBSERVATION_MIN, OBSERVATION_MAX),
    };
    if body_len < min || body_len > max || body_len > MAX_BODY_BYTES {
        return Err(WireError);
    }
    Ok(Header { body_len, kind })
}

pub fn encode(message: &Message) -> Result<Vec<u8>, WireError> {
    let (kind, body) = match message {
        Message::Hello(h) => (Kind::Hello, encode_hello(h)?),
        Message::Welcome { bridge_instance } if *bridge_instance != 0 => {
            (Kind::Welcome, bridge_instance.to_be_bytes().to_vec())
        }
        Message::Welcome { .. } => return Err(WireError),
        Message::Observation(frame) => (Kind::Observation, encode_observation(frame)?),
    };
    if body.len() > MAX_BODY_BYTES {
        return Err(WireError);
    }
    let mut out = vec![0; HEADER_BYTES + body.len()];
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&1u16.to_be_bytes());
    out[10..12].copy_from_slice(
        &(match kind {
            Kind::Hello => 1u16,
            Kind::Welcome => 2,
            Kind::Observation => 3,
        })
        .to_be_bytes(),
    );
    out[12..16].copy_from_slice(&(body.len() as u32).to_be_bytes());
    out[HEADER_BYTES..].copy_from_slice(&body);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Message, WireError> {
    if bytes.len() < HEADER_BYTES {
        return Err(WireError);
    }
    let hdr: &[u8; HEADER_BYTES] = bytes[..HEADER_BYTES].try_into().map_err(|_| WireError)?;
    let header = decode_header(hdr)?;
    if bytes.len() != HEADER_BYTES.checked_add(header.body_len).ok_or(WireError)? {
        return Err(WireError);
    }
    let body = &bytes[HEADER_BYTES..];
    match header.kind {
        Kind::Hello => decode_hello(body).map(Message::Hello),
        Kind::Welcome => {
            let instance = read_u64(body, 0)?;
            if instance == 0 {
                Err(WireError)
            } else {
                Ok(Message::Welcome {
                    bridge_instance: instance,
                })
            }
        }
        Kind::Observation => decode_observation(body).map(Message::Observation),
    }
}

fn encode_hello(h: &Hello) -> Result<Vec<u8>, WireError> {
    if h.secret.iter().all(|b| *b == 0) {
        return Err(WireError);
    }
    validate_target(h.target)?;
    validate_project(&h.project_identity)?;
    validate_version(&h.provider_version)?;
    let mut b = vec![0; HELLO_MIN + h.provider_version.len() + h.project_identity.len()];
    write_target(&mut b, 0, h.target);
    put_u64(&mut b, 40, h.bridge_instance);
    b[48..80].copy_from_slice(&h.secret);
    put_u16(&mut b, 80, h.provider_version.len() as u16);
    put_u16(&mut b, 82, h.project_identity.len() as u16);
    b[88..88 + h.provider_version.len()].copy_from_slice(h.provider_version.as_bytes());
    b[88 + h.provider_version.len()..].copy_from_slice(&h.project_identity);
    Ok(b)
}

fn decode_hello(b: &[u8]) -> Result<Hello, WireError> {
    if b.len() < HELLO_MIN || b[84..88].iter().any(|x| *x != 0) {
        return Err(WireError);
    }
    let vl = read_u16(b, 80)? as usize;
    let pl = read_u16(b, 82)? as usize;
    if vl == 0
        || vl > MAX_PROVIDER_VERSION_BYTES
        || pl == 0
        || pl > MAX_PROJECT_IDENTITY_BYTES
        || HELLO_MIN.checked_add(vl).and_then(|n| n.checked_add(pl)) != Some(b.len())
    {
        return Err(WireError);
    }
    let v = str::from_utf8(&b[88..88 + vl]).map_err(|_| WireError)?;
    let p = &b[88 + vl..];
    Hello::new(
        read_target(b, 0)?,
        p,
        v,
        read_u64(b, 40)?,
        b[48..80].try_into().map_err(|_| WireError)?,
    )
}

fn encode_observation(f: &Frame) -> Result<Vec<u8>, WireError> {
    let version = f.provider_version().as_bytes();
    let project = f.project_identity();
    let (sel, source, session): (u8, u8, &[u8]) = match f.selection() {
        Selection::Session { session_id, source } => (
            1,
            match source {
                VerificationSource::TuiRouteSnapshot => 1,
                VerificationSource::OrderedSelection => 2,
            },
            session_id.as_bytes(),
        ),
        Selection::Home => (2, 0, &[]),
        Selection::Child => (3, 0, &[]),
        Selection::Unknown => (4, 0, &[]),
        Selection::Unavailable => (5, 0, &[]),
    };
    validate_project(project)?;
    validate_version(f.provider_version())?;
    let mut b = vec![0; OBSERVATION_MIN + version.len() + project.len() + session.len()];
    write_target(&mut b, 0, f.target());
    put_u64(&mut b, 40, f.bridge_instance());
    put_u64(&mut b, 48, f.sequence());
    b[56] = match f.scope() {
        FrameScope::TopLevel => 1,
        FrameScope::ChildActivity => 2,
    };
    b[57] = sel;
    b[58] = source;
    put_u16(&mut b, 60, version.len() as u16);
    put_u16(&mut b, 62, project.len() as u16);
    put_u16(&mut b, 64, session.len() as u16);
    let mut i = 68;
    for part in [version, project, session] {
        b[i..i + part.len()].copy_from_slice(part);
        i += part.len();
    }
    Ok(b)
}

fn decode_observation(b: &[u8]) -> Result<Frame, WireError> {
    if b.len() < OBSERVATION_MIN || b[59] != 0 || b[66..68].iter().any(|x| *x != 0) {
        return Err(WireError);
    }
    let vl = read_u16(b, 60)? as usize;
    let pl = read_u16(b, 62)? as usize;
    let sl = read_u16(b, 64)? as usize;
    if vl == 0
        || vl > MAX_PROVIDER_VERSION_BYTES
        || pl == 0
        || pl > MAX_PROJECT_IDENTITY_BYTES
        || sl > MAX_PROVIDER_SESSION_ID_BYTES
        || OBSERVATION_MIN
            .checked_add(vl)
            .and_then(|x| x.checked_add(pl))
            .and_then(|x| x.checked_add(sl))
            != Some(b.len())
    {
        return Err(WireError);
    }
    let scope = match b[56] {
        1 => FrameScope::TopLevel,
        2 => FrameScope::ChildActivity,
        _ => return Err(WireError),
    };
    let version = str::from_utf8(&b[68..68 + vl]).map_err(|_| WireError)?;
    let project = &b[68 + vl..68 + vl + pl];
    let sess = &b[68 + vl + pl..];
    let selection = match (b[57], b[58], sl) {
        (1, 1 | 2, n) if n > 0 => Selection::session(
            str::from_utf8(sess).map_err(|_| WireError)?,
            if b[58] == 1 {
                VerificationSource::TuiRouteSnapshot
            } else {
                VerificationSource::OrderedSelection
            },
        )
        .map_err(|_| WireError)?,
        (2, 0, 0) => Selection::Home,
        (3, 0, 0) => Selection::Child,
        (4, 0, 0) => Selection::Unknown,
        (5, 0, 0) => Selection::Unavailable,
        _ => return Err(WireError),
    };
    Frame::new(
        BRIDGE_PROTOCOL_VERSION,
        read_target(b, 0)?,
        project,
        version,
        read_u64(b, 40)?,
        read_u64(b, 48)?,
        scope,
        selection,
    )
    .map_err(|_| WireError)
}

fn validate_target(t: TargetRef) -> Result<(), WireError> {
    if [
        t.generation,
        t.workspace,
        t.pane,
        t.session,
        t.occupant_revision,
    ]
    .contains(&0)
    {
        Err(WireError)
    } else {
        Ok(())
    }
}
fn validate_project(p: &[u8]) -> Result<(), WireError> {
    if p.is_empty() || p.len() > MAX_PROJECT_IDENTITY_BYTES || p.contains(&0) {
        Err(WireError)
    } else {
        Ok(())
    }
}
fn validate_version(v: &str) -> Result<(), WireError> {
    SupportedVersion::new(v).map(|_| ()).map_err(|_| WireError)
}
fn write_target(b: &mut [u8], o: usize, t: TargetRef) {
    for (i, v) in [
        t.generation,
        t.workspace,
        t.pane,
        t.session,
        t.occupant_revision,
    ]
    .into_iter()
    .enumerate()
    {
        put_u64(b, o + i * 8, v)
    }
}
fn read_target(b: &[u8], o: usize) -> Result<TargetRef, WireError> {
    let t = TargetRef {
        generation: read_u64(b, o)?,
        workspace: read_u64(b, o + 8)?,
        pane: read_u64(b, o + 16)?,
        session: read_u64(b, o + 24)?,
        occupant_revision: read_u64(b, o + 32)?,
    };
    validate_target(t)?;
    Ok(t)
}
fn put_u64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_be_bytes())
}
fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_be_bytes())
}
fn read_u64(b: &[u8], o: usize) -> Result<u64, WireError> {
    Ok(u64::from_be_bytes(
        b.get(o..o + 8)
            .ok_or(WireError)?
            .try_into()
            .map_err(|_| WireError)?,
    ))
}
fn read_u16(b: &[u8], o: usize) -> Result<u16, WireError> {
    Ok(u16::from_be_bytes(
        b.get(o..o + 2)
            .ok_or(WireError)?
            .try_into()
            .map_err(|_| WireError)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_binding::VerificationSource;

    fn target() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 12,
            pane: 13,
            session: 14,
            occupant_revision: 15,
        }
    }
    fn hello() -> Hello {
        Hello::new(target(), b"project", "2.0.32", 41, [7; 32]).unwrap()
    }
    fn frame(scope: FrameScope, selection: Selection) -> Frame {
        Frame::new(1, target(), b"project", "2.0.32", 41, 2, scope, selection).unwrap()
    }

    #[test]
    fn hello_welcome_and_every_observation_state_round_trip() {
        let h = Message::Hello(hello());
        assert_eq!(decode(&encode(&h).unwrap()).unwrap(), h);
        let w = Message::Welcome {
            bridge_instance: 41,
        };
        assert_eq!(decode(&encode(&w).unwrap()).unwrap(), w);
        for (scope, selection) in [
            (
                FrameScope::TopLevel,
                Selection::session("sess_01", VerificationSource::TuiRouteSnapshot).unwrap(),
            ),
            (
                FrameScope::TopLevel,
                Selection::session("sess-02", VerificationSource::OrderedSelection).unwrap(),
            ),
            (FrameScope::TopLevel, Selection::Home),
            (FrameScope::ChildActivity, Selection::Child),
            (FrameScope::TopLevel, Selection::Unknown),
            (FrameScope::TopLevel, Selection::Unavailable),
        ] {
            let m = Message::Observation(frame(scope, selection));
            assert_eq!(decode(&encode(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn max_bounded_fields_fit_caps_and_invalid_hello_fields_are_refused() {
        let project = vec![b'p'; MAX_PROJECT_IDENTITY_BYTES];
        let version = "v".repeat(MAX_PROVIDER_VERSION_BYTES);
        let h = Hello::new(target(), &project, &version, 1, [1; 32]).unwrap();
        let wire = encode(&Message::Hello(h)).unwrap();
        assert_eq!(wire.len(), HEADER_BYTES + HELLO_MAX);
        assert_eq!(
            decode_header(wire[..HEADER_BYTES].try_into().unwrap())
                .unwrap()
                .body_len(),
            HELLO_MAX
        );
        let session = "s".repeat(MAX_PROVIDER_SESSION_ID_BYTES);
        let obs = Frame::new(
            1,
            target(),
            &project,
            &version,
            1,
            1,
            FrameScope::TopLevel,
            Selection::session(&session, VerificationSource::TuiRouteSnapshot).unwrap(),
        )
        .unwrap();
        let max_obs = encode(&Message::Observation(obs)).unwrap();
        assert_eq!(max_obs.len(), MAX_MESSAGE_BYTES);
        assert_eq!(
            decode_header(max_obs[..HEADER_BYTES].try_into().unwrap())
                .unwrap()
                .body_len(),
            MAX_BODY_BYTES
        );
        assert!(Hello::new(target(), &project, &version, 1, [0; 32]).is_err());
        let mut t = target();
        t.pane = 0;
        assert!(Hello::new(t, b"p", "2.0.32", 1, [1; 32]).is_err());
        assert!(Hello::new(target(), b"p", "2.0.32", 0, [1; 32]).is_err());
        assert!(Hello::new(target(), b"p\0x", "2.0.32", 1, [1; 32]).is_err());
        assert!(Hello::new(target(), b"p", "v\u{e9}", 1, [1; 32]).is_err());
    }

    #[test]
    fn decoder_rejects_every_truncated_prefix_and_trailing_data() {
        for message in [
            Message::Hello(hello()),
            Message::Welcome {
                bridge_instance: 41,
            },
            Message::Observation(frame(
                FrameScope::TopLevel,
                Selection::session("session", VerificationSource::TuiRouteSnapshot).unwrap(),
            )),
        ] {
            let wire = encode(&message).unwrap();
            for end in 0..wire.len() {
                assert!(decode(&wire[..end]).is_err(), "prefix {end}");
            }
            let mut extra = wire;
            extra.push(0);
            assert!(decode(&extra).is_err());
        }
    }

    #[test]
    fn header_rejects_reserved_tags_and_kind_specific_bad_lengths() {
        let good = encode(&Message::Welcome { bridge_instance: 9 }).unwrap();
        let mut h: [u8; HEADER_BYTES] = good[..HEADER_BYTES].try_into().unwrap();
        for at in [16usize, 31] {
            let mut bad = h;
            bad[at] = 1;
            assert!(decode_header(&bad).is_err());
        }
        let mut bad_magic = h;
        bad_magic[..8].copy_from_slice(b"PLCTRL01");
        assert!(decode_header(&bad_magic).is_err());
        let mut bad = h;
        bad[10..12].copy_from_slice(&99u16.to_be_bytes());
        assert!(decode_header(&bad).is_err());
        for len in [0u32, 7, 9, u32::MAX] {
            let mut bad = h;
            bad[12..16].copy_from_slice(&len.to_be_bytes());
            assert!(decode_header(&bad).is_err());
        }
        h[8..10].copy_from_slice(&2u16.to_be_bytes());
        assert!(decode_header(&h).is_err());
    }

    #[test]
    fn malformed_observation_tags_lengths_utf8_and_zero_sequence_are_rejected() {
        let base = encode(&Message::Observation(frame(
            FrameScope::TopLevel,
            Selection::session("session", VerificationSource::TuiRouteSnapshot).unwrap(),
        )))
        .unwrap();
        for (at, value) in [(56, 0u8), (57, 9), (58, 9), (59, 1), (66, 1)] {
            let mut b = base.clone();
            b[HEADER_BYTES + at] = value;
            assert!(decode(&b).is_err());
        }
        let mut b = base.clone();
        b[HEADER_BYTES + 48..HEADER_BYTES + 56].fill(0);
        assert!(decode(&b).is_err());
        let mut b = base.clone();
        b[HEADER_BYTES + 68] = 0xff;
        assert!(decode(&b).is_err());
        let mut b = base.clone();
        b[HEADER_BYTES + 68 + 6 + 7] = 0xff;
        assert!(decode(&b).is_err());
        let mut b = base.clone();
        b[HEADER_BYTES + 68 + 6 + 7] = b'@';
        assert!(decode(&b).is_err());
        let mut b = base.clone();
        b[HEADER_BYTES + 57] = 2;
        assert!(decode(&b).is_err());
        let mut b = base;
        b[HEADER_BYTES + 60..HEADER_BYTES + 62].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(decode(&b).is_err());
    }

    #[test]
    fn malformed_hello_rejects_invalid_utf8_ascii_and_exact_length() {
        let base = encode(&Message::Hello(hello())).unwrap();
        let version_at = HEADER_BYTES + HELLO_MIN;
        let mut invalid_utf8 = base.clone();
        invalid_utf8[version_at] = 0xff;
        assert!(decode(&invalid_utf8).is_err());
        let mut invalid_ascii = base.clone();
        invalid_ascii[version_at] = b'/';
        assert!(decode(&invalid_ascii).is_err());
        let mut bad_project_len = base.clone();
        bad_project_len[HEADER_BYTES + 82..HEADER_BYTES + 84]
            .copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(decode(&bad_project_len).is_err());
    }

    #[test]
    fn diagnostics_redact_secret_identity_and_wire_error_is_fixed() {
        let h = hello();
        let rendered = format!("{h:?} {:?}", Message::Hello(h.clone()));
        assert!(!rendered.contains("project"));
        assert!(!rendered.contains("2.0.32"));
        assert!(!rendered.contains("070707"));
        assert_eq!(h.secret(), &[7; 32]);
        assert_eq!(
            format!("{:?}", WireError),
            "WireError { invalid bridge message }"
        );
    }
}
