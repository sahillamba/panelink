//! Bounded borrowed wire codec for the experimental schema-8 pane UI protocol.
//!
//! This module validates and encodes layout and screen queries, retained-history
//! queries, typed key and mouse-wheel input, and absolute root resize requests. It
//! performs no socket, process, layout, ledger or dispatch work and authorizes
//! nothing. All integers are big-endian; offsets are from payload start.
//!
//! Requests (client to server):
//!
//! | Kind | Name | Size | Class |
//! | ---: | --- | ---: | --- |
//! | 1 | LayoutQuery | 8 | Query |
//! | 2 | ScreenQuery | 48 | Query |
//! | 3 | KeyInput | 88 + 8 * count | Input |
//! | 4 | RootResize | 40 | AbsoluteResize |
//! | 6 | HistoryQuery | 56 | Query |
//! | 7 | MouseWheelInput | 88 | Input |
//!
//! LayoutQuery: `0 schema 8 | 2 kind 1 | 4 reserved u32 zero`.
//!
//! ScreenQuery: `0 schema | 2 kind 2 | 4 reserved u32 zero | 8 target (40)`.
//!
//! HistoryQuery: `0 schema | 2 kind 6 | 4 reserved u32 zero | 8 target (40) |
//! 48 offset rows u16 | 50 reserved 6 zero`. It is READ-only; the reply also
//! identifies the active terminal buffer and mouse-reporting mode.
//!
//! MouseWheelInput: `0 schema | 2 kind 7 | 4 operation key (24) | 28 target
//! (40) | 68 expected input-owner revision u64 | 76 column u16 | 78 row u16 |
//! 80 direction u16 (1 up, 2 down) | 82 reserved 6 zero`. It is a
//! non-replayable input operation and uses the normal input ownership and
//! exact-payload deduplication path.
//!
//! KeyInput:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 4 | schema 8, kind 3 |
//! | 4 | 24 | operation key: generation, lease, sequence |
//! | 28 | 40 | target |
//! | 68 | 8 | expected input-owner revision, nonzero |
//! | 76 | 2 | key count, 1..=64 |
//! | 78 | 10 | reserved, zero |
//! | 88 | 8 each | key: code u16, reserved u16 zero, scalar u32 |
//!
//! Key codes 1..=12 preserve their original Char, Enter, Tab, Backspace,
//! Escape, Interrupt, arrows, Home, and End meanings. Code 13 is Control
//! (scalar 1..=26), 14 AltChar (non-control Unicode scalar), and 15 Function
//! (scalar 1..=12). Codes 16..=20 are Delete, Insert, PageUp, PageDown and
//! BackTab with scalar zero. Other non-character keys require zero scalar.
//!
//! RootResize: `0 schema | 2 kind 4 | 4 key (24) | 28 expected layout revision
//! u64 nonzero | 36 rows u16 | 38 columns u16`.
//!
//! AttachTarget Hello trailer (kind 5, 48 bytes): `0 schema | 2 kind 5 | 4
//! reserved u32 zero | 8 target (40)`. A multi-session Hello frame payload is
//! one existing SessionHello (28 or 44 bytes) optionally followed by this
//! trailer. Grants are pinned to one target, so a UI opens one connection per
//! pane and names that pane here; without a trailer a fresh Hello attaches to
//! the first layout pane and a resume Hello to its lease's pinned target.
//!
//! Replies (server to client):
//!
//! | Kind | Name | Size |
//! | ---: | --- | ---: |
//! | `0x8001` | LayoutReply schema 9 | 424 |
//! | `0x8002` | ScreenReply header | 64 + checkpoint (Snapshot frame) |
//! | `0x8003` | ResizeReply | 48 |
//! | `0x8004` | HistoryReply header | 64 + checkpoint (Snapshot frame) |
//!
//! LayoutReply:
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0 | 4 | schema 8, kind `0x8001` |
//! | 4 | 8 | layout revision, nonzero |
//! | 12 | 2 | root rows |
//! | 14 | 2 | root columns |
//! | 16 | 2 | pane count, 1 through 4 |
//! | 18 | 2 | divider count, 0 through 3 |
//! | 20 | 24 | three divider rectangles (8 bytes each; zero when absent) |
//! | 44 | 4 | reserved, zero |
//! | 48 | 288 | four pane entries (72 bytes each; zero when absent) |
//! | 336 | 88 | reserved, zero |
//!
//! Pane entry (72 bytes): target (40), rect x, y, rows, columns (8),
//! pending-resize flag u16 (0 or 1), reserved u16 zero, confirmed rows u16,
//! confirmed columns u16, reserved 16 zero.
//!
//! ScreenReply (carried in one `MessageKind::Snapshot` frame): `0 schema |
//! 2 kind 0x8002 | 4 target (40) | 44 screen revision u64 nonzero | 52
//! checkpoint length u32 | 56 reserved 8 zero | 64 checkpoint bytes` where the
//! checkpoint is one complete `screen_checkpoint` record for the same target.
//!
//! HistoryReply (Snapshot frame) uses the ScreenReply layout with the offset
//! at 52, mode flags at 54 (bit 0 alternate screen, bit 1 mouse reporting),
//! checkpoint length at 56 and checkpoint bytes at 64. Alternate-screen
//! history is clamped to zero so a hidden shell buffer is never shown as app
//! content.
//!
//! ResizeReply: `0 schema | 2 kind 0x8003 | 4 key (24) | 28 status u16 (1
//! Applied, 2 ResizePending) | 30 pending pane flags u16 (bit i = spatial pane
//! i, i < MAX_PANES; higher bits zero) | 32 committed layout revision u64 nonzero | 40 rows u16 | 42
//! columns u16 | 44 reserved 4 zero`.

use crate::control_protocol::TargetRef;
use crate::pane_layout::{
    LayoutError, Rect, SplitDirection, MAX_PANES, MAX_PANE_COLUMNS, MAX_PANE_ROWS,
    MIN_PANE_COLUMNS, MIN_PANE_ROWS,
};
use crate::protocol::{OperationClass, OperationKey};
use crate::terminal::Key;
use std::error::Error;
use std::fmt;

/// Wire schema used for the experimental pane-UI payloads.
pub const UI_SCHEMA: u16 = 8;
/// LayoutReply wire version; the larger fixed payload rejects schema-8 layouts.
pub const LAYOUT_REPLY_SCHEMA: u16 = 9;
/// Exact size of a LayoutQuery request.
pub const LAYOUT_QUERY_BYTES: usize = 8;
/// Exact size of a ScreenQuery request.
pub const SCREEN_QUERY_BYTES: usize = 48;
/// Exact size of a HistoryQuery request.
pub const HISTORY_QUERY_BYTES: usize = 56;
/// Maximum retained-history offset accepted by a HistoryQuery.
pub const MAX_HISTORY_OFFSET_ROWS: u16 = 8_192;
/// Fixed KeyInput prefix before the key entries.
pub const KEY_INPUT_PREFIX_BYTES: usize = 88;
/// Encoded size of one key entry.
pub const KEY_ENTRY_BYTES: usize = 8;
/// Maximum number of keys in one KeyInput request.
pub const MAX_KEYS: usize = 64;
/// Maximum complete KeyInput request.
pub const MAX_KEY_INPUT_BYTES: usize = KEY_INPUT_PREFIX_BYTES + MAX_KEYS * KEY_ENTRY_BYTES;
/// Exact size of a RootResize request.
pub const ROOT_RESIZE_BYTES: usize = 40;
/// Exact size of the AttachTarget Hello trailer.
pub const ATTACH_TARGET_BYTES: usize = 48;
/// Exact size of a LayoutReply.
pub const LAYOUT_REPLY_BYTES: usize = 424;
/// Fixed ScreenReply header before the checkpoint record.
pub const SCREEN_REPLY_HEADER_BYTES: usize = 64;
/// Exact size of a ResizeReply.
pub const RESIZE_REPLY_BYTES: usize = 48;

const KIND_LAYOUT_QUERY: u16 = 1;
const KIND_SCREEN_QUERY: u16 = 2;
const KIND_KEY_INPUT: u16 = 3;
const KIND_ROOT_RESIZE: u16 = 4;
const KIND_ATTACH_TARGET: u16 = 5;
const KIND_HISTORY_QUERY: u16 = 6;
const KIND_MOUSE_WHEEL_INPUT: u16 = 7;
const KIND_LAYOUT_REPLY: u16 = 0x8001;
const KIND_SCREEN_REPLY: u16 = 0x8002;
const KIND_RESIZE_REPLY: u16 = 0x8003;
const KIND_HISTORY_REPLY: u16 = 0x8004;
const PANE_ENTRY_BYTES: usize = 72;
const DIVIDER_OFFSET: usize = 20;
const PANE0_OFFSET: usize = 48;
const MAX_SCREEN_REPLY_CHECKPOINT_BYTES: usize =
    crate::screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES;

/// Maximum complete ScreenReply payload (header plus largest checkpoint).
pub const MAX_SCREEN_REPLY_BYTES: usize =
    SCREEN_REPLY_HEADER_BYTES + MAX_SCREEN_REPLY_CHECKPOINT_BYTES;
/// Maximum complete HistoryReply payload (header plus checkpoint).
pub const MAX_HISTORY_REPLY_BYTES: usize =
    SCREEN_REPLY_HEADER_BYTES + MAX_SCREEN_REPLY_CHECKPOINT_BYTES;
/// Fixed MouseWheelInput request size.
pub const MOUSE_WHEEL_INPUT_BYTES: usize = 88;

/// Fixed, data-free validation errors for schema-9 payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiCodecError {
    /// The payload is shorter than its fixed or declared size.
    Truncated,
    /// Bytes follow a complete fixed or declared payload.
    TrailingBytes,
    /// The payload exceeds its kind's limit.
    PayloadTooLarge,
    /// The schema is not 9.
    UnsupportedSchema,
    /// The request or reply kind is unknown.
    UnknownKind,
    /// A reserved field contains a nonzero byte.
    NonzeroReserved,
    /// An operation-key component is zero.
    ZeroKeyComponent,
    /// A target identity component is zero.
    InvalidTarget,
    /// A target generation differs from the operation-key generation.
    GenerationMismatch,
    /// A required revision is zero.
    ZeroRevision,
    /// The key count is zero or above [`MAX_KEYS`].
    InvalidKeyCount,
    /// A key code is unknown.
    UnknownKeyCode,
    /// A Char scalar is not a non-control Unicode scalar, or a non-Char key
    /// carries a nonzero scalar.
    InvalidKeyScalar,
    /// Rows or columns are outside the supported pane bounds.
    InvalidDimensions,
    /// Layout panes, divider and flags are inconsistent.
    InvalidLayout,
    /// The screen checkpoint is malformed or does not match the reply.
    InvalidCheckpoint,
    /// A status or flag code is unknown.
    InvalidStatus,
    /// A history offset exceeds the supported bound.
    InvalidHistoryOffset,
    /// A mouse wheel direction is invalid.
    InvalidMouseDirection,
}

