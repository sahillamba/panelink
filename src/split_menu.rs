//! Pure, terminal-independent model of the pane context menu (four split
//! items, "Close pane", and "View saved layout") and of the close
//! confirmation prompt.
//!
//! The menu captures one complete pane target and layout revision from a
//! single [`LayoutSnapshot`] when it opens, owns every input while open, and
//! returns exactly the captured values on activation. It performs no terminal
//! IO, rendering, process launch, or layout mutation. Split availability is
//! advisory presentation derived from [`PaneLayout::can_split`]; the trusted
//! split owner remains authoritative and revalidates on submission. The close
//! item is always enabled (the menu always holds a captured target); it never
//! closes anything itself and only asks the caller to show a [`CloseConfirm`].

use crate::control_protocol::TargetRef;
use crate::pane_layout::{
    LayoutError, LayoutSnapshot, PaneGeometry, PaneLayout, Rect, SplitDirection, MIN_PANE_COLUMNS,
    MIN_PANE_ROWS,
};
use std::error::Error;
use std::fmt;

/// Number of split items; the menu always lists all four split directions.
pub const ITEM_COUNT: usize = 4;
/// Number of selectable menu rows: four split items, "Close pane", and
/// "View saved layout".
pub const ROW_COUNT: usize = ITEM_COUNT + 2;
/// Row index of the "Close pane" item, after the four split items.
const CLOSE_ROW: usize = ITEM_COUNT;
/// Row index of the recovery item, after "Close pane".
const RECOVERY_ROW: usize = ITEM_COUNT + 1;
/// Fixed item order presented by the menu.
pub const MENU_DIRECTIONS: [SplitDirection; ITEM_COUNT] = [
    SplitDirection::Left,
    SplitDirection::Right,
    SplitDirection::Up,
    SplitDirection::Down,
];
/// Minimum target width for a left/right split: two minimum panes and a divider.
pub const SPLIT_MIN_COLUMNS: u16 = 2 * MIN_PANE_COLUMNS + 1;
/// Minimum target height for an up/down split: two minimum panes and a divider.
pub const SPLIT_MIN_ROWS: u16 = 2 * MIN_PANE_ROWS + 1;
/// Prototype pane capacity reported in the capacity-disabled reason.
pub const PANE_LIMIT: u8 = crate::pane_layout::MAX_PANES as u8;
/// Height of the normal bordered overlay: title border, six items, status, border.
pub const NORMAL_ROWS: u16 = 9;
/// Preferred width of the normal bordered overlay.
pub const NORMAL_COLUMNS: u16 = 52;
/// Narrowest viewport that still uses the bordered overlay.
pub const MIN_NORMAL_COLUMNS: u16 = 24;
/// Height of the compact overlay: six item rows and one status row. In a
/// viewport of exactly [`MIN_MENU_ROWS`] rows the status row is omitted.
pub const COMPACT_ROWS: u16 = ROW_COUNT as u16 + 1;
/// Shortest viewport in which the menu is interactive: enough rows for every
/// selectable row. Anything shorter is [`OverlayMode::TooSmall`].
pub const MIN_MENU_ROWS: u16 = ROW_COUNT as u16;
/// Narrowest viewport in which the menu is interactive: the terminal-core
/// pane minimum. Anything narrower is [`OverlayMode::TooSmall`].
pub const MIN_MENU_COLUMNS: u16 = MIN_PANE_COLUMNS;
/// Maximum number of overlay rows produced by [`MenuState::overlay_lines`].
pub const MAX_OVERLAY_ROWS: usize = 9;
/// Maximum width in columns (and bytes) of one overlay row.
pub const MAX_OVERLAY_COLUMNS: usize = 240;

const TITLE: &str = " Pane ";
const ENABLED_HINT: &str = "Enter: split  Esc: close";
/// The close row deliberately has no letter shortcut: a stray letter typed
/// into the open menu must never start a close (Ctrl-B `x` is the keyboard path).
const CLOSE_LABEL: &str = "Close pane";
const CLOSE_HINT: &str = "Enter: close this pane (asks y/n)";
const TOO_SMALL_TEXT: &str = "Enlarge terminal to use the split menu";
const ELLIPSIS: &[u8; 3] = b"...";

/// Fixed, sanitized explanation for a disabled split item.
///
/// The value is `Copy` and formats on demand, so it carries no heap storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisabledReason {
    /// The pane is narrower than a left/right split requires.
    TooNarrow {
        /// Requested split direction.
        direction: SplitDirection,
        /// Required pane width in columns.
        needed: u16,
        /// Current pane width in columns.
        actual: u16,
    },
    /// The pane is shorter than an up/down split requires.
    TooShort {
        /// Requested split direction.
        direction: SplitDirection,
        /// Required pane height in rows.
        needed: u16,
        /// Current pane height in rows.
        actual: u16,
    },
    /// A resulting pane would exceed the supported maximum size.
    TooLarge {
        /// Requested split direction.
        direction: SplitDirection,
    },
    /// The layout already holds the prototype's maximum number of panes.
    PaneLimit {
        /// Prototype pane capacity.
        limit: u8,
    },
    /// The layout revision cannot be incremented.
    RevisionExhausted,
    /// The captured target or layout no longer matches.
    TargetStale,
    /// The layout rejected the split for an internal or identity reason.
    Unavailable,
    /// No previous layout is available for viewing.
    NoPreviousLayoutSaved,
}

impl fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooNarrow {
                direction,
                needed,
                actual,
            } => write!(
                f,
                "{} needs {needed} columns; this pane has {actual}",
                direction_label(direction)
            ),
            Self::TooShort {
                direction,
                needed,
                actual,
            } => write!(
                f,
                "{} needs {needed} rows; this pane has {actual}",
                direction_label(direction)
            ),
            Self::TooLarge { direction } => write!(
                f,
                "{} would exceed the maximum pane size",
                direction_label(direction)
            ),
            Self::PaneLimit { limit } => write!(f, "Pane limit reached ({limit})"),
            Self::RevisionExhausted => f.write_str("Layout revision exhausted; splits unavailable"),
            Self::TargetStale => f.write_str("Pane changed; reopen the menu"),
            Self::Unavailable => f.write_str("Split unavailable for this pane"),
            Self::NoPreviousLayoutSaved => f.write_str("No previous layout saved"),
        }
    }
}

/// Whether one menu item can be activated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    /// The item can be activated.
    Enabled,
    /// The item is shown but cannot be activated, for the given reason.
    Disabled(DisabledReason),
}

/// One split action listed in the menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuItem {
    /// Direction the new pane would be placed in.
    pub direction: SplitDirection,
    /// Whether the item is enabled, or why it is disabled.
    pub availability: Availability,
}

impl MenuItem {
    /// Returns whether this item can be activated.
    pub fn is_enabled(self) -> bool {
        self.availability == Availability::Enabled
    }
}

/// Caller-supplied availability for the read-only recovery item.
pub type RecoveryAvailability = Availability;

/// Menu-owned input event, already decoded by the caller.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum MenuInput {
    /// Move the selection up one item (wrapping).
    Up,
    /// Move the selection down one item (wrapping).
    Down,
    /// Select the first item.
    Home,
    /// Select the last item.
    End,
    /// Activate the selected item.
    Activate,
    /// Close the menu without an action.
    Cancel,
    /// A typed character; `l`, `r`, `u`, `d` (any case) select and activate,
    /// while `v` requests recovery when available. No letter selects "Close pane".
    Letter(char),
    /// A mouse button press at a viewport cell.
    MousePress {
        /// Column of the pressed cell.
        x: u16,
        /// Row of the pressed cell.
        y: u16,
        /// Whether the right button was pressed.
        right: bool,
    },
    /// A mouse button release at a viewport cell.
    MouseRelease {
        /// Column of the released cell.
        x: u16,
        /// Row of the released cell.
        y: u16,
        /// Whether the right button was released.
        right: bool,
    },
    /// Any other input; consumed while the menu is open.
    Other,
}

impl fmt::Debug for MenuInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Up => f.write_str("Up"),
            Self::Down => f.write_str("Down"),
            Self::Home => f.write_str("Home"),
            Self::End => f.write_str("End"),
            Self::Activate => f.write_str("Activate"),
            Self::Cancel => f.write_str("Cancel"),
            Self::Letter(_) => f.write_str("Letter([redacted])"),
            Self::MousePress { x, y, right } => f
                .debug_struct("MousePress")
                .field("x", x)
                .field("y", y)
                .field("right", right)
                .finish(),
            Self::MouseRelease { x, y, right } => f
                .debug_struct("MouseRelease")
                .field("x", x)
                .field("y", y)
                .field("right", right)
                .finish(),
            Self::Other => f.write_str("Other"),
        }
    }
}

