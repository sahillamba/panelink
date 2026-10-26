//! Strict schema-13 codec for pane operations and authoritative pane views.
//!
//! This module validates bytes and typed values only. It does not authenticate
//! peers, check capability negotiation, admit operations, or dispatch effects.
//! All integers are big-endian.

use crate::control_protocol::TargetRef;
use crate::pane_layout::{MAX_PANES, MAX_PANE_COLUMNS, MAX_PANE_ROWS};
use crate::protocol::{OperationClass, OperationKey};
use crate::ui_protocol::{
    decode_layout_reply, encode_layout_reply, LayoutReply, UiCodecError, LAYOUT_REPLY_BYTES,
};
use std::error::Error;
use std::fmt;

/// Schema version for pane-operation requests and authoritative views.
pub const PANE_OPERATIONS_SCHEMA: u16 = 13;
/// Exact size of ViewQuery.
pub const VIEW_QUERY_BYTES: usize = 8;
/// Exact size of PaneSwap.
pub const PANE_SWAP_BYTES: usize = 116;
/// Exact size of PaneDividerSet and PaneZoomSet.
pub const PANE_TARGET_MUTATION_BYTES: usize = 80;
/// Exact size of PaneOperationsView.
pub const PANE_OPERATIONS_VIEW_BYTES: usize = 504;

const KIND_VIEW_QUERY: u16 = 1;
const KIND_SWAP: u16 = 2;
const KIND_DIVIDER_SET: u16 = 3;
const KIND_ZOOM_SET: u16 = 4;
const KIND_VIEW: u16 = 5;
const KEY_OFFSET: usize = 4;
const REVISION_OFFSET: usize = 28;
const TARGET_OFFSET: usize = 36;
const SECOND_TARGET_OFFSET: usize = 76;
const VIEW_LAYOUT_OFFSET: usize = 4;
const VIEW_ZOOM_PRESENT_OFFSET: usize = VIEW_LAYOUT_OFFSET + LAYOUT_REPLY_BYTES;
const VIEW_ZOOM_TARGET_OFFSET: usize = VIEW_ZOOM_PRESENT_OFFSET + 4;
const VIEW_SIZES_OFFSET: usize = VIEW_ZOOM_TARGET_OFFSET + 40;
const VIEW_SIZE_ENTRY_BYTES: usize = 8;

/// Fixed, data-free errors produced by this codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// Input ends before the fixed record is complete.
    Truncated,
    /// Bytes follow a complete fixed record.
    TrailingBytes,
    /// Schema is not 13.
    UnsupportedSchema,
    /// Request or view kind is unknown.
    UnknownKind,
    /// A reserved byte or absent optional field is nonzero.
    NonzeroReserved,
    /// Operation key contains a zero component.
    ZeroKeyComponent,
    /// Layout revision is zero.
    ZeroRevision,
    /// Target contains a zero identity component.
    InvalidTarget,
    /// Target generation differs from the operation key or companion target.
    GenerationMismatch,
    /// Swap targets do not share a workspace.
    WorkspaceMismatch,
    /// Swap names the same target twice.
    DuplicateTarget,
    /// Divider index is outside the four-pane traversal bound.
    InvalidDividerIndex,
    /// Divider span is outside the wire-level maximum root bound.
    InvalidDividerPosition,
    /// Boolean has a value other than zero or one.
    InvalidBoolean,
    /// View's explicit sizes, zoom target, or legacy layout are inconsistent.
    InvalidView,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "pane-operations payload is truncated",
            Self::TrailingBytes => "bytes follow the pane-operations payload",
            Self::UnsupportedSchema => "pane-operations schema is unsupported",
            Self::UnknownKind => "pane-operations kind is unknown",
            Self::NonzeroReserved => "pane-operations reserved bytes must be zero",
            Self::ZeroKeyComponent => "pane-operations key components must be nonzero",
            Self::ZeroRevision => "pane-operations layout revision must be nonzero",
            Self::InvalidTarget => "pane-operations target is invalid",
            Self::GenerationMismatch => "pane-operations generations do not match",
            Self::WorkspaceMismatch => "pane-operation targets must share a workspace",
            Self::DuplicateTarget => "pane-operation targets must be distinct",
            Self::InvalidDividerIndex => "pane divider index is out of range",
            Self::InvalidDividerPosition => "pane divider position is out of range",
            Self::InvalidBoolean => "pane zoom flag must be zero or one",
            Self::InvalidView => "pane-operations view fields are inconsistent",
        })
    }
}