impl fmt::Display for UiCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "pane-ui payload is truncated",
            Self::TrailingBytes => "bytes follow the pane-ui payload",
            Self::PayloadTooLarge => "pane-ui payload exceeds its limit",
            Self::UnsupportedSchema => "pane-ui schema is unsupported",
            Self::UnknownKind => "pane-ui kind is unknown",
            Self::NonzeroReserved => "pane-ui reserved bytes must be zero",
            Self::ZeroKeyComponent => "pane-ui key components must be nonzero",
            Self::InvalidTarget => "pane-ui target components must be nonzero",
            Self::GenerationMismatch => "pane-ui target generation does not match the key",
            Self::ZeroRevision => "pane-ui revision must be nonzero",
            Self::InvalidKeyCount => "pane-ui key count is invalid",
            Self::UnknownKeyCode => "pane-ui key code is unknown",
            Self::InvalidKeyScalar => "pane-ui key scalar is invalid",
            Self::InvalidDimensions => "pane-ui dimensions are outside supported bounds",
            Self::InvalidLayout => "pane-ui layout is inconsistent",
            Self::InvalidCheckpoint => "pane-ui screen checkpoint is invalid",
            Self::InvalidStatus => "pane-ui status code is invalid",
            Self::InvalidHistoryOffset => "pane-ui history offset exceeds its limit",
            Self::InvalidMouseDirection => "pane-ui mouse direction is invalid",
        })
    }
}

impl Error for UiCodecError {}

/// Typed KeyInput request for encoding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct KeyInput<'a> {
    /// Operation key; never replayed (`OperationClass::Input`).
    pub key: OperationKey,
    /// Complete target identity receiving the keys.
    pub target: TargetRef,
    /// Expected current input-owner revision.
    pub expected_owner_revision: u64,
    /// One to [`MAX_KEYS`] typed keys.
    pub keys: &'a [Key],
}

impl KeyInput<'_> {
    /// Classify as non-replayable input.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::Input
    }

    /// Validate every field and return the exact encoded size.
    pub fn encoded_len(&self) -> Result<usize, UiCodecError> {
        validate_key(self.key)?;
        validate_target(self.target)?;
        if self.target.generation != self.key.generation {
            return Err(UiCodecError::GenerationMismatch);
        }
        if self.expected_owner_revision == 0 {
            return Err(UiCodecError::ZeroRevision);
        }
        if self.keys.is_empty() || self.keys.len() > MAX_KEYS {
            return Err(UiCodecError::InvalidKeyCount);
        }
        for key in self.keys {
            key_code(*key)?;
        }
        Ok(KEY_INPUT_PREFIX_BYTES + self.keys.len() * KEY_ENTRY_BYTES)
    }
}

impl fmt::Debug for KeyInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyInput")
            .field("key_count", &self.keys.len())
            .finish_non_exhaustive()
    }
}

/// A decoded, fully validated KeyInput borrowing its key entries.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct KeyInputView<'a> {
    key: OperationKey,
    target: TargetRef,
    expected_owner_revision: u64,
    entries: &'a [u8],
}

/// Read-only request for a normal-buffer viewport at an offset from live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryQuery {
    /// Complete target identity.
    pub target: TargetRef,
    /// Rows to move back from the live bottom; zero requests the live view.
    pub offset_rows: u16,
}

/// One non-replayable mouse-wheel event for a pane.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MouseWheelInput {
    /// Operation key; never replayed (`OperationClass::Input`).
    pub key: OperationKey,
    /// Complete target identity receiving the event.
    pub target: TargetRef,
    /// Expected current input-owner revision.
    pub expected_owner_revision: u64,
    /// Zero-based column within the pane content rectangle.
    pub column: u16,
    /// Zero-based row within the pane content rectangle.
    pub row: u16,
    /// `true` scrolls up; `false` scrolls down.
    pub up: bool,
}

impl MouseWheelInput {
    /// Classify as non-replayable input.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::Input
    }

    fn validate(&self) -> Result<(), UiCodecError> {
        validate_key(self.key)?;
        validate_target(self.target)?;
        if self.target.generation != self.key.generation {
            return Err(UiCodecError::GenerationMismatch);
        }
        if self.expected_owner_revision == 0 {
            return Err(UiCodecError::ZeroRevision);
        }
        if self.column >= MAX_PANE_COLUMNS || self.row >= MAX_PANE_ROWS {
            return Err(UiCodecError::InvalidDimensions);
        }
        Ok(())
    }
}

impl fmt::Debug for MouseWheelInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MouseWheelInput")
            .field("direction", &if self.up { "up" } else { "down" })
            .finish_non_exhaustive()
    }
}

impl<'a> KeyInputView<'a> {
    /// Validated operation key.
    pub const fn key(&self) -> OperationKey {
        self.key
    }

    /// Validated complete target.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Nonzero expected input-owner revision.
    pub const fn expected_owner_revision(&self) -> u64 {
        self.expected_owner_revision
    }

    /// Number of validated keys, 1 to [`MAX_KEYS`].
    pub const fn key_count(&self) -> usize {
        self.entries.len() / KEY_ENTRY_BYTES
    }

    /// Iterate the validated keys in order.
    pub fn keys(&self) -> impl ExactSizeIterator<Item = Key> + 'a {
        self.entries
            .as_chunks::<KEY_ENTRY_BYTES>()
            .0
            .iter()
            // Every entry was validated by the decoder.
            .map(|entry| decode_key_entry(entry).unwrap_or(Key::Enter))
    }
}

impl fmt::Debug for KeyInputView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyInputView")
            .field("key_count", &self.key_count())
            .finish_non_exhaustive()
    }
}

/// Absolute root resize request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RootResize {
    /// Operation key; the request is an absolute, reconcilable size.
    pub key: OperationKey,
    /// Layout revision the caller observed.
    pub expected_layout_revision: u64,
    /// Requested root rows.
    pub rows: u16,
    /// Requested root columns.
    pub columns: u16,
}

impl RootResize {
    /// Classify as an absolute resize.
    pub const fn operation_class(&self) -> OperationClass {
        OperationClass::AbsoluteResize
    }

    fn validate(&self) -> Result<(), UiCodecError> {
        validate_key(self.key)?;
        if self.expected_layout_revision == 0 {
            return Err(UiCodecError::ZeroRevision);
        }
        validate_root(self.rows, self.columns)
    }
}

impl fmt::Debug for RootResize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RootResize")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

/// One decoded schema-8 request.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum UiRequest<'a> {
    /// Read the current layout.
    LayoutQuery,
    /// Read one target's styled screen checkpoint.
    ScreenQuery {
        /// Complete target identity.
        target: TargetRef,
    },
    /// Read a bounded normal-buffer viewport without input ownership.
    HistoryQuery(HistoryQuery),
    /// Deliver typed keys to one target.
    KeyInput(KeyInputView<'a>),
    /// Deliver one scroll-wheel event as non-replayable input.
    MouseWheelInput(MouseWheelInput),
    /// Resize the layout root.
    RootResize(RootResize),
}

impl UiRequest<'_> {
    /// Classify this request under the existing operation protocol.
    pub const fn operation_class(&self) -> OperationClass {
        match self {
            Self::LayoutQuery | Self::ScreenQuery { .. } | Self::HistoryQuery(_) => {
                OperationClass::Query
            }
            Self::KeyInput(_) | Self::MouseWheelInput(_) => OperationClass::Input,
            Self::RootResize(_) => OperationClass::AbsoluteResize,
        }
    }
}

impl fmt::Debug for UiRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayoutQuery => f.write_str("LayoutQuery"),
            Self::ScreenQuery { .. } => f.write_str("ScreenQuery { <redacted> }"),
            Self::HistoryQuery(_) => f.write_str("HistoryQuery { <redacted> }"),
            Self::KeyInput(view) => view.fmt(f),
            Self::MouseWheelInput(input) => input.fmt(f),
            Self::RootResize(resize) => resize.fmt(f),
        }
    }
}

/// One pane in a layout reply.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct UiPane {
    /// Complete pane occupant identity.
    pub target: TargetRef,
    /// Pane rectangle in root cells.
    pub rect: Rect,
    /// Last confirmed PTY `(rows, columns)` while an absolute resize is
    /// pending; `None` when the PTY matches `rect`.
    pub pending_confirmed: Option<(u16, u16)>,
}

impl fmt::Debug for UiPane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiPane")
            .field("target", &"<redacted>")
            .field("rect", &self.rect)
            .field("pending_resize", &self.pending_confirmed.is_some())
            .finish()
    }
}

/// Fixed-size layout observation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct LayoutReply {
    /// Nonzero layout revision.
    pub revision: u64,
    /// Root rows.
    pub rows: u16,
    /// Root columns.
    pub columns: u16,
    /// Spatially ordered panes; pane 0 is always present.
    pub panes: [Option<UiPane>; crate::pane_layout::MAX_PANES],
    /// First divider, retained for source compatibility with older callers.
    pub divider: Option<Rect>,
    /// Spatially traversed one-cell dividers; unused entries are `None`.
    pub dividers: [Option<Rect>; crate::pane_layout::MAX_PANES - 1],
}

impl fmt::Debug for LayoutReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutReply")
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("panes", &self.panes)
            .field("divider", &self.divider)
            .finish_non_exhaustive()
    }
}

/// Borrowed decoded screen reply.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ScreenReplyView<'a> {
    target: TargetRef,
    screen_revision: u64,
    checkpoint: &'a [u8],
}

impl<'a> ScreenReplyView<'a> {
    /// Target whose screen this is.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Nonzero screen revision represented by the checkpoint.
    pub const fn screen_revision(&self) -> u64 {
        self.screen_revision
    }

    /// Complete validated `screen_checkpoint` record bytes.
    pub const fn checkpoint(&self) -> &'a [u8] {
        self.checkpoint
    }
}

impl fmt::Debug for ScreenReplyView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenReplyView")
            .field("checkpoint_len", &self.checkpoint.len())
            .finish_non_exhaustive()
    }
}

/// Borrowed decoded history reply with the server's current input mode.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct HistoryReplyView<'a> {
    target: TargetRef,
    screen_revision: u64,
    offset_rows: u16,
    alternate_screen: bool,
    mouse_reporting: bool,
    checkpoint: &'a [u8],
}

impl<'a> HistoryReplyView<'a> {
    /// Target whose view this is.
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    /// Revision represented by the checkpoint.
    pub const fn screen_revision(&self) -> u64 {
        self.screen_revision
    }

    /// Effective bounded offset returned by the server.
    pub const fn offset_rows(&self) -> u16 {
        self.offset_rows
    }

    /// Whether the child currently uses its alternate screen.
    pub const fn alternate_screen(&self) -> bool {
        self.alternate_screen
    }

    /// Whether the child currently requests mouse reporting.
    pub const fn mouse_reporting(&self) -> bool {
        self.mouse_reporting
    }

