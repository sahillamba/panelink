//! Pure, terminal-independent model of the four-item split context menu.
//!
//! The menu captures one complete pane target and layout revision from a
//! single [`LayoutSnapshot`] when it opens, owns every input while open, and
//! returns exactly the captured values on activation. It performs no terminal
//! IO, rendering, process launch, or layout mutation. Item availability is
//! advisory presentation derived from [`PaneLayout::can_split`]; the trusted
//! split owner remains authoritative and revalidates on submission.

use crate::control_protocol::TargetRef;
use crate::pane_layout::{
    LayoutError, LayoutSnapshot, PaneGeometry, PaneLayout, Rect, SplitDirection, MIN_PANE_COLUMNS,
    MIN_PANE_ROWS,
};
use std::error::Error;
use std::fmt;

/// Number of menu items; the menu always lists all four split directions.
pub const ITEM_COUNT: usize = 4;
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
pub const PANE_LIMIT: u8 = 2;
/// Height of the normal bordered overlay: title border, four items, status, border.
pub const NORMAL_ROWS: u16 = 7;
/// Preferred width of the normal bordered overlay.
pub const NORMAL_COLUMNS: u16 = 52;
/// Narrowest viewport that still uses the bordered overlay.
pub const MIN_NORMAL_COLUMNS: u16 = 24;
/// Height of the compact overlay: four item rows and one status row.
pub const COMPACT_ROWS: u16 = 5;
/// Shortest viewport in which the menu is interactive: the terminal-core
/// pane minimum. Anything shorter is [`OverlayMode::TooSmall`].
pub const MIN_MENU_ROWS: u16 = MIN_PANE_ROWS;
/// Narrowest viewport in which the menu is interactive: the terminal-core
/// pane minimum. Anything narrower is [`OverlayMode::TooSmall`].
pub const MIN_MENU_COLUMNS: u16 = MIN_PANE_COLUMNS;
/// Maximum number of overlay rows produced by [`MenuState::overlay_lines`].
pub const MAX_OVERLAY_ROWS: usize = 7;
/// Maximum width in columns (and bytes) of one overlay row.
pub const MAX_OVERLAY_COLUMNS: usize = 240;

const TITLE: &str = " Split pane ";
const ENABLED_HINT: &str = "Enter: split  Esc: close";
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
    /// A typed character; `l`, `r`, `u`, `d` (any case) select and activate.
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
    /// Bordered box with a title, four items and a status row.
    Normal,
    /// Borderless, full-width, five rows: four items and a status row.
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

    /// Returns the four items in order Left, Right, Up, Down.
    pub fn items(&self) -> Option<[MenuItem; ITEM_COUNT]> {
        self.open.map(|menu| menu.items)
    }

    /// Returns the direction of the selected item.
    pub fn selected(&self) -> Option<SplitDirection> {
        self.open.map(|menu| menu.items[menu.selected].direction)
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
            MenuResponse::Consumed => Some(menu),
            MenuResponse::NotOpen | MenuResponse::Closed | MenuResponse::Activated(_) => None,
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
                .field("selected", &menu.items[menu.selected].direction)
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
    items: [MenuItem; ITEM_COUNT],
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
    fn new(
        layout: &PaneLayout,
        snapshot: LayoutSnapshot,
        pane: PaneGeometry,
        anchor: (u16, u16),
        viewport: (u16, u16),
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
        Self {
            target: pane.target,
            revision: snapshot.revision,
            items,
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
                self.selected = (self.selected + ITEM_COUNT - 1) % ITEM_COUNT;
                MenuResponse::Consumed
            }
            MenuInput::Down => {
                self.selected = (self.selected + 1) % ITEM_COUNT;
                MenuResponse::Consumed
            }
            MenuInput::Home => {
                self.selected = 0;
                MenuResponse::Consumed
            }
            MenuInput::End => {
                self.selected = ITEM_COUNT - 1;
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
        let item = self.items[self.selected];
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
                if (1..=ITEM_COUNT).contains(&row) && inner {
                    Hit::Item(row - 1)
                } else {
                    Hit::Chrome
                }
            }
            OverlayMode::Compact if row < ITEM_COUNT => Hit::Item(row),
            OverlayMode::Compact | OverlayMode::TooSmall => Hit::Chrome,
        }
    }

    fn item_style(&self, index: usize) -> OverlayStyle {
        if index == self.selected {
            OverlayStyle::Selected
        } else if self.items[index].is_enabled() {
            OverlayStyle::Normal
        } else {
            OverlayStyle::Disabled
        }
    }

    fn status(&self) -> StatusText {
        match self.items[self.selected].availability {
            Availability::Enabled => StatusText(None),
            Availability::Disabled(reason) => StatusText(Some(reason)),
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
                for index in 0..ITEM_COUNT {
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
                for index in 0..ITEM_COUNT {
                    lines.push(OverlayLine::plain(
                        width,
                        self.item_style(index),
                        &self.item_label(index),
                    ));
                }
                lines.push(OverlayLine::plain(
                    width,
                    OverlayStyle::Status,
                    &self.status(),
                ));
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
            item: self.items[index],
            selected: index == self.selected,
        }
    }
}