/// Exact split request values captured when the menu opened.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SplitIntent {
    /// Complete target captured at menu open; never re-resolved from focus.
    pub target: TargetRef,
    /// Layout revision captured from the same snapshot as `target`.
    pub expected_layout_revision: u64,
    /// Activated split direction.
    pub direction: SplitDirection,
}

impl fmt::Debug for SplitIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitIntent")
            .field("target", &"[redacted]")
            .field("expected_layout_revision", &"[redacted]")
            .field("direction", &self.direction)
            .finish()
    }
}

/// Exact close request values captured when the menu or prompt opened.
///
/// `position` and `last_pane` are presentation only (the prompt text); the
/// server decides from `target` and `expected_layout_revision` alone.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CloseIntent {
    /// Complete target captured at open; never re-resolved from focus.
    pub target: TargetRef,
    /// Layout revision captured from the same snapshot as `target`.
    pub expected_layout_revision: u64,
    /// Spatial pane position, 1 (left/top) or 2 (right/bottom).
    pub position: u8,
    /// Whether this is the only pane, so closing it ends the workspace.
    pub last_pane: bool,
}

impl fmt::Debug for CloseIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloseIntent")
            .field("target", &"[redacted]")
            .field("expected_layout_revision", &"[redacted]")
            .field("position", &self.position)
            .field("last_pane", &self.last_pane)
            .finish()
    }
}

/// Result of offering one input to the menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuResponse {
    /// The menu is closed; the caller should route the input normally.
    NotOpen,
    /// The menu consumed the input and remains open.
    Consumed,
    /// The menu consumed the input and closed without an action.
    Closed,
    /// The menu consumed the input, closed, and requests this split.
    Activated(SplitIntent),
    /// The menu consumed the input, closed, and asks the caller to confirm
    /// closing the captured pane. Nothing has been closed.
    CloseRequested(CloseIntent),
    /// The menu consumed the input and requests the read-only recovery viewer.
    RecoveryRequested,
    /// The recovery row is unavailable; the menu stays open.
    RecoveryUnavailable,
}

/// Failure to open the menu; the previous menu state is left unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuOpenError {
    /// The pointer is on a divider or outside every pane.
    NoPaneAtPointer,
    /// The focused target does not exactly identify a current pane.
    TargetNotInLayout,
}

impl fmt::Display for MenuOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoPaneAtPointer => "no pane under the pointer",
            Self::TargetNotInLayout => "focused target is not in the layout",
        })
    }
}

impl Error for MenuOpenError {}

/// Overlay presentation chosen for the current viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayMode {
    /// Bordered box with a title, five items and a status row.
    Normal,
    /// Borderless, full-width: five items and, when room, a status row.
    Compact,
    /// Viewport too small for items; one status row, only Cancel is accepted.
    TooSmall,
}

/// Presentation style of one overlay row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayStyle {
    /// An enabled, unselected item.
    Normal,
    /// The selected item, enabled or not.
    Selected,
    /// A disabled, unselected item.
    Disabled,
    /// A border or title row.
    Border,
    /// The status row.
    Status,
}

/// One fixed-width overlay row of printable ASCII text.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OverlayLine {
    bytes: [u8; MAX_OVERLAY_COLUMNS],
    len: usize,
    style: OverlayStyle,
}

impl OverlayLine {
    const BLANK: Self = Self {
        bytes: [b' '; MAX_OVERLAY_COLUMNS],
        len: 0,
        style: OverlayStyle::Normal,
    };

    /// Returns the row text; its length equals the overlay width in columns.
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    /// Returns the row style.
    pub fn style(&self) -> OverlayStyle {
        self.style
    }

    fn new(width: usize, style: OverlayStyle) -> Self {
        let mut line = Self::BLANK;
        line.len = width.min(MAX_OVERLAY_COLUMNS);
        line.style = style;
        line
    }

    fn plain(width: usize, style: OverlayStyle, content: &dyn fmt::Display) -> Self {
        let mut line = Self::new(width, style);
        write_clipped(&mut line.bytes[..line.len], content);
        line
    }

    fn boxed(width: usize, style: OverlayStyle, content: &dyn fmt::Display) -> Self {
        let mut line = Self::new(width, style);
        let len = line.len;
        if len >= 4 {
            line.bytes[0] = b'|';
            line.bytes[len - 1] = b'|';
            write_clipped(&mut line.bytes[2..len - 2], content);
        } else {
            write_clipped(&mut line.bytes[..len], content);
        }
        line
    }

    fn border(width: usize, title: Option<&str>) -> Self {
        let mut line = Self::new(width, OverlayStyle::Border);
        let len = line.len;
        if len >= 2 {
            line.bytes[..len].fill(b'-');
            line.bytes[0] = b'+';
            line.bytes[len - 1] = b'+';
        }
        if let (Some(title), true) = (title, len >= 4) {
            write_clipped(&mut line.bytes[2..len - 2], &title);
        }
        line
    }
}

impl fmt::Debug for OverlayLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayLine")
            .field("text", &self.text())
            .field("style", &self.style)
            .finish()
    }
}

/// Fixed-capacity sequence of overlay rows, top to bottom.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OverlayLines {
    lines: [OverlayLine; MAX_OVERLAY_ROWS],
    count: usize,
}

impl OverlayLines {
    const EMPTY: Self = Self {
        lines: [OverlayLine::BLANK; MAX_OVERLAY_ROWS],
        count: 0,
    };

    /// Returns the produced rows; row `i` is drawn at `overlay_rect().y + i`.
    pub fn as_slice(&self) -> &[OverlayLine] {
        &self.lines[..self.count]
    }

    /// Returns the number of produced rows.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns whether no rows were produced.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn push(&mut self, line: OverlayLine) {
        if let Some(slot) = self.lines.get_mut(self.count) {
            *slot = line;
            self.count += 1;
        }
    }
}

impl fmt::Debug for OverlayLines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

/// Split context-menu state: closed, or open with one captured target.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct MenuState {
    open: Option<OpenMenu>,
}

impl MenuState {
    /// Returns a closed menu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the menu is open and owns all input.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Opens the menu for the pane under a pointer cell.
    ///
    /// Divider cells and cells outside every pane open nothing and leave the
    /// current state unchanged. Viewport dimensions select the overlay mode.
    pub fn open_at_pointer(
        &mut self,
        layout: &PaneLayout,
        x: u16,
        y: u16,
        viewport_rows: u16,
        viewport_columns: u16,
    ) -> Result<(), MenuOpenError> {
        self.open_at_pointer_with_recovery(
            layout,
            x,
            y,
            viewport_rows,
            viewport_columns,
            Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        )
    }