    /// Validated screen checkpoint bytes.
    pub const fn checkpoint(&self) -> &'a [u8] {
        self.checkpoint
    }
}

impl fmt::Debug for HistoryReplyView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryReplyView")
            .field("offset_rows", &self.offset_rows)
            .field("alternate_screen", &self.alternate_screen)
            .field("mouse_reporting", &self.mouse_reporting)
            .field("checkpoint_len", &self.checkpoint.len())
            .finish_non_exhaustive()
    }
}

/// Root resize result status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeStatus {
    /// Every pane PTY accepted its absolute size.
    Applied,
    /// The layout committed but at least one pane PTY resize is pending.
    ResizePending,
}

/// Fixed-size root resize result.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ResizeReply {
    /// Key of the RootResize request.
    pub key: OperationKey,
    /// Applied or ResizePending.
    pub status: ResizeStatus,
    /// Which layout panes are still resize-pending.
    pub pending: [bool; crate::pane_layout::MAX_PANES],
    /// Committed (current) layout revision.
    pub revision: u64,
    /// Committed root rows.
    pub rows: u16,
    /// Committed root columns.
    pub columns: u16,
}

impl fmt::Debug for ResizeReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResizeReply")
            .field("status", &self.status)
            .field("pending", &self.pending)
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

/// Compute the new pane's `(rows, columns)` using the same pure geometry the
/// server will apply.
///
/// This is a client-side preview only; the server recomputes and rejects a
/// mismatched launch size. A layout at capacity reports `PaneLimitExceeded`.
pub fn planned_split_size(
    layout: &LayoutReply,
    target: TargetRef,
    direction: SplitDirection,
) -> Result<(u16, u16), LayoutError> {
    let pane = layout
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == target)
        .ok_or(LayoutError::StaleTarget)?;
    let outer = crate::pane_layout::split_geometry(pane.rect, direction)?;
    let content = crate::pane_layout::content_rect(crate::pane_layout::Rect {
        x: 0,
        y: 0,
        rows: outer.0,
        columns: outer.1,
    })
    .ok_or(LayoutError::InvalidLayoutState)?;
    Ok((content.rows, content.columns))
}

/// Encode a LayoutQuery.
pub fn encode_layout_query() -> [u8; LAYOUT_QUERY_BYTES] {
    let mut bytes = [0_u8; LAYOUT_QUERY_BYTES];
    put_header(&mut bytes, KIND_LAYOUT_QUERY);
    bytes
}

/// Validate and encode a ScreenQuery for one complete target.
pub fn encode_screen_query(target: TargetRef) -> Result<[u8; SCREEN_QUERY_BYTES], UiCodecError> {
    validate_target(target)?;
    let mut bytes = [0_u8; SCREEN_QUERY_BYTES];
    put_header(&mut bytes, KIND_SCREEN_QUERY);
    put_target(&mut bytes, 8, target);
    Ok(bytes)
}

/// Validate and encode one bounded history-view query.
pub fn encode_history_query(
    target: TargetRef,
    offset_rows: u16,
) -> Result<[u8; HISTORY_QUERY_BYTES], UiCodecError> {
    validate_target(target)?;
    if offset_rows > MAX_HISTORY_OFFSET_ROWS {
        return Err(UiCodecError::InvalidHistoryOffset);
    }
    let mut bytes = [0_u8; HISTORY_QUERY_BYTES];
    put_header(&mut bytes, KIND_HISTORY_QUERY);
    put_target(&mut bytes, 8, target);
    put_u16(&mut bytes, 48, offset_rows);
    Ok(bytes)
}

/// Validate and encode one non-replayable mouse-wheel input.
pub fn encode_mouse_wheel_input(
    request: &MouseWheelInput,
) -> Result<[u8; MOUSE_WHEEL_INPUT_BYTES], UiCodecError> {
    request.validate()?;
    let mut bytes = [0_u8; MOUSE_WHEEL_INPUT_BYTES];
    put_header(&mut bytes, KIND_MOUSE_WHEEL_INPUT);
    put_key(&mut bytes, 4, request.key);
    put_target(&mut bytes, 28, request.target);
    put_u64(&mut bytes, 68, request.expected_owner_revision);
    put_u16(&mut bytes, 76, request.column);
    put_u16(&mut bytes, 78, request.row);
    put_u16(&mut bytes, 80, if request.up { 1 } else { 2 });
    Ok(bytes)
}

/// Validate and encode the AttachTarget Hello trailer.
pub fn encode_attach_target(target: TargetRef) -> Result<[u8; ATTACH_TARGET_BYTES], UiCodecError> {
    validate_target(target)?;
    let mut bytes = [0_u8; ATTACH_TARGET_BYTES];
    put_header(&mut bytes, KIND_ATTACH_TARGET);
    put_target(&mut bytes, 8, target);
    Ok(bytes)
}

/// Decode one exact AttachTarget Hello trailer.
pub fn decode_attach_target(bytes: &[u8]) -> Result<TargetRef, UiCodecError> {
    exact_size(bytes, ATTACH_TARGET_BYTES)?;
    header(bytes, KIND_ATTACH_TARGET)?;
    zero(&bytes[4..8])?;
    let target = read_target(bytes, 8);
    validate_target(target)?;
    Ok(target)
}

/// Validate a KeyInput completely, then encode it into one bounded buffer.
pub fn encode_key_input(request: &KeyInput<'_>) -> Result<Vec<u8>, UiCodecError> {
    let length = request.encoded_len()?;
    let mut bytes = vec![0_u8; length];
    put_header(&mut bytes, KIND_KEY_INPUT);
    put_key(&mut bytes, 4, request.key);
    put_target(&mut bytes, 28, request.target);
    put_u64(&mut bytes, 68, request.expected_owner_revision);
    put_u16(&mut bytes, 76, request.keys.len() as u16);
    for (index, key) in request.keys.iter().enumerate() {
        let (code, scalar) = key_code(*key)?;
        let offset = KEY_INPUT_PREFIX_BYTES + index * KEY_ENTRY_BYTES;
        put_u16(&mut bytes, offset, code);
        put_u32(&mut bytes, offset + 4, scalar);
    }
    Ok(bytes)
}

/// Validate and encode a RootResize.
pub fn encode_root_resize(request: &RootResize) -> Result<[u8; ROOT_RESIZE_BYTES], UiCodecError> {
    request.validate()?;
    let mut bytes = [0_u8; ROOT_RESIZE_BYTES];
    put_header(&mut bytes, KIND_ROOT_RESIZE);
    put_key(&mut bytes, 4, request.key);
    put_u64(&mut bytes, 28, request.expected_layout_revision);
    put_u16(&mut bytes, 36, request.rows);
    put_u16(&mut bytes, 38, request.columns);
    Ok(bytes)
}

/// Decode and fully validate one schema-8 request.
///
/// The size is checked against the kind's bound before any field is parsed.
pub fn decode_ui_request(bytes: &[u8]) -> Result<UiRequest<'_>, UiCodecError> {
    if bytes.len() < 4 {
        return Err(UiCodecError::Truncated);
    }
    if read_u16(bytes, 0) != UI_SCHEMA {
        return Err(UiCodecError::UnsupportedSchema);
    }
    match read_u16(bytes, 2) {
        KIND_LAYOUT_QUERY => {
            exact_size(bytes, LAYOUT_QUERY_BYTES)?;
            zero(&bytes[4..8])?;
            Ok(UiRequest::LayoutQuery)
        }
        KIND_SCREEN_QUERY => {
            exact_size(bytes, SCREEN_QUERY_BYTES)?;
            zero(&bytes[4..8])?;
            let target = read_target(bytes, 8);
            validate_target(target)?;
            Ok(UiRequest::ScreenQuery { target })
        }
        KIND_HISTORY_QUERY => {
            exact_size(bytes, HISTORY_QUERY_BYTES)?;
            let target = read_target(bytes, 8);
            validate_target(target)?;
            let offset_rows = read_u16(bytes, 48);
            if offset_rows > MAX_HISTORY_OFFSET_ROWS {
                return Err(UiCodecError::InvalidHistoryOffset);
            }
            zero(&bytes[50..])?;
            Ok(UiRequest::HistoryQuery(HistoryQuery {
                target,
                offset_rows,
            }))
        }
        KIND_KEY_INPUT => decode_key_input(bytes).map(UiRequest::KeyInput),
        KIND_MOUSE_WHEEL_INPUT => decode_mouse_wheel_input(bytes).map(UiRequest::MouseWheelInput),
        KIND_ROOT_RESIZE => {
            exact_size(bytes, ROOT_RESIZE_BYTES)?;
            let request = RootResize {
                key: read_key(bytes, 4),
                expected_layout_revision: read_u64(bytes, 28),
                rows: read_u16(bytes, 36),
                columns: read_u16(bytes, 38),
            };
            request.validate()?;
            Ok(UiRequest::RootResize(request))
        }
        _ => Err(UiCodecError::UnknownKind),
    }
}

fn decode_key_input(bytes: &[u8]) -> Result<KeyInputView<'_>, UiCodecError> {
    if bytes.len() > MAX_KEY_INPUT_BYTES {
        return Err(UiCodecError::PayloadTooLarge);
    }
    if bytes.len() < KEY_INPUT_PREFIX_BYTES {
        return Err(UiCodecError::Truncated);
    }
    let key = read_key(bytes, 4);
    validate_key(key)?;
    let target = read_target(bytes, 28);
    validate_target(target)?;
    if target.generation != key.generation {
        return Err(UiCodecError::GenerationMismatch);
    }
    let expected_owner_revision = read_u64(bytes, 68);
    if expected_owner_revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    let count = usize::from(read_u16(bytes, 76));
    if count == 0 || count > MAX_KEYS {
        return Err(UiCodecError::InvalidKeyCount);
    }
    zero(&bytes[78..KEY_INPUT_PREFIX_BYTES])?;
    exact_size(bytes, KEY_INPUT_PREFIX_BYTES + count * KEY_ENTRY_BYTES)?;
    let entries = &bytes[KEY_INPUT_PREFIX_BYTES..];
    for entry in entries.as_chunks::<KEY_ENTRY_BYTES>().0 {
        decode_key_entry(entry)?;
    }
    Ok(KeyInputView {
        key,
        target,
        expected_owner_revision,
        entries,
    })
}

fn decode_mouse_wheel_input(bytes: &[u8]) -> Result<MouseWheelInput, UiCodecError> {
    exact_size(bytes, MOUSE_WHEEL_INPUT_BYTES)?;
    let key = read_key(bytes, 4);
    let target = read_target(bytes, 28);
    let direction = read_u16(bytes, 80);
    let request = MouseWheelInput {
        key,
        target,
        expected_owner_revision: read_u64(bytes, 68),
        column: read_u16(bytes, 76),
        row: read_u16(bytes, 78),
        up: match direction {
            1 => true,
            2 => false,
            _ => return Err(UiCodecError::InvalidMouseDirection),
        },
    };
    zero(&bytes[82..])?;
    request.validate()?;
    Ok(request)
}