impl Error for CodecError {}

/// Same-generation target swap request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneSwap {
    /// Existing operation-ledger key for exact-payload deduplication.
    pub key: OperationKey,
    /// Layout revision observed when targets were selected.
    pub expected_layout_revision: u64,
    /// Target whose current tree location receives the destination target.
    pub source: TargetRef,
    /// Distinct target whose current tree location receives the source target.
    pub destination: TargetRef,
}

impl fmt::Debug for PaneSwap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneSwap")
            .field("key", &"<redacted>")
            .field("expected_layout_revision", &self.expected_layout_revision)
            .field("source", &"<redacted>")
            .field("destination", &"<redacted>")
            .finish()
    }
}

/// Absolute first-child span request for a divider in the target's ancestry.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneDividerSet {
    /// Existing operation-ledger key for exact-payload deduplication.
    pub key: OperationKey,
    /// Layout revision observed when the divider was selected.
    pub expected_layout_revision: u64,
    /// Exact pane whose ancestor divider is being adjusted.
    pub target: TargetRef,
    /// Spatial traversal divider index (0 through 2); runtime checks ancestry.
    pub divider_index: u8,
    /// Desired first-child span; runtime checks actual split bounds.
    pub first_cells: u16,
}

impl fmt::Debug for PaneDividerSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneDividerSet")
            .field("key", &"<redacted>")
            .field("expected_layout_revision", &self.expected_layout_revision)
            .field("target", &"<redacted>")
            .field("divider_index", &self.divider_index)
            .field("first_cells", &self.first_cells)
            .finish()
    }
}

/// Absolute zoom or unzoom request for one exact target.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneZoomSet {
    /// Existing operation-ledger key for exact-payload deduplication.
    pub key: OperationKey,
    /// Layout revision observed when zoom state was requested.
    pub expected_layout_revision: u64,
    /// Exact target to zoom, or the current zoom target to unzoom.
    pub target: TargetRef,
    /// Desired state, encoded strictly as zero or one.
    pub zoomed: bool,
}

impl fmt::Debug for PaneZoomSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneZoomSet")
            .field("key", &"<redacted>")
            .field("expected_layout_revision", &self.expected_layout_revision)
            .field("target", &"<redacted>")
            .field("zoomed", &self.zoomed)
            .finish()
    }
}

/// Typed schema-13 request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Request {
    /// Query the authoritative view.
    ViewQuery,
    /// Exchange two pane identities at their current tree locations.
    Swap(PaneSwap),
    /// Set an absolute first-child span on one current ancestor divider.
    DividerSet(PaneDividerSet),
    /// Set zoom state for an exact pane target.
    ZoomSet(PaneZoomSet),
}

impl Request {
    /// Return the admission class without making an authorization claim.
    pub const fn class(self) -> OperationClass {
        match self {
            Self::ViewQuery => OperationClass::Query,
            Self::Swap(_) | Self::DividerSet(_) | Self::ZoomSet(_) => {
                OperationClass::DurableMutation
            }
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ViewQuery => f.write_str("ViewQuery"),
            Self::Swap(value) => f.debug_tuple("Swap").field(value).finish(),
            Self::DividerSet(value) => f.debug_tuple("DividerSet").field(value).finish(),
            Self::ZoomSet(value) => f.debug_tuple("ZoomSet").field(value).finish(),
        }
    }
}

/// Underlying layout plus exact zoom state and confirmed PTY sizes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneOperationsView {
    /// Complete, unprojected schema-9 layout.
    pub layout: LayoutReply,
    /// Exact occupied target currently zoomed, if any.
    pub zoomed_target: Option<TargetRef>,
    /// Confirmed rows and columns in the same spatial slots as layout panes.
    /// Every occupied pane is present; every unused slot is absent.
    pub confirmed_sizes: [Option<(u16, u16)>; MAX_PANES],
}

impl fmt::Debug for PaneOperationsView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneOperationsView")
            .field("layout", &self.layout)
            .field(
                "zoomed_target",
                &self.zoomed_target.as_ref().map(|_| "<redacted>"),
            )
            .field("confirmed_sizes", &self.confirmed_sizes)
            .finish()
    }
}

