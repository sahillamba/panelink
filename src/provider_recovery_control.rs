//! Typed wire values for explicit, non-replaying provider recovery control.
//!
//! Validation results are kept distinct from selected-session observations.
//! A `ValidatedSession` can only be constructed by decoding a scope-matched
//! response on an accepted `FreshReader` connection.

use crate::provider_binding::MAX_PROVIDER_SESSION_ID_BYTES;
use std::fmt;

pub(crate) const VALIDATE_REQUEST: u16 = 6;
pub(crate) const VALIDATE_RESPONSE: u16 = 7;
pub(crate) const NAVIGATE_REQUEST: u16 = 8;
pub(crate) const NAVIGATE_RESPONSE: u16 = 9;
pub(crate) const CONTROL_DEADLINE_MS: u64 = 300;

const VERIFIED: u8 = 1;
const MISSING: u8 = 2;
const FOREIGN_PROJECT: u8 = 3;
const CHILD: u8 = 4;
const UNSUPPORTED: u8 = 5;
const UNCERTAIN: u8 = 6;
const ACTION_ISSUED: u8 = 1;
const ACTION_NOT_ISSUED: u8 = 2;
const VALIDATION_HEADER: usize = 8;
const NAVIGATION_HEADER: usize = 28;
const TICKET_BYTES: usize = 16;

/// Read-only validation status produced by the provider source.
///
/// `Verified` contains only the provider's canonical project identity. The
/// session ID is echoed from the validated request by the sender implementation.
pub enum ValidationDecision {
    Verified { project_identity: Vec<u8> },
    Missing,
    ForeignProject,
    Child,
    Unsupported,
    Uncertain,
}

impl fmt::Debug for ValidationDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verified { .. } => f.write_str("Verified { <redacted> }"),
            Self::Missing => f.write_str("Missing"),
            Self::ForeignProject => f.write_str("ForeignProject"),
            Self::Child => f.write_str("Child"),
            Self::Unsupported => f.write_str("Unsupported"),
            Self::Uncertain => f.write_str("Uncertain"),
        }
    }
}

/// Outcome of an exact-ID validation request.
pub enum ValidationOutcome {
    Verified(ValidatedSession),
    Missing,
    ForeignProject,
    Child,
    Unsupported,
    Uncertain,
}

impl fmt::Debug for ValidationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verified(session) => f.debug_tuple("Verified").field(session).finish(),
            Self::Missing => f.write_str("Missing"),
            Self::ForeignProject => f.write_str("ForeignProject"),
            Self::Child => f.write_str("Child"),
            Self::Unsupported => f.write_str("Unsupported"),
            Self::Uncertain => f.write_str("Uncertain"),
        }
    }
}

/// Provider session metadata proven by one challenge-bound validation reply.
///
/// This value has no public constructor and is not a selected-session binding.
pub struct ValidatedSession {
    session_id: String,
    project_identity: Vec<u8>,
}

impl ValidatedSession {
    pub(crate) fn from_response(session_id: String, project_identity: Vec<u8>) -> Self {
        Self {
            session_id,
            project_identity,
        }
    }

    /// The exact session ID echoed by the provider source.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Canonical project bytes echoed by the provider source.
    pub fn project_identity(&self) -> &[u8] {
        &self.project_identity
    }
}

impl PartialEq for ValidatedSession {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id && self.project_identity == other.project_identity
    }
}

impl Eq for ValidatedSession {}

impl fmt::Debug for ValidatedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ValidatedSession { <redacted> }")
    }
}

/// Result of the provider router action. This never means the route was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigateOutcome {
    ActionIssued,
    NotIssued,
}

/// Fixed result returned by a source navigation callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationDecision {
    ActionIssued,
    NotIssued,
}

/// Parsed and bounded target for a validation callback.
pub(crate) struct ValidateRequest {
    session_id: String,
}

impl ValidateRequest {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }
}

impl fmt::Debug for ValidateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ValidateRequest { <redacted> }")
    }
}

/// Parsed and bounded target for a one-shot navigation callback.
pub(crate) struct NavigateRequest {
    session_id: String,
    prepared_revision: u64,
    host_ticket: [u8; TICKET_BYTES],
}

impl NavigateRequest {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }
    pub(crate) const fn prepared_revision(&self) -> u64 {
        self.prepared_revision
    }
    pub(crate) const fn host_ticket(&self) -> &[u8; TICKET_BYTES] {
        &self.host_ticket
    }
}

impl fmt::Debug for NavigateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NavigateRequest { <redacted> }")
    }
}

pub(crate) enum RecoveryRequest {
    Validate(ValidateRequest),
    Navigate(NavigateRequest),
}

impl fmt::Debug for RecoveryRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validate(request) => f.debug_tuple("Validate").field(request).finish(),
            Self::Navigate(request) => f.debug_tuple("Navigate").field(request).finish(),
        }
    }
}