/// Validate and encode one LayoutReply.
pub fn encode_layout_reply(reply: &LayoutReply) -> Result<[u8; LAYOUT_REPLY_BYTES], UiCodecError> {
    validate_layout(reply)?;
    let mut bytes = [0_u8; LAYOUT_REPLY_BYTES];
    put_u16(&mut bytes, 0, LAYOUT_REPLY_SCHEMA);
    put_u16(&mut bytes, 2, KIND_LAYOUT_REPLY);
    put_u64(&mut bytes, 4, reply.revision);
    put_u16(&mut bytes, 12, reply.rows);
    put_u16(&mut bytes, 14, reply.columns);
    let count = reply.panes.iter().flatten().count() as u16;
    put_u16(&mut bytes, 16, count);
    let dividers = layout_dividers(reply);
    let divider_count = dividers.iter().flatten().count() as u16;
    put_u16(&mut bytes, 18, divider_count);
    for (index, divider) in dividers.iter().enumerate() {
        if let Some(divider) = divider {
            put_rect(&mut bytes, DIVIDER_OFFSET + index * 8, *divider);
        }
    }
    for (index, pane) in reply.panes.iter().enumerate() {
        if let Some(pane) = pane {
            let offset = PANE0_OFFSET + index * PANE_ENTRY_BYTES;
            put_target(&mut bytes, offset, pane.target);
            put_rect(&mut bytes, offset + 40, pane.rect);
            if let Some((rows, columns)) = pane.pending_confirmed {
                put_u16(&mut bytes, offset + 48, 1);
                put_u16(&mut bytes, offset + 52, rows);
                put_u16(&mut bytes, offset + 54, columns);
            }
        }
    }
    Ok(bytes)
}

/// Decode and validate one exact LayoutReply.
pub fn decode_layout_reply(bytes: &[u8]) -> Result<LayoutReply, UiCodecError> {
    exact_size(bytes, LAYOUT_REPLY_BYTES)?;
    layout_header(bytes)?;
    let count = read_u16(bytes, 16);
    let divider_count = usize::from(read_u16(bytes, 18));
    if divider_count > crate::pane_layout::MAX_PANES - 1 {
        return Err(UiCodecError::InvalidLayout);
    }
    let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
    for (index, divider) in dividers.iter_mut().enumerate() {
        let offset = DIVIDER_OFFSET + index * 8;
        if index >= divider_count {
            zero(&bytes[offset..offset + 8])?;
        } else {
            *divider = Some(read_rect(bytes, offset));
        }
    }
    zero(&bytes[44..PANE0_OFFSET])?;
    zero(&bytes[336..LAYOUT_REPLY_BYTES])?;
    let mut panes = [None; MAX_PANES];
    for (index, pane_slot) in panes.iter_mut().enumerate() {
        let offset = PANE0_OFFSET + index * PANE_ENTRY_BYTES;
        let entry = &bytes[offset..offset + PANE_ENTRY_BYTES];
        if usize::from(count) <= index {
            zero(entry)?;
            continue;
        }
        zero(&entry[50..52])?;
        zero(&entry[56..PANE_ENTRY_BYTES])?;
        let pending_confirmed = match read_u16(entry, 48) {
            0 => {
                zero(&entry[52..56])?;
                None
            }
            1 => Some((read_u16(entry, 52), read_u16(entry, 54))),
            _ => return Err(UiCodecError::InvalidLayout),
        };
        *pane_slot = Some(UiPane {
            target: read_target(entry, 0),
            rect: read_rect(entry, 40),
            pending_confirmed,
        });
    }
    if !(1..=crate::pane_layout::MAX_PANES as u16).contains(&count) {
        return Err(UiCodecError::InvalidLayout);
    }
    if usize::from(count) != divider_count + 1 {
        return Err(UiCodecError::InvalidLayout);
    }
    let reply = LayoutReply {
        revision: read_u64(bytes, 4),
        rows: read_u16(bytes, 12),
        columns: read_u16(bytes, 14),
        panes,
        divider: dividers[0],
        dividers,
    };
    validate_layout(&reply)?;
    Ok(reply)
}

/// Prefix one complete checkpoint record with the ScreenReply header.
///
/// The checkpoint is decoded (zero-copy) and must belong to `target` at
/// `screen_revision`. The caller's buffer is reused: only the 64-byte header
/// is reserved, fallibly, and no second checkpoint-sized buffer is created.
pub fn prefix_screen_reply(
    target: TargetRef,
    screen_revision: u64,
    mut checkpoint: Vec<u8>,
) -> Result<Vec<u8>, UiCodecError> {
    validate_checkpoint(target, screen_revision, &checkpoint)?;
    let mut prefix = [0_u8; SCREEN_REPLY_HEADER_BYTES];
    put_header(&mut prefix, KIND_SCREEN_REPLY);
    put_target(&mut prefix, 4, target);
    put_u64(&mut prefix, 44, screen_revision);
    put_u32(&mut prefix, 52, checkpoint.len() as u32);
    checkpoint
        .try_reserve_exact(SCREEN_REPLY_HEADER_BYTES)
        .map_err(|_| UiCodecError::PayloadTooLarge)?;
    checkpoint.splice(0..0, prefix);
    Ok(checkpoint)
}

/// Decode a ScreenReply and validate its embedded checkpoint record.
pub fn decode_screen_reply(bytes: &[u8]) -> Result<ScreenReplyView<'_>, UiCodecError> {
    if bytes.len() > MAX_SCREEN_REPLY_BYTES {
        return Err(UiCodecError::PayloadTooLarge);
    }
    if bytes.len() < SCREEN_REPLY_HEADER_BYTES {
        return Err(UiCodecError::Truncated);
    }
    header(bytes, KIND_SCREEN_REPLY)?;
    zero(&bytes[56..SCREEN_REPLY_HEADER_BYTES])?;
    let target = read_target(bytes, 4);
    validate_target(target)?;
    let screen_revision = read_u64(bytes, 44);
    if screen_revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    let length = read_u32(bytes, 52) as usize;
    exact_size(bytes, SCREEN_REPLY_HEADER_BYTES + length)?;
    let checkpoint = &bytes[SCREEN_REPLY_HEADER_BYTES..];
    validate_checkpoint(target, screen_revision, checkpoint)?;
    Ok(ScreenReplyView {
        target,
        screen_revision,
        checkpoint,
    })
}

/// Prefix a history checkpoint with its bounded view metadata.
pub fn prefix_history_reply(
    target: TargetRef,
    screen_revision: u64,
    offset_rows: u16,
    alternate_screen: bool,
    mouse_reporting: bool,
    mut checkpoint: Vec<u8>,
) -> Result<Vec<u8>, UiCodecError> {
    validate_target(target)?;
    if screen_revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    if offset_rows > MAX_HISTORY_OFFSET_ROWS {
        return Err(UiCodecError::InvalidHistoryOffset);
    }
    validate_checkpoint(target, screen_revision, &checkpoint)?;
    let mut prefix = [0_u8; SCREEN_REPLY_HEADER_BYTES];
    put_header(&mut prefix, KIND_HISTORY_REPLY);
    put_target(&mut prefix, 4, target);
    put_u64(&mut prefix, 44, screen_revision);
    put_u16(&mut prefix, 52, offset_rows);
    put_u16(
        &mut prefix,
        54,
        u16::from(alternate_screen) | (u16::from(mouse_reporting) << 1),
    );
    put_u32(&mut prefix, 56, checkpoint.len() as u32);
    checkpoint
        .try_reserve_exact(SCREEN_REPLY_HEADER_BYTES)
        .map_err(|_| UiCodecError::PayloadTooLarge)?;
    checkpoint.splice(0..0, prefix);
    if checkpoint.len() > MAX_HISTORY_REPLY_BYTES {
        return Err(UiCodecError::PayloadTooLarge);
    }
    Ok(checkpoint)
}

/// Decode and validate one history checkpoint reply.
pub fn decode_history_reply(bytes: &[u8]) -> Result<HistoryReplyView<'_>, UiCodecError> {
    if bytes.len() > MAX_HISTORY_REPLY_BYTES {
        return Err(UiCodecError::PayloadTooLarge);
    }
    if bytes.len() < SCREEN_REPLY_HEADER_BYTES {
        return Err(UiCodecError::Truncated);
    }
    header(bytes, KIND_HISTORY_REPLY)?;
    let target = read_target(bytes, 4);
    validate_target(target)?;
    let screen_revision = read_u64(bytes, 44);
    if screen_revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    let offset_rows = read_u16(bytes, 52);
    if offset_rows > MAX_HISTORY_OFFSET_ROWS {
        return Err(UiCodecError::InvalidHistoryOffset);
    }
    let flags = read_u16(bytes, 54);
    if flags & !0b11 != 0 {
        return Err(UiCodecError::InvalidStatus);
    }
    let checkpoint_len = read_u32(bytes, 56) as usize;
    zero(&bytes[60..SCREEN_REPLY_HEADER_BYTES])?;
    exact_size(bytes, SCREEN_REPLY_HEADER_BYTES + checkpoint_len)?;
    let checkpoint = &bytes[SCREEN_REPLY_HEADER_BYTES..];
    validate_checkpoint(target, screen_revision, checkpoint)?;
    Ok(HistoryReplyView {
        target,
        screen_revision,
        offset_rows,
        alternate_screen: flags & 1 != 0,
        mouse_reporting: flags & 2 != 0,
        checkpoint,
    })
}

/// Validate and encode one ResizeReply.
pub fn encode_resize_reply(reply: &ResizeReply) -> Result<[u8; RESIZE_REPLY_BYTES], UiCodecError> {
    validate_resize_reply(reply)?;
    let mut bytes = [0_u8; RESIZE_REPLY_BYTES];
    put_header(&mut bytes, KIND_RESIZE_REPLY);
    put_key(&mut bytes, 4, reply.key);
    put_u16(
        &mut bytes,
        28,
        match reply.status {
            ResizeStatus::Applied => 1,
            ResizeStatus::ResizePending => 2,
        },
    );
    put_u16(
        &mut bytes,
        30,
        reply
            .pending
            .iter()
            .enumerate()
            .fold(0_u16, |flags, (index, pending)| {
                flags | (u16::from(*pending) << index)
            }),
    );
    put_u64(&mut bytes, 32, reply.revision);
    put_u16(&mut bytes, 40, reply.rows);
    put_u16(&mut bytes, 42, reply.columns);
    Ok(bytes)
}