/// Encode one validated request to its exact schema-13 record.
pub fn encode_request(request: &Request) -> Result<Vec<u8>, CodecError> {
    let (kind, length) = match request {
        Request::ViewQuery => (KIND_VIEW_QUERY, VIEW_QUERY_BYTES),
        Request::Swap(value) => {
            validate_swap(value)?;
            (KIND_SWAP, PANE_SWAP_BYTES)
        }
        Request::DividerSet(value) => {
            validate_divider(value)?;
            (KIND_DIVIDER_SET, PANE_TARGET_MUTATION_BYTES)
        }
        Request::ZoomSet(value) => {
            validate_zoom(value)?;
            (KIND_ZOOM_SET, PANE_TARGET_MUTATION_BYTES)
        }
    };
    let mut bytes = vec![0; length];
    put_header(&mut bytes, kind);
    match request {
        Request::ViewQuery => {}
        Request::Swap(value) => {
            put_key(&mut bytes, KEY_OFFSET, value.key);
            put_u64(&mut bytes, REVISION_OFFSET, value.expected_layout_revision);
            put_target(&mut bytes, TARGET_OFFSET, value.source);
            put_target(&mut bytes, SECOND_TARGET_OFFSET, value.destination);
        }
        Request::DividerSet(value) => {
            put_key(&mut bytes, KEY_OFFSET, value.key);
            put_u64(&mut bytes, REVISION_OFFSET, value.expected_layout_revision);
            put_target(&mut bytes, TARGET_OFFSET, value.target);
            bytes[76] = value.divider_index;
            put_u16(&mut bytes, 77, value.first_cells);
        }
        Request::ZoomSet(value) => {
            put_key(&mut bytes, KEY_OFFSET, value.key);
            put_u64(&mut bytes, REVISION_OFFSET, value.expected_layout_revision);
            put_target(&mut bytes, TARGET_OFFSET, value.target);
            bytes[76] = u8::from(value.zoomed);
        }
    }
    Ok(bytes)
}

/// Decode exactly one schema-13 request, rejecting trailing and reserved bytes.
pub fn decode_request(bytes: &[u8]) -> Result<Request, CodecError> {
    if bytes.len() < 4 {
        return Err(CodecError::Truncated);
    }
    check_schema(bytes)?;
    match read_u16(bytes, 2) {
        KIND_VIEW_QUERY => {
            exact_size(bytes, VIEW_QUERY_BYTES)?;
            zero(&bytes[4..8])?;
            Ok(Request::ViewQuery)
        }
        KIND_SWAP => {
            exact_size(bytes, PANE_SWAP_BYTES)?;
            let value = PaneSwap {
                key: read_key(bytes, KEY_OFFSET),
                expected_layout_revision: read_u64(bytes, REVISION_OFFSET),
                source: read_target(bytes, TARGET_OFFSET),
                destination: read_target(bytes, SECOND_TARGET_OFFSET),
            };
            validate_swap(&value)?;
            Ok(Request::Swap(value))
        }
        KIND_DIVIDER_SET => {
            exact_size(bytes, PANE_TARGET_MUTATION_BYTES)?;
            if bytes[79] != 0 {
                return Err(CodecError::NonzeroReserved);
            }
            let value = PaneDividerSet {
                key: read_key(bytes, KEY_OFFSET),
                expected_layout_revision: read_u64(bytes, REVISION_OFFSET),
                target: read_target(bytes, TARGET_OFFSET),
                divider_index: bytes[76],
                first_cells: read_u16(bytes, 77),
            };
            validate_divider(&value)?;
            Ok(Request::DividerSet(value))
        }
        KIND_ZOOM_SET => {
            exact_size(bytes, PANE_TARGET_MUTATION_BYTES)?;
            zero(&bytes[77..80])?;
            let zoomed = match bytes[76] {
                0 => false,
                1 => true,
                _ => return Err(CodecError::InvalidBoolean),
            };
            let value = PaneZoomSet {
                key: read_key(bytes, KEY_OFFSET),
                expected_layout_revision: read_u64(bytes, REVISION_OFFSET),
                target: read_target(bytes, TARGET_OFFSET),
                zoomed,
            };
            validate_zoom(&value)?;
            Ok(Request::ZoomSet(value))
        }
        _ => Err(CodecError::UnknownKind),
    }
}