pub(crate) fn valid_session_id(session_id: &str) -> bool {
    let bytes = session_id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_PROVIDER_SESSION_ID_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

pub(crate) fn valid_ticket(ticket: &[u8; TICKET_BYTES]) -> bool {
    ticket.iter().any(|byte| *byte != 0)
}

pub(crate) fn encode_validate_request(session_id: &str) -> Option<Vec<u8>> {
    if !valid_session_id(session_id) {
        return None;
    }
    let bytes = session_id.as_bytes();
    let mut payload = Vec::with_capacity(2 + bytes.len());
    payload.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    payload.extend_from_slice(bytes);
    Some(payload)
}

pub(crate) fn decode_validate_request(payload: &[u8]) -> Option<ValidateRequest> {
    if payload.len() < 3 {
        return None;
    }
    let len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if len == 0 || len > MAX_PROVIDER_SESSION_ID_BYTES || payload.len() != 2 + len {
        return None;
    }
    let session_id = std::str::from_utf8(&payload[2..]).ok()?.to_owned();
    if !valid_session_id(&session_id) {
        return None;
    }
    Some(ValidateRequest { session_id })
}

pub(crate) fn encode_validate_response(
    request: &ValidateRequest,
    decision: ValidationDecision,
    granted_project: &[u8],
) -> Option<Vec<u8>> {
    let (status, session_id, project): (u8, Vec<u8>, Vec<u8>) = match decision {
        ValidationDecision::Verified { project_identity }
            if project_identity == granted_project =>
        {
            (
                VERIFIED,
                request.session_id.as_bytes().to_vec(),
                project_identity,
            )
        }
        ValidationDecision::Verified { .. } | ValidationDecision::ForeignProject => {
            (FOREIGN_PROJECT, vec![], vec![])
        }
        ValidationDecision::Missing => (MISSING, vec![], vec![]),
        ValidationDecision::Child => (CHILD, vec![], vec![]),
        ValidationDecision::Unsupported => (UNSUPPORTED, vec![], vec![]),
        ValidationDecision::Uncertain => (UNCERTAIN, vec![], vec![]),
    };
    if project.len() > 4096 || (status == VERIFIED && project.is_empty()) {
        return None;
    }
    let mut payload = vec![0; VALIDATION_HEADER];
    payload[0] = status;
    payload[2..4].copy_from_slice(&(session_id.len() as u16).to_be_bytes());
    payload[4..6].copy_from_slice(&(project.len() as u16).to_be_bytes());
    payload.extend_from_slice(&session_id);
    payload.extend_from_slice(&project);
    Some(payload)
}

pub(crate) fn decode_validate_response(
    expected_session_id: &str,
    granted_project: &[u8],
    payload: &[u8],
) -> Option<ValidationOutcome> {
    if payload.len() < VALIDATION_HEADER || payload[1] != 0 || payload[6..8] != [0, 0] {
        return None;
    }
    let id_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
    let project_len = u16::from_be_bytes([payload[4], payload[5]]) as usize;
    if payload.len() != VALIDATION_HEADER + id_len + project_len {
        return None;
    }
    let id = &payload[VALIDATION_HEADER..VALIDATION_HEADER + id_len];
    let project = &payload[VALIDATION_HEADER + id_len..];
    match payload[0] {
        VERIFIED
            if id_len > 0
                && id_len <= MAX_PROVIDER_SESSION_ID_BYTES
                && project_len > 0
                && project_len <= 4096
                && id == expected_session_id.as_bytes()
                && project == granted_project =>
        {
            Some(ValidationOutcome::Verified(
                ValidatedSession::from_response(expected_session_id.to_owned(), project.to_vec()),
            ))
        }
        FOREIGN_PROJECT if id_len == 0 && project_len == 0 => {
            Some(ValidationOutcome::ForeignProject)
        }
        MISSING if id_len == 0 && project_len == 0 => Some(ValidationOutcome::Missing),
        CHILD if id_len == 0 && project_len == 0 => Some(ValidationOutcome::Child),
        UNSUPPORTED if id_len == 0 && project_len == 0 => Some(ValidationOutcome::Unsupported),
        UNCERTAIN if id_len == 0 && project_len == 0 => Some(ValidationOutcome::Uncertain),
        _ => None,
    }
}

pub(crate) fn encode_navigate_request(
    session_id: &str,
    prepared_revision: u64,
    host_ticket: &[u8; TICKET_BYTES],
) -> Option<Vec<u8>> {
    if !valid_session_id(session_id) || prepared_revision == 0 || !valid_ticket(host_ticket) {
        return None;
    }
    let id = session_id.as_bytes();
    let mut payload = vec![0; NAVIGATION_HEADER];
    payload[..8].copy_from_slice(&prepared_revision.to_be_bytes());
    payload[8..24].copy_from_slice(host_ticket);
    payload[24..26].copy_from_slice(&(id.len() as u16).to_be_bytes());
    payload.extend_from_slice(id);
    Some(payload)
}

pub(crate) fn decode_navigate_request(payload: &[u8]) -> Option<NavigateRequest> {
    if payload.len() < NAVIGATION_HEADER + 1 || payload[26..28] != [0, 0] {
        return None;
    }
    let revision = u64::from_be_bytes(payload[..8].try_into().ok()?);
    let ticket: [u8; TICKET_BYTES] = payload[8..24].try_into().ok()?;
    let id_len = u16::from_be_bytes([payload[24], payload[25]]) as usize;
    if revision == 0
        || !valid_ticket(&ticket)
        || id_len == 0
        || id_len > MAX_PROVIDER_SESSION_ID_BYTES
        || payload.len() != NAVIGATION_HEADER + id_len
    {
        return None;
    }
    let session_id = std::str::from_utf8(&payload[NAVIGATION_HEADER..])
        .ok()?
        .to_owned();
    if !valid_session_id(&session_id) {
        return None;
    }
    Some(NavigateRequest {
        session_id,
        prepared_revision: revision,
        host_ticket: ticket,
    })
}

pub(crate) fn encode_navigate_response(
    request: &NavigateRequest,
    decision: NavigationDecision,
) -> Vec<u8> {
    let mut payload = vec![0; NAVIGATION_HEADER];
    payload[0] = match decision {
        NavigationDecision::ActionIssued => ACTION_ISSUED,
        NavigationDecision::NotIssued => ACTION_NOT_ISSUED,
    };
    payload[2..4].copy_from_slice(&(request.session_id.len() as u16).to_be_bytes());
    payload[4..12].copy_from_slice(&request.prepared_revision.to_be_bytes());
    payload[12..28].copy_from_slice(&request.host_ticket);
    payload.extend_from_slice(request.session_id.as_bytes());
    payload
}

pub(crate) fn decode_navigate_response(
    request: &NavigateRequest,
    payload: &[u8],
) -> Option<NavigateOutcome> {
    if payload.len() < NAVIGATION_HEADER + 1 || payload[1] != 0 {
        return None;
    }
    let id_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
    let revision = u64::from_be_bytes(payload[4..12].try_into().ok()?);
    let ticket: [u8; TICKET_BYTES] = payload[12..28].try_into().ok()?;
    if id_len != request.session_id.len()
        || payload.len() != NAVIGATION_HEADER + id_len
        || revision != request.prepared_revision
        || ticket != request.host_ticket
        || &payload[NAVIGATION_HEADER..] != request.session_id.as_bytes()
    {
        return None;
    }
    match payload[0] {
        ACTION_ISSUED => Some(NavigateOutcome::ActionIssued),
        ACTION_NOT_ISSUED => Some(NavigateOutcome::NotIssued),
        _ => None,
    }
}

pub(crate) fn decode_recovery_request(kind: u16, payload: &[u8]) -> Option<RecoveryRequest> {
    match kind {
        VALIDATE_REQUEST => decode_validate_request(payload).map(RecoveryRequest::Validate),
        NAVIGATE_REQUEST => decode_navigate_request(payload).map(RecoveryRequest::Navigate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ValidateRequest {
        decode_validate_request(&encode_validate_request("ses_123").unwrap()).unwrap()
    }

    #[test]
    fn validation_codec_requires_exact_ascii_identity_and_project() {
        assert!(encode_validate_request("").is_none());
        assert!(encode_validate_request("session/with/slash").is_none());
        assert!(encode_validate_request(&"x".repeat(MAX_PROVIDER_SESSION_ID_BYTES + 1)).is_none());
        let request = request();
        let payload = encode_validate_response(
            &request,
            ValidationDecision::Verified {
                project_identity: b"/project".to_vec(),
            },
            b"/project",
        )
        .unwrap();
        assert!(matches!(
            decode_validate_response("ses_123", b"/project", &payload),
            Some(ValidationOutcome::Verified(_))
        ));
        assert!(decode_validate_response("ses_other", b"/project", &payload).is_none());
        assert!(decode_validate_response("ses_123", b"/other", &payload).is_none());
        assert!(matches!(
            decode_validate_response(
                "ses_123",
                b"/project",
                &encode_validate_response(
                    &request,
                    ValidationDecision::Verified {
                        project_identity: b"/other".to_vec()
                    },
                    b"/project"
                )
                .unwrap()
            ),
            Some(ValidationOutcome::ForeignProject)
        ));
        let mut malformed = payload;
        malformed[1] = 1;
        assert!(decode_validate_response("ses_123", b"/project", &malformed).is_none());
    }

    #[test]
    fn navigation_codec_requires_nonzero_revision_ticket_and_exact_echo() {
        let ticket = [9; TICKET_BYTES];
        assert!(encode_navigate_request("ses_123", 0, &ticket).is_none());
        assert!(encode_navigate_request("ses_123", 7, &[0; TICKET_BYTES]).is_none());
        let bytes = encode_navigate_request("ses_123", 7, &ticket).unwrap();
        let request = decode_navigate_request(&bytes).unwrap();
        let response = encode_navigate_response(&request, NavigationDecision::ActionIssued);
        assert_eq!(
            decode_navigate_response(&request, &response),
            Some(NavigateOutcome::ActionIssued)
        );
        let mut wrong = response;
        wrong[12] ^= 1;
        assert!(decode_navigate_response(&request, &wrong).is_none());
        let mut wrong_id = encode_navigate_response(&request, NavigationDecision::ActionIssued);
        *wrong_id.last_mut().unwrap() ^= 1;
        assert!(decode_navigate_response(&request, &wrong_id).is_none());
    }
}