struct ItemLabel {
    item: MenuItem,
    selected: bool,
}

impl fmt::Display for ItemLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let marker = if self.selected { "> " } else { "  " };
        let suffix = if self.item.is_enabled() {
            ""
        } else {
            " - disabled"
        };
        write!(
            f,
            "{marker}{} [{}]{suffix}",
            direction_label(self.item.direction),
            shortcut(self.item.direction)
        )
    }
}

struct StatusText(Option<DisabledReason>);

impl fmt::Display for StatusText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(reason) => reason.fmt(f),
            None => f.write_str(ENABLED_HINT),
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
    } else if interactive && rows >= COMPACT_ROWS {
        let rect = Rect {
            x: 0,
            y: anchor_y.min(rows - COMPACT_ROWS),
            rows: COMPACT_ROWS,
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
    use crate::pane_layout::{MAX_PANE_COLUMNS, MAX_PANE_ROWS};

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
    fn committed_split_disables_every_item_with_pane_limit_reason() {
        let layout = split_right(40, 81);
        let menu = open_keyboard(&layout);
        let limit = Availability::Disabled(DisabledReason::PaneLimit { limit: 2 });
        assert_eq!(availability(&menu), [limit; ITEM_COUNT]);
        assert_eq!(reason_text(&menu, 0), "Pane limit reached (2)");
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
        let status = lines.as_slice()[5];
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
        assert_eq!(menu.handle(MenuInput::Up), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Down));
        assert_eq!(menu.handle(MenuInput::Down), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Left));
        assert_eq!(menu.handle(MenuInput::End), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Down));
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
                OverlayStyle::Status,
                OverlayStyle::Border,
            ]));
            assert!(lines.as_slice()[0].text().starts_with("+- Split pane -"));
            assert!(lines.as_slice()[1].text().starts_with("| > Split Left [L]"));
        }
    }

    #[test]
    fn compact_overlay_uses_exactly_five_full_width_rows_at_5x20() {
        let layout = single(5, 20);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 19, 4, 5, 20).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Compact));
        assert_eq!(
            menu.overlay_rect(),
            Some(Rect {
                x: 0,
                y: 0,
                rows: 5,
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
        assert_eq!(texts[4], "Split Left needs ...");
        assert_eq!(lines.as_slice()[1].style(), OverlayStyle::Disabled);
        assert_eq!(lines.as_slice()[4].style(), OverlayStyle::Status);

        // Clicking the second compact row selects Right and stays open (disabled).
        let press = MenuInput::MousePress {
            x: 3,
            y: 1,
            right: false,
        };
        assert_eq!(menu.handle(press), MenuResponse::Consumed);
        assert_eq!(menu.selected(), Some(SplitDirection::Right));
        // The status row is chrome.
        let status = MenuInput::MousePress {
            x: 3,
            y: 4,
            right: false,
        };
        assert_eq!(menu.handle(status), MenuResponse::Consumed);
        assert!(menu.is_open());
    }

    #[test]
    fn compact_overlay_is_clamped_vertically_in_short_wide_viewport() {
        let layout = single(20, 81);
        let mut menu = MenuState::new();
        menu.open_at_pointer(&layout, 70, 5, 6, 81).expect("open");
        assert_eq!(menu.mode(), Some(OverlayMode::Compact));
        let rect = menu.overlay_rect().expect("rect");
        assert_eq!((rect.x, rect.y, rect.rows, rect.columns), (0, 1, 5, 81));
        assert_inside(rect, 6, 81);
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
        assert_eq!((MIN_MENU_ROWS, MIN_MENU_COLUMNS), (5, 20));
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
        for (rows, columns) in [(5, 19), (4, 40)] {
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
        menu.open_at_pointer(&layout, 0, 0, 5, 20).expect("open");
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
        for rendered in [
            format!("{menu:?}"),
            format!("{intent:?}"),
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