/// Encode one strict schema-13 authoritative view.
pub fn encode_view(
    view: &PaneOperationsView,
) -> Result<[u8; PANE_OPERATIONS_VIEW_BYTES], CodecError> {
    validate_view(view)?;
    let mut bytes = [0; PANE_OPERATIONS_VIEW_BYTES];
    put_header(&mut bytes, KIND_VIEW);
    let layout = encode_layout_reply(&view.layout).map_err(map_layout_error)?;
    bytes[VIEW_LAYOUT_OFFSET..VIEW_LAYOUT_OFFSET + LAYOUT_REPLY_BYTES].copy_from_slice(&layout);
    if let Some(target) = view.zoomed_target {
        bytes[VIEW_ZOOM_PRESENT_OFFSET] = 1;
        put_target(&mut bytes, VIEW_ZOOM_TARGET_OFFSET, target);
    }
    for (index, size) in view.confirmed_sizes.iter().enumerate() {
        if let Some((rows, columns)) = size {
            let offset = VIEW_SIZES_OFFSET + index * VIEW_SIZE_ENTRY_BYTES;
            put_u16(&mut bytes, offset, 1);
            put_u16(&mut bytes, offset + 4, *rows);
            put_u16(&mut bytes, offset + 6, *columns);
        }
    }
    Ok(bytes)
}

/// Decode and validate one exact schema-13 authoritative view.
pub fn decode_view(bytes: &[u8]) -> Result<PaneOperationsView, CodecError> {
    exact_size(bytes, PANE_OPERATIONS_VIEW_BYTES)?;
    check_schema(bytes)?;
    if read_u16(bytes, 2) != KIND_VIEW {
        return Err(CodecError::UnknownKind);
    }
    zero(&bytes[VIEW_ZOOM_PRESENT_OFFSET + 1..VIEW_ZOOM_TARGET_OFFSET])?;
    let zoomed_target = match bytes[VIEW_ZOOM_PRESENT_OFFSET] {
        0 => {
            zero(&bytes[VIEW_ZOOM_TARGET_OFFSET..VIEW_SIZES_OFFSET])?;
            None
        }
        1 => Some(read_target(bytes, VIEW_ZOOM_TARGET_OFFSET)),
        _ => return Err(CodecError::InvalidBoolean),
    };
    let layout =
        decode_layout_reply(&bytes[VIEW_LAYOUT_OFFSET..VIEW_LAYOUT_OFFSET + LAYOUT_REPLY_BYTES])
            .map_err(map_layout_error)?;
    let mut confirmed_sizes = [None; MAX_PANES];
    for (index, slot) in confirmed_sizes.iter_mut().enumerate() {
        let offset = VIEW_SIZES_OFFSET + index * VIEW_SIZE_ENTRY_BYTES;
        zero(&bytes[offset + 2..offset + 4])?;
        match read_u16(bytes, offset) {
            0 => zero(&bytes[offset + 4..offset + VIEW_SIZE_ENTRY_BYTES])?,
            1 => *slot = Some((read_u16(bytes, offset + 4), read_u16(bytes, offset + 6))),
            _ => return Err(CodecError::InvalidBoolean),
        }
    }
    let view = PaneOperationsView {
        layout,
        zoomed_target,
        confirmed_sizes,
    };
    validate_view(&view)?;
    Ok(view)
}

fn validate_swap(value: &PaneSwap) -> Result<(), CodecError> {
    validate_key(value.key)?;
    validate_revision(value.expected_layout_revision)?;
    validate_target(value.source)?;
    validate_target(value.destination)?;
    if value.source.generation != value.key.generation
        || value.destination.generation != value.key.generation
    {
        return Err(CodecError::GenerationMismatch);
    }
    if value.source.workspace != value.destination.workspace {
        return Err(CodecError::WorkspaceMismatch);
    }
    if value.source == value.destination
        || (value.source.pane == value.destination.pane
            && value.source.session == value.destination.session)
    {
        return Err(CodecError::DuplicateTarget);
    }
    Ok(())
}

fn validate_divider(value: &PaneDividerSet) -> Result<(), CodecError> {
    validate_key(value.key)?;
    validate_revision(value.expected_layout_revision)?;
    validate_target(value.target)?;
    if value.target.generation != value.key.generation {
        return Err(CodecError::GenerationMismatch);
    }
    if usize::from(value.divider_index) >= MAX_PANES - 1 {
        return Err(CodecError::InvalidDividerIndex);
    }
    if value.first_cells == 0 || value.first_cells > MAX_PANE_ROWS.max(MAX_PANE_COLUMNS) - 1 {
        return Err(CodecError::InvalidDividerPosition);
    }
    Ok(())
}

