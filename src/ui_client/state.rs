//! Pure presentation state for the attach client: layout, local focus, menu,
//! key batching, transient status and frame building.
//!
//! All network effects go through the [`Effects`] trait so the routing rules
//! can be tested with a recording fake. No host IO happens here.

mod viewport;
mod workspace;
use viewport::{ScreenSlot, Viewports};
#[cfg(test)]
use viewport::{ALT_WHEEL_CAPACITY, ALT_WHEEL_TTL};
use workspace::{TabDialog, WorkspaceState};

use std::io;
use std::time::{Duration, Instant};

use crossterm_input::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use panelink::close_protocol::{CloseOutcome, CloseRejection, CloseResult, StopOutcome};
use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{DividerOrientation, PaneLayout, Rect, SplitDirection};
use panelink::pane_operations_protocol::{encode_view, PaneOperationsView};
use panelink::protocol::Outcome;
use panelink::screen_checkpoint::ScreenCheckpoint;
use panelink::split_menu::{
    Availability, CloseConfirm, CloseIntent, ConfirmResponse, MenuResponse, MenuState,
    OverlayStyle as MenuStyle, PaneActionAvailability, PaneActionModel, PaneMenuAction,
    SplitIntent,
};
use panelink::terminal::compose::{
    divider_at, pane_controls, pane_history_controls, pane_version_badge, pointer_content_point,
    scrollbar_geometry, ComposeFrame, Compositor, CopyCaretOverlay, HistoryLabelKind, OverlayLine,
    OverlayStyle, OverlayView, PaneChrome, PaneContent, PaneView, Scrollbar, SelectionOverlay,
};
use panelink::terminal::{Key, ScreenSnapshot, TerminalError};
use panelink::terminal_selection::{Point, SelectionError, ViewIdentity};
use panelink::ui_protocol::{planned_split_size, LayoutReply, WireHistoryCursor, MAX_KEYS};
use panelink::workspace_index::{ProjectId, TabPresentationSnapshot, WorkspaceIndex};
use panelink::workspace_tabs::{TabAction, TabNavigation, TabsLayout, WorkspaceRouteId};

use super::clipboard::ClipboardStatus;
use super::keys::{
    confirm_input, copy_mode_key, map_key, menu_input, mouse_button, mouse_wheel,
    pane_operation_key, workspace_navigation, workspace_tab_command, CopyModeKey, KeyAction,
    PaneOperationKey, WorkspaceTabCommand,
};
#[cfg(test)]
use super::net::ScrollResult;
use super::net::{NetUpdate, ScrollUpdate};
use super::pane_operations::PaneAction;
use super::routes::{MetadataRequestId, MetadataResult};
use super::selection::{capture, reanchor, FrozenDrag};
use crate::workspace_lifecycle::{CreateIntentId, ExpectedRuntimeIdentity};
#[cfg(test)]
use panelink::terminal::TerminalSize;
#[cfg(test)]
use panelink::workspace_index::MAX_DISPLAY_NAME_BYTES;

/// Largest root the client requests (terminal-core and compositor bound).
pub(crate) const MAX_ROOT_ROWS: u16 = 80;
/// Largest root the client requests.
pub(crate) const MAX_ROOT_COLUMNS: u16 = 240;
const MIN_ROOT_ROWS: u16 = panelink::pane_layout::MIN_PANE_ROWS;
const MIN_ROOT_COLUMNS: u16 = panelink::pane_layout::MIN_PANE_COLUMNS;
const STATUS_TTL: Duration = Duration::from_secs(3);
const COPY_FEEDBACK_TTL: Duration = Duration::from_millis(1_500);
/// Navigation mode ends by itself after this long without a key.
const NAV_TIMEOUT: Duration = Duration::from_secs(4);
/// Bottom banner shown while navigation mode (after Ctrl-B) is active.
pub(crate) const NAV_BANNER: &str = " NAV  ? help | s menu | c copy | z zoom | m swap | e divider | v saved | l r u d split | arrows/o focus | x close | ^X detach | Esc ";
const WORKSPACE_NAV_BANNER: &str = " NAV  ? help | c copy | z zoom | m swap | e divider | w workspaces | n/p tabs | [/] projects | s menu | v saved | l r u d split | arrows/o focus | x close | ^X detach | Esc ";
const TOO_SMALL: &str = "Panelink: terminal too small for the pane layout; enlarge it";
const EMPTY_MESSAGE: &str = "No open tabs.";
const EMPTY_ACTIONS: &str = "[ New tab (N) ]   [ Detach (Ctrl-X) ]";
const TAB_FILTER_MAX_BYTES: usize = 64;
const HELP_LINES: [&str; 8] = [
    "Panelink help [Close]",
    "Ctrl-B ?: help | s: menu | c: copy | z: zoom | m: swap | e: divider",
    "Ctrl-B l/r/u/d: split | arrows/Tab/o: focus | PgUp/PgDn: history",
    "Ctrl-B x: close pane (confirm y)",
    "Click [Menu] or [x] on a pane; drag its divider to resize",
    "Click/drag the normal-history scrollbar; right-click opens split menu",
    "Esc: leave navigation or close help | Ctrl-X: detach",
    "Press Esc or ? to close help; run Panelink again to reconnect",
];
const WORKSPACE_HELP_LINES: [&str; 8] = [
    "Panelink help [Close]",
    "Ctrl-B ?: help | s: menu | c: copy | z: zoom | m: swap | e: divider",
    "Ctrl-B l/r/u/d: split | arrows/Tab/o: focus | PgUp/PgDn: history",
    "Ctrl-B n/p tabs | [/] projects | w searchable All tabs",
    "Click tab to switch; active/right-click tab for Rename/Close",
    "[+] / Ctrl-B + new | t actions | All: type, Backspace, Enter, [Clear]",
    "Rename Enter saves/Esc cancels | Close ends processes; Cancel default",
    "Click [Menu]/[x], drag divider/scrollbar | Ctrl-B x pane | Ctrl-X detach",
];

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceMetadataMutation {
    Rename {
        route: WorkspaceRouteId,
        name: String,
        expected_revision: u64,
    },
    Close {
        route: WorkspaceRouteId,
        expected: ExpectedRuntimeIdentity,
        expected_revision: u64,
    },
}

fn history_label_kind(slot: &ScreenSlot) -> Option<HistoryLabelKind> {
    (slot.history_offset > 0).then_some(if slot.history_truncated {
        HistoryLabelKind::Truncated
    } else if slot.new_output {
        HistoryLabelKind::NewOutput
    } else {
        HistoryLabelKind::History
    })
}

fn pointer_seek_offset(drag: PointerSeek, y: u16) -> u16 {
    let travel = drag.track.rows.saturating_sub(drag.thumb_rows);
    if travel == 0 {
        return drag.total_rows;
    }
    let top = y
        .saturating_sub(drag.track.y)
        .saturating_sub(drag.grab_offset)
        .min(travel);
    let older_rows = u64::from(top) * u64::from(drag.total_rows) / u64::from(travel);
    drag.total_rows.saturating_sub(older_rows as u16)
}

/// One fully planned split built from a captured menu intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SplitPlan {
    /// Target captured when the menu opened.
    pub target: TargetRef,
    /// Server layout revision captured when the menu opened.
    pub expected_layout_revision: u64,
    /// Chosen direction.
    pub direction: SplitDirection,
    /// Planned new-pane rows.
    pub rows: u16,
    /// Planned new-pane columns.
    pub columns: u16,
}