    /// Opens the menu with caller-supplied previous-layout availability.
    /// The menu model itself performs no storage access.
    pub fn open_at_pointer_with_recovery(
        &mut self,
        layout: &PaneLayout,
        x: u16,
        y: u16,
        viewport_rows: u16,
        viewport_columns: u16,
        recovery_availability: RecoveryAvailability,
    ) -> Result<(), MenuOpenError> {
        let snapshot = layout.snapshot();
        let pane = snapshot
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.rect.contains(x, y))
            .ok_or(MenuOpenError::NoPaneAtPointer)?;
        self.open = Some(OpenMenu::new(
            layout,
            snapshot,
            pane,
            (x, y),
            (viewport_rows, viewport_columns),
            recovery_availability,
        ));
        Ok(())
    }

    /// Opens the menu for the focused pane, anchored at its top-left cell.
    ///
    /// The target must exactly match a current pane; otherwise the current
    /// state is left unchanged.
    pub fn open_for_focused(
        &mut self,
        layout: &PaneLayout,
        focused: TargetRef,
        viewport_rows: u16,
        viewport_columns: u16,
    ) -> Result<(), MenuOpenError> {
        self.open_for_focused_with_recovery(
            layout,
            focused,
            viewport_rows,
            viewport_columns,
            Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        )
    }

    /// Opens the menu for its focused pane with caller-supplied recovery
    /// availability. The menu model itself performs no storage access.
    pub fn open_for_focused_with_recovery(
        &mut self,
        layout: &PaneLayout,
        focused: TargetRef,
        viewport_rows: u16,
        viewport_columns: u16,
        recovery_availability: RecoveryAvailability,
    ) -> Result<(), MenuOpenError> {
        let snapshot = layout.snapshot();
        let pane = snapshot
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.target == focused)
            .ok_or(MenuOpenError::TargetNotInLayout)?;
        self.open = Some(OpenMenu::new(
            layout,
            snapshot,
            pane,
            (pane.rect.x, pane.rect.y),
            (viewport_rows, viewport_columns),
            recovery_availability,
        ));
        Ok(())
    }

    /// Closes the menu without an action.
    pub fn close(&mut self) {
        self.open = None;
    }

    /// Re-places an open overlay for a new viewport, keeping its captures.
    pub fn set_viewport(&mut self, viewport_rows: u16, viewport_columns: u16) {
        if let Some(menu) = self.open.as_mut() {
            let (mode, rect) = place(menu.anchor, (viewport_rows, viewport_columns));
            menu.mode = mode;
            menu.rect = rect;
        }
    }

    /// Returns the target captured when the menu opened.
    pub fn captured_target(&self) -> Option<TargetRef> {
        self.open.map(|menu| menu.target)
    }

    /// Returns the layout revision captured when the menu opened.
    pub fn captured_revision(&self) -> Option<u64> {
        self.open.map(|menu| menu.revision)
    }

    /// Returns the four split items in order Left, Right, Up, Down.
    pub fn items(&self) -> Option<[MenuItem; ITEM_COUNT]> {
        self.open.map(|menu| menu.items)
    }

    /// Returns the direction of the selected split item; `None` when the
    /// menu is closed or the "Close pane" item is selected.
    pub fn selected(&self) -> Option<SplitDirection> {
        self.open
            .and_then(|menu| menu.items.get(menu.selected).map(|item| item.direction))
    }

    /// Returns whether the open menu has the "Close pane" item selected.
    pub fn close_selected(&self) -> bool {
        self.open.is_some_and(|menu| menu.selected == CLOSE_ROW)
    }

    /// Returns whether the recovery viewer row is selected.
    pub fn recovery_selected(&self) -> bool {
        self.open.is_some_and(|menu| menu.selected == RECOVERY_ROW)
    }

    /// Returns the caller-supplied recovery availability for the open menu.
    pub fn recovery_availability(&self) -> Option<RecoveryAvailability> {
        self.open.map(|menu| menu.recovery_availability)
    }

    /// Returns the overlay mode for the current viewport.
    pub fn mode(&self) -> Option<OverlayMode> {
        self.open.map(|menu| menu.mode)
    }

    /// Returns the overlay rectangle, always inside the viewport.
    pub fn overlay_rect(&self) -> Option<Rect> {
        self.open.map(|menu| menu.rect)
    }

    /// Returns fixed-width overlay rows; empty when closed.
    pub fn overlay_lines(&self) -> OverlayLines {
        match self.open {
            Some(menu) => menu.lines(),
            None => OverlayLines::EMPTY,
        }
    }

    /// Offers one input to the menu.
    ///
    /// When closed this returns [`MenuResponse::NotOpen`]. While open every
    /// input, including the one that closes the menu, is consumed and must
    /// not be forwarded to a pane.
    pub fn handle(&mut self, input: MenuInput) -> MenuResponse {
        let Some(mut menu) = self.open else {
            return MenuResponse::NotOpen;
        };
        let response = menu.handle(input);
        self.open = match response {
            MenuResponse::Consumed | MenuResponse::RecoveryUnavailable => Some(menu),
            MenuResponse::NotOpen
            | MenuResponse::Closed
            | MenuResponse::Activated(_)
            | MenuResponse::CloseRequested(_)
            | MenuResponse::RecoveryRequested => None,
        };
        response
    }
}

impl fmt::Debug for MenuState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.open {
            None => f.debug_struct("MenuState").field("open", &false).finish(),
            Some(menu) => f
                .debug_struct("MenuState")
                .field("open", &true)
                .field("target", &"[redacted]")
                .field("layout_revision", &"[redacted]")
                .field("mode", &menu.mode)
                .field("selected", &self.selected())
                .field("close_selected", &self.close_selected())
                .field("items", &menu.items)
                .field("overlay", &menu.rect)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct OpenMenu {
    target: TargetRef,
    revision: u64,
    position: u8,
    last_pane: bool,
    items: [MenuItem; ITEM_COUNT],
    recovery_availability: RecoveryAvailability,
    selected: usize,
    anchor: (u16, u16),
    mode: OverlayMode,
    rect: Rect,
}

enum Hit {
    Item(usize),
    Chrome,
    Outside,
}

impl OpenMenu {
    #[allow(clippy::too_many_arguments)]
    fn new(
        layout: &PaneLayout,
        snapshot: LayoutSnapshot,
        pane: PaneGeometry,
        anchor: (u16, u16),
        viewport: (u16, u16),
        recovery_availability: RecoveryAvailability,
    ) -> Self {
        let items = MENU_DIRECTIONS.map(|direction| MenuItem {
            direction,
            availability: match layout.can_split(pane.target, direction) {
                Ok(()) => Availability::Enabled,
                Err(error) => Availability::Disabled(reason_for(error, direction, pane.rect)),
            },
        });
        let selected = items.iter().position(|item| item.is_enabled()).unwrap_or(0);
        let (mode, rect) = place(anchor, viewport);
        let index = snapshot
            .panes
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.target == pane.target))
            .unwrap_or(0);
        Self {
            target: pane.target,
            revision: snapshot.revision,
            position: u8::try_from(index + 1).unwrap_or(u8::MAX),
            last_pane: snapshot.pane_count() <= 1,
            items,
            recovery_availability,
            selected,
            anchor,
            mode,
            rect,
        }
    }

    fn handle(&mut self, input: MenuInput) -> MenuResponse {
        if self.mode == OverlayMode::TooSmall {
            return if input == MenuInput::Cancel {
                MenuResponse::Closed
            } else {
                MenuResponse::Consumed
            };
        }
        match input {
            MenuInput::Up => {
                self.selected = (self.selected + ROW_COUNT - 1) % ROW_COUNT;
                MenuResponse::Consumed
            }
            MenuInput::Down => {
                self.selected = (self.selected + 1) % ROW_COUNT;
                MenuResponse::Consumed
            }
            MenuInput::Home => {
                self.selected = 0;
                MenuResponse::Consumed
            }
            MenuInput::End => {
                self.selected = ROW_COUNT - 1;
                MenuResponse::Consumed
            }
            MenuInput::Activate => self.activate_selected(),
            MenuInput::Cancel => MenuResponse::Closed,
            MenuInput::Letter(letter) => match shortcut_index(letter) {
                Some(index) => {
                    self.selected = index;
                    self.activate_selected()
                }
                None => MenuResponse::Consumed,
            },
            MenuInput::MousePress { x, y, .. } => match self.hit(x, y) {
                Hit::Item(index) => {
                    self.selected = index;
                    self.activate_selected()
                }
                Hit::Chrome => MenuResponse::Consumed,
                Hit::Outside => MenuResponse::Closed,
            },
            MenuInput::MouseRelease { .. } | MenuInput::Other => MenuResponse::Consumed,
        }
    }

    fn activate_selected(&self) -> MenuResponse {
        if self.selected == RECOVERY_ROW {
            return if self.recovery_availability == Availability::Enabled {
                MenuResponse::RecoveryRequested
            } else {
                MenuResponse::RecoveryUnavailable
            };
        }
        let Some(item) = self.items.get(self.selected).copied() else {
            if self.selected != CLOSE_ROW {
                return MenuResponse::Consumed;
            }
            return MenuResponse::CloseRequested(CloseIntent {
                target: self.target,
                expected_layout_revision: self.revision,
                position: self.position,
                last_pane: self.last_pane,
            });
        };
        if item.is_enabled() {
            MenuResponse::Activated(SplitIntent {
                target: self.target,
                expected_layout_revision: self.revision,
                direction: item.direction,
            })
        } else {
            MenuResponse::Consumed
        }
    }

    fn hit(&self, x: u16, y: u16) -> Hit {
        if !self.rect.contains(x, y) {
            return Hit::Outside;
        }
        let row = usize::from(y - self.rect.y);
        match self.mode {
            OverlayMode::Normal => {
                let inner = x > self.rect.x && x - self.rect.x < self.rect.columns - 1;
                if (1..=ROW_COUNT).contains(&row) && inner {
                    Hit::Item(row - 1)
                } else {
                    Hit::Chrome
                }
            }
            OverlayMode::Compact if row < ROW_COUNT => Hit::Item(row),
            OverlayMode::Compact | OverlayMode::TooSmall => Hit::Chrome,
        }
    }

    fn item_style(&self, index: usize) -> OverlayStyle {
        if index == self.selected {
            OverlayStyle::Selected
        } else if index == RECOVERY_ROW && self.recovery_availability != Availability::Enabled {
            OverlayStyle::Disabled
        } else if self.items.get(index).is_none_or(|item| item.is_enabled()) {
            OverlayStyle::Normal
        } else {
            OverlayStyle::Disabled
        }
    }

    fn status(&self) -> StatusText {
        match self.selected {
            CLOSE_ROW => StatusText::Close,
            RECOVERY_ROW => match self.recovery_availability {
                Availability::Enabled => StatusText::Recovery(None),
                Availability::Disabled(reason) => StatusText::Recovery(Some(reason)),
            },
            _ => match self.items.get(self.selected).map(|item| item.availability) {
                Some(Availability::Enabled) => StatusText::Split(None),
                Some(Availability::Disabled(reason)) => StatusText::Split(Some(reason)),
                None => StatusText::Close,
            },
        }
    }

    fn lines(&self) -> OverlayLines {
        let mut lines = OverlayLines::EMPTY;
        let width = usize::from(self.rect.columns).min(MAX_OVERLAY_COLUMNS);
        if width == 0 || self.rect.rows == 0 {
            return lines;
        }
        match self.mode {
            OverlayMode::Normal => {
                lines.push(OverlayLine::border(width, Some(TITLE)));
                for index in 0..ROW_COUNT {
                    lines.push(OverlayLine::boxed(
                        width,
                        self.item_style(index),
                        &self.item_label(index),
                    ));
                }
                lines.push(OverlayLine::boxed(
                    width,
                    OverlayStyle::Status,
                    &self.status(),
                ));
                lines.push(OverlayLine::border(width, None));
            }
            OverlayMode::Compact => {
                for index in 0..ROW_COUNT {
                    lines.push(OverlayLine::plain(
                        width,
                        self.item_style(index),
                        &self.item_label(index),
                    ));
                }
                if usize::from(self.rect.rows) > ROW_COUNT {
                    lines.push(OverlayLine::plain(
                        width,
                        OverlayStyle::Status,
                        &self.status(),
                    ));
                }
            }
            OverlayMode::TooSmall => {
                lines.push(OverlayLine::plain(
                    width,
                    OverlayStyle::Status,
                    &TOO_SMALL_TEXT,
                ));
            }
        }
        lines
    }

    fn item_label(&self, index: usize) -> ItemLabel {
        ItemLabel {
            item: self.items.get(index).copied(),
            recovery: index == RECOVERY_ROW,
            recovery_enabled: self.recovery_availability == Availability::Enabled,
            selected: index == self.selected,
        }
    }
}