/// Decode and validate one exact ResizeReply.
pub fn decode_resize_reply(bytes: &[u8]) -> Result<ResizeReply, UiCodecError> {
    exact_size(bytes, RESIZE_REPLY_BYTES)?;
    header(bytes, KIND_RESIZE_REPLY)?;
    zero(&bytes[44..RESIZE_REPLY_BYTES])?;
    let status = match read_u16(bytes, 28) {
        1 => ResizeStatus::Applied,
        2 => ResizeStatus::ResizePending,
        _ => return Err(UiCodecError::InvalidStatus),
    };
    let flags = read_u16(bytes, 30);
    if flags >> crate::pane_layout::MAX_PANES != 0 {
        return Err(UiCodecError::InvalidStatus);
    }
    let reply = ResizeReply {
        key: read_key(bytes, 4),
        status,
        pending: std::array::from_fn(|index| flags & (1 << index) != 0),
        revision: read_u64(bytes, 32),
        rows: read_u16(bytes, 40),
        columns: read_u16(bytes, 42),
    };
    validate_resize_reply(&reply)?;
    Ok(reply)
}

fn validate_resize_reply(reply: &ResizeReply) -> Result<(), UiCodecError> {
    validate_key(reply.key)?;
    if reply.revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    validate_legacy_root(reply.rows, reply.columns)?;
    let any_pending = reply.pending.contains(&true);
    if any_pending != (reply.status == ResizeStatus::ResizePending) {
        return Err(UiCodecError::InvalidStatus);
    }
    Ok(())
}

fn validate_checkpoint(
    target: TargetRef,
    screen_revision: u64,
    checkpoint: &[u8],
) -> Result<(), UiCodecError> {
    validate_target(target)?;
    if screen_revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    if checkpoint.len() > MAX_SCREEN_REPLY_CHECKPOINT_BYTES {
        return Err(UiCodecError::PayloadTooLarge);
    }
    let decoded = crate::screen_checkpoint::decode(checkpoint)
        .map_err(|_| UiCodecError::InvalidCheckpoint)?;
    let metadata = decoded.metadata();
    if metadata.target() != target || metadata.screen_revision() != screen_revision {
        return Err(UiCodecError::InvalidCheckpoint);
    }
    Ok(())
}

fn validate_layout(reply: &LayoutReply) -> Result<(), UiCodecError> {
    if reply.revision == 0 {
        return Err(UiCodecError::ZeroRevision);
    }
    validate_legacy_root(reply.rows, reply.columns)?;
    let Some(first) = reply.panes[0] else {
        return Err(UiCodecError::InvalidLayout);
    };
    let count = reply.panes.iter().flatten().count();
    if !(1..=crate::pane_layout::MAX_PANES).contains(&count) {
        return Err(UiCodecError::InvalidLayout);
    }
    let mut panes = [None; MAX_PANES];
    let mut count = 0;
    for pane in reply.panes.iter().flatten().copied() {
        panes[count] = Some(pane);
        count += 1;
    }
    if reply.panes[..count].iter().any(Option::is_none)
        || reply.panes[count..].iter().any(Option::is_some)
    {
        return Err(UiCodecError::InvalidLayout);
    }
    let dividers = layout_dividers(reply);
    let divider_count = dividers.iter().flatten().count();
    if divider_count != count - 1 || reply.divider != dividers[0] {
        return Err(UiCodecError::InvalidLayout);
    }
    for (index, pane) in panes.iter().take(count).flatten().enumerate() {
        validate_target(pane.target)?;
        validate_rect_in_root(pane.rect, reply.rows, reply.columns)?;
        if let Some((rows, columns)) = pane.pending_confirmed {
            validate_pane_size(rows, columns)?;
        }
        if panes[..index].iter().flatten().any(|previous| {
            previous.target.generation != pane.target.generation
                || previous.target.workspace != pane.target.workspace
                || previous.target.pane == pane.target.pane
                || previous.target.session == pane.target.session
        }) {
            return Err(UiCodecError::InvalidLayout);
        }
    }
    if count == 1
        && first.rect
            != (Rect {
                x: 0,
                y: 0,
                rows: reply.rows,
                columns: reply.columns,
            })
    {
        return Err(UiCodecError::InvalidLayout);
    }

    let mut occupied = [false; 80 * 240];
    for pane in panes.iter().flatten() {
        for y in pane.rect.y..pane.rect.y + pane.rect.rows {
            for x in pane.rect.x..pane.rect.x + pane.rect.columns {
                let index = usize::from(y) * usize::from(MAX_PANE_COLUMNS) + usize::from(x);
                if std::mem::replace(&mut occupied[index], true) {
                    return Err(UiCodecError::InvalidLayout);
                }
            }
        }
    }
    for divider in dividers.iter().flatten() {
        let one_cell_wide = divider.columns == 1 && divider.rows > 0;
        let one_cell_high = divider.rows == 1 && divider.columns > 0;
        if !one_cell_wide && !one_cell_high {
            return Err(UiCodecError::InvalidLayout);
        }
        if count == 2 {
            let full_span = (one_cell_wide && divider.rows == reply.rows)
                || (one_cell_high && divider.columns == reply.columns);
            if !full_span {
                return Err(UiCodecError::InvalidLayout);
            }
        }
        for y in divider.y..divider.y + divider.rows {
            for x in divider.x..divider.x + divider.columns {
                let index = usize::from(y) * usize::from(MAX_PANE_COLUMNS) + usize::from(x);
                if std::mem::replace(&mut occupied[index], true) {
                    return Err(UiCodecError::InvalidLayout);
                }
                // A divider cell is flanked on both sides by pane cells or by
                // another divider's cell (a T-junction in a nested split).
                let covered = |cx: u16, cy: u16| {
                    panes
                        .iter()
                        .flatten()
                        .any(|pane| pane.rect.contains(cx, cy))
                        || dividers
                            .iter()
                            .flatten()
                            .any(|other| other.contains(cx, cy))
                };
                let separates = if one_cell_wide {
                    x > 0 && x + 1 < reply.columns && covered(x - 1, y) && covered(x + 1, y)
                } else {
                    y > 0 && y + 1 < reply.rows && covered(x, y - 1) && covered(x, y + 1)
                };
                if !separates {
                    return Err(UiCodecError::InvalidLayout);
                }
            }
        }
    }
    for y in 0..reply.rows {
        for x in 0..reply.columns {
            if !occupied[usize::from(y) * usize::from(MAX_PANE_COLUMNS) + usize::from(x)] {
                return Err(UiCodecError::InvalidLayout);
            }
        }
    }
    Ok(())
}

fn layout_dividers(reply: &LayoutReply) -> [Option<Rect>; crate::pane_layout::MAX_PANES - 1] {
    let mut dividers = reply.dividers;
    if dividers.iter().all(Option::is_none) {
        dividers[0] = reply.divider;
    }
    dividers
}

fn validate_rect_in_root(rect: Rect, rows: u16, columns: u16) -> Result<(), UiCodecError> {
    validate_pane_size(rect.rows, rect.columns)?;
    if rect_within(rect, rows, columns) {
        Ok(())
    } else {
        Err(UiCodecError::InvalidLayout)
    }
}

fn rect_within(rect: Rect, rows: u16, columns: u16) -> bool {
    rect.rows != 0
        && rect.columns != 0
        && u32::from(rect.x) + u32::from(rect.columns) <= u32::from(columns)
        && u32::from(rect.y) + u32::from(rect.rows) <= u32::from(rows)
}

fn validate_root(rows: u16, columns: u16) -> Result<(), UiCodecError> {
    if (MIN_PANE_ROWS..=MAX_PANE_ROWS).contains(&rows)
        && (MIN_PANE_COLUMNS..=MAX_PANE_COLUMNS).contains(&columns)
    {
        Ok(())
    } else {
        Err(UiCodecError::InvalidDimensions)
    }
}

/// Existing LayoutReply/ResizeReply payloads retain their schema and may
/// describe the pre-padding 5x20 minimum. New resize requests use the larger
/// outer geometry above; accepting old observations keeps metadata clients
/// compatible with servers deployed before padding.
fn validate_legacy_root(rows: u16, columns: u16) -> Result<(), UiCodecError> {
    if (5..=MAX_PANE_ROWS).contains(&rows) && (20..=MAX_PANE_COLUMNS).contains(&columns) {
        Ok(())
    } else {
        Err(UiCodecError::InvalidDimensions)
    }
}

fn validate_pane_size(rows: u16, columns: u16) -> Result<(), UiCodecError> {
    if (5..=MAX_PANE_ROWS).contains(&rows) && (20..=MAX_PANE_COLUMNS).contains(&columns) {
        Ok(())
    } else {
        Err(UiCodecError::InvalidDimensions)
    }
}

fn key_code(key: Key) -> Result<(u16, u32), UiCodecError> {
    Ok(match key {
        Key::Char(scalar) if !scalar.is_control() => (1, u32::from(scalar)),
        Key::Char(_) => return Err(UiCodecError::InvalidKeyScalar),
        Key::Enter => (2, 0),
        Key::Tab => (3, 0),
        Key::Backspace => (4, 0),
        Key::Escape => (5, 0),
        Key::Interrupt => (6, 0),
        Key::Up => (7, 0),
        Key::Down => (8, 0),
        Key::Left => (9, 0),
        Key::Right => (10, 0),
        Key::Home => (11, 0),
        Key::End => (12, 0),
        Key::Control(value @ 1..=26) => (13, u32::from(value)),
        Key::Control(_) => return Err(UiCodecError::InvalidKeyScalar),
        Key::AltChar(scalar) if !scalar.is_control() => (14, u32::from(scalar)),
        Key::AltChar(_) => return Err(UiCodecError::InvalidKeyScalar),
        Key::Function(number @ 1..=12) => (15, u32::from(number)),
        Key::Function(_) => return Err(UiCodecError::InvalidKeyScalar),
        Key::Delete => (16, 0),
        Key::Insert => (17, 0),
        Key::PageUp => (18, 0),
        Key::PageDown => (19, 0),
        Key::BackTab => (20, 0),
    })
}

fn decode_key_entry(entry: &[u8]) -> Result<Key, UiCodecError> {
    let code = read_u16(entry, 0);
    zero(&entry[2..4])?;
    let scalar = read_u32(entry, 4);
    let key = match code {
        1 => {
            let character = char::from_u32(scalar).ok_or(UiCodecError::InvalidKeyScalar)?;
            if character.is_control() {
                return Err(UiCodecError::InvalidKeyScalar);
            }
            return Ok(Key::Char(character));
        }
        2 => Key::Enter,
        3 => Key::Tab,
        4 => Key::Backspace,
        5 => Key::Escape,
        6 => Key::Interrupt,
        7 => Key::Up,
        8 => Key::Down,
        9 => Key::Left,
        10 => Key::Right,
        11 => Key::Home,
        12 => Key::End,
        13 if (1..=26).contains(&scalar) => return Ok(Key::Control(scalar as u8)),
        13 => return Err(UiCodecError::InvalidKeyScalar),
        14 => {
            let character = char::from_u32(scalar).ok_or(UiCodecError::InvalidKeyScalar)?;
            if character.is_control() {
                return Err(UiCodecError::InvalidKeyScalar);
            }
            return Ok(Key::AltChar(character));
        }
        15 if (1..=12).contains(&scalar) => return Ok(Key::Function(scalar as u8)),
        15 => return Err(UiCodecError::InvalidKeyScalar),
        16 => Key::Delete,
        17 => Key::Insert,
        18 => Key::PageUp,
        19 => Key::PageDown,
        20 => Key::BackTab,
        _ => return Err(UiCodecError::UnknownKeyCode),
    };
    if scalar != 0 {
        return Err(UiCodecError::InvalidKeyScalar);
    }
    Ok(key)
}