/// One confirmed explicit pane close, built from a captured intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClosePlan {
    /// Target captured when the menu or prompt opened.
    pub target: TargetRef,
    /// Server layout revision captured together with `target`.
    pub expected_layout_revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConfirmUiGeometry {
    overlay: Rect,
    close: Rect,
    cancel: Rect,
    position: u8,
    consequence: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EmptyWorkspaceGeometry {
    overlay: Rect,
    new_tab: Rect,
    detach: Rect,
}

/// Network effects requested by input routing.
pub(crate) trait Effects {
    /// Deliver typed keys to one pane (schema-8 KeyInput).
    fn send_keys(&mut self, target: TargetRef, keys: &[Key]) -> Result<(), &'static str>;
    /// Deliver one bracketed paste to one pane (control PASTE).
    fn send_paste(&mut self, target: TargetRef, text: &str) -> Result<(), &'static str>;
    /// Submit one bounded wheel action against the captured pane and view epoch.
    fn scroll(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
        column: u16,
        row: u16,
        up: bool,
    ) -> Result<(), &'static str>;
    /// Deliver fresh child input with authoritative server-side mode checks.
    /// This path does not fetch history and must never retry an uncertain send.
    fn alternate_wheel(
        &mut self,
        _target: TargetRef,
        _epoch: u64,
        _column: u16,
        _row: u16,
        _up: bool,
    ) -> Result<(), &'static str> {
        Err("alternate wheel unavailable")
    }
    /// Read-only history seek. Unlike wheel scrolling, this must never route
    /// a pointer event into the child when alternate-screen state changes.
    fn seek_history(
        &mut self,
        _target: TargetRef,
        _epoch: u64,
        _offset_rows: u16,
    ) -> Result<(), &'static str> {
        Err("history seeking unavailable")
    }
    /// Submit one split on the connection pinned to `plan.target`; return the
    /// new pane after it is connected and claimed.
    fn split(
        &mut self,
        plan: SplitPlan,
        request_id: u64,
        focus_generation: u64,
    ) -> Result<Option<TargetRef>, &'static str>;
    /// Submit one schema-9 close with a fresh operation key; return the
    /// validated server result. Never retried with another key.
    fn close(
        &mut self,
        plan: ClosePlan,
        request_id: u64,
    ) -> Result<Option<CloseResult>, &'static str>;

    /// Submit one captured absolute pane operation. Older effect adapters
    /// remain source compatible and report the feature as unavailable.
    fn pane_operation(&mut self, _id: u64, _action: PaneAction) -> Result<(), &'static str> {
        Err("pane operations unavailable")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaneMode {
    Swap {
        source: TargetRef,
        revision: u64,
        choices: [Option<TargetRef>; crate::pane_layout::MAX_PANES],
        selected: usize,
    },
    Divider {
        target: TargetRef,
        revision: u64,
        indices: [Option<u8>; crate::pane_layout::MAX_PANES - 1],
        selected: usize,
        first_cells: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActionsMenu {
    target: TargetRef,
    revision: u64,
    selected: usize,
    model: PaneActionModel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActionsMenuPage {
    rect: Rect,
    start: usize,
    end: usize,
    action_row: usize,
    reason_row: usize,
    navigation_row: usize,
    cancel_row: usize,
    line_count: usize,
    back_column: Option<u16>,
    more_column: Option<u16>,
    cancel_column: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PointerSeek {
    target: TargetRef,
    layout_revision: u64,
    pane_rect: Rect,
    track: Rect,
    thumb_rows: u16,
    total_rows: u16,
    start_offset: u16,
    preview_offset: u16,
    grab_offset: u16,
    last_y: u16,
    awaiting_extent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PointerDivider {
    target: TargetRef,
    revision: u64,
    divider_index: u8,
    initial_span: u16,
    region: Rect,
    orientation: DividerOrientation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalAction {
    Split(SplitDirection),
    Zoom,
    Swap,
    Divider,
    Copy,
    SavedLayout,
    Help,
    Close,
    Detach,
}

const LOCAL_ACTIONS: [LocalAction; panelink::split_menu::PANE_ACTION_COUNT] = [
    LocalAction::Split(SplitDirection::Left),
    LocalAction::Split(SplitDirection::Right),
    LocalAction::Split(SplitDirection::Up),
    LocalAction::Split(SplitDirection::Down),
    LocalAction::Divider,
    LocalAction::Copy,
    LocalAction::Close,
    LocalAction::Zoom,
    LocalAction::Swap,
    LocalAction::SavedLayout,
    LocalAction::Help,
    LocalAction::Detach,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusMove {
    Left,
    Right,
    Up,
    Down,
    Next,
}

/// Result of one routed host event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flow {
    Continue,
    ViewRecovery,
    Quit,
}

/// Client presentation state; PTYs and sessions stay server-owned.
pub(crate) struct App {
    layout: Option<LayoutReply>,
    pane_operations: Option<PaneOperationsView>,
    local: Option<PaneLayout>,
    pane_mode: Option<PaneMode>,
    pending_pane_operation: Option<(u64, PaneAction)>,
    focus: Option<TargetRef>,
    menu: MenuState,
    menu_revision: u64,
    /// Explicit y/n prompt before any close request is sent.
    confirm: CloseConfirm,
    /// Pane-close buttons default to Cancel.
    confirm_close_selected: bool,
    /// One-shot keyboard that opened the prompt; its repeat/release stays local.
    confirm_open_key: Option<(KeyCode, KeyModifiers)>,
    actions_menu: Option<ActionsMenu>,
    pointer_seek: Option<PointerSeek>,
    /// A modal menu closed by a mouse press; consume its matching release too.
    menu_release_latch: Option<MouseButton>,
    /// A thumb gesture released before its extent discovery returned. The
    /// discovery reply applies its final pointer row once, then drops it.
    deferred_thumb_release: Option<PointerSeek>,
    pointer_divider: Option<PointerDivider>,
    /// Persistent local help modal, independent of the NAV timeout.
    help_open: bool,
    /// Navigation mode (entered with Ctrl-B): when it started, if active.
    prefix: Option<Instant>,
    pending: [Key; MAX_KEYS],
    pending_len: usize,
    pending_target: Option<TargetRef>,
    pending_blocked: bool,
    pending_split: Option<(u64, u64, TargetRef, Option<WorkspaceRouteId>)>,
    pending_split_focus: Option<(u64, TargetRef, Option<WorkspaceRouteId>)>,
    pending_close: Option<(u64, TargetRef, bool)>,
    next_request_id: u64,
    focus_generation: u64,
    status: Option<(&'static str, Instant)>,
    persistent_status: Option<&'static str>,
    server_present: bool,
    host: (u16, u16),
    screens: Viewports,
    selection: Option<FrozenDrag>,
    copy_mode: bool,
    copy_anchor: Option<Point>,
    copy_exit_key: Option<(KeyCode, KeyModifiers)>,
    pane_mode_exit_key: Option<(KeyCode, KeyModifiers)>,
    pending_copy: Option<String>,
    pending_copy_target: Option<TargetRef>,
    pending_copy_id: Option<u64>,
    next_copy_id: u64,
    latest_copy_attempt: [Option<(TargetRef, u64)>; crate::pane_layout::MAX_PANES],
    copied_feedback: [Option<(TargetRef, Instant)>; crate::pane_layout::MAX_PANES],
    workspace: WorkspaceState,
    preferred_focus: Option<TargetRef>,
    dirty: bool,
}

impl App {
    fn apply_workspace_feedback(&mut self) {
        let feedback = self.workspace.take_feedback();
        self.dirty |= feedback.dirty;
        if feedback.cancel_selection {
            self.cancel_selection();
        }
        if feedback.clear_prefix {
            self.prefix = None;
        }
        if feedback.clear_actions {
            self.actions_menu = None;
        }
        if feedback.clear_applying {
            self.clear_applying_status();
        }
        if let Some((label, now)) = feedback.persistent {
            self.set_persistent_status(label, now);
        }
        if let Some((label, now)) = feedback.status {
            self.set_status(label, now);
        }
        if let Some(now) = feedback.begin_switch {
            self.begin_route_switch(now);
        }
    }

    /// Preallocate both screen caches for the largest supported pane.
    pub(crate) fn new(host_rows: u16, host_columns: u16) -> Result<Self, TerminalError> {
        let screens = Viewports::new()?;
        Ok(Self {
            layout: None,
            pane_operations: None,
            local: None,
            pane_mode: None,
            pending_pane_operation: None,
            focus: None,
            menu: MenuState::new(),
            menu_revision: 0,
            confirm: CloseConfirm::new(),
            confirm_close_selected: false,
            confirm_open_key: None,
            actions_menu: None,
            pointer_seek: None,
            menu_release_latch: None,
            deferred_thumb_release: None,
            pointer_divider: None,
            help_open: false,
            prefix: None,
            pending: [Key::Enter; MAX_KEYS],
            pending_len: 0,
            pending_target: None,
            pending_blocked: false,
            pending_split: None,
            pending_split_focus: None,
            pending_close: None,
            next_request_id: 1,
            focus_generation: 0,
            status: None,
            persistent_status: None,
            server_present: true,
            host: (host_rows, host_columns),
            screens,
            selection: None,
            copy_mode: false,
            copy_anchor: None,
            copy_exit_key: None,
            pane_mode_exit_key: None,
            pending_copy: None,
            pending_copy_target: None,
            pending_copy_id: None,
            next_copy_id: 1,
            latest_copy_attempt: [None; crate::pane_layout::MAX_PANES],
            copied_feedback: [None; crate::pane_layout::MAX_PANES],
            workspace: WorkspaceState::default(),
            preferred_focus: None,
            dirty: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn layout(&self) -> Option<&LayoutReply> {
        self.layout.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn focus(&self) -> Option<TargetRef> {
        self.focus
    }

    /// Enable the route strip from an immutable metadata snapshot. The App
    /// keeps only the bounded index model and never opens routes or storage.
    pub(crate) fn install_workspace_tabs(
        &mut self,
        snapshot: TabPresentationSnapshot,
        current: Option<WorkspaceRouteId>,
    ) -> bool {
        let result = self.workspace.install_workspace_tabs(snapshot, current);
        self.apply_workspace_feedback();
        result
    }

    /// Compatibility helper for route callers that still hold the v1 catalog.
    pub(crate) fn install_workspace_index(
        &mut self,
        index: WorkspaceIndex,
        current: WorkspaceRouteId,
    ) {
        self.workspace.install_workspace_index(index, current);
        self.apply_workspace_feedback();
    }

    /// Build a client-only state for a validated snapshot with no visible
    /// routes. It allocates presentation caches only; no server connection or
    /// route-start effect is created here.
    pub(crate) fn new_workspace_empty(
        host_rows: u16,
        host_columns: u16,
        snapshot: TabPresentationSnapshot,
    ) -> Result<Self, TerminalError> {
        let mut app = Self::new(host_rows, host_columns)?;
        assert!(
            app.install_workspace_tabs(snapshot, None),
            "empty workspace constructor requires an empty presentation snapshot"
        );
        Ok(app)
    }

    pub(crate) fn current_route(&self) -> Option<WorkspaceRouteId> {
        self.workspace.view().current_route
    }

    pub(crate) fn focused_target(&self) -> Option<TargetRef> {
        self.focus
    }

    /// Restore one exact previously focused target after its route is prepared.
    /// The preference is consumed by `set_layout` only if that occupant remains.
    pub(crate) fn prefer_focus_when_valid(&mut self, target: TargetRef) {
        if let Some(layout) = self.layout {
            if layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
            {
                self.focus_target(target);
            } else {
                self.preferred_focus = None;
            }
        } else {
            self.preferred_focus = Some(target);
        }
    }

    pub(crate) fn set_workspace_revision(&mut self, revision: u64) {
        self.workspace.set_workspace_revision(revision);
        self.apply_workspace_feedback();
    }

    /// Record the expected route and revision for one future rename/close
    /// result. The UI must call this only after the metadata worker admits it.
    pub(crate) fn track_metadata_request(
        &mut self,
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        expected_revision: u64,
    ) {
        self.workspace
            .track_metadata_request(request_id, route, expected_revision);
        self.apply_workspace_feedback();
    }

    pub(crate) fn take_workspace_metadata_mutation(&mut self) -> Option<WorkspaceMetadataMutation> {
        let result = self.workspace.take_workspace_metadata_mutation();
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn reject_workspace_metadata_mutation(&mut self, label: &'static str, now: Instant) {
        self.workspace
            .reject_workspace_metadata_mutation(label, now);
        self.apply_workspace_feedback();
    }

    /// Accept only a result for the exact admitted request, route and current
    /// catalog revision. A stale result never changes the active presentation.
    #[cfg(test)]
    pub(crate) fn accept_metadata_result(
        &mut self,
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        observed_revision: u64,
    ) -> bool {
        let result = self
            .workspace
            .accept_metadata_result(request_id, route, observed_revision);
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn complete_rename_metadata(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> bool {
        let result = self.workspace.complete_rename_metadata(metadata, now);
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn complete_close_metadata(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> bool {
        let result = self.workspace.complete_close_metadata(metadata, now);
        self.apply_workspace_feedback();
        result
    }

    /// Next create dispatch: the project plus a prior intent to reuse when this
    /// is an explicit reconciliation of an unknown outcome.
    pub(crate) fn take_create_request(&mut self) -> Option<(ProjectId, Option<CreateIntentId>)> {
        let result = self.workspace.take_create_request();
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn record_create_request(
        &mut self,
        request_id: MetadataRequestId,
        intent_id: CreateIntentId,
    ) -> bool {
        let result = self.workspace.record_create_request(request_id, intent_id);
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn reject_create_request(&mut self, label: &'static str, now: Instant) {
        self.workspace.reject_create_request(label, now);
        self.apply_workspace_feedback();
    }

    pub(crate) fn complete_create_result(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> Option<WorkspaceRouteId> {
        let result = self.workspace.complete_create_result(metadata, now);
        self.apply_workspace_feedback();
        result
    }

    /// Replace the immutable catalog without disturbing an in-flight route
    /// request or its input freeze. An invalid active route leaves the last
    /// valid snapshot and chooser untouched.
    #[allow(dead_code)] // Retained for state-level legacy catalog scenarios.
    pub(crate) fn update_workspace_catalog(&mut self, index: WorkspaceIndex) -> bool {
        let result = self.workspace.update_workspace_catalog(index);
        self.apply_workspace_feedback();
        result
    }

    pub(crate) fn update_workspace_presentation(
        &mut self,
        source_route: Option<WorkspaceRouteId>,
        snapshot: TabPresentationSnapshot,
        observed_revision: u64,
    ) -> bool {
        let result =
            self.workspace
                .update_workspace_presentation(source_route, snapshot, observed_revision);
        self.apply_workspace_feedback();
        result
    }

    /// Take the coalesced request to refresh metadata after the chooser opens.
    pub(crate) fn take_catalog_refresh_request(&mut self) -> bool {
        let result = self.workspace.take_catalog_refresh_request();
        self.apply_workspace_feedback();
        result
    }

    #[cfg(test)]
    fn route_exists(&self, route: WorkspaceRouteId) -> bool {
        self.workspace.route_exists(route)
    }

    fn open_workspace_picker(&mut self) {
        self.workspace.open_workspace_picker();
        self.apply_workspace_feedback();
    }

    /// Take the one typed destination selected by the user, if any.
    pub(crate) fn take_route_request(&mut self) -> Option<WorkspaceRouteId> {
        let result = self.workspace.take_route_request();
        self.apply_workspace_feedback();
        result
    }

    /// Whether the host must keep resize and child-bound work local.
    pub(crate) const fn route_switching(&self) -> bool {
        self.workspace.view().route_switching
    }

    pub(crate) const fn pane_mutations_blocked(&self) -> bool {
        !self.server_present || self.pane_mode.is_some() || self.pending_pane_operation.is_some()
    }

    pub(crate) fn tab_close_pending(&self) -> bool {
        self.workspace.tab_close_pending()
    }

    pub(crate) const fn keep_tab_visible_on_server_end(&self) -> bool {
        self.workspace.view().keep_tab_visible_on_server_end
    }

    /// Freeze child effects while the host prepares a fresh route attachment.
    pub(crate) fn begin_route_switch(&mut self, now: Instant) {
        if self.workspace.view().route_switching {
            return;
        }
        self.deferred_thumb_release = None;
        // An admitted server operation is not rolled back, but its stale UI
        // completion must not be applied to the destination route.
        self.pane_mode = None;
        self.pane_mode_exit_key = None;
        self.pointer_seek = None;
        self.pointer_divider = None;
        self.actions_menu = None;
        self.menu.handle(panelink::split_menu::MenuInput::Cancel);
        self.dismiss_close_confirmation();
        self.help_open = false;
        self.pending_pane_operation = None;
        self.pane_operations = None;
        self.prefix = None;
        self.cancel_selection();
        self.pending_copy = None;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        self.latest_copy_attempt = [None; crate::pane_layout::MAX_PANES];
        self.copied_feedback = [None; crate::pane_layout::MAX_PANES];
        self.screens.invalidate_pending();
        let abandoned = self.abandon_pending_keys();
        let notice = if abandoned {
            "workspace switch preparing; queued keys discarded"
        } else {
            "workspace switch preparing"
        };
        self.workspace.freeze(notice);
        self.set_status(notice, now);
        self.dirty = true;
    }

    /// Resume the active App after preparation failure or successful install.
    pub(crate) fn finish_route_switch(&mut self, failed: bool, now: Instant) {
        self.workspace.finish_route_switch(failed, now);
        self.apply_workspace_feedback();
    }

    #[cfg(test)]
    pub(crate) fn menu_open(&self) -> bool {
        self.menu.is_open() || self.actions_menu.is_some()
    }

    pub(crate) fn needs_recovery_availability(&self, event: &Event) -> bool {
        if let Some(menu) = self.actions_menu {
            return match event {
                Event::Key(key)
                    if key.code == KeyCode::Enter && (key.is_press() || key.is_repeat()) =>
                {
                    LOCAL_ACTIONS.get(menu.selected) == Some(&LocalAction::SavedLayout)
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    self.actions_page_geometry(menu).is_some_and(|page| {
                        page.start <= 9
                            && 9 < page.end
                            && mouse.row
                                == page
                                    .rect
                                    .y
                                    .saturating_add(page.action_row as u16)
                                    .saturating_add(u16::from(
                                        self.workspace.view().workspace_index.is_some(),
                                    ))
                                    .saturating_add((9 - page.start) as u16)
                            && mouse.column >= page.rect.x
                            && mouse.column < page.rect.x.saturating_add(page.rect.columns)
                    })
                }
                _ => false,
            };
        }
        if self.copy_mode
            || self.workspace.view().route_switching
            || self.pane_mode.is_some()
            || self.pending_pane_operation.is_some()
            || self.help_open
            || self.workspace.view().picker_open
            || self.workspace.view().tab_dialog.is_some()
            || (self.workspace.view().workspace_index.is_some()
                && matches!(event, Event::Mouse(mouse) if mouse.row == 0))
            || self.confirm.is_open()
            || self.menu.is_open()
        {
            return false;
        }
        match event {
            Event::Mouse(mouse) => {
                self.pane_control_at(mouse.column, mouse.row)
                    .is_some_and(|(_, close)| !close)
                    || matches!(mouse_button(mouse), Some((true, true)))
            }
            Event::Key(key) if self.prefix.is_some() && (key.is_press() || key.is_repeat()) => {
                (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::Char('s' | 'S' | 'v' | 'V'))
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn confirm_open(&self) -> bool {
        self.confirm.is_open()
    }

    #[cfg(test)]
    pub(crate) fn help_open(&self) -> bool {
        self.help_open
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Menu and overlay viewport: the host clamped to the compositor bound.
    pub(crate) fn viewport(&self) -> (u16, u16) {
        if let Some(layout) = self.tabs_layout() {
            let root = layout.pane_root();
            (root.rows, root.columns)
        } else if self.workspace.view().workspace_index.is_some() {
            let (rows, columns) = self.screen_viewport();
            (rows.saturating_sub(if rows > 0 { 1 } else { 0 }), columns)
        } else {
            self.screen_viewport()
        }
    }

    fn confirm_ui_geometry(&self, rows: u16, columns: u16) -> Option<ConfirmUiGeometry> {
        let intent = self.confirm.intent()?;
        if rows == 0 || columns == 0 {
            return None;
        }
        let width = columns.min(48);
        let left = columns.saturating_sub(width) / 2;
        let overlay_rows = rows.min(2);
        let top = rows.saturating_sub(overlay_rows);
        let button_y = top.saturating_add(overlay_rows.saturating_sub(1));
        let header_offset = u16::from(self.workspace.view().workspace_index.is_some());
        Some(ConfirmUiGeometry {
            overlay: Rect {
                x: left,
                y: top,
                rows: overlay_rows,
                columns: width,
            },
            close: Rect {
                x: left,
                y: button_y.saturating_add(header_offset),
                rows: 1,
                columns: 7,
            },
            cancel: Rect {
                x: left + 9,
                y: button_y.saturating_add(header_offset),
                rows: 1,
                columns: 8,
            },
            position: intent.position,
            consequence: if intent.last_pane {
                "Ends the workspace"
            } else {
                "Ends this process"
            },
        })
    }

    fn screen_viewport(&self) -> (u16, u16) {
        (
            self.host.0.min(MAX_ROOT_ROWS),
            self.host.1.min(MAX_ROOT_COLUMNS),
        )
    }

    fn tabs_layout(&self) -> Option<TabsLayout> {
        self.workspace.tabs_layout(self.screen_viewport())
    }

    fn is_empty_workspace(&self) -> bool {
        self.workspace.is_empty_workspace()
    }

    fn empty_action_geometry(&self) -> Option<EmptyWorkspaceGeometry> {
        self.workspace.empty_action_geometry(self.viewport())
    }

    fn request_empty_create(&mut self, now: Instant) {
        self.workspace.request_empty_create(now);
        self.apply_workspace_feedback();
    }

    fn request_tab_create(&mut self, project: ProjectId, now: Instant) {
        self.workspace.request_tab_create(project, now);
        self.apply_workspace_feedback();
    }

    fn open_tab_actions(&mut self, route: WorkspaceRouteId) {
        self.workspace.open_tab_actions(route);
        self.apply_workspace_feedback();
    }

    #[cfg(test)]
    fn begin_tab_close(&mut self, route: WorkspaceRouteId) {
        let generation = self.layout.and_then(|layout| {
            layout
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| pane.target.generation)
        });
        self.workspace.begin_tab_close(route, generation);
        self.apply_workspace_feedback();
    }

    /// Empty clients have no Net worker. Keep their complete input path local
    /// so it cannot accidentally target the legacy route or forward input.
    pub(crate) fn handle_empty_workspace_event(
        &mut self,
        event: &Event,
        now: Instant,
    ) -> Option<Flow> {
        if !self.is_empty_workspace() {
            return None;
        }
        if matches!(event, Event::Mouse(mouse) if mouse.row == 0) {
            self.handle_header_event(event, now);
            return Some(Flow::Continue);
        }
        match event {
            Event::Key(key) if (key.is_press() || key.is_repeat()) => {
                if map_key(*key) == KeyAction::Quit {
                    return Some(Flow::Quit);
                }
                let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                if plain && matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Enter) {
                    self.request_empty_create(now);
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some(geometry) = self.empty_action_geometry() {
                    if geometry.new_tab.contains(mouse.column, mouse.row) {
                        self.request_empty_create(now);
                    } else if geometry.detach.contains(mouse.column, mouse.row) {
                        return Some(Flow::Quit);
                    }
                }
            }
            Event::Resize(_, _) => self.dirty = true,
            _ => {}
        }
        let _ = now;
        Some(Flow::Continue)
    }

    fn request_route(&mut self, route: WorkspaceRouteId, now: Instant) {
        self.workspace.request_route(route, now);
        self.apply_workspace_feedback();
    }

    fn navigate_workspace(&mut self, action: TabNavigation, now: Instant) {
        self.workspace.navigate_workspace(action, now);
        self.apply_workspace_feedback();
    }

    /// Root size to request from the server, or `None` below the minimum.
    pub(crate) fn desired_root(&self) -> Option<(u16, u16)> {
        let (rows, columns) = self.viewport();
        (rows >= MIN_ROOT_ROWS && columns >= MIN_ROOT_COLUMNS).then_some((rows, columns))
    }

    pub(crate) fn set_host(&mut self, rows: u16, columns: u16) {
        if self.host != (rows, columns) {
            self.pane_mode = None;
            self.pane_mode_exit_key = None;
            self.actions_menu = None;
            self.menu.close();
            self.cancel_pointer_seek();
            self.pointer_divider = None;
            self.cancel_selection();
            self.screens.invalidate_pending();
            self.host = (rows, columns);
            let (rows, columns) = self.viewport();
            self.menu.set_viewport(rows, columns);
            self.dirty = true;
        }
    }

    fn fits(&self) -> bool {
        let (rows, columns) = self.viewport();
        self.layout
            .is_some_and(|layout| layout.rows <= rows && layout.columns <= columns)
    }

    /// Renderer and pointer geometry are derived from, but never written
    /// back to, the authoritative full layout.
    fn display_layout(&self) -> Option<LayoutReply> {
        let mut display = self.layout?;
        if let Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected,
            first_cells,
        }) = self.pane_mode
        {
            if let Some(index) = indices.get(selected).copied().flatten() {
                if let Some(mut preview) = local_layout(&display) {
                    if let Ok(snapshot) = preview.set_divider(target, revision, index, first_cells)
                    {
                        for pane in display.panes.iter_mut().flatten() {
                            if let Some(geometry) = snapshot
                                .panes
                                .iter()
                                .flatten()
                                .find(|geometry| geometry.target == pane.target)
                            {
                                pane.rect = geometry.rect;
                            }
                        }
                        display.divider = snapshot.divider;
                        display.dividers = snapshot.dividers;
                    }
                }
            }
        }
        if let Some(view) = self.pane_operations {
            if let Some(target) = view.zoomed_target {
                let index = display
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == target))?;
                let pane = display.panes[index].as_mut()?;
                pane.rect = Rect {
                    x: 0,
                    y: 0,
                    rows: display.rows,
                    columns: display.columns,
                };
                let actual = view.confirmed_sizes[index];
                let effective = panelink::pane_layout::content_rect(pane.rect)
                    .map(|rect| (rect.rows, rect.columns));
                pane.pending_confirmed = match (actual, effective) {
                    (Some(actual), Some(effective)) if actual != effective => Some(actual),
                    _ => None,
                };
                for (slot, candidate) in display.panes.iter_mut().enumerate() {
                    if slot != index {
                        *candidate = None;
                    }
                }
                display.divider = None;
                display.dividers = [None; crate::pane_layout::MAX_PANES - 1];
            }
        }
        Some(display)
    }

    /// Adopt one server layout observation. Focus is kept when its pane is
    /// still present, otherwise it moves to the first pane. An open menu
    /// keeps its captured target and revision; the server rejects stale use.
    /// Revision ordering is scoped to one server generation and workspace.
    ///
    /// When a pane leaves the layout (explicit close or its shell exited and
    /// the server collapsed it), batched keys typed for it are dropped (never
    /// redirected to another pane), an open close prompt for it is cancelled,
    /// and its screen cache is released. The compositor repaints fully
    /// because pane rectangles and the divider change.
    pub(crate) fn set_layout(&mut self, reply: LayoutReply) {
        if let Some(current) = self.layout {
            let current_identity = current
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| (pane.target.generation, pane.target.workspace));
            let reply_identity = reply
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| (pane.target.generation, pane.target.workspace));
            if current_identity == reply_identity && reply.revision < current.revision {
                return;
            }
        }
        if self.layout == Some(reply) {
            return;
        }
        let captured_target_present = |target: TargetRef| {
            reply
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        };
        if self.menu.is_open()
            && (self.menu_revision != reply.revision
                || self
                    .menu
                    .captured_target()
                    .is_some_and(|target| !captured_target_present(target)))
        {
            self.menu.handle(panelink::split_menu::MenuInput::Cancel);
            self.set_status("menu cancelled: layout changed", Instant::now());
        }
        if self.confirm.intent().is_some_and(|intent| {
            intent.expected_layout_revision != reply.revision
                || !captured_target_present(intent.target)
        }) {
            self.dismiss_close_confirmation();
            self.set_status("close cancelled: layout changed", Instant::now());
        }
        if self.actions_menu.is_some_and(|menu| {
            menu.revision != reply.revision
                || !reply
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == menu.target)
        }) {
            self.actions_menu = None;
            self.set_status("menu cancelled: layout changed", Instant::now());
        }
        if self.pointer_seek.is_some_and(|drag| {
            drag.layout_revision != reply.revision
                || self.layout.is_some_and(|layout| {
                    layout.rows != reply.rows || layout.columns != reply.columns
                })
                || !reply
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == drag.target && pane.rect == drag.pane_rect)
        }) {
            self.cancel_pointer_seek();
        }
        if self.pointer_divider.is_some_and(|drag| {
            drag.revision != reply.revision
                || !reply
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == drag.target)
        }) {
            self.pointer_divider = None;
            self.pane_mode = None;
        }
        let divider_mode_stale = match self.pane_mode {
            Some(PaneMode::Divider {
                target, revision, ..
            }) => {
                revision != reply.revision
                    || !captured_target_present(target)
                    || self.layout.is_some_and(|layout| {
                        layout.rows != reply.rows || layout.columns != reply.columns
                    })
            }
            _ => false,
        };
        if divider_mode_stale {
            self.pointer_divider = None;
            self.pane_mode_exit_key = None;
            self.pane_mode = None;
        } else if self.layout.is_some_and(|layout| {
            layout.revision != reply.revision
                || layout.rows != reply.rows
                || layout.columns != reply.columns
        }) {
            self.pane_mode = None;
        }
        if let Some(drag) = self.selection.as_ref() {
            let next_rect = reply
                .panes
                .iter()
                .flatten()
                .find(|pane| pane.target == drag.target)
                .and_then(|pane| panelink::pane_layout::content_rect(pane.rect));
            if next_rect != Some(drag.content_rect) {
                self.cancel_selection();
            }
        }
        self.local = local_layout(&reply);
        let present = |target: TargetRef| reply.panes.iter().flatten().any(|p| p.target == target);
        let preferred = self
            .preferred_focus
            .take()
            .filter(|target| present(*target));
        if let Some(target) = preferred {
            self.set_focus(Some(target));
        } else if !self.focus.is_some_and(present) {
            self.set_focus(reply.panes[0].map(|pane| pane.target));
        }
        if self.pending_target.is_some_and(|target| !present(target)) {
            self.pending_len = 0;
            self.pending_target = None;
            self.pending_blocked = false;
            self.status = Some(("pane left before queued keys were sent", Instant::now()));
        }
        if self
            .confirm
            .intent()
            .is_some_and(|intent| !present(intent.target))
        {
            self.dismiss_close_confirmation();
            self.status = Some(("pane already closed", Instant::now()));
        }
        self.screens.reconcile_layout(self.layout, &reply);
        self.layout = Some(reply);
        for attempt in &mut self.latest_copy_attempt {
            if attempt.is_some_and(|(target, _)| !captured_target_present(target)) {
                *attempt = None;
            }
        }
        for feedback in &mut self.copied_feedback {
            if feedback.is_some_and(|(target, _)| !captured_target_present(target)) {
                *feedback = None;
            }
        }
        if let Some((generation, target, route)) = self.pending_split_focus {
            let present = reply
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target);
            if generation != self.focus_generation
                || self.workspace.view().route_switching
                || self.workspace.view().current_route != route
            {
                self.pending_split_focus = None;
            } else if present {
                self.pending_split_focus = None;
                self.focus_target(target);
                self.set_status("split applied; new pane focused", Instant::now());
            }
        }
        self.dirty = true;
    }

    /// Focus a pane locally (presentation only; no ownership change).
    pub(crate) fn focus_target(&mut self, target: TargetRef) {
        self.set_focus(Some(target));
        self.dirty = true;
    }

    fn set_focus(&mut self, target: Option<TargetRef>) {
        if self.focus != target {
            self.pane_mode = None;
            self.cancel_selection();
            self.focus = target;
            self.focus_generation = self.focus_generation.wrapping_add(1);
            self.pending_split_focus = None;
        }
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }

    /// Apply one split completion on the host thread. Focus changes only when
    /// no deliberate focus move occurred since this request was admitted.
    pub(crate) fn complete_split(
        &mut self,
        request_id: u64,
        focus_generation: u64,
        target: TargetRef,
        result: Result<TargetRef, &'static str>,
        now: Instant,
    ) {
        let Some((active_id, admitted_focus, source, route)) = self.pending_split else {
            return;
        };
        if active_id != request_id || admitted_focus != focus_generation || source != target {
            return;
        }
        self.pending_split = None;
        self.clear_applying_status();
        match result {
            Ok(new_target) => {
                if self.focus_generation == focus_generation
                    && !self.workspace.view().route_switching
                    && self.workspace.view().current_route == route
                {
                    self.pending_split_focus = Some((focus_generation, new_target, route));
                    if self.layout.is_some_and(|layout| {
                        layout
                            .panes
                            .iter()
                            .flatten()
                            .any(|pane| pane.target == new_target)
                    }) {
                        self.pending_split_focus = None;
                        self.focus_target(new_target);
                        self.set_status("split applied; new pane focused", now);
                    } else {
                        self.set_status("split applied; waiting for layout", now);
                    }
                } else {
                    self.set_status("split applied", now);
                }
            }
            Err(label) => {
                if is_uncertain_mutation_error(label) {
                    self.set_persistent_status("Outcome unknown", now);
                }
                self.set_status(label, now)
            }
        }
    }

    /// Apply a close completion for the exact admitted target and request.
    pub(crate) fn complete_close(
        &mut self,
        request_id: u64,
        target: TargetRef,
        result: Result<CloseResult, &'static str>,
        now: Instant,
    ) {
        let Some((active_id, active_target, last_pane)) = self.pending_close else {
            return;
        };
        if active_id != request_id || active_target != target {
            return;
        }
        self.pending_close = None;
        match result {
            Ok(result) => {
                if matches!(result.outcome, CloseOutcome::Stopped(StopOutcome::Unknown)) {
                    self.set_persistent_status("Outcome unknown", now);
                } else {
                    self.clear_applying_status();
                }
                if matches!(
                    result.outcome,
                    CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated)
                ) {
                    self.forget_pane(target);
                }
                self.set_status(close_message(&result, last_pane), now);
            }
            Err(label) => {
                if is_uncertain_mutation_error(label) {
                    self.set_persistent_status("Outcome unknown", now);
                } else {
                    self.clear_applying_status();
                }
                self.set_status(label, now);
            }
        }
    }

    /// Apply only the completion paired with the one admitted action. A
    /// successful or partial operation carries its authoritative view even
    /// when the operation has a partial terminal resize outcome.
    pub(crate) fn complete_pane_operation(
        &mut self,
        id: u64,
        action: PaneAction,
        result: Result<super::pane_operations::PaneOperationUpdate, &'static str>,
        now: Instant,
    ) {
        if self.pending_pane_operation != Some((id, action)) {
            return;
        }
        self.pending_pane_operation = None;
        match result {
            Ok(update) if encode_view(&update.view).is_ok() => {
                let outcome = update.outcome;
                let view = update.view;
                let may_install = self.pane_operations.is_none_or(|current| {
                    let current_identity = current
                        .layout
                        .panes
                        .iter()
                        .flatten()
                        .next()
                        .map(|pane| (pane.target.generation, pane.target.workspace));
                    let next_identity = view
                        .layout
                        .panes
                        .iter()
                        .flatten()
                        .next()
                        .map(|pane| (pane.target.generation, pane.target.workspace));
                    current_identity != next_identity
                        || view.layout.revision >= current.layout.revision
                });
                if !may_install {
                    if outcome == Outcome::Unknown {
                        self.set_persistent_status("Outcome unknown", now);
                        self.set_status("pane operation outcome unknown", now);
                    } else {
                        self.clear_applying_status();
                        self.set_status("pane operation applied; newer layout retained", now);
                    }
                    self.dirty = true;
                    return;
                }
                self.apply_net_update(
                    NetUpdate {
                        layout: Some(view.layout),
                        pane_operations: Some(view),
                        screens: Vec::new(),
                        status: None,
                        server_present: true,
                    },
                    now,
                );
                if outcome == Outcome::Unknown {
                    self.set_persistent_status("Outcome unknown", now);
                } else {
                    self.clear_applying_status();
                    self.set_status(
                        if outcome == Outcome::Partial {
                            "pane operation applied; resize pending"
                        } else {
                            "pane operation applied"
                        },
                        now,
                    );
                }
            }
            Ok(_) => {
                self.set_persistent_status("Outcome unknown", now);
                self.set_status("pane view unavailable", now);
            }
            Err(label) => {
                if is_uncertain_mutation_error(label) {
                    self.set_persistent_status("Outcome unknown", now);
                    self.set_status("pane operation status unknown; layout unchanged", now);
                } else {
                    self.clear_applying_status();
                    self.set_status(label, now);
                }
            }
        }
        self.dirty = true;
    }

    pub(crate) fn report_worker_status(&mut self, label: &'static str, now: Instant) {
        if is_uncertain_mutation_error(label) {
            self.set_persistent_status("Outcome unknown", now);
        } else {
            self.set_status(label, now);
        }
    }

    pub(crate) fn retry_pending_keys(&self) -> bool {
        self.pending_blocked && self.pending_len > 0
    }

    /// Explicit user exit discards only keys that were never admitted to the
    /// worker; the host reports this after restoring the terminal.
    pub(crate) fn abandon_pending_keys(&mut self) -> bool {
        let abandoned = self.pending_len > 0;
        self.pending_len = 0;
        self.pending_target = None;
        self.pending_blocked = false;
        abandoned
    }

    pub(crate) fn apply_net_update(&mut self, update: NetUpdate, now: Instant) {
        let reconciled =
            update.server_present && (update.layout.is_some() || update.pane_operations.is_some());
        if !update.server_present {
            self.server_present = false;
            self.screens.discard_alternate();
            if self.pending_len > 0 {
                self.abandon_pending_keys();
            }
            self.dirty = true;
        } else if reconciled {
            self.server_present = true;
        }
        let mut installed_operations = false;
        if let Some(view) = update.pane_operations {
            let ordering_ok = self.pane_operations.is_none_or(|current| {
                let current_identity = current
                    .layout
                    .panes
                    .iter()
                    .flatten()
                    .next()
                    .map(|pane| (pane.target.generation, pane.target.workspace));
                let next_identity = view
                    .layout
                    .panes
                    .iter()
                    .flatten()
                    .next()
                    .map(|pane| (pane.target.generation, pane.target.workspace));
                current_identity != next_identity || view.layout.revision >= current.layout.revision
            });
            if encode_view(&view).is_ok() && ordering_ok {
                let geometry_changed = self.pane_operations.is_some_and(|current| current != view);
                if geometry_changed {
                    self.pane_mode = None;
                    self.cancel_pointer_seek();
                    self.cancel_selection();
                }
                self.pane_operations = Some(view);
                self.set_layout(view.layout);
                if let Some(target) = view.zoomed_target {
                    if self.focus != Some(target) {
                        self.focus_target(target);
                    }
                }
                installed_operations = true;
            } else if self.pane_operations.is_none() {
                self.set_status("pane view unavailable", now);
            }
        }
        if !installed_operations {
            if let Some(layout) = update.layout {
                if self.pane_operations.is_none() {
                    self.set_layout(layout);
                }
            }
        }
        for screen in update.screens {
            if self.layout.is_some_and(|layout| {
                layout
                    .panes
                    .iter()
                    .flatten()
                    .any(|p| p.target == screen.target)
            }) {
                if let Ok(checkpoint) = panelink::screen_checkpoint::decode(&screen.checkpoint) {
                    let loaded = match screen.row_wraps.as_deref() {
                        Some(wraps) => self.load_screen_with_history_cursor(
                            screen.target,
                            screen.revision,
                            &checkpoint,
                            Some(wraps),
                            screen.history_cursor,
                        ),
                        None => self.load_screen_with_history_cursor(
                            screen.target,
                            screen.revision,
                            &checkpoint,
                            None,
                            screen.history_cursor,
                        ),
                    };
                    if loaded.is_err() {
                        self.set_status("screen invalid", now);
                    } else {
                        self.refresh_pointer_seek_on_output(screen.target);
                    }
                } else {
                    self.set_status("screen invalid", now);
                }
            }
        }
        if let Some(status) = update.status {
            self.set_status(status, now);
        }
    }

    /// Apply a history or wheel completion only to the exact pane/view epoch
    /// that initiated it. Live screen polls keep updating `snapshot` while a
    /// history view is pinned.
    pub(crate) fn apply_scroll_update(&mut self, update: ScrollUpdate, now: Instant) {
        if self.copy_mode
            && self.selection.as_ref().is_some_and(|drag| {
                drag.target == update.target
                    && self.screens.iter().any(|slot| {
                        slot.target == Some(update.target) && slot.view_epoch == update.epoch
                    })
            })
        {
            self.cancel_selection();
        }
        let target = update.target;
        let change = self.screens.apply_scroll_update(update);
        self.dirty |= change.dirty;
        if let Some(label) = change.status {
            self.set_status(label, now);
        }
        if change.finish_extent {
            self.finish_unknown_extent_capture(target);
        }
    }

    fn finish_unknown_extent_capture(&mut self, target: TargetRef) {
        let (mut drag, released) = if let Some(drag) = self
            .pointer_seek
            .filter(|drag| drag.target == target && drag.awaiting_extent)
        {
            (drag, false)
        } else if let Some(drag) = self
            .deferred_thumb_release
            .filter(|drag| drag.target == target)
        {
            // Consume the released gesture exactly once, whatever the reply.
            self.deferred_thumb_release = None;
            (drag, true)
        } else {
            return;
        };
        let layout_current = self
            .layout
            .is_some_and(|layout| layout.revision == drag.layout_revision);
        let Some(slot) = self.screens.iter().find(|slot| slot.target == Some(target)) else {
            if !released {
                self.pointer_seek = None;
            }
            return;
        };
        if !slot.history_extent_known || (released && !layout_current) {
            return;
        }
        drag.total_rows = slot.history_total.max(slot.history_offset);
        let rect = shifted_rect(
            drag.pane_rect,
            u16::from(self.workspace.view().workspace_index.is_some()),
        );
        let Some(geometry) = scrollbar_geometry(
            rect,
            Scrollbar {
                offset_rows: slot.history_offset.min(drag.total_rows),
                total_rows: drag.total_rows,
            },
        ) else {
            self.pointer_seek = None;
            return;
        };
        drag.track = geometry.track;
        drag.thumb_rows = geometry.thumb.rows;
        drag.grab_offset = drag.grab_offset.min(geometry.thumb.rows.saturating_sub(1));
        drag.awaiting_extent = false;
        drag.preview_offset = pointer_seek_offset(drag, drag.last_y);
        self.screens.queue_seek(target, drag.preview_offset);
        if !released {
            self.pointer_seek = Some(drag);
        }
        self.dirty = true;
    }

    fn refresh_pointer_seek_on_output(&mut self, target: TargetRef) {
        let Some(mut drag) = self
            .pointer_seek
            .filter(|drag| drag.target == target && !drag.awaiting_extent)
        else {
            return;
        };
        let Some(slot) = self.screens.iter().find(|slot| slot.target == Some(target)) else {
            self.cancel_pointer_seek();
            return;
        };
        if !slot.history_extent_known {
            return;
        }
        let total_rows = slot.history_total.max(slot.history_offset);
        let rect = shifted_rect(
            drag.pane_rect,
            u16::from(self.workspace.view().workspace_index.is_some()),
        );
        let Some(geometry) = scrollbar_geometry(
            rect,
            Scrollbar {
                offset_rows: slot.history_offset.min(total_rows),
                total_rows,
            },
        ) else {
            self.cancel_pointer_seek();
            return;
        };
        drag.track = geometry.track;
        drag.thumb_rows = geometry.thumb.rows;
        drag.total_rows = total_rows;
        drag.grab_offset = drag.grab_offset.min(geometry.thumb.rows.saturating_sub(1));
        let previous = drag.preview_offset;
        drag.preview_offset = pointer_seek_offset(drag, drag.last_y);
        self.screens.queue_seek(target, drag.preview_offset);
        self.pointer_seek = Some(drag);
        self.dirty |= previous != drag.preview_offset;
    }

    fn return_to_live(&mut self, target: TargetRef) {
        if self
            .deferred_thumb_release
            .is_some_and(|drag| drag.target == target)
        {
            self.deferred_thumb_release = None;
        }
        self.dirty |= self.screens.return_to_live(target);
    }

    /// Seek the focused normal buffer to an absolute retained-history offset.
    fn seek_history(&mut self, now: Instant, requested: u16, effects: &mut impl Effects) {
        let Some(target) = self.focus else {
            return;
        };
        self.seek_history_for_target(target, now, requested, effects);
    }

    fn seek_history_for_target(
        &mut self,
        target: TargetRef,
        now: Instant,
        requested: u16,
        effects: &mut impl Effects,
    ) {
        self.seek_history_for_target_mode(target, now, requested, false, effects);
    }

    fn discover_history_extent_for_target(
        &mut self,
        target: TargetRef,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let current_offset = self
            .screens
            .iter()
            .find(|slot| slot.target == Some(target))
            .map_or(0, |slot| slot.history_offset);
        self.seek_history_for_target_mode(target, now, current_offset, true, effects);
    }

    fn seek_history_for_target_mode(
        &mut self,
        target: TargetRef,
        now: Instant,
        requested: u16,
        refresh_extent: bool,
        effects: &mut impl Effects,
    ) {
        if let Some(label) = self
            .screens
            .seek(target, requested, refresh_extent, effects)
        {
            self.set_status(label, now);
        }
    }

    fn scroll_pane(
        &mut self,
        target: TargetRef,
        screen_x: u16,
        screen_y: u16,
        up: bool,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let Some(pane) = self.display_layout().and_then(|layout| {
            layout
                .panes
                .into_iter()
                .flatten()
                .find(|pane| pane.target == target)
        }) else {
            return;
        };
        let Some(content) = panelink::pane_layout::content_rect(pane.rect) else {
            return;
        };
        let content = shifted_rect(
            content,
            u16::from(self.workspace.view().workspace_index.is_some()),
        );
        let point = pointer_content_point(content, screen_x, screen_y);
        let (column, row) = (point.column, point.row);
        if let Some(label) = self.screens.wheel(target, column, row, up, now, effects) {
            self.set_status(label, now);
        }
    }

    /// Admit fresh alternate-screen gestures or accumulated normal-history
    /// motion after the previous result. Never reinsert admitted child input.
    pub(crate) fn flush_scroll(&mut self, now: Instant, effects: &mut impl Effects) {
        if self.workspace.view().route_switching
            || self.copy_mode
            || !self.server_present
            || self.help_open
            || self.confirm.is_open()
            || self.menu.is_open()
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
            || self.pane_mutations_blocked()
        {
            return;
        }
        if let Some(label) = self.screens.flush(now, effects) {
            self.set_status(label, now);
        }
    }

    /// Last loaded screen revision for one pane.
    #[cfg(test)]
    pub(crate) fn screen_revision(&self, target: TargetRef) -> Option<u64> {
        self.screens
            .iter()
            .find(|slot| slot.target == Some(target))
            .map(|slot| slot.revision)
    }

    /// Copy one decoded checkpoint into the pane's preallocated cache.
    #[cfg(test)]
    pub(crate) fn load_screen(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
    ) -> Result<(), TerminalError> {
        self.load_screen_with_wraps(target, revision, checkpoint, None)
    }

    #[cfg(test)]
    pub(crate) fn load_screen_with_wraps(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
        row_wraps: Option<&[bool]>,
    ) -> Result<(), TerminalError> {
        self.load_screen_with_history_cursor(target, revision, checkpoint, row_wraps, None)
    }

    fn load_screen_with_history_cursor(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
        row_wraps: Option<&[bool]>,
        history_cursor: Option<WireHistoryCursor>,
    ) -> Result<(), TerminalError> {
        if self.selection.as_ref().is_some_and(|drag| {
            drag.target == target
                && (row_wraps.is_none()
                    || (self.copy_mode && row_wraps.is_some_and(|wraps| drag.row_wraps != wraps)))
        }) {
            self.cancel_selection();
        }
        let buffer_changed =
            self.screens
                .load_screen(target, revision, checkpoint, row_wraps, history_cursor)?;
        if buffer_changed {
            self.cancel_selection();
            if self.pointer_divider.take().is_some()
                || matches!(self.pane_mode, Some(PaneMode::Divider { .. }))
            {
                self.pane_mode = None;
                self.pane_mode_exit_key = None;
            }
            if self.pointer_seek.is_some_and(|drag| drag.target == target) {
                self.cancel_pointer_seek();
            }
        }
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn set_status(&mut self, label: &'static str, now: Instant) {
        self.status = Some((label, now));
        self.dirty = true;
    }

    fn set_persistent_status(&mut self, label: &'static str, now: Instant) {
        if self.persistent_status == Some("Outcome unknown") && label == "Applying…" {
            self.set_status(label, now);
            return;
        }
        self.persistent_status = Some(label);
        self.set_status(label, now);
    }

    fn clear_applying_status(&mut self) {
        if self.persistent_status == Some("Applying…") {
            self.persistent_status = None;
            if self.status.is_some_and(|(label, _)| label == "Applying…") {
                self.status = None;
            }
            self.dirty = true;
        }
    }

    #[cfg(test)]
    pub(crate) fn take_copy_text(&mut self) -> Option<String> {
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        self.pending_copy.take()
    }

    pub(crate) fn take_copy_request(&mut self) -> Option<(TargetRef, u64, String)> {
        let target = self.pending_copy_target?;
        let request_id = self.pending_copy_id?;
        let text = self.pending_copy.take()?;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        Some((target, request_id, text))
    }

    pub(crate) fn report_clipboard_unavailable(&mut self, now: Instant) {
        self.report_clipboard_status(None, None, ClipboardStatus::Unavailable, now);
    }

    pub(crate) fn report_clipboard_status(
        &mut self,
        target: Option<TargetRef>,
        request_id: Option<u64>,
        status: ClipboardStatus,
        now: Instant,
    ) {
        let current_attempt = target.zip(request_id).and_then(|(target, request_id)| {
            self.latest_copy_attempt
                .iter()
                .flatten()
                .any(|attempt| *attempt == (target, request_id))
                .then_some((target, request_id))
        });
        if target.is_some() && request_id.is_some() && current_attempt.is_none() {
            return;
        }
        if status == ClipboardStatus::Copied {
            let Some((target, _request_id)) = current_attempt else {
                return;
            };
            let Some(index) = self.layout.and_then(|layout| {
                layout
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == target))
            }) else {
                return;
            };
            self.copied_feedback[index] = Some((target, now + COPY_FEEDBACK_TTL));
            if self
                .status
                .is_some_and(|(label, _)| is_clipboard_failure_label(label))
            {
                self.status = None;
            }
            self.dirty = true;
            return;
        }

        if let Some((target, _request_id)) = current_attempt {
            if let Some(index) = self.layout.and_then(|layout| {
                layout
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == target))
            }) {
                self.copied_feedback[index] = None;
                self.dirty = true;
            }
        }
        self.set_status(status.label(), now);
    }

    pub(crate) fn cancel_for_detach(&mut self) {
        self.cancel_selection();
        self.pending_copy = None;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
    }

    fn cancel_selection(&mut self) {
        let active =
            self.selection.take().is_some() || self.copy_mode || self.copy_anchor.is_some();
        self.copy_mode = false;
        self.copy_anchor = None;
        if active {
            self.dirty = true;
        }
    }

    fn queue_copy_text(&mut self, target: TargetRef, text: String, now: Instant) {
        let request_id = self.mark_copy_attempt(target);
        if self.pending_copy.is_some() {
            self.report_clipboard_status(
                Some(target),
                Some(request_id),
                ClipboardStatus::Busy,
                now,
            );
        } else {
            self.pending_copy = Some(text);
            self.pending_copy_target = Some(target);
            self.pending_copy_id = Some(request_id);
            self.dirty = true;
        }
    }

    fn mark_copy_attempt(&mut self, target: TargetRef) -> u64 {
        let request_id = self.next_copy_id;
        self.next_copy_id = self.next_copy_id.wrapping_add(1).max(1);
        if let Some(index) = self.layout.and_then(|layout| {
            layout
                .panes
                .iter()
                .position(|pane| pane.is_some_and(|pane| pane.target == target))
        }) {
            for attempt in &mut self.latest_copy_attempt {
                if attempt.is_some_and(|(attempt_target, _)| attempt_target == target) {
                    *attempt = None;
                }
            }
            self.latest_copy_attempt[index] = Some((target, request_id));
            self.copied_feedback[index] = None;
            self.dirty = true;
        }
        request_id
    }

    fn fail_copy_attempt(&mut self, target: TargetRef, label: &'static str, now: Instant) {
        self.mark_copy_attempt(target);
        self.set_status(label, now);
    }

    fn begin_copy_mode(&mut self, now: Instant) {
        let Some(target) = self.focus else {
            self.set_status("Copy unavailable", now);
            return;
        };
        self.begin_copy_mode_for_target(target, now);
    }

    fn begin_copy_mode_for_target(&mut self, target: TargetRef, now: Instant) {
        let Some((slot, content, snapshot, source_history, epoch)) = self.visible_source(target)
        else {
            self.fail_copy_attempt(target, "Copy unavailable", now);
            return;
        };
        if snapshot.row_wraps().is_none() || content.rows == 0 || content.columns == 0 {
            self.fail_copy_attempt(target, "Copy unavailable", now);
            return;
        }
        let mut caret = Point {
            row: snapshot.cursor_row(),
            column: snapshot.cursor_column(),
        };
        if snapshot.cursor_hidden() || caret.row >= content.rows || caret.column >= content.columns
        {
            caret = Point { row: 0, column: 0 };
        }
        caret = normalize_point(snapshot, caret);
        match capture(
            target,
            slot,
            epoch,
            content,
            source_history,
            snapshot,
            caret,
        ) {
            Ok(mut drag) => {
                drag.end = caret;
                self.selection = Some(drag);
                self.copy_mode = true;
                self.copy_anchor = None;
                self.prefix = None;
                self.dirty = true;
            }
            Err(_) => self.fail_copy_attempt(target, "Copy unavailable", now),
        }
    }

    fn handle_copy_mode_event(
        &mut self,
        event: &Event,
        now: Instant,
        effects: &mut impl Effects,
    ) -> Flow {
        let Event::Key(key) = event else {
            return Flow::Continue;
        };
        match copy_mode_key(*key) {
            CopyModeKey::MoveLeft => self.move_copy_caret(-1, 0),
            CopyModeKey::MoveRight => self.move_copy_caret(1, 0),
            CopyModeKey::MoveUp => self.move_copy_caret(0, -1),
            CopyModeKey::MoveDown => self.move_copy_caret(0, 1),
            CopyModeKey::ToggleAnchor => {
                if self.copy_anchor.is_some() {
                    self.copy_anchor = None;
                } else {
                    let point = self.selection.as_ref().map(|drag| drag.end);
                    let anchored = point.is_some_and(|point| {
                        self.selection
                            .as_mut()
                            .is_some_and(|drag| reanchor(drag, point).is_ok())
                    });
                    if anchored {
                        self.copy_anchor = point;
                    } else {
                        self.set_status("Copy unavailable", now);
                    }
                }
                self.dirty = true;
            }
            CopyModeKey::Copy => {
                if let Event::Key(key) = event {
                    self.copy_exit_key = Some((key.code, key.modifiers));
                }
                self.finish_copy_mode(now);
            }
            CopyModeKey::Cancel => self.cancel_selection(),
            CopyModeKey::Detach => {
                self.cancel_selection();
                self.flush(now, effects);
                return Flow::Quit;
            }
            CopyModeKey::Ignore => {}
        }
        Flow::Continue
    }

    fn move_copy_caret(&mut self, dx: i8, dy: i8) {
        if self.selection.is_none() {
            self.cancel_selection();
            return;
        }
        let drag = self.selection.as_mut().expect("checked above");
        let rows = drag.frozen.rows();
        let columns = drag.frozen.columns();
        if rows == 0 || columns == 0 {
            self.cancel_selection();
            return;
        }
        let current = drag.end;
        let point = if dx != 0 {
            let linear =
                usize::from(current.row) * usize::from(columns) + usize::from(current.column);
            let last = usize::from(rows) * usize::from(columns) - 1;
            let next = if dx < 0 {
                linear.saturating_sub(1)
            } else if drag
                .frozen
                .cell(current.row, current.column)
                .is_some_and(|cell| cell.is_wide())
            {
                linear.saturating_add(2).min(last)
            } else {
                linear.saturating_add(1).min(last)
            };
            Point {
                row: (next / usize::from(columns)) as u16,
                column: (next % usize::from(columns)) as u16,
            }
        } else {
            Point {
                row: if dy < 0 {
                    current.row.saturating_sub(1)
                } else {
                    current.row.saturating_add(1).min(rows - 1)
                },
                column: current.column,
            }
        };
        drag.end = normalize_point(&drag.frozen, point);
        if self.copy_anchor.is_some() {
            drag.selection.extend(drag.end);
        }
        self.dirty = true;
    }

    fn finish_copy_mode(&mut self, now: Instant) {
        let anchor = self.copy_anchor;
        let drag = self.selection.take();
        self.copy_mode = false;
        self.copy_anchor = None;
        self.dirty = true;
        let Some(drag) = drag else { return };
        if anchor.is_none() || anchor == Some(drag.end) {
            return;
        }
        let result = self
            .current_selection_identity(&drag)
            .ok_or(SelectionError::StaleView)
            .and_then(|identity| drag.selection.finish(identity));
        match result {
            Ok(text) => self.queue_copy_text(drag.target, text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => {
                self.fail_copy_attempt(drag.target, "Copy unavailable", now)
            }
            Err(SelectionError::TooLarge) => {
                self.fail_copy_attempt(drag.target, "Copy too large", now)
            }
            Err(_) => self.fail_copy_attempt(drag.target, "Copy cancelled", now),
        }
    }

    /// Whether navigation mode (after Ctrl-B) is active.
    #[cfg(test)]
    pub(crate) fn nav_active(&self) -> bool {
        self.prefix.is_some()
    }

    /// Drop an expired status or navigation mode; marks a repaint if needed.
    pub(crate) fn expire_status(&mut self, now: Instant) {
        if self
            .prefix
            .is_some_and(|at| now.saturating_duration_since(at) >= NAV_TIMEOUT)
        {
            self.prefix = None;
            self.dirty = true;
        }
        if self
            .status
            .is_some_and(|(_, at)| now.saturating_duration_since(at) >= STATUS_TTL)
        {
            self.status = None;
            self.dirty = true;
        }
        for feedback in &mut self.copied_feedback {
            if feedback.is_some_and(|(_, expires_at)| now >= expires_at) {
                *feedback = None;
                self.dirty = true;
            }
        }
    }

    /// Route one decoded host event. While the menu is open every event is
    /// consumed by the menu and nothing reaches a pane.
    pub(crate) fn handle_event(
        &mut self,
        event: &Event,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let is_wheel = matches!(event, Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown));
        if !is_wheel
            || self.help_open
            || self.confirm.is_open()
            || self.menu.is_open()
            || self.copy_mode
            || self.workspace.view().route_switching
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
            || self.pane_mode.is_some()
            || self.prefix.is_some()
        {
            self.screens.discard_alternate();
        }
        if let (Some(button), Event::Mouse(mouse)) = (self.menu_release_latch, event) {
            if mouse.kind == MouseEventKind::Up(button) {
                self.menu_release_latch = None;
                return Flow::Continue;
            }
        }
        if let Event::Key(key) = event {
            if let Some((code, modifiers)) = self.pane_mode_exit_key {
                match key.kind {
                    KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                        return Flow::Continue;
                    }
                    KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                        self.pane_mode_exit_key = None;
                        return Flow::Continue;
                    }
                    KeyEventKind::Press => self.pane_mode_exit_key = None,
                    _ => {}
                }
            }
            if let Some((code, modifiers)) = self.copy_exit_key {
                match key.kind {
                    KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                        return Flow::Continue;
                    }
                    KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                        self.copy_exit_key = None;
                        return Flow::Continue;
                    }
                    KeyEventKind::Press => {
                        self.copy_exit_key = None;
                    }
                    _ => {}
                }
            }
        }
        if matches!(event, Event::FocusLost | Event::Resize(_, _))
            && matches!(self.pane_mode, Some(PaneMode::Divider { .. }))
        {
            self.pointer_divider = None;
            self.pane_mode = None;
            self.pane_mode_exit_key = None;
            self.dirty = true;
        }
        if let Some(flow) = self.handle_empty_workspace_event(event, now) {
            return flow;
        }
        if self.help_open
            || self.confirm.is_open()
            || self.menu.is_open()
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
        {
            self.cancel_selection();
        }
        match event {
            Event::Key(key) if (key.is_press() || key.is_repeat()) && !self.copy_mode => {
                self.cancel_selection()
            }
            Event::Paste(_) if !self.copy_mode => self.cancel_selection(),
            Event::FocusLost | Event::Resize(_, _) => self.cancel_selection(),
            Event::Mouse(mouse)
                if !self.copy_mode
                    && (mouse.kind == MouseEventKind::ScrollUp
                        || mouse.kind == MouseEventKind::ScrollDown) =>
            {
                self.cancel_selection()
            }
            _ => {}
        }
        if let Event::Key(key) = event {
            if (key.is_press() || key.is_repeat()) && map_key(*key) == KeyAction::Quit {
                self.cancel_selection();
                self.help_open = false;
                self.prefix = None;
                if self.workspace.view().route_switching {
                    return Flow::Quit;
                }
                self.flush(now, effects);
                return Flow::Quit;
            }
        }
        if self.workspace.view().route_switching {
            // Host resize and repaint remain local while a destination is
            // prepared. Every other event is consumed without child effects.
            if matches!(event, Event::Resize(_, _)) {
                self.dirty = true;
            }
            return Flow::Continue;
        }
        if self.workspace.view().tab_dialog.is_some() {
            self.handle_tab_dialog_event(event, now);
            return Flow::Continue;
        }
        if self.pointer_seek.is_some() || self.pointer_divider.is_some() {
            self.handle_pointer_seek_event(event, now, effects);
            return Flow::Continue;
        }
        if self.pane_mode.is_some() {
            self.handle_pane_mode_event(event, now, effects);
            return Flow::Continue;
        }
        if self.actions_menu.is_some() {
            if let Event::Mouse(mouse) = event {
                if let MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) =
                    mouse.kind
                {
                    self.menu_release_latch = Some(button);
                }
            }
            return self.handle_actions_menu_event(event, now, recovery_availability, effects);
        }
        if self.help_open {
            if let Event::Mouse(mouse) = event {
                let (rows, columns) = self.viewport();
                let height = rows.min(HELP_LINES.len() as u16);
                let width = columns.min(72);
                let left = columns.saturating_sub(width) / 2;
                let top = rows.saturating_sub(height) / 2
                    + u16::from(self.workspace.view().workspace_index.is_some());
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && mouse.row == top
                    && mouse.column >= left
                    && mouse.column < left.saturating_add(width)
                {
                    self.help_open = false;
                    self.dirty = true;
                }
            }
            if let Event::Key(key) = event {
                match map_key(*key) {
                    KeyAction::Quit => {
                        self.help_open = false;
                        self.dirty = true;
                        self.flush(now, effects);
                        return Flow::Quit;
                    }
                    _ if key.is_press() || key.is_repeat() => {
                        let plain =
                            key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                        if plain && (key.code == KeyCode::Esc || key.code == KeyCode::Char('?')) {
                            self.help_open = false;
                            self.dirty = true;
                        }
                    }
                    _ => {}
                }
            }
            // Help owns keys, paste and mouse. Resize is observed by the host
            // loop, which updates `host` through `set_host`.
            return Flow::Continue;
        }
        if self.confirm.is_open() {
            // The prompt owns every event; none reaches a pane.
            self.dirty = true;
            if let (Some((code, modifiers)), Event::Key(key)) = (self.confirm_open_key, event) {
                match key.kind {
                    KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                        return Flow::Continue;
                    }
                    KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                        self.confirm_open_key = None;
                        return Flow::Continue;
                    }
                    KeyEventKind::Press => self.confirm_open_key = None,
                    _ => {}
                }
            }
            if let Event::Mouse(mouse) = event {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    let Some(intent) = self.confirm.intent() else {
                        return Flow::Continue;
                    };
                    let (rows, columns) = self.viewport();
                    let geometry = self.confirm_ui_geometry(rows, columns);
                    if geometry
                        .is_some_and(|geometry| geometry.close.contains(mouse.column, mouse.row))
                    {
                        self.dismiss_close_confirmation();
                        self.close_pane(intent, now, effects);
                        return Flow::Continue;
                    } else if geometry
                        .is_some_and(|geometry| geometry.cancel.contains(mouse.column, mouse.row))
                    {
                        self.dismiss_close_confirmation();
                        self.set_status("close cancelled", now);
                        return Flow::Continue;
                    }
                }
            }
            match self.confirm.handle(confirm_input(event)) {
                ConfirmResponse::Confirmed(intent) => {
                    self.dismiss_close_confirmation();
                    self.close_pane(intent, now, effects);
                }
                ConfirmResponse::Cancelled => {
                    self.dismiss_close_confirmation();
                    self.set_status("close cancelled", now);
                }
                ConfirmResponse::Consumed | ConfirmResponse::NotOpen => {}
            }
            match confirm_input(event) {
                panelink::split_menu::ConfirmInput::Toggle => {
                    self.confirm_close_selected = !self.confirm_close_selected;
                }
                panelink::split_menu::ConfirmInput::Activate => {
                    if self.confirm_close_selected {
                        if let ConfirmResponse::Confirmed(intent) =
                            self.confirm.handle(panelink::split_menu::ConfirmInput::Yes)
                        {
                            self.dismiss_close_confirmation();
                            self.close_pane(intent, now, effects);
                        }
                    } else {
                        self.dismiss_close_confirmation();
                        self.set_status("close cancelled", now);
                    }
                }
                _ => {}
            }
            return Flow::Continue;
        }
        if self.menu.is_open() {
            if let Event::Mouse(mouse) = event {
                if let MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) =
                    mouse.kind
                {
                    self.menu_release_latch = Some(button);
                }
            }
            let mut input = if self.workspace.view().workspace_index.is_some()
                && matches!(event, Event::Mouse(mouse) if mouse.row == 0)
            {
                panelink::split_menu::MenuInput::Other
            } else {
                menu_input(event)
            };
            if self.workspace.view().workspace_index.is_some() {
                match &mut input {
                    panelink::split_menu::MenuInput::MousePress { y, .. }
                    | panelink::split_menu::MenuInput::MouseRelease { y, .. } => {
                        *y = y.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            let response = self.menu.handle(input);
            self.dirty = true;
            match response {
                MenuResponse::Activated(intent) => self.activate(
                    SplitIntent {
                        expected_layout_revision: self.menu_revision,
                        ..intent
                    },
                    now,
                    effects,
                ),
                // The menu's mirror revision is local; use the server
                // revision captured when the menu opened.
                MenuResponse::CloseRequested(intent) => self.open_close_confirmation(CloseIntent {
                    expected_layout_revision: self.menu_revision,
                    ..intent
                }),
                MenuResponse::RecoveryRequested => return Flow::ViewRecovery,
                MenuResponse::RecoveryUnavailable => {
                    self.set_status("No previous layout saved", now)
                }
                MenuResponse::NotOpen | MenuResponse::Consumed | MenuResponse::Closed => {}
            }
            return Flow::Continue;
        }
        if self.workspace.view().picker_open || self.workspace.view().picker_selection.is_some() {
            self.handle_workspace_picker_event(event, now);
            return Flow::Continue;
        }
        if self.handle_header_event(event, now) {
            return Flow::Continue;
        }
        if self.copy_mode {
            if let Event::Mouse(mouse) = event {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let Some((target, close)) = self.pane_control_at(mouse.column, mouse.row) {
                        self.cancel_selection();
                        if close {
                            self.confirm_close_target(
                                target,
                                self.layout.map_or(0, |layout| layout.revision),
                                now,
                            );
                        } else {
                            self.open_actions_menu(
                                target,
                                Some(MouseButton::Left),
                                recovery_availability,
                                now,
                                effects,
                            );
                        }
                        return Flow::Continue;
                    }
                }
            }
            return self.handle_copy_mode_event(event, now, effects);
        }
        let recovery_availability = if self.needs_recovery_availability(event) {
            recovery_availability
        } else {
            Availability::Disabled(panelink::split_menu::DisabledReason::NoPreviousLayoutSaved)
        };
        match event {
            Event::Key(key) => {
                let action = map_key(*key);
                if self.prefix.is_some() {
                    if !key.is_press() && !key.is_repeat() {
                        return Flow::Continue;
                    }
                    let operation_key = pane_operation_key(key.code, key.modifiers).is_some();
                    let close_confirmation_key = (key.is_press() || key.is_repeat())
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                        && matches!(key.code, KeyCode::Char('x' | 'X'));
                    let flow =
                        self.navigate(key.code, key.modifiers, now, recovery_availability, effects);
                    if close_confirmation_key && self.confirm.is_open() {
                        self.confirm_open_key = Some((key.code, key.modifiers));
                    }
                    if operation_key {
                        self.pane_mode_exit_key = Some((key.code, key.modifiers));
                    }
                    return flow;
                }
                match action {
                    KeyAction::Forward(key) => self.push_key(key, now, effects),
                    KeyAction::Quit => {
                        self.flush(now, effects);
                        return Flow::Quit;
                    }
                    KeyAction::Prefix => {
                        self.flush(now, effects);
                        self.prefix = Some(now);
                        self.dirty = true;
                    }
                    KeyAction::Ignore => {}
                }
            }
            Event::Paste(text) => {
                self.flush(now, effects);
                if self.pending_len != 0 {
                    self.set_status("busy: queued keys must be sent first", now);
                    return Flow::Continue;
                }
                match self.focus {
                    Some(target) => {
                        self.return_to_live(target);
                        if let Err(label) = effects.send_paste(target, text) {
                            self.set_status(label, now);
                        }
                    }
                    None => self.set_status("no focused pane", now),
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(target) = self.history_back_at(mouse.column, mouse.row) {
                        self.return_to_live(target);
                        return Flow::Continue;
                    }
                    if let Some((target, close)) = self.pane_control_at(mouse.column, mouse.row) {
                        self.flush(now, effects);
                        if close {
                            self.confirm_close_target(
                                target,
                                self.layout.map_or(0, |layout| layout.revision),
                                now,
                            );
                        } else {
                            self.open_actions_menu(
                                target,
                                Some(MouseButton::Left),
                                recovery_availability,
                                now,
                                effects,
                            );
                        }
                        return Flow::Continue;
                    }
                    if self.begin_scrollbar_pointer(mouse.column, mouse.row, now, effects) {
                        return Flow::Continue;
                    }
                    if self.begin_divider_pointer(mouse.column, mouse.row, now, effects) {
                        return Flow::Continue;
                    }
                    if let Some(target) = self.pane_at(mouse.column, mouse.row) {
                        self.flush(now, effects);
                        self.focus_target(target);
                        if let Some(row) = self.pane_row(mouse.row) {
                            self.begin_selection(target, mouse.column, row, now);
                        }
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let Some(row) = self.pane_row(mouse.row) {
                        self.extend_selection(mouse.column, row);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    if let Some(row) = self.pane_row(mouse.row) {
                        self.extend_selection(mouse.column, row);
                        self.finish_selection(now);
                    } else {
                        self.cancel_selection();
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.cancel_selection();
                    self.flush(now, effects);
                    if self.pending_pane_operation.is_some() {
                        self.set_status("pane operation pending", now);
                    } else if let Some(row) = self.pane_row(mouse.row) {
                        self.open_at_pointer(
                            mouse.column,
                            row,
                            now,
                            recovery_availability,
                            effects,
                        );
                    }
                }
                _ => {
                    if let (Some(up), Some(target)) =
                        (mouse_wheel(mouse), self.pane_at(mouse.column, mouse.row))
                    {
                        self.flush(now, effects);
                        if let Some(row) = self.pane_row(mouse.row) {
                            self.scroll_pane(target, mouse.column, row, up, now, effects);
                        }
                    }
                }
            },
            Event::FocusGained => {}
            Event::FocusLost => self.cancel_selection(),
            Event::Resize(_, _) => self.cancel_selection(),
        }
        Flow::Continue
    }

    /// One key in navigation mode. Every key here is consumed locally (never
    /// sent to a pane). Focus keys keep the mode on so several moves can be
    /// chained; any other key ends it.
    fn navigate(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        self.dirty = true;
        let plain = modifiers.is_empty() || modifiers == KeyModifiers::SHIFT;
        if self.workspace.view().workspace_index.is_some()
            && plain
            && matches!(code, KeyCode::Char('w' | 'W'))
        {
            self.open_workspace_picker();
            return Flow::Continue;
        }
        if self.workspace.view().workspace_index.is_some() {
            if let Some(command) = workspace_tab_command(code, modifiers) {
                match command {
                    WorkspaceTabCommand::Create => {
                        let project = self
                            .workspace
                            .view()
                            .current_route
                            .map(|route| route.project)
                            .unwrap_or_else(|| {
                                ProjectId::new(1).expect("Default project ID is nonzero")
                            });
                        self.request_tab_create(project, now);
                    }
                    WorkspaceTabCommand::Actions => {
                        if let Some(route) = self.workspace.view().current_route {
                            self.open_tab_actions(route);
                        }
                    }
                }
                return Flow::Continue;
            }
            if let Some(action) = workspace_navigation(code, modifiers) {
                self.navigate_workspace(action, now);
                return Flow::Continue;
            }
        }
        let focus_move = match code {
            KeyCode::Left if plain => Some(FocusMove::Left),
            KeyCode::Right if plain => Some(FocusMove::Right),
            KeyCode::Up if plain => Some(FocusMove::Up),
            KeyCode::Down if plain => Some(FocusMove::Down),
            KeyCode::Tab | KeyCode::BackTab => Some(FocusMove::Next),
            KeyCode::Char('o' | 'O') if plain => Some(FocusMove::Next),
            _ => None,
        };
        if let Some(movement) = focus_move {
            self.move_focus(movement);
            self.prefix = Some(now);
            return Flow::Continue;
        }
        self.prefix = None;
        if plain && matches!(code, KeyCode::Char('c' | 'C')) {
            if self.pending_pane_operation.is_some() {
                self.set_status("pane operation pending", now);
                return Flow::Continue;
            }
            self.begin_copy_mode(now);
            return Flow::Continue;
        }
        if plain {
            let seek = match code {
                KeyCode::PageUp => self.focus.and_then(|target| {
                    let slot = self
                        .screens
                        .iter()
                        .find(|slot| slot.target == Some(target))?;
                    let page = self
                        .display_layout()?
                        .panes
                        .iter()
                        .flatten()
                        .find(|pane| pane.target == target)
                        .and_then(|pane| panelink::pane_layout::content_rect(pane.rect))
                        .map_or(1, |rect| rect.rows.saturating_sub(1).max(1));
                    Some(slot.history_offset.saturating_add(page))
                }),
                KeyCode::PageDown => self.focus.and_then(|target| {
                    let slot = self
                        .screens
                        .iter()
                        .find(|slot| slot.target == Some(target))?;
                    let page = self
                        .display_layout()?
                        .panes
                        .iter()
                        .flatten()
                        .find(|pane| pane.target == target)
                        .and_then(|pane| panelink::pane_layout::content_rect(pane.rect))
                        .map_or(1, |rect| rect.rows.saturating_sub(1).max(1));
                    Some(slot.history_offset.saturating_sub(page))
                }),
                KeyCode::Home => self.focus.and_then(|target| {
                    self.screens
                        .iter()
                        .find(|slot| slot.target == Some(target))
                        .map(|slot| {
                            if !slot.history_extent_known {
                                panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS
                            } else {
                                slot.history_total.max(slot.history_offset)
                            }
                        })
                }),
                KeyCode::End => Some(0),
                _ => None,
            };
            if let Some(offset) = seek {
                self.seek_history(now, offset, effects);
                return Flow::Continue;
            }
        }
        let direction = match code {
            KeyCode::Char('l' | 'L') if plain => Some(SplitDirection::Left),
            KeyCode::Char('r' | 'R' | '%') if plain => Some(SplitDirection::Right),
            KeyCode::Char('u' | 'U') if plain => Some(SplitDirection::Up),
            KeyCode::Char('d' | 'D' | '"') if plain => Some(SplitDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            self.split_focused(direction, now, effects);
            return Flow::Continue;
        }
        match code {
            KeyCode::Char('?') if plain => {
                self.prefix = None;
                self.flush(now, effects);
                self.help_open = true;
                self.dirty = true;
            }
            KeyCode::Char('s' | 'S') if plain => {
                if self.pending_pane_operation.is_some() {
                    self.set_status("pane operation pending", now);
                } else {
                    if let Some(target) = self.focus {
                        self.open_actions_menu(target, None, recovery_availability, now, effects);
                    }
                }
            }
            KeyCode::Char('v' | 'V') if plain => {
                if self.pending_pane_operation.is_some() {
                    self.set_status("pane operation pending", now);
                    return Flow::Continue;
                }
                if recovery_availability == Availability::Enabled {
                    return Flow::ViewRecovery;
                }
                self.set_status("No previous layout saved", now);
            }
            KeyCode::Char('x' | 'X') if plain => {
                if self.pending_pane_operation.is_some() {
                    self.set_status("pane operation pending", now);
                } else {
                    self.confirm_close_focused(now)
                }
            }
            // Ctrl-X detaches, also from navigation mode.
            KeyCode::Char('x' | 'X') if modifiers == KeyModifiers::CONTROL => {
                self.flush(now, effects);
                return Flow::Quit;
            }
            // Esc, Ctrl-B again, or anything else just leaves the mode.
            _ => {}
        }
        match pane_operation_key(code, modifiers) {
            Some(PaneOperationKey::Zoom) => self.toggle_zoom(now, effects),
            Some(PaneOperationKey::Swap) => self.open_swap_mode(now, effects),
            Some(PaneOperationKey::Divider) => self.open_divider_mode(now, effects),
            None => {}
        }
        Flow::Continue
    }

    fn operation_ready(&mut self, now: Instant, effects: &mut impl Effects) -> bool {
        if !self.server_present {
            self.set_status("Disconnected", now);
            return false;
        }
        if self.pane_operations.is_none() {
            self.set_status("pane controls unsupported by this server", now);
            return false;
        }
        if self.pending_pane_operation.is_some() {
            self.set_status("pane operation pending", now);
            return false;
        }
        self.flush(now, effects);
        if self.pending_len != 0 {
            self.set_status("busy: queued keys must be sent first", now);
            return false;
        }
        if self.pending_split.is_some() || self.pending_close.is_some() {
            self.set_status("pane action pending", now);
            return false;
        }
        true
    }

    fn toggle_zoom(&mut self, now: Instant, effects: &mut impl Effects) {
        if !self.operation_ready(now, effects) {
            return;
        }
        let Some(view) = self.pane_operations else {
            return;
        };
        let target = view.zoomed_target.or(self.focus);
        let Some(target) = target else {
            self.set_status("zoom unavailable: no focused pane", now);
            return;
        };
        self.toggle_zoom_for_target(target, view.layout.revision, now, effects);
    }

    fn toggle_zoom_for_target(
        &mut self,
        target: TargetRef,
        revision: u64,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        if !self.operation_ready(now, effects) {
            return;
        }
        let Some(view) = self.pane_operations else {
            return;
        };
        if view.layout.revision != revision
            || !view
                .layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            self.set_status("zoom cancelled: layout changed", now);
            return;
        }
        let zoomed = match view.zoomed_target {
            None => true,
            Some(current) if current == target => false,
            Some(_) => {
                self.set_status("unzoom the active pane first", now);
                return;
            }
        };
        self.submit_pane_operation(
            PaneAction::ZoomSet {
                target,
                expected_layout_revision: revision,
                zoomed,
            },
            now,
            effects,
        );
    }

    fn open_swap_mode(&mut self, now: Instant, effects: &mut impl Effects) {
        let Some(view) = self.pane_operations else {
            self.operation_ready(now, effects);
            return;
        };
        let Some(source) = self.focus else {
            self.set_status("swap unavailable: no focused pane", now);
            return;
        };
        self.open_swap_mode_for_target(source, view.layout.revision, now, effects);
    }

    fn open_swap_mode_for_target(
        &mut self,
        source: TargetRef,
        revision: u64,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        if !self.operation_ready(now, effects) {
            return;
        }
        let Some(view) = self.pane_operations else {
            return;
        };
        if view.layout.revision != revision
            || !view
                .layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == source)
        {
            self.set_status("swap cancelled: layout changed", now);
            return;
        }
        if view.zoomed_target.is_some() {
            self.set_status("unzoom before swapping panes", now);
            return;
        }
        let mut choices = [None; crate::pane_layout::MAX_PANES];
        let mut count = 0;
        for pane in view.layout.panes.iter().flatten() {
            if pane.target != source {
                choices[count] = Some(pane.target);
                count += 1;
            }
        }
        if count == 0 {
            self.set_status("swap unavailable: another pane is required", now);
            return;
        }
        self.cancel_selection();
        self.pane_mode = Some(PaneMode::Swap {
            source,
            revision,
            choices,
            selected: 0,
        });
        self.prefix = None;
        self.dirty = true;
    }

    fn open_divider_mode(&mut self, now: Instant, effects: &mut impl Effects) {
        let (Some(view), Some(target)) = (self.pane_operations, self.focus) else {
            self.set_status("divider adjustment unavailable", now);
            return;
        };
        self.open_divider_mode_for_target(target, view.layout.revision, now, effects);
    }

    fn open_divider_mode_for_target(
        &mut self,
        target: TargetRef,
        revision: u64,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        if !self.operation_ready(now, effects) {
            return;
        }
        let (Some(view), Some(local)) = (self.pane_operations, self.local.as_ref()) else {
            self.set_status("divider adjustment unavailable", now);
            return;
        };
        if view.layout.revision != revision
            || !view
                .layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            self.set_status("divider adjustment cancelled: layout changed", now);
            return;
        }
        if view.zoomed_target.is_some() {
            self.set_status("unzoom before adjusting dividers", now);
            return;
        }
        let mut indices = [None; crate::pane_layout::MAX_PANES - 1];
        let mut count = 0;
        let mut initial = None;
        for index in 0..indices.len() {
            let Ok(info) = local.divider_info(target, revision, index as u8) else {
                continue;
            };
            indices[count] = Some(index as u8);
            if initial.is_none() {
                initial = Some((count, info.first_span));
            }
            count += 1;
        }
        let Some((selected, first_cells)) = initial else {
            self.set_status("divider adjustment unavailable: no ancestor", now);
            return;
        };
        self.cancel_selection();
        self.pane_mode = Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected,
            first_cells,
        });
        self.prefix = None;
        self.dirty = true;
    }

    fn submit_pane_operation(
        &mut self,
        action: PaneAction,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let id = self.next_request_id();
        match effects.pane_operation(id, action) {
            Ok(()) => {
                self.pending_pane_operation = Some((id, action));
                self.set_persistent_status("Applying…", now);
            }
            Err("pane operations unavailable") => {
                self.set_status("pane controls unsupported by this server", now)
            }
            Err(_) => self.set_status("busy: pane operation was not queued", now),
        }
        self.dirty = true;
    }

    fn handle_pane_mode_event(&mut self, event: &Event, now: Instant, effects: &mut impl Effects) {
        if let Event::Mouse(mouse) = event {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.handle_pane_mode_pointer(mouse.column, mouse.row, now, effects);
            }
            return;
        }
        if let Event::Key(key) = event {
            if let Some((code, modifiers)) = self.pane_mode_exit_key {
                match key.kind {
                    KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                        return;
                    }
                    KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                        self.pane_mode_exit_key = None;
                        return;
                    }
                    KeyEventKind::Press => self.pane_mode_exit_key = None,
                    _ => return,
                }
            }
            if key.kind == KeyEventKind::Release {
                return;
            }
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                return;
            }
            if key.modifiers == KeyModifiers::CONTROL
                && matches!(key.code, KeyCode::Char('x' | 'X'))
            {
                self.pane_mode = None;
                self.cancel_selection();
                self.flush(now, effects);
                return;
            }
            if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
            {
                return;
            }
            if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                self.pane_mode = None;
                self.pane_mode_exit_key = Some((key.code, key.modifiers));
                self.set_status("pane operation cancelled", now);
                return;
            }
            if key.code == KeyCode::Enter && key.modifiers.is_empty() {
                let mode = self.pane_mode.take();
                self.pane_mode_exit_key = Some((key.code, key.modifiers));
                match mode {
                    Some(PaneMode::Swap {
                        source,
                        revision,
                        choices,
                        selected,
                    }) => {
                        if let Some(destination) = choices.get(selected).copied().flatten() {
                            self.submit_pane_operation(
                                PaneAction::Swap {
                                    source,
                                    destination,
                                    expected_layout_revision: revision,
                                },
                                now,
                                effects,
                            );
                        }
                    }
                    Some(PaneMode::Divider {
                        target,
                        revision,
                        indices,
                        selected,
                        first_cells,
                    }) => {
                        if let Some(divider_index) = indices
                            .get(selected)
                            .copied()
                            .flatten()
                            .filter(|divider_index| {
                                self.layout.is_some_and(|layout| {
                                    layout.revision == revision
                                        && layout
                                            .panes
                                            .iter()
                                            .flatten()
                                            .any(|pane| pane.target == target)
                                }) && self.divider_value_valid(
                                    target,
                                    revision,
                                    *divider_index,
                                    first_cells,
                                )
                            })
                        {
                            self.submit_pane_operation(
                                PaneAction::DividerSet {
                                    target,
                                    expected_layout_revision: revision,
                                    divider_index,
                                    first_cells,
                                },
                                now,
                                effects,
                            );
                        }
                    }
                    None => {}
                }
                return;
            }
            match self.pane_mode {
                Some(PaneMode::Swap {
                    source,
                    revision,
                    choices,
                    mut selected,
                }) => {
                    if matches!(key.code, KeyCode::Down | KeyCode::Right | KeyCode::Tab)
                        || (key.modifiers.is_empty()
                            && matches!(key.code, KeyCode::Char('j' | 'l')))
                    {
                        let count = choices.iter().flatten().count();
                        selected = (selected + 1) % count;
                    } else if matches!(key.code, KeyCode::Up | KeyCode::Left | KeyCode::BackTab)
                        || (key.modifiers.is_empty()
                            && matches!(key.code, KeyCode::Char('k' | 'h')))
                    {
                        let count = choices.iter().flatten().count();
                        selected = (selected + count - 1) % count;
                    }
                    self.pane_mode = Some(PaneMode::Swap {
                        source,
                        revision,
                        choices,
                        selected,
                    });
                }
                Some(PaneMode::Divider {
                    target,
                    revision,
                    indices,
                    mut selected,
                    mut first_cells,
                }) => {
                    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                        let count = indices.iter().flatten().count();
                        selected = if key.code == KeyCode::BackTab {
                            (selected + count - 1) % count
                        } else {
                            (selected + 1) % count
                        };
                        first_cells = self.divider_first_span(
                            target,
                            revision,
                            indices[selected].unwrap_or(0),
                        );
                    } else if let Some(index) = indices[selected] {
                        self.adjust_divider_preview(
                            target,
                            revision,
                            index,
                            *key,
                            &mut first_cells,
                        );
                    }
                    self.pane_mode = Some(PaneMode::Divider {
                        target,
                        revision,
                        indices,
                        selected,
                        first_cells,
                    });
                }
                None => {}
            }
            self.dirty = true;
        }
        // Paste, mouse, focus, and resize are owned by the mode and never
        // become child input. Resize cancellation is handled by set_host.
    }

    fn handle_pane_mode_pointer(
        &mut self,
        x: u16,
        y: u16,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let Some(mode) = self.pane_mode else { return };
        let (view_rows, view_columns) = self.viewport();
        let (choice_count, total_rows) = match mode {
            PaneMode::Swap { choices, .. } => {
                let count = choices.iter().flatten().count();
                (count, 1 + count + 2)
            }
            PaneMode::Divider { indices, .. } => {
                let count = indices.iter().flatten().count();
                (count, 1 + count + 3)
            }
        };
        let rows = u16::try_from(total_rows).unwrap_or(u16::MAX).min(view_rows);
        let columns = view_columns.min(76);
        if rows == 0 || columns == 0 {
            return;
        }
        let left = view_columns.saturating_sub(columns) / 2;
        let top = view_rows.saturating_sub(rows) / 2;
        let offset = u16::from(self.workspace.view().workspace_index.is_some());
        if x < left || x >= left.saturating_add(columns) || y < top.saturating_add(offset) {
            return;
        }
        let row = usize::from(y - top - offset);
        if row >= total_rows || row == 0 {
            return;
        }
        let adjustment = usize::from(matches!(mode, PaneMode::Divider { .. }));
        let apply_row = 1 + choice_count + adjustment;
        let cancel_row = apply_row + 1;
        if adjustment == 1 && row == 1 + choice_count {
            let code = if x < left + 3 {
                Some(KeyCode::Char('h'))
            } else if (left + 4..left + 7).contains(&x) {
                Some(KeyCode::Char('l'))
            } else {
                None
            };
            if let Some(code) = code {
                let key = Event::Key(crossterm_input::KeyEvent::new(code, KeyModifiers::NONE));
                self.handle_pane_mode_event(&key, now, effects);
            }
            return;
        }
        if row == apply_row {
            let enter = Event::Key(crossterm_input::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ));
            self.handle_pane_mode_event(&enter, now, effects);
            return;
        }
        if row == cancel_row {
            let escape = Event::Key(crossterm_input::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            ));
            self.handle_pane_mode_event(&escape, now, effects);
            return;
        }
        let selected = row - 1;
        match mode {
            PaneMode::Swap {
                source,
                revision,
                choices,
                ..
            } if selected < choice_count => {
                self.pane_mode = Some(PaneMode::Swap {
                    source,
                    revision,
                    choices,
                    selected,
                });
            }
            PaneMode::Divider {
                target,
                revision,
                indices,
                ..
            } if selected < choice_count => {
                let first_cells = indices[selected]
                    .map_or(0, |index| self.divider_first_span(target, revision, index));
                self.pane_mode = Some(PaneMode::Divider {
                    target,
                    revision,
                    indices,
                    selected,
                    first_cells,
                });
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn divider_first_span(&self, target: TargetRef, revision: u64, index: u8) -> u16 {
        self.local
            .as_ref()
            .and_then(|layout| layout.divider_info(target, revision, index).ok())
            .map_or(0, |info| info.first_span)
    }

    fn adjust_divider_preview(
        &self,
        target: TargetRef,
        revision: u64,
        index: u8,
        key: crossterm_input::KeyEvent,
        first_cells: &mut u16,
    ) {
        let Some(info) = self
            .local
            .as_ref()
            .and_then(|layout| layout.divider_info(target, revision, index).ok())
        else {
            return;
        };
        let delta = match (info.orientation, key.code) {
            (DividerOrientation::Horizontal, KeyCode::Left)
            | (DividerOrientation::Vertical, KeyCode::Up) => -1_i16,
            (DividerOrientation::Horizontal, KeyCode::Right)
            | (DividerOrientation::Vertical, KeyCode::Down) => 1_i16,
            (_, KeyCode::Char('h' | 'k')) if key.modifiers.is_empty() => -1,
            (_, KeyCode::Char('l' | 'j')) if key.modifiers.is_empty() => 1,
            _ => return,
        };
        let candidate = if delta < 0 {
            first_cells.saturating_sub(1)
        } else {
            first_cells.saturating_add(1)
        };
        if let Some(candidate) = self.clamp_divider_span(target, revision, index, candidate) {
            *first_cells = candidate;
        }
    }

    fn handle_header_event(&mut self, event: &Event, now: Instant) -> bool {
        if self.workspace.view().workspace_index.is_some() {
            let (rows, columns) = self.screen_viewport();
            if rows == 0 || columns == 0 {
                return matches!(event, Event::Mouse(_));
            }
        }
        let Some(layout) = self.tabs_layout() else {
            return false;
        };
        let Event::Mouse(mouse) = event else {
            return false;
        };
        if mouse.row != 0 {
            return false;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
        ) {
            self.cancel_selection();
            match (mouse.kind, layout.hit_test(mouse.column, mouse.row)) {
                (MouseEventKind::Down(MouseButton::Right), Some(TabAction::Select(route))) => {
                    self.open_tab_actions(route);
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::ProjectChooser)) => {
                    self.open_workspace_picker();
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::Select(route)))
                    if self.workspace.view().current_route == Some(route) =>
                {
                    self.open_tab_actions(route);
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::Select(route))) => {
                    self.request_route(route, now)
                }
                (_, Some(TabAction::TabActions(route))) => self.open_tab_actions(route),
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::Create)) => {
                    let project = self
                        .workspace
                        .view()
                        .current_route
                        .map(|route| route.project)
                        .unwrap_or_else(|| {
                            ProjectId::new(1).expect("Default project ID is nonzero")
                        });
                    self.request_tab_create(project, now);
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::AllTabs)) => {
                    self.open_workspace_picker();
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::PreviousTab)) => {
                    self.navigate_workspace(TabNavigation::PreviousTab, now)
                }
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::NextTab)) => {
                    self.navigate_workspace(TabNavigation::NextTab, now)
                }
                _ => {}
            }
        }
        // Every event on row zero is local, including blank strip cells.
        true
    }

    fn padded_tab_actions(&self) -> bool {
        self.workspace.padded_tab_actions(self.viewport())
    }

    fn tab_dialog_geometry(&self) -> Option<Rect> {
        self.workspace.tab_dialog_geometry(self.viewport())
    }

    fn handle_tab_dialog_event(&mut self, event: &Event, now: Instant) {
        let viewport = self.viewport();
        let generation = self.layout.and_then(|layout| {
            layout
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| pane.target.generation)
        });
        self.workspace
            .handle_tab_dialog_event(event, now, viewport, generation);
        self.apply_workspace_feedback();
    }

    fn handle_workspace_picker_event(&mut self, event: &Event, now: Instant) {
        let viewport = self.viewport();
        self.workspace
            .handle_workspace_picker_event(event, now, viewport);
        self.apply_workspace_feedback();
    }

    fn picker_routes(&self) -> Vec<WorkspaceRouteId> {
        self.workspace.picker_routes()
    }

    fn picker_horizontal_bounds(&self) -> std::ops::Range<u16> {
        self.workspace.picker_horizontal_bounds(self.viewport())
    }

    #[cfg(test)]
    fn picker_clear_rect(&self) -> Option<Rect> {
        self.workspace.picker_clear_rect(self.viewport())
    }

    #[cfg(test)]
    fn picker_route_at_row(&self, row: u16) -> Option<WorkspaceRouteId> {
        self.workspace.picker_route_at_row(row, self.viewport())
    }

    fn picker_screen_geometry(&self) -> Option<(u16, u16)> {
        self.workspace.picker_screen_geometry(self.viewport())
    }

    #[cfg(test)]
    fn picker_visible_route_count(&self) -> usize {
        self.workspace.picker_visible_route_count(self.viewport())
    }

    fn picker_scroll_offset(&self) -> Option<usize> {
        self.workspace.picker_scroll_offset(self.viewport())
    }

    /// Split the focused pane directly in `direction` (same checks and
    /// captured-revision rules as the menu).
    fn split_focused(
        &mut self,
        direction: SplitDirection,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        if self.pending_pane_operation.is_some() {
            self.set_status("pane operation pending", now);
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before splitting panes", now);
            return;
        }
        let (Some(local), Some(layout), Some(focus), true) =
            (self.local.as_ref(), self.layout, self.focus, self.fits())
        else {
            self.set_status("split unavailable: terminal too small", now);
            return;
        };
        if local.can_split(focus, direction).is_err() {
            let label = if layout.panes.iter().flatten().count() >= crate::pane_layout::MAX_PANES {
                "split unavailable: pane limit reached (4)"
            } else {
                "split unavailable: pane too small for that direction"
            };
            self.set_status(label, now);
            return;
        }
        self.menu_revision = layout.revision;
        self.activate(
            SplitIntent {
                target: focus,
                expected_layout_revision: layout.revision,
                direction,
            },
            now,
            effects,
        );
    }

    /// Open the close prompt for the focused pane, capturing its target and
    /// the current server layout revision now.
    fn confirm_close_focused(&mut self, now: Instant) {
        let Some(layout) = self.layout else {
            self.set_status("close unavailable: no layout", now);
            return;
        };
        let Some(focus) = self.focus else {
            self.set_status("close unavailable: no focused pane", now);
            return;
        };
        self.confirm_close_target(focus, layout.revision, now);
    }

    fn confirm_close_target(&mut self, target: TargetRef, revision: u64, now: Instant) {
        if !self.server_present {
            self.set_status("Disconnected", now);
            return;
        }
        if self.pending_pane_operation.is_some() {
            self.set_status("pane operation pending", now);
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before closing a pane", now);
            return;
        }
        self.cancel_selection();
        let Some(layout) = self.layout else {
            self.set_status("close unavailable: no layout", now);
            return;
        };
        if layout.revision != revision {
            self.set_status("close unavailable: layout changed", now);
            return;
        }
        let Some(index) = layout
            .panes
            .iter()
            .position(|pane| pane.is_some_and(|pane| pane.target == target))
        else {
            self.set_status("close unavailable: pane changed", now);
            return;
        };
        self.open_close_confirmation(CloseIntent {
            target,
            expected_layout_revision: revision,
            position: u8::try_from(index + 1).unwrap_or(u8::MAX),
            last_pane: layout.panes.iter().flatten().count() <= 1,
        });
    }

    fn open_close_confirmation(&mut self, intent: CloseIntent) {
        self.confirm.open(intent);
        self.confirm_close_selected = false;
        self.confirm_open_key = None;
        self.dirty = true;
    }

    fn dismiss_close_confirmation(&mut self) {
        self.confirm = CloseConfirm::new();
        self.confirm_close_selected = false;
        self.confirm_open_key = None;
        self.dirty = true;
    }

    /// Send one confirmed close (one fresh key; never retried) and report
    /// the server's result. After a confirmed stop, local focus and batched
    /// keys leave the closed pane at once; the authoritative collapsed layout
    /// arrives with the next layout poll.
    fn close_pane(&mut self, intent: CloseIntent, now: Instant, effects: &mut impl Effects) {
        if !self.server_present {
            self.set_status("Disconnected", now);
            return;
        }
        let captured_target_is_current = self.layout.is_some_and(|layout| {
            layout.revision == intent.expected_layout_revision
                && layout
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == intent.target)
        });
        if !captured_target_is_current {
            self.set_status("close cancelled: layout changed", now);
            return;
        }
        if self.pending_pane_operation.is_some() {
            self.set_status("pane operation pending", now);
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before closing a pane", now);
            return;
        }
        self.flush(now, effects);
        if self.pending_len != 0 {
            self.set_status("busy: queued keys must be sent first", now);
            return;
        }
        let plan = ClosePlan {
            target: intent.target,
            expected_layout_revision: intent.expected_layout_revision,
        };
        if self.pending_close.is_some() {
            self.set_status("close already pending", now);
            return;
        }
        let request_id = self.next_request_id();
        self.pending_close = Some((request_id, intent.target, intent.last_pane));
        match effects.close(plan, request_id) {
            Ok(Some(result)) => self.complete_close(request_id, intent.target, Ok(result), now),
            Ok(None) => self.set_persistent_status("Applying…", now),
            Err(label) => {
                self.pending_close = None;
                if is_uncertain_mutation_error(label) {
                    self.set_persistent_status("Outcome unknown", now);
                }
                self.set_status(label, now)
            }
        }
    }

    /// Stop routing anything to a pane that the server confirmed stopped.
    fn forget_pane(&mut self, target: TargetRef) {
        if self.pending_target == Some(target) {
            self.pending_len = 0;
            self.pending_target = None;
            self.pending_blocked = false;
        }
        if self.focus == Some(target) {
            let focus = self
                .layout
                .and_then(|layout| {
                    layout
                        .panes
                        .into_iter()
                        .flatten()
                        .find(|pane| pane.target != target)
                })
                .map(|pane| pane.target)
                .or(Some(target));
            self.set_focus(focus);
        }
        self.dirty = true;
    }

    /// Move local focus between panes (presentation only).
    fn move_focus(&mut self, movement: FocusMove) {
        let Some(layout) = self.display_layout() else {
            return;
        };
        let panes: Vec<_> = layout.panes.iter().flatten().copied().collect();
        if panes.len() < 2 {
            return;
        }
        let Some(current) = panes
            .iter()
            .position(|pane| Some(pane.target) == self.focus)
        else {
            self.set_focus(Some(panes[0].target));
            return;
        };
        let here = panes[current].rect;
        let center = |rect: Rect| {
            (
                u32::from(rect.x) * 2 + u32::from(rect.columns),
                u32::from(rect.y) * 2 + u32::from(rect.rows),
            )
        };
        let origin = center(here);
        let candidate = panes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != current)
            .filter_map(|(index, pane)| {
                let point = center(pane.rect);
                let (primary, secondary) = match movement {
                    FocusMove::Left if point.0 < origin.0 => {
                        (origin.0 - point.0, origin.1.abs_diff(point.1))
                    }
                    FocusMove::Right if point.0 > origin.0 => {
                        (point.0 - origin.0, origin.1.abs_diff(point.1))
                    }
                    FocusMove::Up if point.1 < origin.1 => {
                        (origin.1 - point.1, origin.0.abs_diff(point.0))
                    }
                    FocusMove::Down if point.1 > origin.1 => {
                        (point.1 - origin.1, origin.0.abs_diff(point.0))
                    }
                    FocusMove::Next => (index as u32, 0),
                    _ => return None,
                };
                Some((primary, secondary, index))
            })
            .min();
        if let Some((_, _, index)) = candidate {
            self.set_focus(Some(panes[index].target));
        }
    }

    /// Send any batched keys to the pane they were typed into.
    pub(crate) fn flush(&mut self, now: Instant, effects: &mut impl Effects) {
        if self.workspace.view().route_switching {
            self.abandon_pending_keys();
            return;
        }
        if self.pending_len == 0 {
            return;
        }
        let count = self.pending_len;
        if let Some(target) = self.pending_target {
            match effects.send_keys(target, &self.pending[..count]) {
                Ok(()) => {
                    self.pending_len = 0;
                    self.pending_target = None;
                    self.pending_blocked = false;
                }
                Err(label) => {
                    self.pending_blocked = true;
                    self.set_status(label, now);
                }
            }
        }
    }

    fn push_key(&mut self, key: Key, now: Instant, effects: &mut impl Effects) {
        let Some(focus) = self.focus else {
            self.set_status("no focused pane", now);
            return;
        };
        self.return_to_live(focus);
        if self.pending_target != Some(focus) || self.pending_len == MAX_KEYS {
            self.flush(now, effects);
            if self.pending_len != 0 {
                self.set_status("busy: queued keys retained for their pane", now);
                return;
            }
            self.pending_target = Some(focus);
            self.pending_blocked = false;
        }
        self.pending[self.pending_len] = key;
        self.pending_len += 1;
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<TargetRef> {
        if !self.fits() {
            return None;
        }
        let y = self.pane_row(y)?;
        self.display_layout()?
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.rect.contains(x, y))
            .map(|pane| pane.target)
    }

    fn scrollbar_hit_at(&self, x: u16, y: u16) -> Option<(TargetRef, Rect, Rect, u16, u16, u16)> {
        if !self.fits() {
            return None;
        }
        let header_offset = u16::from(self.workspace.view().workspace_index.is_some());
        let layout = self.display_layout()?;
        layout.panes.iter().flatten().find_map(|pane| {
            let slot = self
                .screens
                .iter()
                .find(|slot| slot.target == Some(pane.target))?;
            let content = panelink::pane_layout::content_rect(pane.rect)?;
            let pending = if let Some(view) = self.pane_operations {
                let index = view.layout.panes.iter().position(|candidate| {
                    candidate.is_some_and(|candidate| candidate.target == pane.target)
                })?;
                view.confirmed_sizes[index]
                    .is_some_and(|size| size != (content.rows, content.columns))
            } else {
                pane.pending_confirmed.is_some()
            };
            if pending
                || slot.snapshot.alternate_screen()
                || slot.snapshot.rows() != content.rows
                || slot.snapshot.columns() != content.columns
            {
                return None;
            }
            let rect = shifted_rect(pane.rect, header_offset);
            let bar = Scrollbar {
                offset_rows: slot.history_offset,
                total_rows: slot.history_total.max(slot.history_offset),
            };
            let geometry = scrollbar_geometry(rect, bar)?;
            geometry.track.contains(x, y).then_some((
                pane.target,
                pane.rect,
                geometry.track,
                geometry.thumb.rows,
                slot.history_offset,
                bar.total_rows,
            ))
        })
    }

    fn history_back_at(&self, x: u16, y: u16) -> Option<TargetRef> {
        if !self.fits() {
            return None;
        }
        let (host_rows, host_columns) = self.screen_viewport();
        let header_offset = u16::from(self.workspace.view().workspace_index.is_some());
        self.display_layout()?
            .panes
            .iter()
            .flatten()
            .find_map(|pane| {
                let slot = self
                    .screens
                    .iter()
                    .find(|slot| slot.target == Some(pane.target))?;
                let label = history_label_kind(slot)?;
                let rect = shifted_rect(pane.rect, header_offset);
                let badge = pane_version_badge(rect, host_rows, host_columns);
                pane_history_controls(rect, label, badge)
                    .filter(|controls| controls.back.contains(x, y))
                    .map(|_| pane.target)
            })
    }

    fn begin_scrollbar_pointer(
        &mut self,
        x: u16,
        y: u16,
        now: Instant,
        effects: &mut impl Effects,
    ) -> bool {
        let Some((target, pane_rect, track, thumb_rows, offset, total)) =
            self.scrollbar_hit_at(x, y)
        else {
            return false;
        };
        self.cancel_selection();
        self.flush(now, effects);
        self.prefix = None;
        self.deferred_thumb_release = None;
        let layout_revision = self.layout.map_or(0, |layout| layout.revision);
        let (extent_known, current_offset, known_total) = self
            .screens
            .iter()
            .find(|slot| slot.target == Some(target))
            .map_or((false, 0, total), |slot| {
                (
                    slot.history_extent_known,
                    slot.history_offset,
                    slot.history_total.max(slot.history_offset),
                )
            });
        let rect = self.display_layout().and_then(|layout| {
            layout
                .panes
                .iter()
                .flatten()
                .find(|pane| pane.target == target)
                .map(|pane| {
                    shifted_rect(
                        pane.rect,
                        u16::from(self.workspace.view().workspace_index.is_some()),
                    )
                })
        });
        let Some(rect) = rect else { return true };

        if !extent_known {
            // Before extent discovery, use the protocol's bounded maximum only
            // for the local thumb mapping. The read itself stays at the current
            // offset and cannot jump the frozen view to the oldest row.
            let conservative_total = panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS;
            let Some(geometry) = scrollbar_geometry(
                rect,
                Scrollbar {
                    offset_rows: current_offset.min(conservative_total),
                    total_rows: conservative_total,
                },
            ) else {
                return true;
            };
            let grab_offset = if geometry.thumb.contains(x, y) {
                y.saturating_sub(geometry.thumb.y)
            } else {
                0
            };
            self.pointer_seek = Some(PointerSeek {
                target,
                layout_revision,
                pane_rect,
                track: geometry.track,
                thumb_rows: geometry.thumb.rows,
                total_rows: conservative_total,
                start_offset: current_offset,
                preview_offset: current_offset,
                grab_offset,
                last_y: y,
                awaiting_extent: true,
            });
            self.discover_history_extent_for_target(target, now, effects);
            self.dirty = true;
            return true;
        }
        let Some(geometry) = scrollbar_geometry(
            rect,
            Scrollbar {
                offset_rows: offset,
                total_rows: known_total,
            },
        ) else {
            return true;
        };
        if geometry.thumb.contains(x, y) {
            self.pointer_seek = Some(PointerSeek {
                target,
                layout_revision,
                pane_rect,
                track,
                thumb_rows,
                total_rows: known_total,
                start_offset: offset,
                preview_offset: offset,
                grab_offset: y.saturating_sub(geometry.thumb.y),
                last_y: y,
                awaiting_extent: false,
            });
        } else {
            let page = panelink::pane_layout::content_rect(pane_rect)
                .map_or(1, |content| content.rows.saturating_sub(1).max(1));
            let requested = if y < geometry.thumb.y {
                offset.saturating_add(page).min(total)
            } else {
                offset.saturating_sub(page)
            };
            self.seek_history_for_target(target, now, requested, effects);
        }
        self.dirty = true;
        true
    }

    fn handle_pointer_seek_event(
        &mut self,
        event: &Event,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        if let Some(capture) = self.pointer_divider {
            match event {
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Drag(MouseButton::Left) => {
                    self.update_pointer_divider(mouse.column, mouse.row);
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Up(MouseButton::Left) => {
                    self.update_pointer_divider(mouse.column, mouse.row);
                    let mode = self.pane_mode.take();
                    self.pointer_divider = None;
                    if let Some(PaneMode::Divider {
                        first_cells,
                        revision,
                        ..
                    }) = mode
                    {
                        if revision == capture.revision
                            && first_cells != capture.initial_span
                            && self.layout.is_some_and(|layout| {
                                layout.revision == capture.revision
                                    && layout
                                        .panes
                                        .iter()
                                        .flatten()
                                        .any(|pane| pane.target == capture.target)
                            })
                            && self.divider_value_valid(
                                capture.target,
                                capture.revision,
                                capture.divider_index,
                                first_cells,
                            )
                        {
                            self.submit_pane_operation(
                                PaneAction::DividerSet {
                                    target: capture.target,
                                    expected_layout_revision: capture.revision,
                                    divider_index: capture.divider_index,
                                    first_cells,
                                },
                                now,
                                effects,
                            );
                        }
                    }
                    self.dirty = true;
                }
                Event::Key(key) if key.code == KeyCode::Esc && key.is_press() => {
                    self.pointer_divider = None;
                    self.pane_mode = None;
                    self.dirty = true;
                }
                Event::Resize(_, _) | Event::FocusLost => {
                    self.pointer_divider = None;
                    self.pane_mode = None;
                    self.dirty = true;
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Drag(MouseButton::Left) => {
                self.update_pointer_seek(mouse.row, now, effects);
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Up(MouseButton::Left) => {
                self.update_pointer_seek(mouse.row, now, effects);
                let drag = self.pointer_seek.take();
                if let Some(drag) = drag.filter(|drag| drag.awaiting_extent) {
                    // Extent discovery is still in flight: apply this release
                    // when it returns instead of silently dropping the gesture.
                    self.deferred_thumb_release = Some(drag);
                } else if let Some(drag) = drag {
                    if !drag.awaiting_extent
                        && self.latest_history_request(drag.target) != Some(drag.preview_offset)
                        && self
                            .layout
                            .is_some_and(|layout| layout.revision == drag.layout_revision)
                    {
                        self.seek_history_for_target(
                            drag.target,
                            now,
                            drag.preview_offset,
                            effects,
                        );
                    }
                }
                self.dirty = true;
            }
            Event::Key(key) if key.code == KeyCode::Esc && key.is_press() => {
                if let Some(drag) = self.pointer_seek.take() {
                    self.seek_history_for_target(drag.target, now, drag.start_offset, effects);
                }
                self.dirty = true;
            }
            Event::Resize(_, _) | Event::FocusLost => {
                self.cancel_pointer_seek();
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn update_pointer_seek(&mut self, y: u16, now: Instant, effects: &mut impl Effects) {
        let Some(mut drag) = self.pointer_seek else {
            return;
        };
        drag.last_y = y;
        let previous = drag.preview_offset;
        drag.preview_offset = pointer_seek_offset(drag, y);

        self.pointer_seek = Some(drag);
        self.dirty = true;
        if !drag.awaiting_extent && previous != drag.preview_offset {
            self.seek_history_for_target(drag.target, now, drag.preview_offset, effects);
        }
    }

    fn latest_history_request(&self, target: TargetRef) -> Option<u16> {
        self.screens
            .iter()
            .find(|slot| slot.target == Some(target))
            .map(|slot| slot.history_requested)
    }

    fn cancel_pointer_seek(&mut self) {
        if let Some(drag) = self.pointer_seek.take() {
            self.screens.cancel_seek(drag.target);
        }
    }

    fn begin_divider_pointer(
        &mut self,
        x: u16,
        screen_y: u16,
        now: Instant,
        effects: &mut impl Effects,
    ) -> bool {
        let Some((target, revision, divider_index, info)) = self.divider_hit_at(x, screen_y) else {
            return false;
        };
        if !self.operation_ready(now, effects) {
            return true;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before adjusting dividers", now);
            return true;
        }
        let mut indices = [None; crate::pane_layout::MAX_PANES - 1];
        indices[0] = Some(divider_index);
        self.pointer_divider = Some(PointerDivider {
            target,
            revision,
            divider_index,
            initial_span: info.first_span,
            region: info.region,
            orientation: info.orientation,
        });
        self.pane_mode = Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected: 0,
            first_cells: info.first_span,
        });
        self.cancel_selection();
        self.prefix = None;
        self.dirty = true;
        true
    }

    fn divider_hit_at(
        &self,
        x: u16,
        screen_y: u16,
    ) -> Option<(TargetRef, u64, u8, panelink::pane_layout::DividerInfo)> {
        if !self.fits() {
            return None;
        }
        let layout = self.display_layout()?;
        let local = self.local.as_ref()?;
        let revision = self.layout?.revision;
        let root_y =
            screen_y.checked_sub(u16::from(self.workspace.view().workspace_index.is_some()))?;
        let (index, _) = divider_at(layout.dividers, x, root_y)?;
        for pane in layout.panes.iter().flatten() {
            if let Ok(info) = local.divider_info(pane.target, revision, index as u8) {
                return Some((pane.target, revision, index as u8, info));
            }
        }
        None
    }

    fn update_pointer_divider(&mut self, x: u16, screen_y: u16) {
        let Some(capture) = self.pointer_divider else {
            return;
        };
        let Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected,
            ..
        }) = self.pane_mode
        else {
            return;
        };
        if revision != capture.revision
            || self
                .layout
                .is_none_or(|layout| layout.revision != capture.revision)
        {
            self.pointer_divider = None;
            self.pane_mode = None;
            return;
        }
        let root_y =
            screen_y.saturating_sub(u16::from(self.workspace.view().workspace_index.is_some()));
        let requested = match capture.orientation {
            DividerOrientation::Horizontal => x.saturating_sub(capture.region.x),
            DividerOrientation::Vertical => root_y.saturating_sub(capture.region.y),
        };
        if let Some(candidate) = self.clamp_divider_span(
            capture.target,
            capture.revision,
            capture.divider_index,
            requested,
        ) {
            self.pane_mode = Some(PaneMode::Divider {
                target,
                revision,
                indices,
                selected,
                first_cells: candidate,
            });
            self.dirty = true;
        }
    }

    fn clamp_divider_span(
        &self,
        target: TargetRef,
        revision: u64,
        index: u8,
        requested: u16,
    ) -> Option<u16> {
        let info = self
            .local
            .as_ref()?
            .divider_info(target, revision, index)
            .ok()?;
        let valid = |value| self.divider_value_valid(target, revision, index, value);
        if valid(requested) {
            return Some(requested);
        }
        // Subtree minimum extents bound a contiguous feasible interval. Search
        // from the known valid current span instead of rebuilding every span
        // in the root on each pointer-motion event.
        if requested < info.first_span {
            let (mut low, mut high) = (0, info.first_span);
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                if valid(middle) {
                    high = middle;
                } else {
                    low = middle;
                }
            }
            Some(high)
        } else {
            let (mut low, mut high) = (info.first_span, info.total_cells);
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                if valid(middle) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            Some(low)
        }
    }

    fn divider_value_valid(&self, target: TargetRef, revision: u64, index: u8, value: u16) -> bool {
        self.layout.is_some_and(|layout| {
            layout.revision == revision
                && layout
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == target)
        }) && self
            .layout
            .and_then(|layout| local_layout(&layout))
            .is_some_and(|mut layout| layout.set_divider(target, revision, index, value).is_ok())
    }

    fn pane_control_at(&self, x: u16, y: u16) -> Option<(TargetRef, bool)> {
        if !self.fits() {
            return None;
        }
        let header_offset = u16::from(self.workspace.view().workspace_index.is_some());
        self.display_layout()?
            .panes
            .iter()
            .flatten()
            .find_map(|pane| {
                let controls = pane_controls(pane.rect)?;
                let menu = shifted_rect(controls.menu, header_offset);
                let close = shifted_rect(controls.close, header_offset);
                if menu.contains(x, y) {
                    Some((pane.target, false))
                } else if close.contains(x, y) {
                    Some((pane.target, true))
                } else {
                    None
                }
            })
    }

    fn pane_row(&self, screen_row: u16) -> Option<u16> {
        if self.workspace.view().workspace_index.is_some() {
            screen_row.checked_sub(1)
        } else {
            Some(screen_row)
        }
    }

    fn visible_source(
        &self,
        target: TargetRef,
    ) -> Option<(usize, Rect, &ScreenSnapshot, bool, u64)> {
        if !self.fits() {
            return None;
        }
        let layout = self.display_layout()?;
        let pane = layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == target)?;
        let content = panelink::pane_layout::content_rect(pane.rect)?;
        let (slot_index, slot) = self
            .screens
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.target == Some(target))?;
        if pane.pending_confirmed.is_some() || slot.history_pending {
            return None;
        }
        if slot.history_offset > 0
            && slot.history_revision > 0
            && slot.history.rows() == content.rows
            && slot.history.columns() == content.columns
        {
            Some((slot_index, content, &slot.history, true, slot.view_epoch))
        } else if slot.snapshot.rows() == content.rows && slot.snapshot.columns() == content.columns
        {
            Some((slot_index, content, &slot.snapshot, false, slot.view_epoch))
        } else {
            None
        }
    }

    fn begin_selection(&mut self, target: TargetRef, x: u16, y: u16, now: Instant) {
        let Some((slot, content, snapshot, source_history, epoch)) = self.visible_source(target)
        else {
            self.set_status("selection unavailable", now);
            return;
        };
        if !content.contains(x, y) || snapshot.row_wraps().is_none() {
            self.set_status("selection unavailable", now);
            return;
        }
        let point = pointer_content_point(content, x, y);
        match capture(
            target,
            slot,
            epoch,
            content,
            source_history,
            snapshot,
            point,
        ) {
            Ok(drag) => {
                self.selection = Some(drag);
                self.dirty = true;
            }
            Err(SelectionError::TooLarge) => self.fail_copy_attempt(target, "Copy too large", now),
            Err(_) => self.set_status("Copy unavailable", now),
        }
    }

    fn selection_point(drag: &FrozenDrag, x: u16, y: u16) -> Point {
        pointer_content_point(drag.content_rect, x, y)
    }

    fn extend_selection(&mut self, x: u16, y: u16) {
        if let Some(drag) = self.selection.as_mut() {
            let point = Self::selection_point(drag, x, y);
            drag.end = point;
            drag.selection.extend(point);
            self.dirty = true;
        }
    }

    fn current_selection_identity(&self, drag: &FrozenDrag) -> Option<ViewIdentity> {
        let (slot, content, snapshot, source_history, epoch) = self.visible_source(drag.target)?;
        let identity = ViewIdentity {
            target: drag.target,
            view_id: slot as u64,
            epoch,
            rows: snapshot.rows(),
            columns: snapshot.columns(),
            buffer_id: u64::from(snapshot.alternate_screen()),
        };
        (slot == drag.slot
            && content == drag.content_rect
            && source_history == drag.source_history
            && identity == drag.identity)
            .then_some(identity)
    }

    fn finish_selection(&mut self, now: Instant) {
        let Some(drag) = self.selection.take() else {
            return;
        };
        self.dirty = true;
        let result = self
            .current_selection_identity(&drag)
            .ok_or(SelectionError::StaleView)
            .and_then(|identity| drag.selection.finish(identity));
        match result {
            Ok(text) => self.queue_copy_text(drag.target, text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => {
                self.fail_copy_attempt(drag.target, "Copy unavailable", now)
            }
            Err(SelectionError::TooLarge) => {
                self.fail_copy_attempt(drag.target, "Copy too large", now)
            }
            Err(_) => self.fail_copy_attempt(drag.target, "Copy cancelled", now),
        }
    }

    fn open_at_pointer(
        &mut self,
        x: u16,
        y: u16,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) {
        self.cancel_selection();
        let (Some(local), Some(layout), true) = (self.local.as_ref(), self.layout, self.fits())
        else {
            self.set_status("menu unavailable: terminal too small", now);
            return;
        };
        let (rows, columns) = self.viewport();
        if self
            .menu
            .open_at_pointer_with_recovery(local, x, y, rows, columns, recovery_availability)
            .is_ok()
        {
            self.menu.set_compact_reasons(true);
            self.menu_revision = layout.revision;
            if let Some(target) = self.menu.captured_target() {
                self.open_actions_menu(
                    target,
                    Some(MouseButton::Right),
                    recovery_availability,
                    now,
                    effects,
                );
            }
            self.dirty = true;
        }
    }

    #[cfg(test)]
    fn open_for_focused(&mut self, now: Instant, recovery_availability: Availability) {
        let (Some(layout), Some(target)) = (self.layout, self.focus) else {
            self.set_status("menu unavailable: terminal too small", now);
            return;
        };
        self.open_for_target(target, layout.revision, now, recovery_availability);
    }

    #[cfg(test)]
    fn open_for_target(
        &mut self,
        target: TargetRef,
        revision: u64,
        now: Instant,
        recovery_availability: Availability,
    ) {
        self.cancel_selection();
        let (Some(local), Some(layout), true) = (self.local.as_ref(), self.layout, self.fits())
        else {
            self.set_status("menu unavailable: terminal too small", now);
            return;
        };
        if layout.revision != revision
            || !layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            self.set_status("menu cancelled: layout changed", now);
            return;
        }
        let (rows, columns) = self.viewport();
        if self
            .menu
            .open_for_focused_with_recovery(local, target, rows, columns, recovery_availability)
            .is_ok()
        {
            self.menu.set_compact_reasons(true);
            self.menu_revision = revision;
            self.dirty = true;
        }
    }

    fn open_actions_menu(
        &mut self,
        target: TargetRef,
        opening_release: Option<MouseButton>,
        recovery_availability: Availability,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let Some(layout) = self.layout else {
            self.set_status("pane actions unavailable: no layout", now);
            return;
        };
        if !self.fits()
            || !layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            self.set_status("pane actions unavailable: layout changed", now);
            return;
        }
        self.flush(now, effects);
        self.cancel_selection();
        self.prefix = None;
        let Some(model) = self.pane_action_model(target, layout.revision, recovery_availability)
        else {
            self.set_status("pane actions unavailable: layout changed", now);
            return;
        };
        self.actions_menu = Some(ActionsMenu {
            target,
            revision: layout.revision,
            selected: 0,
            model,
        });
        self.menu_release_latch = opening_release;
        self.dirty = true;
    }

    fn pane_action_model(
        &self,
        target: TargetRef,
        revision: u64,
        saved_layout: Availability,
    ) -> Option<PaneActionModel> {
        let local = self.local.as_ref()?;
        let pending = self.pending_pane_operation.is_some()
            || self.pending_split.is_some()
            || self.pending_close.is_some();
        let queued_input = self.pending_len != 0;
        let pane_ops_available = self.pane_operations.is_some();
        let zoomed_target = self.pane_operations.and_then(|view| view.zoomed_target);
        let zoomed = zoomed_target == Some(target);
        let zoom_conflict = zoomed_target.is_some_and(|zoomed| zoomed != target);
        let has_divider = (0..crate::pane_layout::MAX_PANES - 1)
            .any(|index| local.divider_info(target, revision, index as u8).is_ok());
        let has_other_pane = self.layout.is_some_and(|layout| {
            layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target != target)
        });
        let copy_available =
            self.visible_source(target)
                .is_some_and(|(_, content, snapshot, _, _)| {
                    content.rows > 0 && content.columns > 0 && snapshot.row_wraps().is_some()
                });
        let showing_history = self
            .screens
            .iter()
            .find(|slot| slot.target == Some(target))
            .is_some_and(|slot| slot.history_offset > 0);
        let available = Availability::Enabled;
        let pending_reason =
            Availability::Disabled(panelink::split_menu::DisabledReason::OperationPending);
        let queued_reason =
            Availability::Disabled(panelink::split_menu::DisabledReason::QueuedInput);
        let server_reason = Availability::Disabled(if self.server_present {
            panelink::split_menu::DisabledReason::ServerIncompatible
        } else {
            panelink::split_menu::DisabledReason::Disconnected
        });
        let zoom_reason = Availability::Disabled(panelink::split_menu::DisabledReason::ZoomActive);
        let resize = if !self.server_present || !pane_ops_available {
            server_reason
        } else if pending {
            pending_reason
        } else if queued_input {
            queued_reason
        } else if zoomed_target.is_some() {
            zoom_reason
        } else if !has_divider {
            Availability::Disabled(panelink::split_menu::DisabledReason::NoDivider)
        } else {
            available
        };
        let copy = if copy_available {
            available
        } else {
            Availability::Disabled(panelink::split_menu::DisabledReason::CopyUnavailable)
        };
        let back_to_live = if showing_history {
            available
        } else {
            Availability::Disabled(panelink::split_menu::DisabledReason::PaneUnavailable)
        };
        let close = if !self.server_present {
            server_reason
        } else if pending {
            pending_reason
        } else if queued_input {
            queued_reason
        } else if zoomed_target.is_some() {
            zoom_reason
        } else {
            available
        };
        let zoom = if !self.server_present || !pane_ops_available {
            server_reason
        } else if pending {
            pending_reason
        } else if queued_input {
            queued_reason
        } else if zoom_conflict {
            zoom_reason
        } else {
            available
        };
        let swap = if !self.server_present || !pane_ops_available {
            server_reason
        } else if pending {
            pending_reason
        } else if queued_input {
            queued_reason
        } else if zoomed_target.is_some() {
            zoom_reason
        } else if !has_other_pane {
            Availability::Disabled(panelink::split_menu::DisabledReason::NoOtherPane)
        } else {
            available
        };
        let split = if !self.server_present {
            server_reason
        } else if pending {
            pending_reason
        } else if queued_input {
            queued_reason
        } else {
            available
        };
        PaneActionModel::capture(
            local,
            target,
            revision,
            PaneActionAvailability {
                split,
                resize,
                copy,
                back_to_live,
                close,
                zoom,
                swap,
                saved_layout,
                help: available,
                detach: available,
                showing_history,
                zoomed,
            },
        )
        .ok()
    }

    fn actions_menu_rect(&self) -> Option<Rect> {
        let (root_rows, root_columns) = self.viewport();
        let rows = root_rows.min(panelink::split_menu::MAX_OVERLAY_ROWS as u16);
        let columns = root_columns.min(42);
        (rows > 0 && columns > 0).then_some(Rect {
            x: root_columns.saturating_sub(columns) / 2,
            y: root_rows.saturating_sub(rows) / 2,
            rows,
            columns,
        })
    }

    fn local_action_item(
        &self,
        menu: ActionsMenu,
        index: usize,
    ) -> Option<(PaneMenuAction, &'static str, Availability)> {
        LOCAL_ACTIONS.get(index).copied()?;
        let item = menu.model.item(index)?;
        let label = if item.action == PaneMenuAction::ClosePane {
            "--- Close pane"
        } else {
            item.label
        };
        Some((item.action, label, item.availability))
    }

    /// Leave room for a title, page navigation and Cancel even at minimum
    /// pane height. Keyboard selection and pointer pages use this same slice.
    #[cfg(test)]
    fn actions_page(&self, menu: ActionsMenu) -> (usize, usize) {
        self.actions_page_geometry(menu)
            .map_or((0, 0), |page| (page.start, page.end))
    }

    fn actions_page_geometry(&self, menu: ActionsMenu) -> Option<ActionsMenuPage> {
        let rect = self.actions_menu_rect()?;
        let capacity = usize::from(rect.rows.saturating_sub(3)).max(1);
        let last_action = LOCAL_ACTIONS.len() - 1;
        let start = menu.selected.min(last_action) / capacity * capacity;
        let end = (start + capacity).min(last_action + 1);
        let reason_row = 1 + end - start;
        let navigation_row = reason_row + 1;
        let back = start > 0;
        let more = end < LOCAL_ACTIONS.len();
        let (back_column, more_column, cancel_column) = if menu.selected == LOCAL_ACTIONS.len() {
            (None, None, 0)
        } else {
            match (back, more) {
                (true, true) => (Some(0), Some(7), 14),
                (true, false) => (Some(0), None, 7),
                (false, true) => (None, Some(0), 7),
                (false, false) => (None, None, 0),
            }
        };
        let cancel_row = navigation_row;
        Some(ActionsMenuPage {
            rect,
            start,
            end,
            action_row: 1,
            reason_row,
            navigation_row,
            cancel_row,
            line_count: cancel_row + 1,
            back_column,
            more_column,
            cancel_column,
        })
    }

    fn handle_actions_menu_event(
        &mut self,
        event: &Event,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let Some(mut menu) = self.actions_menu else {
            return Flow::Continue;
        };
        match event {
            Event::Key(key) if key.is_press() || key.is_repeat() => {
                if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                    self.actions_menu = None;
                    self.menu.close();
                } else if key.code == KeyCode::Up {
                    menu.selected =
                        (menu.selected + LOCAL_ACTIONS.len()) % (LOCAL_ACTIONS.len() + 1);
                    self.actions_menu = Some(menu);
                } else if key.code == KeyCode::Down {
                    menu.selected = (menu.selected + 1) % (LOCAL_ACTIONS.len() + 1);
                    self.actions_menu = Some(menu);
                } else if key.code == KeyCode::Enter {
                    return self.activate_local_action(menu, now, recovery_availability, effects);
                } else if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && matches!(
                        key.code,
                        KeyCode::Char('l' | 'L' | 'r' | 'R' | 'u' | 'U' | 'd' | 'D')
                    )
                {
                    let direction = match key.code {
                        KeyCode::Char('l' | 'L') => SplitDirection::Left,
                        KeyCode::Char('r' | 'R') => SplitDirection::Right,
                        KeyCode::Char('u' | 'U') => SplitDirection::Up,
                        KeyCode::Char('d' | 'D') => SplitDirection::Down,
                        _ => unreachable!("guarded split direction"),
                    };
                    let split_index = match direction {
                        SplitDirection::Left => 0,
                        SplitDirection::Right => 1,
                        SplitDirection::Up => 2,
                        SplitDirection::Down => 3,
                    };
                    if menu.model.items()[split_index].availability == Availability::Enabled {
                        self.actions_menu = None;
                        self.menu.close();
                        self.activate(
                            SplitIntent {
                                target: menu.target,
                                expected_layout_revision: menu.revision,
                                direction,
                            },
                            now,
                            effects,
                        );
                        return Flow::Continue;
                    }
                    menu.selected = split_index;
                    self.actions_menu = Some(menu);
                    self.set_status("split unavailable; see the menu reason", now);
                } else if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::Char('v' | 'V'))
                {
                    if recovery_availability == Availability::Enabled {
                        self.actions_menu = None;
                        self.menu.close();
                        return Flow::ViewRecovery;
                    }
                    self.set_status("No previous layout saved", now);
                }
                self.dirty = true;
            }
            Event::Mouse(mouse)
                if matches!(
                    mouse.kind,
                    MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
                ) =>
            {
                if let Some(page) = self.actions_page_geometry(menu) {
                    let rect = page.rect;
                    let offset = u16::from(self.workspace.view().workspace_index.is_some());
                    let inside = mouse.column >= rect.x
                        && mouse.column < rect.x.saturating_add(rect.columns)
                        && mouse.row >= rect.y.saturating_add(offset)
                        && mouse.row < rect.y.saturating_add(rect.rows + offset);
                    if !inside {
                        self.actions_menu = None;
                        self.menu.close();
                        self.dirty = true;
                        return Flow::Continue;
                    }
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                        && mouse.row >= rect.y.saturating_add(1 + offset)
                    {
                        let row = usize::from(mouse.row - rect.y - offset);
                        if (page.action_row..page.reason_row).contains(&row) {
                            menu.selected = page.start + row - page.action_row;
                            return self.activate_local_action(
                                menu,
                                now,
                                recovery_availability,
                                effects,
                            );
                        } else if row == page.navigation_row {
                            let column = mouse.column.saturating_sub(rect.x);
                            if column >= page.cancel_column
                                && column < page.cancel_column.saturating_add(8)
                            {
                                self.actions_menu = None;
                                self.menu.close();
                                self.dirty = true;
                            } else if page.back_column.is_some_and(|start| {
                                column >= start && column < start.saturating_add(6)
                            }) {
                                menu.selected = page.start.saturating_sub(1);
                                self.actions_menu = Some(menu);
                                self.dirty = true;
                            } else if page.more_column.is_some_and(|start| {
                                column >= start && column < start.saturating_add(6)
                            }) {
                                menu.selected = page.end;
                                self.actions_menu = Some(menu);
                                self.dirty = true;
                            }
                        }
                    }
                }
                // The menu owns every pointer event, including outside clicks.
            }
            _ => {}
        }
        Flow::Continue
    }

    fn activate_local_action(
        &mut self,
        menu: ActionsMenu,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let Some(layout) = self.layout else {
            self.actions_menu = None;
            self.menu.close();
            return Flow::Continue;
        };
        if layout.revision != menu.revision
            || !layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == menu.target)
        {
            self.actions_menu = None;
            self.menu.close();
            self.set_status("pane actions cancelled: layout changed", now);
            return Flow::Continue;
        }
        let Some(action) = LOCAL_ACTIONS.get(menu.selected).copied() else {
            self.actions_menu = None;
            self.menu.close();
            self.dirty = true;
            return Flow::Continue;
        };
        if menu.selected < LOCAL_ACTIONS.len()
            && self
                .local_action_item(menu, menu.selected)
                .is_some_and(|(_, _, availability)| availability != Availability::Enabled)
        {
            self.actions_menu = Some(menu);
            self.set_status(
                if action == LocalAction::SavedLayout {
                    "No previous layout saved"
                } else {
                    "action unavailable; see the menu reason"
                },
                now,
            );
            return Flow::Continue;
        }
        self.actions_menu = None;
        if !matches!(action, LocalAction::Split(_)) {
            self.menu.close();
        }
        match action {
            LocalAction::Split(direction) => self.activate(
                SplitIntent {
                    target: menu.target,
                    expected_layout_revision: menu.revision,
                    direction,
                },
                now,
                effects,
            ),
            LocalAction::Zoom => {
                self.toggle_zoom_for_target(menu.target, menu.revision, now, effects)
            }
            LocalAction::Swap => {
                self.open_swap_mode_for_target(menu.target, menu.revision, now, effects)
            }
            LocalAction::Divider => {
                self.open_divider_mode_for_target(menu.target, menu.revision, now, effects)
            }
            LocalAction::Copy => match menu.model.item(5).map(|item| item.action) {
                Some(PaneMenuAction::BackToLive) => self.return_to_live(menu.target),
                _ => self.begin_copy_mode_for_target(menu.target, now),
            },
            LocalAction::SavedLayout => {
                if recovery_availability == Availability::Enabled {
                    return Flow::ViewRecovery;
                }
                self.set_status("No previous layout saved", now);
            }
            LocalAction::Help => self.help_open = true,
            LocalAction::Close => self.confirm_close_target(menu.target, menu.revision, now),
            LocalAction::Detach => {
                self.flush(now, effects);
                return Flow::Quit;
            }
        }
        self.dirty = true;
        Flow::Continue
    }

    /// Turn a captured intent into one split. The local `PaneLayout` mirror
    /// always starts at revision 1, so the server revision captured together
    /// with the menu (`menu_revision`) replaces the mirror's revision.
    fn activate(&mut self, intent: SplitIntent, now: Instant, effects: &mut impl Effects) {
        if !self.server_present {
            self.set_status("Disconnected", now);
            return;
        }
        if self.pending_pane_operation.is_some() {
            self.set_status("pane operation pending", now);
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before splitting panes", now);
            return;
        }
        self.flush(now, effects);
        if self.pending_len != 0 {
            self.set_status("busy: queued keys must be sent first", now);
            return;
        }
        if self.pending_split.is_some() {
            self.set_status("split already pending", now);
            return;
        }
        let Some(layout) = self.layout else {
            self.set_status("split unavailable: no layout", now);
            return;
        };
        if layout.revision != intent.expected_layout_revision
            || !layout
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == intent.target)
        {
            self.set_status("split unavailable: layout changed; reopen the menu", now);
            return;
        }
        let Ok((rows, columns)) = planned_split_size(&layout, intent.target, intent.direction)
        else {
            self.set_status("split unavailable: layout changed; reopen the menu", now);
            return;
        };
        let plan = SplitPlan {
            target: intent.target,
            expected_layout_revision: intent.expected_layout_revision,
            direction: intent.direction,
            rows,
            columns,
        };
        let request_id = self.next_request_id();
        let focus_generation = self.focus_generation;
        let route = self.workspace.view().current_route;
        self.pending_split = Some((request_id, focus_generation, plan.target, route));
        match effects.split(plan, request_id, focus_generation) {
            Ok(Some(new_target)) => {
                self.complete_split(
                    request_id,
                    focus_generation,
                    plan.target,
                    Ok(new_target),
                    now,
                );
            }
            Ok(None) => self.set_persistent_status("Applying…", now),
            Err(label) => {
                self.pending_split = None;
                if is_uncertain_mutation_error(label) {
                    self.set_persistent_status("Outcome unknown", now);
                }
                self.set_status(label, now)
            }
        }
    }

    /// Compose the current frame into `output` (not committed).
    pub(crate) fn render<W: io::Write>(
        &self,
        compositor: &mut Compositor,
        output: &mut W,
        now: Instant,
    ) -> io::Result<()> {
        let tabs_layout = self.tabs_layout();
        let header_enabled = self.workspace.view().workspace_index.is_some();
        let header_offset = if header_enabled { 1 } else { 0 };
        // Both legacy attach and workspace tabs use the bounded render area,
        // even when the physical terminal is larger than the screen caches.
        let (host_rows, host_columns) = self.screen_viewport();
        if header_enabled && (host_rows == 0 || host_columns == 0) {
            return Ok(());
        }
        let mut panes = [None; crate::pane_layout::MAX_PANES];
        let mut scrollbars = [None; crate::pane_layout::MAX_PANES];
        let mut history_controls = [None; crate::pane_layout::MAX_PANES];
        let mut labels = [None; crate::pane_layout::MAX_PANES];
        let mut divider = None;
        let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
        let mut focused = None;
        let fits = self.fits();
        let display_layout = self.display_layout();
        if let (Some(layout), true) = (display_layout, fits) {
            for (index, pane) in layout.panes.iter().enumerate() {
                let Some(pane) = pane else { continue };
                if self.copied_feedback[index]
                    .is_some_and(|(target, expires_at)| target == pane.target && now < expires_at)
                {
                    labels[index] = Some("Copied");
                }
                let screen = self
                    .screens
                    .iter()
                    .find(|slot| slot.target == Some(pane.target));
                let content_rect = panelink::pane_layout::content_rect(pane.rect);
                let actual = self.pane_operations.and_then(|view| {
                    let full = self.layout?;
                    let index = full.panes.iter().position(|candidate| {
                        candidate.is_some_and(|candidate| candidate.target == pane.target)
                    })?;
                    view.confirmed_sizes[index]
                });
                let pending = if self.pane_operations.is_some() {
                    actual.is_some_and(|actual| {
                        content_rect.is_some_and(|rect| actual != (rect.rows, rect.columns))
                    })
                } else {
                    pane.pending_confirmed.is_some()
                };
                let content = match (pending, screen, content_rect) {
                    (true, _, _) => PaneContent::Placeholder("resize pending"),
                    (false, Some(_), Some(rect))
                        if self.copy_mode
                            && self.selection.as_ref().is_some_and(|drag| {
                                drag.target == pane.target
                                    && drag.content_rect == rect
                                    && self.current_selection_identity(drag).is_some()
                            }) =>
                    {
                        PaneContent::Snapshot(
                            &self.selection.as_ref().expect("checked above").frozen,
                        )
                    }
                    (false, Some(slot), Some(rect))
                        if slot.history_offset > 0
                            && slot.history_revision > 0
                            && slot.history.rows() == rect.rows
                            && slot.history.columns() == rect.columns =>
                    {
                        PaneContent::Snapshot(&slot.history)
                    }
                    (false, Some(slot), Some(rect))
                        if slot.snapshot.rows() == rect.rows
                            && slot.snapshot.columns() == rect.columns =>
                    {
                        PaneContent::Snapshot(&slot.snapshot)
                    }
                    (false, Some(slot), Some(_))
                        if slot.snapshot.rows() == pane.rect.rows
                            && slot.snapshot.columns() == pane.rect.columns =>
                    {
                        PaneContent::Placeholder("restart server for pane padding")
                    }
                    (false, Some(_), Some(_)) => PaneContent::Placeholder("resizing..."),
                    (false, None, _) => PaneContent::Placeholder("connecting..."),
                    (false, Some(_), None) => PaneContent::Placeholder("pane too small"),
                };
                if let (false, Some(slot), PaneContent::Snapshot(_)) = (pending, screen, content) {
                    history_controls[index] = history_label_kind(slot);
                    if !slot.snapshot.alternate_screen() {
                        scrollbars[index] = Some(Scrollbar {
                            // A capture still awaiting extent discovery has
                            // only a conservative range; draw the real view
                            // until the discovered extent can map the pointer.
                            offset_rows: self
                                .pointer_seek
                                .filter(|drag| drag.target == pane.target && !drag.awaiting_extent)
                                .map_or(slot.history_offset, |drag| drag.preview_offset),
                            total_rows: self
                                .pointer_seek
                                .filter(|drag| drag.target == pane.target && !drag.awaiting_extent)
                                .map_or(slot.history_total.max(slot.history_offset), |drag| {
                                    drag.total_rows
                                }),
                        });
                    }
                }
                panes[index] = Some(PaneView {
                    rect: shifted_rect(pane.rect, header_offset),
                    content,
                });
                if Some(pane.target) == self.focus {
                    focused = Some(index);
                }
            }
            divider = layout.divider.map(|rect| shifted_rect(rect, header_offset));
            dividers = layout
                .dividers
                .map(|rect| rect.map(|rect| shifted_rect(rect, header_offset)));
        }

        let (view_rows, view_columns) = self.viewport();
        let menu_lines = self.menu.overlay_lines();
        let confirm = self.confirm_ui_geometry(view_rows, view_columns);
        let confirm_text: String;
        let mut lines = [OverlayLine {
            text: "",
            style: OverlayStyle::Normal,
        }; panelink::split_menu::MAX_OVERLAY_ROWS];
        let mut action_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let action_count = if let Some(menu) = self.actions_menu {
            let Some(page) = self.actions_page_geometry(menu) else {
                return Ok(());
            };
            let pane = self
                .layout
                .and_then(|layout| {
                    layout
                        .panes
                        .iter()
                        .position(|pane| pane.is_some_and(|pane| pane.target == menu.target))
                })
                .map_or(1, |index| index + 1);
            let (start, end) = (page.start, page.end);
            action_text[0] = if end < LOCAL_ACTIONS.len() {
                format!("Pane {pane} actions: Split pane... (More: Zoom)")
            } else {
                format!("Pane {pane} actions (Esc cancels)")
            };
            for (row, index) in (start..end).enumerate() {
                let (action, label, availability) = self
                    .local_action_item(menu, index)
                    .expect("bounded local action index");
                let marker = if menu.selected == index { "> " } else { "  " };
                action_text[page.action_row + row] = match (action, availability) {
                    (PaneMenuAction::Split(direction), Availability::Disabled(reason)) => {
                        format!(
                            "{marker}{} - {}",
                            split_menu_direction(direction),
                            reason.compact_label()
                        )
                    }
                    _ => format!("{marker}{label}"),
                };
                if menu.selected == index {
                    action_text[page.reason_row] = match availability {
                        Availability::Enabled => String::from("Enter: activate"),
                        Availability::Disabled(reason) if page.rect.columns < 36 => {
                            reason.compact_label()
                        }
                        Availability::Disabled(reason) => reason.to_string(),
                    };
                }
            }
            if menu.selected == LOCAL_ACTIONS.len() {
                action_text[page.reason_row] = "Cancel menu".into();
            }
            action_text[page.navigation_row] = if menu.selected == LOCAL_ACTIONS.len() {
                "> [Cancel]".into()
            } else {
                match (start > 0, end < LOCAL_ACTIONS.len()) {
                    (true, true) => "[Back] [More] [Cancel]".into(),
                    (true, false) if start > 3 => "[Back] [Cancel] Close on prior page".into(),
                    (true, false) => "[Back] [Cancel]".into(),
                    (false, true) => "[More] [Cancel]".into(),
                    (false, false) => "[Cancel]".into(),
                }
            };
            page.line_count
        } else {
            0
        };
        let mut picker_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let picker_count = if self.workspace.view().picker_open {
            self.picker_overlay_text(&mut picker_text)
        } else {
            0
        };
        for (index, text) in picker_text.iter().enumerate().take(picker_count) {
            lines[index] = OverlayLine {
                text,
                style: if index == 0 {
                    OverlayStyle::Status
                } else if self
                    .picker_routes()
                    .get(self.picker_scroll_offset().unwrap_or(0) + index.saturating_sub(1))
                    .is_some_and(|route| Some(*route) == self.workspace.view().picker_selection)
                {
                    OverlayStyle::Selected
                } else {
                    OverlayStyle::Normal
                },
            };
        }
        let mut tab_dialog_text: [String; 9] = std::array::from_fn(|_| String::new());
        let mut tab_dialog_selected = [false; 9];
        let tab_dialog_count = match self.workspace.view().tab_dialog.as_ref() {
            Some(TabDialog::Actions { selected, .. }) => {
                let padded = self.padded_tab_actions();
                let first = if padded { 3 } else { 1 };
                tab_dialog_text[if padded { 1 } else { 0 }] = if padded {
                    "  Tab actions".into()
                } else {
                    "Tab actions (Esc cancels)".into()
                };
                let labels = ["Select tab", "Rename", "Close tab"];
                for (index, label) in labels.iter().enumerate() {
                    tab_dialog_text[index + first] = format!(
                        "{}{} [{}]",
                        if padded { "  " } else { "" },
                        if *selected == index { ">" } else { " " },
                        label
                    );
                    tab_dialog_selected[index + first] = *selected == index;
                }
                if padded {
                    tab_dialog_text[7] = "  Enter: select  Esc: close".into();
                    9
                } else {
                    4
                }
            }
            Some(TabDialog::Rename {
                draft,
                error,
                saving,
                ..
            }) => {
                tab_dialog_text[0] = "Rename tab (Enter saves; Esc cancels)".into();
                tab_dialog_text[1] =
                    format!("Name: {draft}{}", if *saving { " (saving)" } else { "_" });
                tab_dialog_text[2] = error.unwrap_or(" ").to_owned();
                tab_dialog_text[3] = "[Save] [Cancel]".into();
                tab_dialog_selected[0] = true;
                if error.is_some() {
                    tab_dialog_selected[2] = true;
                }
                4
            }
            Some(TabDialog::Close {
                close_selected,
                pending,
                ..
            }) => {
                tab_dialog_text[0] = if *close_selected {
                    "Close selected: close tab"
                } else {
                    "Cancel selected: close tab"
                }
                .into();
                tab_dialog_text[1] = "Ends all tab processes".into();
                tab_dialog_text[2] = "[Close tab] [Cancel]".into();
                if *pending {
                    tab_dialog_text[1] = "Closing tab; verifying process stop...".into();
                    tab_dialog_text[2] = "Close pending".into();
                }
                tab_dialog_selected[0] = true;
                tab_dialog_selected[2] = !close_selected && !pending;
                3
            }
            None => 0,
        };
        for (index, text) in tab_dialog_text.iter().enumerate().take(tab_dialog_count) {
            lines[index] = OverlayLine {
                text,
                style: if tab_dialog_selected[index] {
                    OverlayStyle::Selected
                } else if index == 0 || (self.padded_tab_actions() && index == 1) {
                    OverlayStyle::Status
                } else {
                    OverlayStyle::Normal
                },
            };
        }
        let mut pane_mode_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let pane_mode_count = match self.pane_mode {
            Some(PaneMode::Swap {
                source,
                choices,
                selected,
                ..
            }) => {
                pane_mode_text[0] =
                    "Swap pane: arrows/hjkl choose; Enter applies, Esc cancels".into();
                let mut count = 1;
                for (index, candidate) in choices.iter().flatten().enumerate() {
                    let pane_number =
                        self.layout
                            .and_then(|layout| {
                                layout.panes.iter().position(|pane| {
                                    pane.is_some_and(|pane| pane.target == *candidate)
                                })
                            })
                            .map_or(index + 1, |index| index + 1);
                    if count < pane_mode_text.len() {
                        pane_mode_text[count] = format!(
                            "{} Pane {pane_number}",
                            if index == selected { ">" } else { " " }
                        );
                        count += 1;
                    }
                }
                if count < pane_mode_text.len() {
                    pane_mode_text[count] = "[Apply]".into();
                    count += 1;
                }
                if count < pane_mode_text.len() {
                    pane_mode_text[count] = "[Cancel]".into();
                    count += 1;
                }
                let _ = source;
                count
            }
            Some(PaneMode::Divider {
                target,
                revision,
                indices,
                selected,
                first_cells,
            }) => {
                let index = indices.get(selected).copied().flatten().unwrap_or(0);
                let axis = self
                    .local
                    .as_ref()
                    .and_then(|layout| layout.divider_info(target, revision, index).ok())
                    .map_or("axis", |info| match info.orientation {
                        DividerOrientation::Horizontal => "left/right",
                        DividerOrientation::Vertical => "up/down",
                    });
                pane_mode_text[0] = format!(
                    "Divider {} ({axis}) span {first_cells}; Tab changes ancestor, Enter applies, Esc cancels",
                    index + 1
                );
                let mut count = 1;
                for (candidate, divider_index) in indices
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, index)| index.map(|index| (slot, index)))
                {
                    if count < pane_mode_text.len() {
                        pane_mode_text[count] = format!(
                            "{} Divider {}",
                            if candidate == selected { ">" } else { " " },
                            divider_index + 1
                        );
                        count += 1;
                    }
                }
                if count < pane_mode_text.len() {
                    pane_mode_text[count] = "[-] [+]".into();
                    count += 1;
                }
                if count < pane_mode_text.len() {
                    pane_mode_text[count] = "[Apply]".into();
                    count += 1;
                }
                if count < pane_mode_text.len() {
                    pane_mode_text[count] = "[Cancel]".into();
                    count += 1;
                }
                count
            }
            None => 0,
        };
        let empty_geometry = self.empty_action_geometry();
        let mut empty_text = [String::new(), String::new()];
        let overlay = if self.help_open && view_rows > 0 && view_columns > 0 {
            let help_lines: &[&str] = if header_enabled {
                &WORKSPACE_HELP_LINES
            } else {
                &HELP_LINES
            };
            let rows = view_rows.min(help_lines.len() as u16);
            let columns = view_columns.min(72);
            let x = view_columns.saturating_sub(columns) / 2;
            let y = view_rows.saturating_sub(rows) / 2;
            for (slot, text) in lines
                .iter_mut()
                .zip(help_lines.iter())
                .take(usize::from(rows))
            {
                *slot = OverlayLine {
                    text,
                    style: if text == &help_lines[0] {
                        OverlayStyle::Selected
                    } else if text == &help_lines[help_lines.len() - 1] {
                        OverlayStyle::Status
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            Some((
                Rect {
                    x,
                    y,
                    rows,
                    columns,
                },
                usize::from(rows),
            ))
        } else if tab_dialog_count > 0 {
            let rect = self.tab_dialog_geometry().unwrap_or(Rect {
                x: 0,
                y: 0,
                rows: 0,
                columns: 0,
            });
            Some((rect, tab_dialog_count.min(usize::from(rect.rows))))
        } else if action_count > 0 {
            let page = self
                .actions_menu
                .and_then(|menu| self.actions_page_geometry(menu))
                .expect("actions menu page was rendered above");
            for (index, text) in action_text.iter().enumerate().take(action_count) {
                lines[index] = OverlayLine {
                    text,
                    style: if index == 0 {
                        OverlayStyle::Selected
                    } else if index == page.reason_row {
                        OverlayStyle::Status
                    } else if text.starts_with('>') {
                        OverlayStyle::Selected
                    } else if self.actions_menu.is_some_and(|menu| {
                        (page.start..page.end)
                            .contains(&(page.start + index.saturating_sub(page.action_row)))
                            && self
                                .local_action_item(
                                    menu,
                                    page.start + index.saturating_sub(page.action_row),
                                )
                                .is_some_and(|(_, _, availability)| {
                                    availability != Availability::Enabled
                                })
                    }) {
                        OverlayStyle::Disabled
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            Some((page.rect, action_count.min(usize::from(page.rect.rows))))
        } else if pane_mode_count > 0 {
            for (index, text) in pane_mode_text.iter().enumerate().take(pane_mode_count) {
                lines[index] = OverlayLine {
                    text,
                    style: if index == 0 || text.starts_with('>') {
                        OverlayStyle::Selected
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            let rows = u16::try_from(pane_mode_count)
                .unwrap_or(u16::MAX)
                .min(view_rows);
            let width = view_columns.min(76);
            Some((
                Rect {
                    x: view_columns.saturating_sub(width) / 2,
                    y: view_rows.saturating_sub(rows) / 2,
                    rows,
                    columns: width,
                },
                pane_mode_count.min(usize::from(rows)),
            ))
        } else if let (Some(rect), true) = (self.menu.overlay_rect(), self.menu.is_open()) {
            for (slot, line) in lines.iter_mut().zip(menu_lines.as_slice()) {
                *slot = OverlayLine {
                    text: line.text(),
                    style: compose_style(line.style()),
                };
            }
            Some((rect, menu_lines.len()))
        } else if let Some(geometry) = confirm {
            confirm_text = format!(
                "Close pane {}? {}. (y/n)",
                geometry.position, geometry.consequence
            );
            lines[0] = OverlayLine {
                text: &confirm_text,
                style: OverlayStyle::Status,
            };
            lines[1] = OverlayLine {
                text: if self.confirm_close_selected {
                    "[Close]> [Cancel]"
                } else {
                    "[Close] >[Cancel]"
                },
                style: OverlayStyle::Normal,
            };
            Some((geometry.overlay, usize::from(geometry.overlay.rows)))
        } else if let (Some((top, rows)), true) = (
            self.picker_screen_geometry(),
            self.workspace.view().picker_open,
        ) {
            Some((
                Rect {
                    x: self.picker_horizontal_bounds().start,
                    y: top.saturating_sub(1),
                    rows,
                    columns: self.picker_horizontal_bounds().len() as u16,
                },
                usize::from(rows).min(picker_count),
            ))
        } else if self.is_empty_workspace() {
            empty_geometry.map(|geometry| {
                empty_text[0] = centered_ascii(
                    if let Some(pending) = self.workspace.view().pending_create {
                        if pending.awaiting_reconcile {
                            "Create outcome unknown; New tab rechecks it"
                        } else {
                            "Opening tab..."
                        }
                    } else {
                        EMPTY_MESSAGE
                    },
                    view_columns,
                );
                empty_text[1] = centered_ascii(EMPTY_ACTIONS, view_columns);
                lines[0] = OverlayLine {
                    text: &empty_text[0],
                    style: OverlayStyle::Status,
                };
                lines[1] = OverlayLine {
                    text: &empty_text[1],
                    style: OverlayStyle::Selected,
                };
                (geometry.overlay, 2)
            })
        } else {
            let notice = if !self.server_present {
                Some(("Disconnected", view_rows.saturating_sub(1)))
            } else if let Some(label) = self.persistent_status {
                Some((label, view_rows.saturating_sub(1)))
            } else if self.copy_mode {
                None
            } else if self.workspace.view().route_switching {
                Some((
                    self.workspace
                        .view()
                        .route_switch_notice
                        .unwrap_or("workspace switch preparing"),
                    view_rows.saturating_sub(1),
                ))
            } else if let Some(pending) = self.workspace.view().pending_create {
                Some((
                    if pending.awaiting_reconcile {
                        "Create outcome unknown; press + to reconcile"
                    } else {
                        "Opening tab..."
                    },
                    view_rows.saturating_sub(1),
                ))
            } else if !fits && self.layout.is_some() {
                Some((TOO_SMALL, 0))
            } else if self.prefix.is_some() {
                Some((
                    if header_enabled {
                        WORKSPACE_NAV_BANNER
                    } else {
                        NAV_BANNER
                    },
                    view_rows.saturating_sub(1),
                ))
            } else {
                self.status
                    .filter(|(_, at)| now.saturating_duration_since(*at) < STATUS_TTL)
                    .map(|(text, _)| (text, view_rows.saturating_sub(1)))
            };
            notice.and_then(|(text, row)| {
                lines[0] = OverlayLine {
                    text,
                    style: if self.prefix.is_some() && fits {
                        OverlayStyle::Selected
                    } else {
                        OverlayStyle::Status
                    },
                };
                (view_rows > 0 && view_columns > 0).then_some((
                    Rect {
                        x: 0,
                        y: row,
                        rows: 1,
                        columns: view_columns,
                    },
                    1,
                ))
            })
        };
        let frame = ComposeFrame {
            host_rows,
            host_columns,
            panes,
            divider,
            dividers,
            focused,
            overlay: overlay.map(|(rect, count)| OverlayView {
                rect: shifted_rect(rect, header_offset),
                lines: &lines[..count],
            }),
        };
        let selection = self.selection.as_ref().and_then(|drag| {
            let identity = self.current_selection_identity(drag)?;
            if identity != drag.identity || (self.copy_mode && self.copy_anchor.is_none()) {
                return None;
            }
            let slot = display_layout?.panes.iter().position(|pane| {
                pane.is_some_and(|pane| {
                    pane.target == drag.target
                        && panelink::pane_layout::content_rect(pane.rect) == Some(drag.content_rect)
                })
            })?;
            Some(SelectionOverlay {
                slot,
                frozen: &drag.frozen,
                anchor: self.copy_anchor.unwrap_or(drag.anchor),
                end: drag.end,
            })
        });
        let caret = if self.copy_mode && overlay.is_none() {
            self.selection.as_ref().and_then(|drag| {
                if self.current_selection_identity(drag)? != drag.identity {
                    return None;
                }
                let slot = display_layout?
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == drag.target))?;
                Some(CopyCaretOverlay {
                    slot,
                    frozen: &drag.frozen,
                    point: drag.end,
                })
            })
        } else {
            None
        };
        compositor.render_with_history_controls(
            &frame,
            selection,
            caret,
            tabs_layout.as_ref().map(|layout| layout.header_cells()),
            PaneChrome {
                scrollbars,
                history: history_controls,
                labels,
            },
            output,
        )
    }

    fn picker_overlay_text(
        &self,
        lines: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
    ) -> usize {
        self.workspace.picker_overlay_text(lines, self.viewport())
    }
}

fn shifted_rect(mut rect: Rect, rows: u16) -> Rect {
    rect.y = rect.y.saturating_add(rows);
    rect
}

fn is_clipboard_failure_label(label: &str) -> bool {
    label == ClipboardStatus::Unavailable.label() || label.starts_with("Copy ")
}

fn is_uncertain_mutation_error(label: &str) -> bool {
    label.contains("unknown")
        || label.contains("pending")
        || label.contains("progress")
        || label.contains("connection")
        || label.contains("timed out")
        || label.contains("lost")
}

fn normalize_point(snapshot: &ScreenSnapshot, point: Point) -> Point {
    if point.column > 0
        && snapshot
            .cell(point.row, point.column)
            .is_some_and(|cell| cell.is_wide_continuation())
    {
        Point {
            column: point.column - 1,
            ..point
        }
    } else {
        point
    }
}

/// Fixed status text for one validated close result.
pub(crate) fn close_message(result: &CloseResult, last_pane: bool) -> &'static str {
    match result.outcome {
        CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated) if last_pane => {
            "last pane closed; workspace closing"
        }
        CloseOutcome::Stopped(StopOutcome::Exited) if result.survivor_resize_pending => {
            "pane closed (resize pending)"
        }
        CloseOutcome::Stopped(StopOutcome::Exited) => "pane closed",
        CloseOutcome::Stopped(StopOutcome::Escalated) if result.survivor_resize_pending => {
            "pane closed (forced; resize pending)"
        }
        CloseOutcome::Stopped(StopOutcome::Escalated) => "pane closed (forced)",
        CloseOutcome::Stopped(StopOutcome::Unknown) => {
            "close status unknown; the pane may still be running"
        }
        CloseOutcome::Rejected(CloseRejection::StaleLayout) => "close rejected: layout changed",
        CloseOutcome::Rejected(CloseRejection::StaleTarget) => "close rejected: pane changed",
        CloseOutcome::Rejected(CloseRejection::AlreadyClosed) => "pane already closed",
        CloseOutcome::Rejected(CloseRejection::NoLayout) => "close rejected: no layout",
        CloseOutcome::Rejected(CloseRejection::NotAdmitted) => "close rejected by the server",
    }
}

fn compose_style(style: MenuStyle) -> OverlayStyle {
    match style {
        MenuStyle::Normal => OverlayStyle::Normal,
        MenuStyle::Selected => OverlayStyle::Selected,
        MenuStyle::Disabled => OverlayStyle::Disabled,
        MenuStyle::Border => OverlayStyle::Border,
        MenuStyle::Status => OverlayStyle::Status,
    }
}

fn split_menu_direction(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Left => "Split Left",
        SplitDirection::Right => "Split Right",
        SplitDirection::Up => "Split Up",
        SplitDirection::Down => "Split Down",
    }
}

fn centered_ascii(text: &str, columns: u16) -> String {
    let padding = usize::from(columns).saturating_sub(text.len()) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

/// Rebuild a local geometry mirror (for menu hit-testing and feasibility)
/// from one server layout. Two-pane geometry does not depend on which pane
/// is the original, so pane 0 is treated as the original.
fn local_layout(reply: &LayoutReply) -> Option<PaneLayout> {
    let snapshot = crate::pane_layout::LayoutSnapshot {
        revision: reply.revision,
        rows: reply.rows,
        columns: reply.columns,
        panes: std::array::from_fn(|index| {
            reply.panes[index].map(|pane| crate::pane_layout::PaneGeometry {
                target: pane.target,
                rect: pane.rect,
            })
        }),
        divider: reply.divider,
        dividers: reply.dividers,
    };
    PaneLayout::from_snapshot(snapshot).ok()
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