/// One menu row label; the final two rows are close and recovery.
struct ItemLabel {
    item: Option<MenuItem>,
    recovery: bool,
    recovery_enabled: bool,
    selected: bool,
}

impl fmt::Display for ItemLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let marker = if self.selected { "> " } else { "  " };
        if self.recovery {
            if !self.recovery_enabled {
                return f.write_str("  No previous layout saved");
            }
            return write!(f, "{marker}View saved layout [V]");
        }
        let Some(item) = self.item else {
            return write!(f, "{marker}{CLOSE_LABEL}");
        };
        let suffix = if item.is_enabled() { "" } else { " - disabled" };
        write!(
            f,
            "{marker}{} [{}]{suffix}",
            direction_label(item.direction),
            shortcut(item.direction)
        )
    }
}

enum StatusText {
    Split(Option<DisabledReason>),
    Recovery(Option<DisabledReason>),
    Close,
}

impl fmt::Display for StatusText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Split(Some(reason)) => reason.fmt(f),
            Self::Split(None) => f.write_str(ENABLED_HINT),
            Self::Recovery(Some(reason)) => reason.fmt(f),
            Self::Recovery(None) => f.write_str("Enter: view saved layout"),
            Self::Close => f.write_str(CLOSE_HINT),
        }
    }
}

fn direction_label(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Left => "Split Left",
        SplitDirection::Right => "Split Right",
        SplitDirection::Up => "Split Up",
        SplitDirection::Down => "Split Down",
    }
}

fn shortcut(direction: SplitDirection) -> char {
    match direction {
        SplitDirection::Left => 'L',
        SplitDirection::Right => 'R',
        SplitDirection::Up => 'U',
        SplitDirection::Down => 'D',
    }
}

fn shortcut_index(letter: char) -> Option<usize> {
    let upper = letter.to_ascii_uppercase();
    if upper == 'V' {
        return Some(RECOVERY_ROW);
    }
    MENU_DIRECTIONS
        .iter()
        .position(|direction| shortcut(*direction) == upper)
}

fn reason_for(error: LayoutError, direction: SplitDirection, pane: Rect) -> DisabledReason {
    match error {
        LayoutError::LayoutTooSmall => match direction {
            SplitDirection::Left | SplitDirection::Right => DisabledReason::TooNarrow {
                direction,
                needed: SPLIT_MIN_COLUMNS,
                actual: pane.columns,
            },
            SplitDirection::Up | SplitDirection::Down => DisabledReason::TooShort {
                direction,
                needed: SPLIT_MIN_ROWS,
                actual: pane.rows,
            },
        },
        LayoutError::LayoutTooLarge => DisabledReason::TooLarge { direction },
        LayoutError::PaneLimitExceeded => DisabledReason::PaneLimit { limit: PANE_LIMIT },
        LayoutError::RevisionExhausted => DisabledReason::RevisionExhausted,
        LayoutError::StaleTarget | LayoutError::StaleRevision | LayoutError::PreviewStale => {
            DisabledReason::TargetStale
        }
        LayoutError::InvalidTarget
        | LayoutError::InvalidDimensions
        | LayoutError::TargetScopeMismatch
        | LayoutError::TargetIdentityConflict
        | LayoutError::InvalidLayoutState => DisabledReason::Unavailable,
    }
}

fn place(anchor: (u16, u16), viewport: (u16, u16)) -> (OverlayMode, Rect) {
    let (anchor_x, anchor_y) = anchor;
    let (rows, columns) = viewport;
    let capped_columns = columns.min(MAX_OVERLAY_COLUMNS as u16);
    // Below the terminal-core minimum the menu is never interactive.
    let interactive = rows >= MIN_MENU_ROWS && columns >= MIN_MENU_COLUMNS;
    if interactive && rows >= NORMAL_ROWS && columns >= MIN_NORMAL_COLUMNS {
        let width = NORMAL_COLUMNS.min(columns);
        let rect = Rect {
            x: anchor_x.min(columns - width),
            y: anchor_y.min(rows - NORMAL_ROWS),
            rows: NORMAL_ROWS,
            columns: width,
        };
        (OverlayMode::Normal, rect)
    } else if interactive {
        // `interactive` guarantees at least MIN_MENU_ROWS (= ROW_COUNT) rows.
        let height = rows.min(COMPACT_ROWS);
        let rect = Rect {
            x: 0,
            y: anchor_y.min(rows - height),
            rows: height,
            columns: capped_columns,
        };
        (OverlayMode::Compact, rect)
    } else if rows >= 1 && columns >= 1 {
        let rect = Rect {
            x: 0,
            y: anchor_y.min(rows - 1),
            rows: 1,
            columns: capped_columns,
        };
        (OverlayMode::TooSmall, rect)
    } else {
        let rect = Rect {
            x: 0,
            y: 0,
            rows: 0,
            columns: 0,
        };
        (OverlayMode::TooSmall, rect)
    }
}

// The compact overlay must fit all selectable rows at the interactive minimum.
const _: () = assert!(MIN_MENU_ROWS as usize >= ROW_COUNT);

/// Input to the close confirmation prompt, already decoded by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmInput {
    /// `y` or `Y`: confirm the close.
    Yes,
    /// Any other key press, paste or mouse press: cancel without closing.
    Cancel,
    /// Releases, focus and resize events: consumed, prompt stays open.
    Ignore,
}

/// Result of offering one input to the confirmation prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmResponse {
    /// No prompt is open; the caller should route the input normally.
    NotOpen,
    /// The prompt consumed the input and stays open.
    Consumed,
    /// The prompt consumed the input and closed without an action.
    Cancelled,
    /// The prompt consumed the input, closed, and requests this close.
    Confirmed(CloseIntent),
}

/// Explicit y/n confirmation for closing one captured pane.
///
/// While open the prompt owns every input; nothing reaches a pane. Only
/// [`ConfirmInput::Yes`] confirms. The intent is returned exactly as
/// captured; it is never re-resolved from focus or a newer layout.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct CloseConfirm {
    pending: Option<CloseIntent>,
}

impl CloseConfirm {
    /// Returns a closed prompt.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens (or replaces) the prompt for one captured intent.
    pub fn open(&mut self, intent: CloseIntent) {
        self.pending = Some(intent);
    }

    /// Returns whether the prompt is open and owns all input.
    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// Returns the captured intent while open.
    pub fn intent(&self) -> Option<CloseIntent> {
        self.pending
    }

    /// Offers one input. While open every input is consumed.
    pub fn handle(&mut self, input: ConfirmInput) -> ConfirmResponse {
        let Some(intent) = self.pending else {
            return ConfirmResponse::NotOpen;
        };
        match input {
            ConfirmInput::Yes => {
                self.pending = None;
                ConfirmResponse::Confirmed(intent)
            }
            ConfirmInput::Cancel => {
                self.pending = None;
                ConfirmResponse::Cancelled
            }
            ConfirmInput::Ignore => ConfirmResponse::Consumed,
        }
    }