fn validate_key(key: OperationKey) -> Result<(), UiCodecError> {
    if key.generation == 0 || key.lease == 0 || key.sequence == 0 {
        Err(UiCodecError::ZeroKeyComponent)
    } else {
        Ok(())
    }
}

fn validate_target(target: TargetRef) -> Result<(), UiCodecError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        Err(UiCodecError::InvalidTarget)
    } else {
        Ok(())
    }
}

fn exact_size(bytes: &[u8], expected: usize) -> Result<(), UiCodecError> {
    match bytes.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(UiCodecError::Truncated),
        std::cmp::Ordering::Greater => Err(UiCodecError::TrailingBytes),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

/// Callers guarantee at least four bytes.
fn header(bytes: &[u8], kind: u16) -> Result<(), UiCodecError> {
    if read_u16(bytes, 0) != UI_SCHEMA {
        return Err(UiCodecError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != kind {
        return Err(UiCodecError::UnknownKind);
    }
    Ok(())
}

fn layout_header(bytes: &[u8]) -> Result<(), UiCodecError> {
    if read_u16(bytes, 0) != LAYOUT_REPLY_SCHEMA {
        return Err(UiCodecError::UnsupportedSchema);
    }
    if read_u16(bytes, 2) != KIND_LAYOUT_REPLY {
        return Err(UiCodecError::UnknownKind);
    }
    Ok(())
}

fn zero(bytes: &[u8]) -> Result<(), UiCodecError> {
    if bytes.iter().all(|byte| *byte == 0) {
        Ok(())
    } else {
        Err(UiCodecError::NonzeroReserved)
    }
}

// Fixed-offset helpers are only called after the caller checked the length.

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut field = [0_u8; 4];
    field.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_be_bytes(field)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut field = [0_u8; 8];
    field.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_be_bytes(field)
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

fn read_rect(bytes: &[u8], offset: usize) -> Rect {
    Rect {
        x: read_u16(bytes, offset),
        y: read_u16(bytes, offset + 2),
        rows: read_u16(bytes, offset + 4),
        columns: read_u16(bytes, offset + 6),
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn put_header(bytes: &mut [u8], kind: u16) {
    put_u16(bytes, 0, UI_SCHEMA);
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

fn put_rect(bytes: &mut [u8], offset: usize, rect: Rect) {
    put_u16(bytes, offset, rect.x);
    put_u16(bytes, offset + 2, rect.y);
    put_u16(bytes, offset + 4, rect.rows);
    put_u16(bytes, offset + 6, rect.columns);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_checkpoint::{encode as encode_checkpoint, CheckpointMetadata};
    use crate::terminal::{ScreenSnapshot, TerminalSize};

    const KEY: OperationKey = OperationKey {
        generation: 1,
        lease: 2,
        sequence: 3,
    };
    const TARGET: TargetRef = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 5,
        session: 6,
        occupant_revision: 7,
    };
    const SIBLING: TargetRef = TargetRef {
        generation: 1,
        workspace: 4,
        pane: 8,
        session: 9,
        occupant_revision: 1,
    };

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn two_pane_layout() -> LayoutReply {
        LayoutReply {
            revision: 2,
            rows: 24,
            columns: 81,
            panes: [
                Some(UiPane {
                    target: TARGET,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    pending_confirmed: Some((24, 81)),
                }),
                Some(UiPane {
                    target: SIBLING,
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    pending_confirmed: None,
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 40,
                y: 0,
                rows: 24,
                columns: 1,
            }),
            dividers: [
                Some(Rect {
                    x: 40,
                    y: 0,
                    rows: 24,
                    columns: 1,
                }),
                None,
                None,
            ],
        }
    }

    fn checkpoint(target: TargetRef, revision: u64) -> Vec<u8> {
        let snapshot = ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap();
        let metadata = CheckpointMetadata::new(target, 0, 1, revision).unwrap();
        encode_checkpoint(&snapshot, &metadata).unwrap()
    }

    #[test]
    fn golden_request_bytes() {
        assert_eq!(hex(&encode_layout_query()), "0008000100000000");
        let screen = encode_screen_query(TARGET).unwrap();
        assert_eq!(
            hex(&screen),
            concat!(
                "0008000200000000",
                "0000000000000001",
                "0000000000000004",
                "0000000000000005",
                "0000000000000006",
                "0000000000000007"
            )
        );
        let keys = [Key::Char('é'), Key::Enter];
        let input = encode_key_input(&KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 11,
            keys: &keys,
        })
        .unwrap();
        assert_eq!(input.len(), 104);
        assert_eq!(
            hex(&input),
            concat!(
                "00080003",
                "0000000000000001",
                "0000000000000002",
                "0000000000000003",
                "0000000000000001",
                "0000000000000004",
                "0000000000000005",
                "0000000000000006",
                "0000000000000007",
                "000000000000000b",
                "0002",
                "00000000000000000000",
                "00010000000000e9",
                "0002000000000000"
            )
        );
        let resize = encode_root_resize(&RootResize {
            key: KEY,
            expected_layout_revision: 5,
            rows: 30,
            columns: 101,
        })
        .unwrap();
        assert_eq!(
            hex(&resize),
            concat!(
                "00080004",
                "0000000000000001",
                "0000000000000002",
                "0000000000000003",
                "0000000000000005",
                "001e",
                "0065"
            )
        );
    }

    #[test]
    fn planned_split_size_matches_pane_layout_geometry() {
        let single = LayoutReply {
            revision: 7,
            rows: 24,
            columns: 81,
            panes: [
                Some(UiPane {
                    target: TARGET,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 81,
                    },
                    pending_confirmed: None,
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        assert_eq!(
            planned_split_size(&single, TARGET, SplitDirection::Right),
            Ok((22, 38))
        );
        assert_eq!(
            planned_split_size(&single, TARGET, SplitDirection::Left),
            Ok((22, 38))
        );
        // 23 outer rows split into 11 and 12; borders leave 9 and 10.
        assert_eq!(
            planned_split_size(&single, TARGET, SplitDirection::Down),
            Ok((10, 79))
        );
        assert_eq!(
            planned_split_size(&single, TARGET, SplitDirection::Up),
            Ok((9, 79))
        );
        assert_eq!(
            planned_split_size(&single, SIBLING, SplitDirection::Right),
            Err(LayoutError::StaleTarget)
        );
        assert!(planned_split_size(&two_pane_layout(), TARGET, SplitDirection::Right).is_err());
    }

    #[test]
    fn attach_target_trailer_is_exact() {
        let bytes = encode_attach_target(TARGET).unwrap();
        assert_eq!(&bytes[..8], &[0, 8, 0, 5, 0, 0, 0, 0]);
        assert_eq!(decode_attach_target(&bytes), Ok(TARGET));
        assert_eq!(
            decode_attach_target(&bytes[..47]),
            Err(UiCodecError::Truncated)
        );
        let mut reserved = bytes;
        reserved[5] = 1;
        assert_eq!(
            decode_attach_target(&reserved),
            Err(UiCodecError::NonzeroReserved)
        );
        let mut zero = bytes;
        zero[40..48].fill(0);
        assert_eq!(
            decode_attach_target(&zero),
            Err(UiCodecError::InvalidTarget)
        );
        assert_eq!(
            encode_attach_target(TargetRef {
                workspace: 0,
                ..TARGET
            }),
            Err(UiCodecError::InvalidTarget)
        );
        // An attach trailer is not a request.
        assert_eq!(decode_ui_request(&bytes), Err(UiCodecError::UnknownKind));
    }

    #[test]
    fn golden_reply_bytes_and_round_trips() {
        let layout = two_pane_layout();
        let bytes = encode_layout_reply(&layout).unwrap();
        assert_eq!(
            &bytes[..32],
            &[
                0, 9, 0x80, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 24, 0, 81, 0, 2, 0, 1, 0, 40, 0, 0, 0,
                24, 0, 1, 0, 0, 0, 0,
            ]
        );
        // Pane 0 rect, pending flag and confirmed size.
        assert_eq!(
            &bytes[PANE0_OFFSET + 40..PANE0_OFFSET + 56],
            &[0, 0, 0, 0, 0, 24, 0, 40, 0, 1, 0, 0, 0, 24, 0, 81]
        );
        assert_eq!(decode_layout_reply(&bytes).unwrap(), layout);

        let single = LayoutReply {
            revision: 1,
            rows: 24,
            columns: 80,
            panes: [
                Some(UiPane {
                    target: TARGET,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 80,
                    },
                    pending_confirmed: None,
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        let bytes = encode_layout_reply(&single).unwrap();
        assert!(bytes[PANE0_OFFSET + PANE_ENTRY_BYTES..336]
            .iter()
            .all(|byte| *byte == 0));
        assert_eq!(decode_layout_reply(&bytes).unwrap(), single);

        let resize = ResizeReply {
            key: KEY,
            status: ResizeStatus::ResizePending,
            pending: [false, true, false, false],
            revision: 3,
            rows: 30,
            columns: 101,
        };
        let bytes = encode_resize_reply(&resize).unwrap();
        assert_eq!(
            hex(&bytes),
            concat!(
                "00088003",
                "0000000000000001",
                "0000000000000002",
                "0000000000000003",
                "0002",
                "0002",
                "0000000000000003",
                "001e",
                "0065",
                "00000000"
            )
        );
        assert_eq!(decode_resize_reply(&bytes).unwrap(), resize);

        let record = checkpoint(TARGET, 9);
        let record_len = record.len();
        let reply = prefix_screen_reply(TARGET, 9, record.clone()).unwrap();
        assert_eq!(reply.len(), SCREEN_REPLY_HEADER_BYTES + record_len);
        assert_eq!(&reply[..4], &[0, 8, 0x80, 2]);
        assert_eq!(&reply[44..56], &[0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0x13, 0x20]);
        let view = decode_screen_reply(&reply).unwrap();
        assert_eq!(view.target(), TARGET);
        assert_eq!(view.screen_revision(), 9);
        assert_eq!(view.checkpoint(), record.as_slice());
    }

    #[test]
    fn layout_reply_keeps_legacy_minimum_geometry_decodable() {
        let legacy = LayoutReply {
            revision: 1,
            rows: 5,
            columns: 20,
            panes: [
                Some(UiPane {
                    target: TARGET,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 5,
                        columns: 20,
                    },
                    pending_confirmed: Some((5, 20)),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; MAX_PANES - 1],
        };
        let encoded = encode_layout_reply(&legacy).unwrap();
        assert_eq!(decode_layout_reply(&encoded).unwrap(), legacy);
        // New client resize requests still require the padded outer minimum.
        let too_small = RootResize {
            key: KEY,
            expected_layout_revision: 1,
            rows: 5,
            columns: 20,
        };
        assert_eq!(
            encode_root_resize(&too_small),
            Err(UiCodecError::InvalidDimensions)
        );
    }

    #[test]
    fn request_round_trips_every_key_and_class() {
        let keys = [
            Key::Char('a'),
            Key::Char('\u{1F600}'),
            Key::Enter,
            Key::Tab,
            Key::Backspace,
            Key::Escape,
            Key::Interrupt,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::Control(1),
            Key::Control(26),
            Key::AltChar('é'),
            Key::Function(1),
            Key::Function(12),
            Key::Delete,
            Key::Insert,
            Key::PageUp,
            Key::PageDown,
            Key::BackTab,
        ];
        let bytes = encode_key_input(&KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 1,
            keys: &keys,
        })
        .unwrap();
        let UiRequest::KeyInput(view) = decode_ui_request(&bytes).unwrap() else {
            panic!("KeyInput decodes as KeyInput");
        };
        assert_eq!(view.key(), KEY);
        assert_eq!(view.target(), TARGET);
        assert_eq!(view.expected_owner_revision(), 1);
        assert_eq!(view.keys().collect::<Vec<_>>(), keys);
        assert_eq!(
            decode_ui_request(&bytes).unwrap().operation_class(),
            OperationClass::Input
        );
        assert_eq!(
            decode_ui_request(&encode_layout_query()).unwrap(),
            UiRequest::LayoutQuery
        );
        assert_eq!(
            decode_ui_request(&encode_screen_query(TARGET).unwrap()).unwrap(),
            UiRequest::ScreenQuery { target: TARGET }
        );
        let resize = RootResize {
            key: KEY,
            expected_layout_revision: 2,
            rows: 7,
            columns: 240,
        };
        let encoded = encode_root_resize(&resize).unwrap();
        let decoded = decode_ui_request(&encoded).unwrap();
        assert_eq!(decoded, UiRequest::RootResize(resize));
        assert_eq!(decoded.operation_class(), OperationClass::AbsoluteResize);
    }

    #[test]
    fn history_and_mouse_wheel_requests_round_trip_with_bounds() {
        let query = encode_history_query(TARGET, 17).unwrap();
        assert_eq!(query.len(), HISTORY_QUERY_BYTES);
        assert_eq!(
            decode_ui_request(&query).unwrap(),
            UiRequest::HistoryQuery(HistoryQuery {
                target: TARGET,
                offset_rows: 17,
            })
        );
        assert_eq!(
            decode_ui_request(&query).unwrap().operation_class(),
            OperationClass::Query
        );
        assert_eq!(
            encode_history_query(TARGET, MAX_HISTORY_OFFSET_ROWS + 1),
            Err(UiCodecError::InvalidHistoryOffset)
        );

        let input = MouseWheelInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 9,
            column: 3,
            row: 2,
            up: true,
        };
        let encoded = encode_mouse_wheel_input(&input).unwrap();
        assert_eq!(encoded.len(), MOUSE_WHEEL_INPUT_BYTES);
        assert_eq!(
            decode_ui_request(&encoded).unwrap(),
            UiRequest::MouseWheelInput(input)
        );
        assert_eq!(
            decode_ui_request(&encoded).unwrap().operation_class(),
            OperationClass::Input
        );
        let mut invalid_direction = encoded;
        invalid_direction[81] = 3;
        assert_eq!(
            decode_ui_request(&invalid_direction),
            Err(UiCodecError::InvalidMouseDirection)
        );
        let mut reserved = encoded;
        reserved[87] = 1;
        assert_eq!(
            decode_ui_request(&reserved),
            Err(UiCodecError::NonzeroReserved)
        );
    }

    #[test]
    fn history_reply_preserves_view_metadata_and_validates_checkpoint() {
        let record = checkpoint(TARGET, 9);
        let reply = prefix_history_reply(TARGET, 9, 7, true, false, record.clone()).unwrap();
        let view = decode_history_reply(&reply).unwrap();
        assert_eq!(view.target(), TARGET);
        assert_eq!(view.screen_revision(), 9);
        assert_eq!(view.offset_rows(), 7);
        assert!(view.alternate_screen());
        assert!(!view.mouse_reporting());
        assert_eq!(view.checkpoint(), record.as_slice());

        let mut bad_flags = reply.clone();
        bad_flags[55] |= 0x80;
        assert_eq!(
            decode_history_reply(&bad_flags),
            Err(UiCodecError::InvalidStatus)
        );
        let mut bad_target = reply;
        bad_target[43] = 8;
        assert_eq!(
            decode_history_reply(&bad_target),
            Err(UiCodecError::InvalidCheckpoint)
        );
    }

    #[test]
    fn zero_ids_and_revisions_are_rejected() {
        let keys = [Key::Enter];
        let base = KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 1,
            keys: &keys,
        };
        let valid = encode_key_input(&base).unwrap();
        for offset in [4, 12, 20] {
            let mut bytes = valid.clone();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(
                decode_ui_request(&bytes),
                Err(UiCodecError::ZeroKeyComponent)
            );
        }
        for offset in [28, 36, 44, 52, 60] {
            let mut bytes = valid.clone();
            bytes[offset..offset + 8].fill(0);
            assert_eq!(decode_ui_request(&bytes), Err(UiCodecError::InvalidTarget));
        }
        let mut bytes = valid.clone();
        bytes[68..76].fill(0);
        assert_eq!(decode_ui_request(&bytes), Err(UiCodecError::ZeroRevision));
        let mut bytes = valid.clone();
        bytes[35] = 9;
        assert_eq!(
            decode_ui_request(&bytes),
            Err(UiCodecError::GenerationMismatch)
        );
        assert_eq!(
            KeyInput {
                expected_owner_revision: 0,
                ..base
            }
            .encoded_len(),
            Err(UiCodecError::ZeroRevision)
        );
        let zero_target = TargetRef { pane: 0, ..TARGET };
        assert_eq!(
            encode_screen_query(zero_target),
            Err(UiCodecError::InvalidTarget)
        );
        let mut screen = encode_screen_query(TARGET).unwrap();
        screen[47] = 0;
        assert_eq!(decode_ui_request(&screen), Err(UiCodecError::InvalidTarget));
        let resize = RootResize {
            key: KEY,
            expected_layout_revision: 0,
            rows: 24,
            columns: 80,
        };
        assert_eq!(encode_root_resize(&resize), Err(UiCodecError::ZeroRevision));
    }

    #[test]
    fn key_count_bounds_and_unknown_codes_are_rejected() {
        let many = [Key::Char('x'); MAX_KEYS + 1];
        let too_many = KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 1,
            keys: &many,
        };
        assert_eq!(too_many.encoded_len(), Err(UiCodecError::InvalidKeyCount));
        let max = KeyInput {
            keys: &many[..MAX_KEYS],
            ..too_many
        };
        let bytes = encode_key_input(&max).unwrap();
        assert_eq!(bytes.len(), MAX_KEY_INPUT_BYTES);
        assert!(decode_ui_request(&bytes).is_ok());
        let mut over = bytes.clone();
        over.extend_from_slice(&[0, 2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode_ui_request(&over), Err(UiCodecError::PayloadTooLarge));
        let mut count = bytes.clone();
        count[76..78].copy_from_slice(&65_u16.to_be_bytes());
        assert_eq!(
            decode_ui_request(&count),
            Err(UiCodecError::InvalidKeyCount)
        );
        let empty = KeyInput {
            keys: &[],
            ..too_many
        };
        assert_eq!(empty.encoded_len(), Err(UiCodecError::InvalidKeyCount));

        let one = encode_key_input(&KeyInput {
            keys: &many[..1],
            ..too_many
        })
        .unwrap();
        let mut zero_count = one.clone();
        zero_count[76..78].fill(0);
        assert_eq!(
            decode_ui_request(&zero_count),
            Err(UiCodecError::InvalidKeyCount)
        );
        for code in [0_u16, 21, 0xffff] {
            let mut bytes = one.clone();
            bytes[88..90].copy_from_slice(&code.to_be_bytes());
            bytes[92..96].fill(0);
            assert_eq!(decode_ui_request(&bytes), Err(UiCodecError::UnknownKeyCode));
        }
        for (code, scalar) in [
            (13_u16, 0_u32),
            (13, 27),
            (14, 0x0a),
            (14, 0xd800),
            (15, 0),
            (15, 13),
        ] {
            let mut bytes = one.clone();
            bytes[88..90].copy_from_slice(&code.to_be_bytes());
            bytes[92..96].copy_from_slice(&scalar.to_be_bytes());
            assert_eq!(
                decode_ui_request(&bytes),
                Err(UiCodecError::InvalidKeyScalar)
            );
        }
        for code in 16_u16..=20 {
            let mut bytes = one.clone();
            bytes[88..90].copy_from_slice(&code.to_be_bytes());
            assert_eq!(
                decode_ui_request(&bytes),
                Err(UiCodecError::InvalidKeyScalar)
            );
        }
        // Control, surrogate and out-of-range Char scalars.
        for scalar in [0x0a_u32, 0x1b, 0x7f, 0xd800, 0x11_0000] {
            let mut bytes = one.clone();
            bytes[92..96].copy_from_slice(&scalar.to_be_bytes());
            assert_eq!(
                decode_ui_request(&bytes),
                Err(UiCodecError::InvalidKeyScalar)
            );
        }
        let control = [Key::Char('\n')];
        assert_eq!(
            KeyInput {
                keys: &control,
                ..too_many
            }
            .encoded_len(),
            Err(UiCodecError::InvalidKeyScalar)
        );
        // A non-Char key must carry a zero scalar; entry reserved must be zero.
        let enter = encode_key_input(&KeyInput {
            keys: &[Key::Enter],
            ..too_many
        })
        .unwrap();
        let mut scalar = enter.clone();
        scalar[95] = 1;
        assert_eq!(
            decode_ui_request(&scalar),
            Err(UiCodecError::InvalidKeyScalar)
        );
        let mut reserved = enter.clone();
        reserved[90] = 1;
        assert_eq!(
            decode_ui_request(&reserved),
            Err(UiCodecError::NonzeroReserved)
        );
        let mut prefix = enter.clone();
        prefix[80] = 1;
        assert_eq!(
            decode_ui_request(&prefix),
            Err(UiCodecError::NonzeroReserved)
        );
    }

    #[test]
    fn sizes_schema_kind_and_reserved_bytes_are_strict() {
        assert_eq!(decode_ui_request(&[0, 8, 0]), Err(UiCodecError::Truncated));
        assert_eq!(
            decode_ui_request(&[0, 7, 0, 1, 0, 0, 0, 0]),
            Err(UiCodecError::UnsupportedSchema)
        );
        assert_eq!(
            decode_ui_request(&[0, 8, 0, 9, 0, 0, 0, 0]),
            Err(UiCodecError::UnknownKind)
        );
        let layout = encode_layout_query();
        assert_eq!(
            decode_ui_request(&layout[..7]),
            Err(UiCodecError::Truncated)
        );
        let mut long = layout.to_vec();
        long.push(0);
        assert_eq!(decode_ui_request(&long), Err(UiCodecError::TrailingBytes));
        let mut reserved = layout;
        reserved[7] = 1;
        assert_eq!(
            decode_ui_request(&reserved),
            Err(UiCodecError::NonzeroReserved)
        );
        let keys = [Key::Tab];
        let input = encode_key_input(&KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 1,
            keys: &keys,
        })
        .unwrap();
        for length in 0..input.len() {
            assert!(decode_ui_request(&input[..length]).is_err());
        }
        for (rows, columns) in [(4, 80), (81, 80), (24, 19), (24, 241)] {
            let resize = RootResize {
                key: KEY,
                expected_layout_revision: 1,
                rows,
                columns,
            };
            assert_eq!(
                encode_root_resize(&resize),
                Err(UiCodecError::InvalidDimensions)
            );
        }
    }

    #[test]
    fn inconsistent_replies_are_rejected() {
        let mut no_divider = two_pane_layout();
        no_divider.divider = None;
        assert_eq!(
            encode_layout_reply(&no_divider),
            Err(UiCodecError::InvalidLayout)
        );
        let mut same = two_pane_layout();
        same.panes[1] = Some(UiPane {
            target: TARGET,
            ..same.panes[1].unwrap()
        });
        assert_eq!(encode_layout_reply(&same), Err(UiCodecError::InvalidLayout));
        let mut outside = two_pane_layout();
        outside.panes[1] = Some(UiPane {
            rect: Rect {
                x: 60,
                y: 0,
                rows: 24,
                columns: 40,
            },
            ..outside.panes[1].unwrap()
        });
        assert_eq!(
            encode_layout_reply(&outside),
            Err(UiCodecError::InvalidLayout)
        );
        let mut zero = two_pane_layout();
        zero.revision = 0;
        assert_eq!(encode_layout_reply(&zero), Err(UiCodecError::ZeroRevision));

        let bytes = encode_layout_reply(&two_pane_layout()).unwrap();
        let mut count = bytes;
        count[17] = 3;
        assert!(decode_layout_reply(&count).is_err());
        let mut flag = bytes;
        flag[PANE0_OFFSET + 49] = 2;
        assert_eq!(decode_layout_reply(&flag), Err(UiCodecError::InvalidLayout));
        let mut tail = bytes;
        tail[183] = 1;
        assert_eq!(
            decode_layout_reply(&tail),
            Err(UiCodecError::NonzeroReserved)
        );

        let applied_pending = ResizeReply {
            key: KEY,
            status: ResizeStatus::Applied,
            pending: [true, false, false, false],
            revision: 1,
            rows: 24,
            columns: 80,
        };
        assert_eq!(
            encode_resize_reply(&applied_pending),
            Err(UiCodecError::InvalidStatus)
        );
        let mut ok = encode_resize_reply(&ResizeReply {
            pending: [false, false, false, false],
            ..applied_pending
        })
        .unwrap();
        ok[31] = 1 << crate::pane_layout::MAX_PANES;
        assert_eq!(decode_resize_reply(&ok), Err(UiCodecError::InvalidStatus));
        // Every pane slot's pending flag round-trips (not only the first two).
        for index in 0..crate::pane_layout::MAX_PANES {
            let mut pending = [false; crate::pane_layout::MAX_PANES];
            pending[index] = true;
            let reply = ResizeReply {
                status: ResizeStatus::ResizePending,
                pending,
                ..applied_pending
            };
            let bytes = encode_resize_reply(&reply).unwrap();
            assert_eq!(decode_resize_reply(&bytes), Ok(reply));
        }

        // Checkpoint for another target or revision is refused.
        assert_eq!(
            prefix_screen_reply(TARGET, 9, checkpoint(SIBLING, 9)),
            Err(UiCodecError::InvalidCheckpoint)
        );
        assert_eq!(
            prefix_screen_reply(TARGET, 8, checkpoint(TARGET, 9)),
            Err(UiCodecError::InvalidCheckpoint)
        );
        let reply = prefix_screen_reply(TARGET, 9, checkpoint(TARGET, 9)).unwrap();
        assert_eq!(
            decode_screen_reply(&reply[..reply.len() - 1]),
            Err(UiCodecError::Truncated)
        );
        let mut extra = reply.clone();
        extra.push(0);
        assert_eq!(
            decode_screen_reply(&extra),
            Err(UiCodecError::TrailingBytes)
        );
        let mut reserved = reply.clone();
        reserved[60] = 1;
        assert_eq!(
            decode_screen_reply(&reserved),
            Err(UiCodecError::NonzeroReserved)
        );
        let mut cell = reply;
        cell[SCREEN_REPLY_HEADER_BYTES] ^= 0xff;
        assert_eq!(
            decode_screen_reply(&cell),
            Err(UiCodecError::InvalidCheckpoint)
        );
    }

    fn nested_three_pane_layout() -> LayoutReply {
        let target = |pane: u64| TargetRef {
            pane,
            session: pane + 1,
            ..TARGET
        };
        let pane = |pane: u64, x: u16, y: u16, rows: u16, columns: u16| {
            Some(UiPane {
                target: target(pane),
                rect: Rect {
                    x,
                    y,
                    rows,
                    columns,
                },
                pending_confirmed: None,
            })
        };
        let vertical = Rect {
            x: 39,
            y: 0,
            rows: 24,
            columns: 1,
        };
        LayoutReply {
            revision: 3,
            rows: 24,
            columns: 80,
            panes: [
                pane(10, 0, 0, 11, 39),
                pane(20, 0, 12, 12, 39),
                pane(30, 40, 0, 24, 40),
                None,
            ],
            divider: Some(vertical),
            dividers: [
                Some(vertical),
                Some(Rect {
                    x: 0,
                    y: 11,
                    rows: 1,
                    columns: 39,
                }),
                None,
            ],
        }
    }

    #[test]
    fn nested_split_with_partial_span_divider_encodes_and_round_trips() {
        let layout = nested_three_pane_layout();
        let bytes = encode_layout_reply(&layout).unwrap();
        assert_eq!(decode_layout_reply(&bytes), Ok(layout));
    }

    #[test]
    fn partial_span_divider_without_flanking_panes_is_rejected() {
        let mut layout = nested_three_pane_layout();
        layout.dividers[1] = Some(Rect {
            x: 0,
            y: 11,
            rows: 1,
            columns: 41,
        });
        assert_eq!(
            encode_layout_reply(&layout),
            Err(UiCodecError::InvalidLayout)
        );
    }

    #[test]
    fn two_pane_layout_must_be_one_exact_partition() {
        let base = two_pane_layout();
        assert!(encode_layout_reply(&base).is_ok());
        let stacked = LayoutReply {
            rows: 25,
            columns: 80,
            panes: [
                Some(UiPane {
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 12,
                        columns: 80,
                    },
                    ..base.panes[0].unwrap()
                }),
                Some(UiPane {
                    rect: Rect {
                        x: 0,
                        y: 13,
                        rows: 12,
                        columns: 80,
                    },
                    ..base.panes[1].unwrap()
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 0,
                y: 12,
                rows: 1,
                columns: 80,
            }),
            dividers: [
                Some(Rect {
                    x: 0,
                    y: 12,
                    rows: 1,
                    columns: 80,
                }),
                None,
                None,
            ],
            ..base
        };
        assert!(encode_layout_reply(&stacked).is_ok());

        let with = |first: Rect, divider: Rect, second: Rect| LayoutReply {
            panes: [
                Some(UiPane {
                    rect: first,
                    ..base.panes[0].unwrap()
                }),
                Some(UiPane {
                    rect: second,
                    ..base.panes[1].unwrap()
                }),
                None,
                None,
            ],
            divider: Some(divider),
            dividers: [Some(divider), None, None],
            ..base
        };
        let r = |x, y, rows, columns| Rect {
            x,
            y,
            rows,
            columns,
        };
        let cases = [
            // Overlap: second starts on the divider.
            with(r(0, 0, 24, 40), r(40, 0, 24, 1), r(40, 0, 24, 41)),
            // Gap: second starts one cell after the divider's neighbour.
            with(r(0, 0, 24, 40), r(40, 0, 24, 1), r(42, 0, 24, 39)),
            // Reversed order.
            with(r(41, 0, 24, 40), r(40, 0, 24, 1), r(0, 0, 24, 40)),
            // Short divider (does not span all rows).
            with(r(0, 0, 24, 40), r(40, 0, 23, 1), r(41, 0, 24, 40)),
            // Misplaced divider.
            with(r(0, 0, 24, 40), r(39, 0, 24, 1), r(41, 0, 24, 40)),
            // Panes do not cover the root (short height).
            with(r(0, 0, 23, 40), r(40, 0, 24, 1), r(41, 0, 24, 40)),
            // Panes do not cover the root (narrow second).
            with(r(0, 0, 24, 40), r(40, 0, 24, 1), r(41, 0, 24, 39)),
            // Two-cell divider.
            with(r(0, 0, 24, 39), r(39, 0, 24, 2), r(41, 0, 24, 40)),
        ];
        for (index, case) in cases.iter().enumerate() {
            let result = encode_layout_reply(case);
            if index == 2 {
                assert!(result.is_ok(), "nested divider partition is valid");
            } else {
                assert_eq!(result, Err(UiCodecError::InvalidLayout), "case {index}");
            }
        }
        // The decoder applies the same rule to wire bytes.
        let mut bytes = encode_layout_reply(&base).unwrap();
        bytes[22..24].copy_from_slice(&5_u16.to_be_bytes()); // divider y
        assert_eq!(
            decode_layout_reply(&bytes),
            Err(UiCodecError::InvalidLayout)
        );
    }

    #[test]
    fn debug_and_error_text_are_redacted() {
        let keys = [Key::Char('s'), Key::Char('e')];
        let request = KeyInput {
            key: KEY,
            target: TARGET,
            expected_owner_revision: 77_777,
            keys: &keys,
        };
        let bytes = encode_key_input(&request).unwrap();
        let decoded = decode_ui_request(&bytes).unwrap();
        let screen = UiRequest::ScreenQuery { target: TARGET };
        let layout = two_pane_layout();
        for text in [
            format!("{request:?}"),
            format!("{decoded:?}"),
            format!("{screen:?}"),
            format!("{:?}", layout.panes[0].unwrap()),
        ] {
            assert!(!text.contains("77777"), "{text}");
            assert!(!text.contains('s') || !text.contains("'s'"), "{text}");
            assert!(!text.contains("pane: 5"), "{text}");
        }
        for error in [
            UiCodecError::Truncated,
            UiCodecError::InvalidKeyScalar,
            UiCodecError::InvalidCheckpoint,
        ] {
            assert!(!error.to_string().chars().any(|c| c.is_ascii_digit()));
        }
    }
}