fn validate_zoom(value: &PaneZoomSet) -> Result<(), CodecError> {
    validate_key(value.key)?;
    validate_revision(value.expected_layout_revision)?;
    validate_target(value.target)?;
    if value.target.generation != value.key.generation {
        return Err(CodecError::GenerationMismatch);
    }
    Ok(())
}

fn validate_view(view: &PaneOperationsView) -> Result<(), CodecError> {
    let count = view.layout.panes.iter().flatten().count();
    if view.confirmed_sizes.iter().flatten().count() != count {
        return Err(CodecError::InvalidView);
    }
    if let Some(target) = view.zoomed_target {
        validate_target(target)?;
        if !view
            .layout
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == target)
        {
            return Err(CodecError::InvalidView);
        }
    }
    for (index, pane) in view.layout.panes.iter().enumerate() {
        match (pane, view.confirmed_sizes[index]) {
            (None, None) => {}
            (Some(pane), Some((rows, columns))) => {
                if !(5..=MAX_PANE_ROWS - 2).contains(&rows)
                    || !(20..=MAX_PANE_COLUMNS - 2).contains(&columns)
                {
                    return Err(CodecError::InvalidView);
                }
                if pane
                    .pending_confirmed
                    .is_some_and(|size| size != (rows, columns))
                {
                    return Err(CodecError::InvalidView);
                }
                let underlying = (
                    pane.rect.rows.saturating_sub(2),
                    pane.rect.columns.saturating_sub(2),
                );
                if pane.pending_confirmed.is_none() && (rows, columns) != underlying {
                    return Err(CodecError::InvalidView);
                }
                if pane.pending_confirmed.is_some() && (rows, columns) == underlying {
                    return Err(CodecError::InvalidView);
                }
            }
            _ => return Err(CodecError::InvalidView),
        }
    }
    Ok(())
}

fn validate_key(key: OperationKey) -> Result<(), CodecError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        Err(CodecError::ZeroKeyComponent)
    } else {
        Ok(())
    }
}

fn validate_revision(revision: u64) -> Result<(), CodecError> {
    if revision == 0 {
        Err(CodecError::ZeroRevision)
    } else {
        Ok(())
    }
}

fn validate_target(target: TargetRef) -> Result<(), CodecError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        Err(CodecError::InvalidTarget)
    } else {
        Ok(())
    }
}

fn map_layout_error(_: UiCodecError) -> CodecError {
    CodecError::InvalidView
}

fn check_schema(bytes: &[u8]) -> Result<(), CodecError> {
    if read_u16(bytes, 0) == PANE_OPERATIONS_SCHEMA {
        Ok(())
    } else {
        Err(CodecError::UnsupportedSchema)
    }
}

fn exact_size(bytes: &[u8], expected: usize) -> Result<(), CodecError> {
    match bytes.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(CodecError::Truncated),
        std::cmp::Ordering::Greater => Err(CodecError::TrailingBytes),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn zero(bytes: &[u8]) -> Result<(), CodecError> {
    if bytes.iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(CodecError::NonzeroReserved)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_be_bytes(raw)
}

fn read_key(bytes: &[u8], offset: usize) -> OperationKey {
    OperationKey {
        generation: read_u64(bytes, offset),
        lease: read_u64(bytes, offset + 8),
        sequence: read_u64(bytes, offset + 16),
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

fn put_header(bytes: &mut [u8], kind: u16) {
    put_u16(bytes, 0, PANE_OPERATIONS_SCHEMA);
    put_u16(bytes, 2, kind);
}

fn put_key(bytes: &mut [u8], offset: usize, key: OperationKey) {
    put_u64(bytes, offset, key.generation);
    put_u64(bytes, offset + 8, key.lease);
    put_u64(bytes, offset + 16, key.sequence);
}

fn put_target(bytes: &mut [u8], offset: usize, target: TargetRef) {
    put_u64(bytes, offset, target.generation);
    put_u64(bytes, offset + 8, target.workspace);
    put_u64(bytes, offset + 16, target.pane);
    put_u64(bytes, offset + 24, target.session);
    put_u64(bytes, offset + 32, target.occupant_revision);
}