    /// One fixed-width prompt row of `width` columns (at most
    /// [`MAX_OVERLAY_COLUMNS`]); `None` when closed or `width` is zero.
    pub fn overlay_line(&self, width: u16) -> Option<OverlayLine> {
        let intent = self.pending?;
        let width = usize::from(width).min(MAX_OVERLAY_COLUMNS);
        (width > 0)
            .then(|| OverlayLine::plain(width, OverlayStyle::Selected, &ConfirmPrompt(intent)))
    }
}

impl fmt::Debug for CloseConfirm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloseConfirm")
            .field("pending", &self.pending)
            .finish()
    }
}

struct ConfirmPrompt(CloseIntent);

impl fmt::Display for ConfirmPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.last_pane {
            write!(
                f,
                " Close pane {}? This is the last pane; the workspace ends. (y/n) ",
                self.0.position
            )
        } else {
            write!(f, " Close pane {}? (y/n) ", self.0.position)
        }
    }
}

/// Writes printable ASCII into `buf`, replacing other characters with `?`
/// and ending overflowing text with an ASCII ellipsis. Never wraps.
fn write_clipped(buf: &mut [u8], content: &dyn fmt::Display) {
    let mut writer = ClipWriter {
        buf,
        len: 0,
        overflow: false,
    };
    // `ClipWriter` never fails and menu `Display` impls only propagate its
    // result, so an error here cannot occur; clipped output is still valid.
    let _ = fmt::write(&mut writer, format_args!("{content}"));
    if writer.overflow {
        let width = writer.buf.len();
        let dots = width.min(ELLIPSIS.len());
        writer.buf[width - dots..].copy_from_slice(&ELLIPSIS[..dots]);
    }
}

struct ClipWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl fmt::Write for ClipWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for character in text.chars() {
            let byte = match u8::try_from(character) {
                Ok(byte) if byte == b' ' || byte.is_ascii_graphic() => byte,
                _ => b'?',
            };
            match self.buf.get_mut(self.len) {
                Some(slot) => {
                    *slot = byte;
                    self.len += 1;
                }
                None => {
                    self.overflow = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_layout::{MAX_PANES, MAX_PANE_COLUMNS, MAX_PANE_ROWS};

    fn target_a() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 33,
            session: 44,
            occupant_revision: 55,
        }
    }

    fn target_b() -> TargetRef {
        TargetRef {
            generation: 11,
            workspace: 22,
            pane: 66,
            session: 77,
            occupant_revision: 88,
        }
    }

    fn single(rows: u16, columns: u16) -> PaneLayout {
        PaneLayout::new(target_a(), rows, columns).expect("valid root")
    }

    /// Two panes: A on the left (columns 0..40), divider at 40, B at 41..81.
    fn split_right(rows: u16, columns: u16) -> PaneLayout {
        let mut layout = single(rows, columns);
        let preview = layout
            .preview_split(target_a(), 1, SplitDirection::Right, target_b())
            .expect("preview");
        layout.commit_split(preview).expect("commit");
        layout
    }

    fn open_keyboard(layout: &PaneLayout) -> MenuState {
        let mut menu = MenuState::new();
        menu.open_for_focused(layout, target_a(), 40, 120)
            .expect("open");
        menu
    }

    fn availability(menu: &MenuState) -> [Availability; ITEM_COUNT] {
        menu.items().expect("open").map(|item| item.availability)
    }

    fn reason_text(menu: &MenuState, index: usize) -> String {
        match menu.items().expect("open")[index].availability {
            Availability::Disabled(reason) => reason.to_string(),
            Availability::Enabled => String::from("enabled"),
        }
    }

    fn assert_line_invariants(menu: &MenuState) {
        let rect = menu.overlay_rect().expect("open");
        let lines = menu.overlay_lines();
        assert_eq!(lines.len(), usize::from(rect.rows));
        for line in lines.as_slice() {
            assert_eq!(line.text().len(), usize::from(rect.columns));
            assert!(line
                .text()
                .bytes()
                .all(|byte| byte == b' ' || byte.is_ascii_graphic()));
        }
    }

    fn assert_inside(rect: Rect, rows: u16, columns: u16) {
        assert!(u32::from(rect.x) + u32::from(rect.columns) <= u32::from(columns));
        assert!(u32::from(rect.y) + u32::from(rect.rows) <= u32::from(rows));
    }

    #[test]
    fn all_four_directions_enabled_in_order_at_a_large_size() {
        let menu = open_keyboard(&single(MAX_PANE_ROWS, MAX_PANE_COLUMNS));
        let items = menu.items().expect("open");
        assert_eq!(items.map(|item| item.direction), MENU_DIRECTIONS);
        assert!(items.iter().all(|item| item.is_enabled()));
        assert_eq!(menu.selected(), Some(SplitDirection::Left));
    }

    #[test]
    fn column_threshold_is_41_and_reason_names_both_sizes() {
        let menu = open_keyboard(&single(11, 41));
        assert_eq!(availability(&menu), [Availability::Enabled; ITEM_COUNT]);

        let menu = open_keyboard(&single(11, 40));
        assert_eq!(
            reason_text(&menu, 0),
            "Split Left needs 41 columns; this pane has 40"
        );
        assert_eq!(
            reason_text(&menu, 1),
            "Split Right needs 41 columns; this pane has 40"
        );
        assert_eq!(availability(&menu)[2], Availability::Enabled);
        assert_eq!(availability(&menu)[3], Availability::Enabled);
        assert_eq!(menu.selected(), Some(SplitDirection::Up));

        let menu = open_keyboard(&single(11, 38));
        assert_eq!(
            reason_text(&menu, 0),
            "Split Left needs 41 columns; this pane has 38"
        );
    }

    #[test]
    fn row_threshold_is_11_and_reason_names_both_sizes() {
        let menu = open_keyboard(&single(10, 41));
        assert_eq!(availability(&menu)[0], Availability::Enabled);
        assert_eq!(availability(&menu)[1], Availability::Enabled);
        assert_eq!(
            reason_text(&menu, 2),
            "Split Up needs 11 rows; this pane has 10"
        );
        assert_eq!(
            reason_text(&menu, 3),
            "Split Down needs 11 rows; this pane has 10"
        );

        let menu = open_keyboard(&single(9, 41));
        assert_eq!(
            reason_text(&menu, 2),
            "Split Up needs 11 rows; this pane has 9"
        );
    }

    #[test]
    fn menu_availability_agrees_with_layout_can_split_across_a_size_sweep() {
        for rows in [5, 6, 9, 10, 11, 12, 21, 79, 80] {
            for columns in [20, 21, 39, 40, 41, 42, 81, 239, 240] {
                let layout = single(rows, columns);
                let menu = open_keyboard(&layout);
                for item in menu.items().expect("open") {
                    let allowed = layout.can_split(target_a(), item.direction).is_ok();
                    assert_eq!(item.is_enabled(), allowed, "{rows}x{columns} {item:?}");
                    let rule = match item.direction {
                        SplitDirection::Left | SplitDirection::Right => {
                            columns >= SPLIT_MIN_COLUMNS
                        }
                        SplitDirection::Up | SplitDirection::Down => rows >= SPLIT_MIN_ROWS,
                    };
                    assert_eq!(allowed, rule, "{rows}x{columns} {item:?}");
                }
            }
        }
    }

    #[test]
    fn committed_tree_disables_items_only_when_full_or_too_small() {
        let mut layout = split_right(40, 161);
        let first = layout.snapshot();
        let third = TargetRef {
            pane: 88,
            session: 99,
            ..target_b()
        };
        let preview = layout
            .preview_split(target_a(), first.revision, SplitDirection::Down, third)
            .unwrap();
        layout.commit_split(preview).unwrap();
        let second_revision = layout.snapshot().revision;
        let fourth = TargetRef {
            pane: 90,
            session: 101,
            ..target_b()
        };
        let preview = layout
            .preview_split(target_a(), second_revision, SplitDirection::Up, fourth)
            .unwrap();
        layout.commit_split(preview).unwrap();
        let menu = open_keyboard(&layout);
        let limit = Availability::Disabled(DisabledReason::PaneLimit {
            limit: MAX_PANES as u8,
        });
        assert_eq!(availability(&menu), [limit; ITEM_COUNT]);
        assert_eq!(reason_text(&menu, 0), "Pane limit reached (4)");
        assert_eq!(menu.selected(), Some(SplitDirection::Left));
    }

    #[test]
    fn layout_errors_map_to_fixed_reasons() {
        let pane = Rect {
            x: 0,
            y: 0,
            rows: 9,
            columns: 38,
        };
        assert_eq!(
            reason_for(LayoutError::RevisionExhausted, SplitDirection::Up, pane).to_string(),
            "Layout revision exhausted; splits unavailable"
        );
        assert_eq!(
            reason_for(LayoutError::LayoutTooLarge, SplitDirection::Down, pane),
            DisabledReason::TooLarge {
                direction: SplitDirection::Down
            }
        );
        assert_eq!(
            reason_for(LayoutError::StaleTarget, SplitDirection::Left, pane),
            DisabledReason::TargetStale
        );
        assert_eq!(
            reason_for(LayoutError::InvalidLayoutState, SplitDirection::Left, pane),
            DisabledReason::Unavailable
        );
        assert_eq!(
            reason_for(LayoutError::LayoutTooSmall, SplitDirection::Up, pane).to_string(),
            "Split Up needs 11 rows; this pane has 9"
        );
    }

    #[test]
    fn pointer_selects_pane_under_cell_regardless_of_focus_and_divider_opens_nothing() {
        let layout = split_right(20, 81);
        let revision = layout.snapshot().revision;
        let mut menu = MenuState::new();
        // Keyboard focus would be A; the pointer on B wins.
        menu.open_at_pointer(&layout, 60, 3, 20, 81)
            .expect("pane B");
        assert_eq!(menu.captured_target(), Some(target_b()));
        assert_eq!(menu.captured_revision(), Some(revision));

        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 5, 3, 20, 81).expect("pane A");
        assert_eq!(menu.captured_target(), Some(target_a()));

        let mut menu = MenuState::new();
        assert_eq!(
            menu.open_at_pointer(&layout, 40, 3, 20, 81),
            Err(MenuOpenError::NoPaneAtPointer)
        );
        assert_eq!(
            menu.open_at_pointer(&layout, 81, 3, 20, 81),
            Err(MenuOpenError::NoPaneAtPointer)
        );
        assert_eq!(
            menu.open_at_pointer(&layout, 5, 20, 20, 81),
            Err(MenuOpenError::NoPaneAtPointer)
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn keyboard_open_rejects_target_not_in_layout() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        let mut stale = target_a();
        stale.occupant_revision += 1;
        assert_eq!(
            menu.open_for_focused(&layout, stale, 20, 81),
            Err(MenuOpenError::TargetNotInLayout)
        );
        assert_eq!(
            menu.open_for_focused(&layout, target_b(), 20, 81),
            Err(MenuOpenError::TargetNotInLayout)
        );
        assert!(!menu.is_open());
        assert_eq!(
            MenuOpenError::TargetNotInLayout.to_string(),
            "focused target is not in the layout"
        );
    }

    #[test]
    fn activation_returns_captured_target_and_revision_after_focus_moves() {
        let mut layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_for_focused(&layout, target_a(), 20, 81)
            .expect("open on A");
        // Focus moves to a new pane B and the layout revision advances.
        let preview = layout
            .preview_split(target_a(), 1, SplitDirection::Right, target_b())
            .expect("preview");
        layout.commit_split(preview).expect("commit");
        assert_eq!(layout.snapshot().revision, 2);
        assert_eq!(
            menu.handle(MenuInput::Letter('r')),
            MenuResponse::Activated(SplitIntent {
                target: target_a(),
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
            })
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn every_input_while_open_is_owned_by_the_menu() {
        let inputs = [
            MenuInput::Up,
            MenuInput::Down,
            MenuInput::Home,
            MenuInput::End,
            MenuInput::Activate,
            MenuInput::Cancel,
            MenuInput::Letter('l'),
            MenuInput::Letter('x'),
            MenuInput::MousePress {
                x: 0,
                y: 0,
                right: false,
            },
            MenuInput::MousePress {
                x: 200,
                y: 30,
                right: true,
            },
            MenuInput::MouseRelease {
                x: 0,
                y: 0,
                right: true,
            },
            MenuInput::Other,
        ];
        for (rows, columns) in [(40, 120), (5, 20), (4, 20), (40, 10)] {
            for (layout, label) in [(single(20, 81), "single"), (split_right(20, 81), "full")] {
                for input in inputs {
                    let mut menu = MenuState::new();
                    menu.open_for_focused(&layout, target_a(), rows, columns)
                        .expect("open");
                    let response = menu.handle(input);
                    assert_ne!(response, MenuResponse::NotOpen, "{label} {input:?}");
                    assert_eq!(
                        menu.is_open(),
                        response == MenuResponse::Consumed,
                        "{label} {input:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn closed_menu_returns_not_open_for_every_input() {
        let mut menu = MenuState::new();
        for input in [MenuInput::Activate, MenuInput::Cancel, MenuInput::Other] {
            assert_eq!(menu.handle(input), MenuResponse::NotOpen);
        }
        let mut menu = open_keyboard(&single(20, 81));
        assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::Closed);
        assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::NotOpen);
    }

    #[test]
    fn activating_a_disabled_item_stays_open_and_shows_its_reason() {
        let mut menu = open_keyboard(&single(10, 41));
        assert_eq!(menu.handle(MenuInput::Letter('U')), MenuResponse::Consumed);
        assert!(menu.is_open());
        assert_eq!(menu.selected(), Some(SplitDirection::Up));
        let lines = menu.overlay_lines();
        let status = lines.as_slice()[7];
        assert_eq!(status.style(), OverlayStyle::Status);
        assert!(status
            .text()
            .contains("Split Up needs 11 rows; this pane has 10"));
        assert_eq!(menu.handle(MenuInput::Activate), MenuResponse::Consumed);
        assert!(menu.is_open());
    }

    #[test]
    fn navigation_wraps_and_letter_shortcuts_are_case_insensitive() {
        let mut menu = open_keyboard(&single(20, 81));
        // Up from the first item wraps to the last row, recovery.
        assert_eq!(menu.handle(MenuInput::Up), MenuResponse::Consumed);
        assert!(menu.recovery_selected());
        assert_eq!(menu.selected(), None);
        assert_eq!(menu.handle(MenuInput::Up), MenuResponse::Consumed);
        assert!(menu.close_selected());
        assert_eq!(menu.handle(MenuInput::Down), MenuResponse::Consumed);
        assert!(menu.recovery_selected());
        assert_eq!(menu.handle(MenuInput::Down), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Left));
        assert_eq!(menu.handle(MenuInput::End), MenuResponse::Consumed);
        assert!(menu.recovery_selected());
        assert_eq!(menu.handle(MenuInput::Home), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Left));
        assert_eq!(menu.handle(MenuInput::Letter('q')), MenuResponse::Consumed);

        for (letter, direction) in [
            ('l', SplitDirection::Left),
            ('L', SplitDirection::Left),
            ('r', SplitDirection::Right),
            ('R', SplitDirection::Right),
            ('u', SplitDirection::Up),
            ('U', SplitDirection::Up),
            ('d', SplitDirection::Down),
            ('D', SplitDirection::Down),
        ] {
            let mut menu = open_keyboard(&single(20, 81));
            match menu.handle(MenuInput::Letter(letter)) {
                MenuResponse::Activated(intent) => assert_eq!(intent.direction, direction),
                other => panic!("{letter}: {other:?}"),
            }
        }
        let mut menu = open_keyboard(&single(20, 81));
        assert_eq!(
            menu.handle(MenuInput::Letter('v')),
            MenuResponse::RecoveryUnavailable
        );
        assert!(menu.is_open());
        assert_eq!(
            menu.recovery_availability(),
            Some(Availability::Disabled(
                DisabledReason::NoPreviousLayoutSaved
            ))
        );
    }

    #[test]
    fn recovery_row_is_after_close_and_never_closes_a_pane() {
        assert_eq!(ROW_COUNT, 6);
        let layout = single(20, 81);
        let mut enabled = MenuState::new();
        enabled
            .open_for_focused_with_recovery(&layout, target_a(), 20, 81, Availability::Enabled)
            .expect("open");
        let lines = enabled.overlay_lines();
        assert!(lines.as_slice()[5].text().contains("Close pane"));
        assert!(lines.as_slice()[6].text().contains("View saved layout [V]"));
        assert_eq!(
            enabled.handle(MenuInput::Letter('v')),
            MenuResponse::RecoveryRequested
        );
        assert!(!enabled.is_open());

        let mut disabled = MenuState::new();
        disabled
            .open_for_focused_with_recovery(
                &layout,
                target_a(),
                20,
                81,
                Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
            )
            .expect("open");
        let lines = disabled.overlay_lines();
        assert_eq!(lines.as_slice()[6].style(), OverlayStyle::Disabled);
        assert!(lines.as_slice()[6]
            .text()
            .contains("No previous layout saved"));
        assert_eq!(
            disabled.handle(MenuInput::Letter('v')),
            MenuResponse::RecoveryUnavailable
        );
        assert!(disabled.is_open());
        assert!(!disabled.close_selected());
    }

    #[test]
    fn mouse_press_on_item_activates_and_outside_closes() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 10, 2, 20, 81).expect("open");
        let rect = menu.overlay_rect().expect("rect");
        assert_eq!((rect.x, rect.y), (10, 2));
        // Border/title row is chrome.
        let border = MenuInput::MousePress {
            x: rect.x + 3,
            y: rect.y,
            right: false,
        };
        assert_eq!(menu.handle(border), MenuResponse::Consumed);
        let release = MenuInput::MouseRelease {
            x: rect.x + 3,
            y: rect.y + 3,
            right: true,
        };
        assert_eq!(menu.handle(release), MenuResponse::Consumed);
        let up_row = MenuInput::MousePress {
            x: rect.x + 3,
            y: rect.y + 3,
            right: false,
        };
        assert_eq!(
            menu.handle(up_row),
            MenuResponse::Activated(SplitIntent {
                target: target_a(),
                expected_layout_revision: 1,
                direction: SplitDirection::Up,
            })
        );

        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 10, 2, 20, 81).expect("open");
        let outside = MenuInput::MousePress {
            x: 0,
            y: 0,
            right: true,
        };
        assert_eq!(menu.handle(outside), MenuResponse::Closed);
        assert!(!menu.is_open());
    }

    #[test]
    fn normal_overlay_is_clamped_inside_viewport_at_all_four_corners() {
        let layout = single(30, 100);
        let (rows, columns) = (30, 100);
        for (x, y, expected_x, expected_y) in [
            (0, 0, 0, 0),
            (99, 0, 100 - NORMAL_COLUMNS, 0),
            (0, 29, 0, 30 - NORMAL_ROWS),
            (99, 29, 100 - NORMAL_COLUMNS, 30 - NORMAL_ROWS),
        ] {
            let mut menu = MenuState::new();
            menu.open_at_pointer(&layout, x, y, rows, columns)
                .expect("open");
            let rect = menu.overlay_rect().expect("rect");
            assert_eq!(menu.mode(), Some(OverlayMode::Normal));
            assert_eq!((rect.x, rect.y), (expected_x, expected_y));
            assert_eq!((rect.rows, rect.columns), (NORMAL_ROWS, NORMAL_COLUMNS));
            assert_inside(rect, rows, columns);
            assert_line_invariants(&menu);
            let lines = menu.overlay_lines();
            let styles = lines.as_slice().iter().map(|line| line.style());
            assert!(styles.eq([
                OverlayStyle::Border,
                OverlayStyle::Selected,
                OverlayStyle::Normal,
                OverlayStyle::Normal,
                OverlayStyle::Normal,
                OverlayStyle::Normal,
                OverlayStyle::Disabled,
                OverlayStyle::Status,
                OverlayStyle::Border,
            ]));
            assert!(lines.as_slice()[0].text().starts_with("+- Pane -"));
            assert!(lines.as_slice()[1].text().starts_with("| > Split Left [L]"));
            let close_row = lines.as_slice()[5].text();
            assert_eq!(close_row.trim_end_matches(['|', ' ']), "|   Close pane");
            assert!(lines.as_slice()[6]
                .text()
                .contains("No previous layout saved"));
        }
    }

    #[test]
    fn compact_overlay_uses_six_item_rows_at_6x20_and_adds_status_at_7_rows() {
        let layout = single(6, 20);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 19, 5, 6, 20).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Compact));
        assert_eq!(
            menu.overlay_rect(),
            Some(Rect {
                x: 0,
                y: 0,
                rows: 6,
                columns: 20
            })
        );
        assert_line_invariants(&menu);
        let lines = menu.overlay_lines();
        let texts: Vec<&str> = lines.as_slice().iter().map(|line| line.text()).collect();
        assert_eq!(texts[0], "> Split Left [L] ...");
        assert_eq!(texts[1], "  Split Right [R]...");
        assert_eq!(texts[2], "  Split Up [U] - ...");
        assert_eq!(texts[3], "  Split Down [D] ...");
        assert_eq!(texts[4], "  Close pane        ");
        assert_eq!(texts[5], "  No previous lay...");
        assert_eq!(lines.as_slice()[1].style(), OverlayStyle::Disabled);
        assert_eq!(lines.as_slice()[4].style(), OverlayStyle::Normal);
        assert_eq!(lines.as_slice()[5].style(), OverlayStyle::Disabled);

        // One more row adds the status row.
        let mut tall = MenuState::new();
        tall.open_at_pointer(&single(7, 20), 0, 0, 7, 20)
            .expect("open");
        assert_eq!(tall.mode(), Some(OverlayMode::Compact));
        assert_eq!(
            tall.overlay_rect().map(|rect| rect.rows),
            Some(COMPACT_ROWS)
        );
        assert_line_invariants(&tall);
        let lines = tall.overlay_lines();
        assert_eq!(lines.as_slice()[6].text(), "Split Left needs ...");
        assert_eq!(lines.as_slice()[6].style(), OverlayStyle::Status);

        // Clicking the second compact row selects Right and stays open (disabled).
        let press = MenuInput::MousePress {
            x: 3,
            y: 1,
            right: false,
        };
        assert_eq!(menu.handle(press), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Right));
        // The status row (7-row viewport) is chrome.
        let status = MenuInput::MousePress {
            x: 3,
            y: 6,
            right: false,
        };
        assert_eq!(tall.handle(status), MenuResponse::Consumed);
        assert!(tall.is_open());
        assert!(menu.is_open());
    }

    #[test]
    fn compact_overlay_is_clamped_vertically_in_short_wide_viewport() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 70, 5, 7, 81).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Compact));
        let rect = menu.overlay_rect().expect("rect");
        assert_eq!((rect.x, rect.y, rect.rows, rect.columns), (0, 0, 7, 81));
        assert_inside(rect, 7, 81);
        assert_line_invariants(&menu);
    }

    #[test]
    fn too_small_viewport_shows_one_status_line_and_only_cancel_closes() {
        for (rows, columns) in [(4, 81), (20, MIN_MENU_COLUMNS - 1), (1, 1)] {
            let layout = single(20, 81);
            let mut menu = MenuState::new();
            menu.open_at_pointer(&layout, 0, 0, rows, columns)
                .expect("open");
            assert_eq!(menu.mode(), Some(OverlayMode::TooSmall));
            let rect = menu.overlay_rect().expect("rect");
            assert_eq!(rect.rows, 1);
            assert_inside(rect, rows, columns);
            assert_line_invariants(&menu);
            let lines = menu.overlay_lines();
            assert_eq!(lines.as_slice()[0].style(), OverlayStyle::Status);
            for input in [
                MenuInput::Activate,
                MenuInput::Letter('r'),
                MenuInput::MousePress {
                    x: 0,
                    y: 0,
                    right: false,
                },
                MenuInput::MousePress {
                    x: 0,
                    y: 3,
                    right: false,
                },
            ] {
                assert_eq!(menu.handle(input), MenuResponse::Consumed);
            }
            assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::Closed);
        }

        let mut menu = MenuState::new();
        menu.open_at_pointer(&single(20, 81), 0, 0, 0, 0)
            .expect("open");
        assert!(menu.overlay_lines().is_empty());
        assert_eq!(menu.handle(MenuInput::Activate), MenuResponse::Consumed);
        assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::Closed);
    }

    #[test]
    fn below_terminal_minimum_viewport_never_activates() {
        assert_eq!((MIN_MENU_ROWS, MIN_MENU_COLUMNS), (6, 20));
        let layout = single(24, 81);
        let blocked = [
            MenuInput::Letter('r'),
            MenuInput::Letter('L'),
            MenuInput::Activate,
            MenuInput::Up,
            MenuInput::Other,
            MenuInput::MousePress {
                x: 0,
                y: 0,
                right: false,
            },
            MenuInput::MouseRelease {
                x: 0,
                y: 0,
                right: false,
            },
        ];
        for (rows, columns) in [(6, 19), (5, 40)] {
            let mut menu = MenuState::new();
            menu.open_at_pointer(&layout, 0, 0, rows, columns)
                .expect("open");
            assert_eq!(menu.mode(), Some(OverlayMode::TooSmall), "{rows}x{columns}");
            assert_eq!(menu.overlay_lines().len(), 1);
            for input in blocked {
                assert_eq!(menu.handle(input), MenuResponse::Consumed, "{input:?}");
            }
            assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::Closed);
            assert_eq!(menu.handle(MenuInput::Cancel), MenuResponse::NotOpen);
        }

        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 0, 0, 6, 20).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Compact));
        assert_eq!(
            menu.handle(MenuInput::Letter('r')),
            MenuResponse::Activated(SplitIntent {
                target: target_a(),
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
            })
        );
    }

    #[test]
    fn shrinking_below_minimum_blocks_activation_until_enlarged() {
        let mut layout = single(24, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 3, 3, 24, 81).expect("open");
        assert_eq!(menu.handle(MenuInput::Down), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Right));
        // The layout advances meanwhile; captures must not follow it.
        let preview = layout
            .preview_split(target_a(), 1, SplitDirection::Down, target_b())
            .expect("preview");
        layout.commit_split(preview).expect("commit");

        menu.set_viewport(5, 19);
        assert_eq!(menu.mode(), Some(OverlayMode::TooSmall));
        for input in [MenuInput::Activate, MenuInput::Letter('d')] {
            assert_eq!(menu.handle(input), MenuResponse::Consumed);
        }
        assert_eq!(menu.selected(), Some(SplitDirection::Right));

        menu.set_viewport(24, 81);
        assert_eq!(menu.mode(), Some(OverlayMode::Normal));
        assert_eq!(menu.selected(), Some(SplitDirection::Right));
        assert_eq!(
            menu.handle(MenuInput::Activate),
            MenuResponse::Activated(SplitIntent {
                target: target_a(),
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
            })
        );
    }

    #[test]
    fn set_viewport_replaces_overlay_and_keeps_captures() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 70, 15, 20, 81).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Normal));
        menu.set_viewport(3, 81);
        assert_eq!(menu.mode(), Some(OverlayMode::TooSmall));
        menu.set_viewport(20, 81);
        assert_eq!(menu.mode(), Some(OverlayMode::Normal));
        assert_eq!(menu.captured_target(), Some(target_a()));
        assert_eq!(menu.captured_revision(), Some(1));
    }

    #[test]
    fn status_text_is_clipped_with_ellipsis_never_wrapped() {
        let reason = DisabledReason::TooNarrow {
            direction: SplitDirection::Right,
            needed: 41,
            actual: 38,
        };
        for width in 0..60 {
            let mut buf = [b' '; 60];
            write_clipped(&mut buf[..width], &reason);
            let full = reason.to_string();
            let text = std::str::from_utf8(&buf[..width]).expect("ascii");
            if width >= full.len() {
                assert!(text.starts_with(&full));
            } else {
                assert!(text.ends_with(&"..."[..width.min(3)]), "{width}: {text}");
                if width > 3 {
                    assert_eq!(&text[..width - 3], &full[..width - 3]);
                }
            }
            assert!(!text.contains('\n'));
        }
        let mut buf = [b' '; 8];
        write_clipped(&mut buf, &"a\u{1b}[\u{e9}\nb");
        assert_eq!(&buf, b"a?[??b  ");
    }

    #[test]
    fn close_item_is_always_enabled_and_requests_confirmation_with_captures() {
        // Close stays enabled for every live pane, even when splits are feasible.
        let layout = split_right(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 60, 3, 20, 81)
            .expect("pane B");
        // No letter reaches the close row; `x` is consumed, menu stays.
        assert_eq!(menu.handle(MenuInput::Letter('x')), MenuResponse::Consumed);
        assert_eq!(menu.handle(MenuInput::Letter('y')), MenuResponse::Consumed);
        assert_eq!(menu.handle(MenuInput::End), MenuResponse::Consumed);
        assert_eq!(menu.handle(MenuInput::Up), MenuResponse::Consumed);
        assert!(menu.close_selected());
        let status = menu.overlay_lines().as_slice()[7];
        assert!(status.text().contains("close this pane"), "{status:?}");
        assert_eq!(
            menu.handle(MenuInput::Activate),
            MenuResponse::CloseRequested(CloseIntent {
                target: target_b(),
                expected_layout_revision: 2,
                position: 2,
                last_pane: false,
            })
        );
        assert!(!menu.is_open());

        // A single pane is position 1 and the last pane. End now selects the
        // recovery row; Up from there reaches Close.
        let mut menu = open_keyboard(&single(20, 81));
        assert_eq!(menu.handle(MenuInput::End), MenuResponse::Consumed);
        assert!(menu.recovery_selected());
        assert_eq!(menu.handle(MenuInput::Up), MenuResponse::Consumed);
        assert!(menu.close_selected());
        assert_eq!(
            menu.handle(MenuInput::Activate),
            MenuResponse::CloseRequested(CloseIntent {
                target: target_a(),
                expected_layout_revision: 1,
                position: 1,
                last_pane: true,
            })
        );
    }

    #[test]
    fn mouse_press_on_close_row_requests_confirmation() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 10, 2, 20, 81).expect("open");
        let rect = menu.overlay_rect().expect("rect");
        let close_row = MenuInput::MousePress {
            x: rect.x + 3,
            y: rect.y + 5,
            right: false,
        };
        assert!(matches!(
            menu.handle(close_row),
            MenuResponse::CloseRequested(intent) if intent.target == target_a()
        ));
        // Compact: the fifth row is Close.
        let mut menu = MenuState::new();
        menu.open_at_pointer(&single(6, 20), 0, 0, 6, 20)
            .expect("open");
        let press = MenuInput::MousePress {
            x: 1,
            y: 4,
            right: false,
        };
        assert!(matches!(
            menu.handle(press),
            MenuResponse::CloseRequested(_)
        ));
    }

    fn intent(last_pane: bool) -> CloseIntent {
        CloseIntent {
            target: target_b(),
            expected_layout_revision: 9,
            position: 2,
            last_pane,
        }
    }

    #[test]
    fn confirm_only_yes_confirms_and_every_other_input_cancels_or_is_consumed() {
        let mut confirm = CloseConfirm::new();
        assert_eq!(confirm.handle(ConfirmInput::Yes), ConfirmResponse::NotOpen);
        assert!(confirm.overlay_line(40).is_none());

        confirm.open(intent(false));
        assert!(confirm.is_open());
        assert_eq!(
            confirm.handle(ConfirmInput::Ignore),
            ConfirmResponse::Consumed
        );
        assert!(confirm.is_open());
        assert_eq!(
            confirm.handle(ConfirmInput::Yes),
            ConfirmResponse::Confirmed(intent(false))
        );
        assert!(!confirm.is_open());
        assert_eq!(confirm.handle(ConfirmInput::Yes), ConfirmResponse::NotOpen);

        confirm.open(intent(false));
        assert_eq!(
            confirm.handle(ConfirmInput::Cancel),
            ConfirmResponse::Cancelled
        );
        assert!(!confirm.is_open());
        assert_eq!(confirm.intent(), None);
    }

    #[test]
    fn confirm_prompt_names_the_pane_and_warns_for_the_last_pane() {
        let mut confirm = CloseConfirm::new();
        confirm.open(intent(false));
        let line = confirm.overlay_line(81).expect("open");
        assert_eq!(line.text().len(), 81);
        assert!(line.text().starts_with(" Close pane 2? (y/n) "));
        assert_eq!(line.style(), OverlayStyle::Selected);
        assert!(confirm.overlay_line(0).is_none());
        assert_eq!(confirm.overlay_line(10).expect("open").text(), " Close ...");
        assert_eq!(
            confirm.overlay_line(500).expect("open").text().len(),
            MAX_OVERLAY_COLUMNS
        );

        confirm.open(intent(true));
        let line = confirm.overlay_line(81).expect("open");
        assert!(line
            .text()
            .starts_with(" Close pane 2? This is the last pane; the workspace ends. (y/n)"));
    }

    #[test]
    fn debug_output_redacts_targets_and_revisions() {
        let mut secret = target_a();
        secret.generation = 9_876_543_210_123;
        secret.workspace = 9_876_543_210_124;
        secret.pane = 9_876_543_210_125;
        secret.session = 9_876_543_210_126;
        secret.occupant_revision = 9_876_543_210_127;
        let layout = PaneLayout::new(secret, 20, 81).expect("layout");
        let mut menu = MenuState::new();
        menu.open_for_focused(&layout, secret, 20, 81)
            .expect("open");
        let intent = SplitIntent {
            target: secret,
            expected_layout_revision: 9_876_543_210_128,
            direction: SplitDirection::Left,
        };
        let close = CloseIntent {
            target: secret,
            expected_layout_revision: 9_876_543_210_128,
            position: 1,
            last_pane: true,
        };
        let mut confirm = CloseConfirm::new();
        confirm.open(close);
        for rendered in [
            format!("{menu:?}"),
            format!("{intent:?}"),
            format!("{close:?}"),
            format!("{confirm:?}"),
            format!("{:?}", MenuResponse::CloseRequested(close)),
            format!("{:?}", ConfirmResponse::Confirmed(close)),
            format!("{:?}", MenuResponse::Activated(intent)),
            format!("{:?}", MenuInput::Letter('z')),
        ] {
            for marker in [
                "9876543210123",
                "9876543210124",
                "9876543210125",
                "9876543210126",
                "9876543210127",
                "9876543210128",
                "'z'",
            ] {
                assert!(!rendered.contains(marker), "debug output leaked {marker}");
            }
        }
        assert!(format!("{menu:?}").contains("[redacted]"));
    }
}
