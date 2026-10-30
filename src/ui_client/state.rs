//! Pure presentation state for the attach client: layout, local focus, menu,
//! key batching, transient status and frame building.
//!
//! All network effects go through the [`Effects`] trait so the routing rules
//! can be tested with a recording fake. No host IO happens here.

mod copy;
mod interaction;
#[cfg(test)]
use interaction::ConfirmUiGeometry;
mod render;
use interaction::{
    ActionsMenu, ActionsMenuPage, Interaction, InteractionContext, LocalAction, MenuDispatch,
    PaneMode, LOCAL_ACTIONS,
};
mod viewport;
use copy::{CopyContext, CopyState};
mod workspace;
use viewport::{ScreenSlot, Viewports};
#[cfg(test)]
use viewport::{ALT_WHEEL_CAPACITY, ALT_WHEEL_TTL};
#[cfg(test)]
use workspace::TabDialog;
use workspace::WorkspaceState;

use std::io;
use std::time::{Duration, Instant};

use crossterm_input::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use panelink::close_protocol::{CloseOutcome, CloseRejection, CloseResult, StopOutcome};
use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{DividerOrientation, PaneLayout, Rect, SplitDirection};
use panelink::pane_operations_protocol::{encode_view, PaneOperationsView};
use panelink::protocol::Outcome;
use panelink::screen_checkpoint::ScreenCheckpoint;
use panelink::split_menu::{
    Availability, CloseIntent, MenuResponse, PaneActionAvailability, PaneActionModel,
    PaneMenuAction, SplitIntent,
};
use panelink::terminal::compose::{
    divider_at, pane_controls, pane_history_controls, pane_version_badge, pointer_content_point,
    scrollbar_geometry, Compositor, HistoryLabelKind, Scrollbar,
};
use panelink::terminal::{Key, ScreenSnapshot, TerminalError};
use panelink::ui_protocol::{planned_split_size, LayoutReply, WireHistoryCursor, MAX_KEYS};
use panelink::workspace_index::{ProjectId, TabPresentationSnapshot, WorkspaceIndex};
use panelink::workspace_tabs::{TabAction, TabNavigation, TabsLayout, WorkspaceRouteId};

use super::clipboard::ClipboardStatus;
use super::keys::{
    map_key, mouse_button, mouse_wheel, pane_operation_key, workspace_navigation,
    workspace_tab_command, KeyAction, PaneOperationKey, WorkspaceTabCommand,
};
#[cfg(test)]
use super::net::ScrollResult;
use super::net::{NetUpdate, ScrollUpdate};
use super::pane_operations::PaneAction;
use super::routes::{MetadataRequestId, MetadataResult};
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
#[derive(Clone, Copy)]
struct HeaderPress {
    focus_generation: u64,
    target: TargetRef,
    close: bool,
    revision: u64,
    route: Option<WorkspaceRouteId>,
    host: (u16, u16),
    canceled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceAction {
    Strip(TabAction),
    History(TargetRef),
    NewTab,
    Detach,
}

struct StripPress {
    layout: Option<panelink::workspace_tabs::TabsLayout>,
    action: SurfaceAction,
    bounds: Rect,
    revision: Option<u64>,
    button: MouseButton,
    route: Option<WorkspaceRouteId>,
    catalog_revision: Option<u64>,
    host: (u16, u16),
    focus_generation: u64,
    canceled: bool,
}

struct WorkspacePress {
    control: workspace::WorkspaceControl,
    bounds: Rect,
    stamp: workspace::WorkspaceStamp,
    host: (u16, u16),
    focus: u64,
    generation: Option<u64>,
}

pub(crate) struct App {
    layout: Option<LayoutReply>,
    pane_operations: Option<PaneOperationsView>,
    local: Option<PaneLayout>,
    pending_pane_operation: Option<(u64, PaneAction)>,
    focus: Option<TargetRef>,
    pointer_seek: Option<PointerSeek>,
    /// A thumb gesture released before its extent discovery returned. The
    /// discovery reply applies its final pointer row once, then drops it.
    deferred_thumb_release: Option<PointerSeek>,
    pointer_divider: Option<PointerDivider>,
    header_press: Option<HeaderPress>,
    strip_press: Option<StripPress>,
    workspace_press: Option<WorkspacePress>,
    modal_press: Option<(interaction::ModalControl, HeaderPress)>,
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
    copy: CopyState,
    interaction: Interaction,
    workspace: WorkspaceState,
    preferred_focus: Option<TargetRef>,
    dirty: bool,
}

impl App {
    fn interaction_context(&self) -> InteractionContext<'_> {
        InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        }
    }
    fn apply_interaction_feedback(&mut self) {
        let feedback = self.interaction.take_feedback();
        self.dirty |= feedback.dirty;
        if feedback.cancel_selection {
            self.cancel_selection();
        }
        if let Some((label, now)) = feedback.status {
            self.set_status(label, now);
        }
    }
    fn apply_interaction_commands(&mut self, now: Instant, effects: &mut impl Effects) {
        if let Some(action) = self.interaction.take_operation() {
            self.submit_pane_operation(action, now, effects);
        }
        if let Some(at) = self.interaction.take_flush() {
            self.flush(at, effects);
        }
    }
    fn open_close_confirmation(&mut self, intent: CloseIntent) {
        self.interaction.open_close_confirmation(intent);
        self.apply_interaction_feedback();
    }
    fn dismiss_close_confirmation(&mut self) {
        self.interaction.dismiss_close_confirmation();
        self.apply_interaction_feedback();
    }
    #[cfg(test)]
    fn confirm_ui_geometry(&self, rows: u16, columns: u16) -> Option<ConfirmUiGeometry> {
        self.interaction.confirm_ui_geometry(
            rows,
            columns,
            u16::from(self.workspace.view().workspace_index.is_some()),
        )
    }
    fn handle_pane_mode_event(&mut self, event: &Event, now: Instant, effects: &mut impl Effects) {
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        self.interaction.handle_pane_mode_event(context, event, now);
        self.apply_interaction_feedback();
        self.apply_interaction_commands(now, effects);
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
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        self.interaction
            .open_swap_mode_for_target(context, source, revision, now);
        self.apply_interaction_feedback();
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
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        self.interaction
            .open_divider_mode_for_target(context, target, revision, now);
        self.apply_interaction_feedback();
    }

    fn actions_page_geometry(&self, menu: ActionsMenu) -> Option<ActionsMenuPage> {
        self.interaction
            .actions_page_geometry(self.interaction_context(), menu)
    }
    fn handle_actions_menu_event(
        &mut self,
        event: &Event,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        let result =
            self.interaction
                .handle_actions_menu_event(context, event, now, recovery_availability);
        self.apply_interaction_feedback();
        match result {
            MenuDispatch::Continue => Flow::Continue,
            MenuDispatch::Activate(menu) => {
                self.activate_local_action(menu, now, recovery_availability, effects)
            }
            MenuDispatch::Split(intent) => {
                self.activate(intent, now, effects);
                Flow::Continue
            }
            MenuDispatch::ViewRecovery => Flow::ViewRecovery,
        }
    }
    fn divider_value_valid(&self, target: TargetRef, revision: u64, index: u8, value: u16) -> bool {
        self.interaction_context()
            .divider_value_valid(target, revision, index, value)
    }

    fn copy_context(&self) -> CopyContext<'_> {
        CopyContext {
            layout: self.layout,
            display_layout: self.display_layout(),
            fits: self.fits(),
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        }
    }
    fn visible_source(
        &self,
        target: TargetRef,
    ) -> Option<(usize, Rect, &ScreenSnapshot, bool, u64)> {
        self.copy_context().visible_source(target)
    }

    fn apply_copy_feedback(&mut self) {
        let feedback = self.copy.take_feedback();
        self.dirty |= feedback.dirty;
        if feedback.clear_prefix {
            self.interaction.set_prefix(None);
        }
        if feedback.clear_clipboard_failure
            && self
                .status
                .is_some_and(|(label, _)| is_clipboard_failure_label(label))
        {
            self.status = None;
        }
        if let Some((label, now)) = feedback.status {
            self.set_status(label, now);
        }
    }

    #[cfg(test)]
    pub(crate) fn take_copy_text(&mut self) -> Option<String> {
        let result = self.copy.take_copy_text();
        self.apply_copy_feedback();
        result
    }
    pub(crate) fn take_copy_request(&mut self) -> Option<(TargetRef, u64, String)> {
        let result = self.copy.take_copy_request();
        self.apply_copy_feedback();
        result
    }
    pub(crate) fn report_clipboard_unavailable(&mut self, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.report_clipboard_unavailable(&context, now);
        self.apply_copy_feedback();
    }
    pub(crate) fn report_clipboard_status(
        &mut self,
        target: Option<TargetRef>,
        request_id: Option<u64>,
        status: ClipboardStatus,
        now: Instant,
    ) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy
            .report_clipboard_status(&context, target, request_id, status, now);
        self.apply_copy_feedback();
    }
    pub(crate) fn cancel_for_detach(&mut self) {
        self.copy.cancel_for_detach();
        self.apply_copy_feedback();
    }
    fn cancel_selection(&mut self) {
        self.copy.cancel_selection();
        self.apply_copy_feedback();
    }
    #[cfg(test)]
    fn queue_copy_text(&mut self, target: TargetRef, text: String, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.queue_copy_text(&context, target, text, now);
        self.apply_copy_feedback();
    }
    fn begin_copy_mode(&mut self, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.begin_copy_mode(&context, now);
        self.apply_copy_feedback();
    }
    fn begin_copy_mode_for_target(&mut self, target: TargetRef, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.begin_copy_mode_for_target(&context, target, now);
        self.apply_copy_feedback();
    }
    fn handle_copy_mode_event(
        &mut self,
        event: &Event,
        now: Instant,
        effects: &mut impl Effects,
    ) -> Flow {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        let result = self.copy.handle_copy_mode_event(&context, event, now);
        self.apply_copy_feedback();
        if result == Flow::Quit {
            self.flush(now, effects);
        }
        result
    }
    fn begin_selection(&mut self, target: TargetRef, x: u16, y: u16, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.begin_selection(&context, target, x, y, now);
        self.apply_copy_feedback();
    }
    fn extend_selection(&mut self, x: u16, y: u16) {
        self.copy.extend_selection(x, y);
        self.apply_copy_feedback();
    }
    fn finish_selection(&mut self, now: Instant) {
        let display_layout = self.display_layout();
        let fits = self.fits();
        let context = CopyContext {
            layout: self.layout,
            display_layout,
            fits,
            screens: &self.screens,
            focus: self.focus,
            status: self.status,
        };
        self.copy.finish_selection(&context, now);
        self.apply_copy_feedback();
    }

    fn apply_workspace_feedback(&mut self) {
        let feedback = self.workspace.take_feedback();
        self.dirty |= feedback.dirty;
        if feedback.cancel_selection {
            self.cancel_selection();
        }
        if feedback.clear_prefix {
            self.interaction.set_prefix(None);
        }
        if feedback.clear_actions {
            self.interaction.replace_actions(None);
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
            pending_pane_operation: None,
            focus: None,
            pointer_seek: None,
            deferred_thumb_release: None,
            pointer_divider: None,
            header_press: None,
            strip_press: None,
            workspace_press: None,
            modal_press: None,
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
            copy: CopyState::new(),
            interaction: Interaction::new(),
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
        self.modal_press = None;
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
        !self.server_present
            || self.interaction.view().pane_mode.is_some()
            || self.pending_pane_operation.is_some()
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
        self.interaction.replace_mode(None);
        self.interaction.record_mode_exit(None);
        self.pointer_seek = None;
        self.pointer_divider = None;
        self.interaction.replace_actions(None);
        self.interaction.cancel_menu();
        self.dismiss_close_confirmation();
        self.interaction.show_help(false);
        self.pending_pane_operation = None;
        self.pane_operations = None;
        self.interaction.set_prefix(None);
        self.cancel_selection();
        self.copy.reset_route();
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
        self.interaction.view().menu.is_open() || self.interaction.view().actions_menu.is_some()
    }

    pub(crate) fn needs_recovery_availability(&self, event: &Event) -> bool {
        if let Some(menu) = self.interaction.view().actions_menu {
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
        if self.copy.view().copy_mode
            || self.workspace.view().route_switching
            || self.interaction.view().pane_mode.is_some()
            || self.pending_pane_operation.is_some()
            || self.interaction.view().help_open
            || self.workspace.view().picker_open
            || self.workspace.view().tab_dialog.is_some()
            || (self.workspace.view().workspace_index.is_some()
                && matches!(event, Event::Mouse(mouse) if mouse.row == 0))
            || self.interaction.view().confirm.is_open()
            || self.interaction.view().menu.is_open()
        {
            return false;
        }
        match event {
            Event::Mouse(mouse) => {
                self.pane_control_at(mouse.column, mouse.row)
                    .is_some_and(|(_, close)| !close)
                    || matches!(mouse_button(mouse), Some((true, true)))
            }
            Event::Key(key)
                if self.interaction.view().prefix.is_some()
                    && (key.is_press() || key.is_repeat()) =>
            {
                (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::Char('s' | 'S' | 'v' | 'V'))
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn confirm_open(&self) -> bool {
        self.interaction.view().confirm.is_open()
    }

    #[cfg(test)]
    pub(crate) fn help_open(&self) -> bool {
        self.interaction.view().help_open
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
        if let Some(flow) = self.capture_strip_event(event, now) {
            return Some(flow);
        }
        if self.capture_workspace_event(event, now) {
            return Some(Flow::Continue);
        }

        if self.workspace.tab_dialog.is_some() {
            self.handle_tab_dialog_event(event, now);
            return Some(Flow::Continue);
        }
        if self.workspace.picker_open {
            self.handle_workspace_picker_event(event, now);
            return Some(Flow::Continue);
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
            self.modal_press = None;
            self.interaction.replace_mode(None);
            self.interaction.record_mode_exit(None);
            self.interaction.replace_actions(None);
            self.interaction.close_menu();
            self.cancel_pointer_seek();
            self.pointer_divider = None;
            self.cancel_selection();
            self.screens.invalidate_pending();
            self.host = (rows, columns);
            let (rows, columns) = self.viewport();
            self.interaction.set_menu_viewport(rows, columns);
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
        }) = self.interaction.view().pane_mode
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
        self.modal_press = None;
        self.interaction.reconcile_modals(&reply);
        self.apply_interaction_feedback();
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
            self.interaction.replace_mode(None);
        }
        let divider_mode_stale = match self.interaction.view().pane_mode {
            Some(PaneMode::Divider {
                target, revision, ..
            }) => {
                revision != reply.revision
                    || !reply
                        .panes
                        .iter()
                        .flatten()
                        .any(|pane| pane.target == target)
                    || self.layout.is_some_and(|layout| {
                        layout.rows != reply.rows || layout.columns != reply.columns
                    })
            }
            _ => false,
        };
        if divider_mode_stale {
            self.pointer_divider = None;
            self.interaction.record_mode_exit(None);
            self.interaction.replace_mode(None);
        } else if self.layout.is_some_and(|layout| {
            layout.revision != reply.revision
                || layout.rows != reply.rows
                || layout.columns != reply.columns
        }) {
            self.interaction.replace_mode(None);
        }
        self.copy.reconcile_geometry(&reply);
        self.apply_copy_feedback();
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
            .interaction
            .view()
            .confirm
            .intent()
            .is_some_and(|intent| !present(intent.target))
        {
            self.dismiss_close_confirmation();
            self.status = Some(("pane already closed", Instant::now()));
        }
        self.screens.reconcile_layout(self.layout, &reply);
        self.layout = Some(reply);
        self.copy.reconcile_targets(&reply);
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
            self.interaction.replace_mode(None);
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
                    self.interaction.replace_mode(None);
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
        if self.copy.view().copy_mode
            && self.copy.view().selection.as_ref().is_some_and(|drag| {
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
            || self.copy.view().copy_mode
            || !self.server_present
            || self.interaction.view().help_open
            || self.interaction.view().confirm.is_open()
            || self.interaction.view().menu.is_open()
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
        if self.copy.view().selection.as_ref().is_some_and(|drag| {
            drag.target == target
                && (row_wraps.is_none()
                    || (self.copy.view().copy_mode
                        && row_wraps.is_some_and(|wraps| drag.row_wraps != wraps)))
        }) {
            self.cancel_selection();
        }
        let buffer_changed =
            self.screens
                .load_screen(target, revision, checkpoint, row_wraps, history_cursor)?;
        if buffer_changed {
            self.cancel_selection();
            if self.pointer_divider.take().is_some()
                || matches!(
                    self.interaction.view().pane_mode,
                    Some(PaneMode::Divider { .. })
                )
            {
                self.interaction.replace_mode(None);
                self.interaction.record_mode_exit(None);
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

    /// Whether navigation mode (after Ctrl-B) is active.
    #[cfg(test)]
    pub(crate) fn nav_active(&self) -> bool {
        self.interaction.view().prefix.is_some()
    }

    /// Drop an expired status or navigation mode; marks a repaint if needed.
    pub(crate) fn expire_status(&mut self, now: Instant) {
        if self
            .interaction
            .view()
            .prefix
            .is_some_and(|at| now.saturating_duration_since(at) >= NAV_TIMEOUT)
        {
            self.interaction.set_prefix(None);
            self.dirty = true;
        }
        if self
            .status
            .is_some_and(|(_, at)| now.saturating_duration_since(at) >= STATUS_TTL)
        {
            self.status = None;
            self.dirty = true;
        }
        self.copy.expire_feedback(now);
        self.apply_copy_feedback();
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
        if self.is_empty_workspace() {
            return self
                .handle_empty_workspace_event(event, now)
                .unwrap_or(Flow::Continue);
        }
        if let Some(flow) = self.capture_strip_event(event, now) {
            return flow;
        }
        if self.capture_workspace_event(event, now) {
            return Flow::Continue;
        }

        if let Some((control, mut press)) = self.modal_press.take() {
            let valid = self.pane_modal_available()
                && self.focus_generation == press.focus_generation
                && self.layout.map(|l| l.revision) == Some(press.revision)
                && self.workspace.view().current_route == press.route
                && self.host == press.host
                && self
                    .layout
                    .is_some_and(|l| l.panes.iter().flatten().any(|p| p.target == control.target));
            self.dirty = true;
            if let Event::Mouse(mouse) = event {
                let same = valid
                    && self.interaction.modal_control_at(
                        self.interaction_context(),
                        mouse.column,
                        mouse.row,
                    ) == Some(control);
                if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                    if same && !press.canceled {
                        let activation = Event::Mouse(crossterm_input::MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            ..*mouse
                        });
                        return self.dispatch_event(
                            &activation,
                            now,
                            recovery_availability,
                            effects,
                        );
                    }
                    return Flow::Continue;
                }
                press.canceled |= !same
                    || !matches!(
                        mouse.kind,
                        MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                    );
                self.modal_press = Some((control, press));
                return Flow::Continue;
            }
            if matches!(event,Event::Key(key) if key.code==KeyCode::Esc) {
                return Flow::Continue;
            }
        }
        if self.pane_modal_available() {
            if let Event::Mouse(mouse) = event {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let (Some(control), Some(layout)) = (
                        self.interaction.modal_control_at(
                            self.interaction_context(),
                            mouse.column,
                            mouse.row,
                        ),
                        self.layout,
                    ) {
                        self.modal_press = Some((
                            control,
                            HeaderPress {
                                focus_generation: self.focus_generation,
                                target: control.target,
                                close: false,
                                revision: layout.revision,
                                route: self.workspace.view().current_route,
                                host: self.host,
                                canceled: false,
                            },
                        ));
                        self.dirty = true;
                        return Flow::Continue;
                    }
                }
            }
        }
        let chrome_available = !self.interaction.view().help_open
            && !self.interaction.view().confirm.is_open()
            && !self.interaction.view().menu.is_open()
            && self.interaction.view().actions_menu.is_none()
            && self.interaction.view().pane_mode.is_none()
            && !self.workspace.view().route_switching
            && self.workspace.view().tab_dialog.is_none()
            && !self.workspace.view().picker_open
            && self.pointer_seek.is_none()
            && self.pointer_divider.is_none();
        if let Some(mut press) = self.header_press.take() {
            let valid = chrome_available
                && self.focus_generation == press.focus_generation
                && self.layout.map(|layout| layout.revision) == Some(press.revision)
                && self.workspace.view().current_route == press.route
                && self.host == press.host;
            match event {
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Up(MouseButton::Left) => {
                    self.dirty = true;
                    if valid
                        && !press.canceled
                        && self.pane_control_at(mouse.column, mouse.row)
                            == Some((press.target, press.close))
                    {
                        let activation = Event::Mouse(crossterm_input::MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            ..*mouse
                        });
                        let flow =
                            self.dispatch_event(&activation, now, recovery_availability, effects);
                        // The opener's physical release is already consumed here.
                        self.interaction.consume_release(event);
                        return flow;
                    }
                    return Flow::Continue;
                }
                Event::Mouse(mouse) => {
                    press.canceled |= !valid
                        || self.pane_control_at(mouse.column, mouse.row)
                            != Some((press.target, press.close))
                        || !matches!(
                            mouse.kind,
                            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved
                        );
                    self.header_press = Some(press);
                    self.dirty = true;
                    return Flow::Continue;
                }
                _ => {
                    self.dirty = true;
                    if matches!(event, Event::Key(key) if key.code == KeyCode::Esc) {
                        return Flow::Continue;
                    }
                }
            }
        }
        if chrome_available {
            if let Event::Mouse(mouse) = event {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let (Some((target, close)), Some(layout)) =
                        (self.pane_control_at(mouse.column, mouse.row), self.layout)
                    {
                        self.cancel_selection();
                        self.header_press = Some(HeaderPress {
                            focus_generation: self.focus_generation,
                            target,
                            close,
                            revision: layout.revision,
                            route: self.workspace.view().current_route,
                            host: self.host,
                            canceled: false,
                        });
                        self.dirty = true;
                        return Flow::Continue;
                    }
                }
            }
        }
        if self.interaction.pane_mode.is_some()
            && matches!(event, Event::Mouse(m) if m.kind==MouseEventKind::Down(MouseButton::Left))
        {
            return Flow::Continue;
        }
        self.dispatch_event(event, now, recovery_availability, effects)
    }

    fn pane_modal_available(&self) -> bool {
        !self.interaction.help_open
            && !self.workspace.view().route_switching
            && self.workspace.view().tab_dialog.is_none()
            && !self.workspace.view().picker_open
            && self.pointer_divider.is_none()
            && self.pointer_seek.is_none()
    }

    fn visible_modal_press(&self) -> Option<interaction::ModalControl> {
        let (control, press) = self.modal_press?;
        if !self.pane_modal_available()
            || press.canceled
            || self.focus_generation != press.focus_generation
            || self.host != press.host
            || self.workspace.view().current_route != press.route
            || !self.layout.is_some_and(|l| {
                l.revision == press.revision
                    && l.panes.iter().flatten().any(|p| p.target == control.target)
            })
        {
            return None;
        }
        (self.interaction.modal_control_at(
            self.interaction_context(),
            control.bounds.x,
            control.bounds.y,
        ) == Some(control))
        .then_some(control)
    }

    fn dispatch_event(
        &mut self,
        event: &Event,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let is_wheel = matches!(event, Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown));
        if !is_wheel
            || self.interaction.view().help_open
            || self.interaction.view().confirm.is_open()
            || self.interaction.view().menu.is_open()
            || self.copy.view().copy_mode
            || self.workspace.view().route_switching
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
            || self.interaction.view().pane_mode.is_some()
            || self.interaction.view().prefix.is_some()
        {
            self.screens.discard_alternate();
        }
        if self.interaction.consume_release(event) {
            return Flow::Continue;
        }
        if let Event::Key(key) = event {
            if self.copy.consume_exit_key(*key) {
                return Flow::Continue;
            }
        }
        if matches!(event, Event::FocusLost | Event::Resize(_, _))
            && matches!(
                self.interaction.view().pane_mode,
                Some(PaneMode::Divider { .. })
            )
        {
            self.pointer_divider = None;
            self.interaction.replace_mode(None);
            self.interaction.record_mode_exit(None);
            self.dirty = true;
        }
        if let Some(flow) = self.handle_empty_workspace_event(event, now) {
            return flow;
        }
        if self.interaction.view().help_open
            || self.interaction.view().confirm.is_open()
            || self.interaction.view().menu.is_open()
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
        {
            self.cancel_selection();
        }
        match event {
            Event::Key(key)
                if (key.is_press() || key.is_repeat()) && !self.copy.view().copy_mode =>
            {
                self.cancel_selection()
            }
            Event::Paste(_) if !self.copy.view().copy_mode => self.cancel_selection(),
            Event::FocusLost | Event::Resize(_, _) => self.cancel_selection(),
            Event::Mouse(mouse)
                if !self.copy.view().copy_mode
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
                self.interaction.show_help(false);
                self.interaction.set_prefix(None);
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
        if self.interaction.view().pane_mode.is_some() {
            self.handle_pane_mode_event(event, now, effects);
            return Flow::Continue;
        }
        if self.interaction.view().actions_menu.is_some() {
            return self.handle_actions_menu_event(event, now, recovery_availability, effects);
        }
        if self.interaction.view().help_open {
            let viewport = self.viewport();
            let header = u16::from(self.workspace.view().workspace_index.is_some());
            let flow = self.interaction.handle_help_event(event, viewport, header);
            self.apply_interaction_feedback();
            if flow == Flow::Quit {
                self.flush(now, effects);
            }
            return flow;
        }
        if self.interaction.view().confirm.is_open() {
            let viewport = self.viewport();
            let header = u16::from(self.workspace.view().workspace_index.is_some());
            let intent = self
                .interaction
                .handle_confirm_event(event, now, viewport, header);
            self.apply_interaction_feedback();
            if let Some(intent) = intent {
                self.close_pane(intent, now, effects);
            }
            return Flow::Continue;
        }
        if self.interaction.view().menu.is_open() {
            let header = u16::from(self.workspace.view().workspace_index.is_some());
            let response = self.interaction.handle_split_menu_event(event, header);
            self.apply_interaction_feedback();
            match response {
                MenuResponse::Activated(intent) => self.activate(intent, now, effects),
                MenuResponse::CloseRequested(intent) => self.open_close_confirmation(intent),
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
        if self.copy.view().copy_mode {
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
                if self.interaction.view().prefix.is_some() {
                    if !key.is_press() && !key.is_repeat() {
                        return Flow::Continue;
                    }
                    let operation_key = pane_operation_key(key.code, key.modifiers).is_some();
                    let close_confirmation_key = (key.is_press() || key.is_repeat())
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                        && matches!(key.code, KeyCode::Char('x' | 'X'));
                    let flow =
                        self.navigate(key.code, key.modifiers, now, recovery_availability, effects);
                    if close_confirmation_key && self.interaction.view().confirm.is_open() {
                        self.interaction
                            .record_confirm_open_key(Some((key.code, key.modifiers)));
                    }
                    if operation_key {
                        self.interaction
                            .record_mode_exit(Some((key.code, key.modifiers)));
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
                        self.interaction.set_prefix(Some(now));
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
            self.interaction.set_prefix(Some(now));
            return Flow::Continue;
        }
        self.interaction.set_prefix(None);
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
                self.interaction.set_prefix(None);
                self.flush(now, effects);
                self.interaction.show_help(true);
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

    fn open_divider_mode(&mut self, now: Instant, effects: &mut impl Effects) {
        let (Some(view), Some(target)) = (self.pane_operations, self.focus) else {
            self.set_status("divider adjustment unavailable", now);
            return;
        };
        self.open_divider_mode_for_target(target, view.layout.revision, now, effects);
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

    fn surface_at(&self, x: u16, y: u16) -> Option<(SurfaceAction, Rect)> {
        if let Some(layout) = self.tabs_layout() {
            if let Some(action) = layout.hit_test(x, y) {
                let mut start = x;
                while start > 0 && layout.hit_test(start - 1, y) == Some(action) {
                    start -= 1;
                }
                let mut end = x + 1;
                while end < layout.columns() && layout.hit_test(end, y) == Some(action) {
                    end += 1;
                }
                return Some((
                    SurfaceAction::Strip(action),
                    Rect {
                        x: start,
                        y,
                        rows: 1,
                        columns: end - start,
                    },
                ));
            }
        }
        if self.is_empty_workspace() {
            let geometry = self.empty_action_geometry()?;
            return [
                (SurfaceAction::NewTab, geometry.new_tab),
                (SurfaceAction::Detach, geometry.detach),
            ]
            .into_iter()
            .find(|(_, r)| r.contains(x, y));
        }
        let target = self.history_back_at(x, y)?;
        let mut start = x;
        while start > 0 && self.history_back_at(start - 1, y) == Some(target) {
            start -= 1;
        }
        let mut end = x + 1;
        while end < self.screen_viewport().1 && self.history_back_at(end, y) == Some(target) {
            end += 1;
        }
        Some((
            SurfaceAction::History(target),
            Rect {
                x: start,
                y,
                rows: 1,
                columns: end - start,
            },
        ))
    }

    fn surface_capture_available(&self) -> bool {
        let view = self.interaction.view();
        !view.help_open
            && !view.confirm.is_open()
            && !view.menu.is_open()
            && view.actions_menu.is_none()
            && view.pane_mode.is_none()
            && !self.workspace.view().route_switching
            && self.workspace.view().tab_dialog.is_none()
            && !self.workspace.view().picker_open
            && self.pointer_seek.is_none()
            && self.pointer_divider.is_none()
    }

    fn visible_surface_press(&self) -> Option<(SurfaceAction, Rect)> {
        let press = self.strip_press.as_ref()?;
        (!press.canceled
            && self.surface_capture_available()
            && self.host == press.host
            && self.focus_generation == press.focus_generation
            && self.workspace.view().current_route == press.route
            && self.workspace.view().workspace_revision == press.catalog_revision
            && self.layout.map(|l| l.revision) == press.revision
            && self.tabs_layout() == press.layout
            && self.surface_at(press.bounds.x, press.bounds.y)
                == Some((press.action, press.bounds)))
        .then_some((press.action, press.bounds))
    }

    /// Local strip capture also works in the empty host without a network worker.
    fn workspace_generation(&self) -> Option<u64> {
        self.layout
            .and_then(|l| l.panes.iter().flatten().next().map(|p| p.target.generation))
    }
    fn valid_workspace_press(&self) -> Option<Rect> {
        let p = self.workspace_press.as_ref()?;
        (p.stamp == self.workspace.control_stamp()
            && p.host == self.host
            && p.focus == self.focus_generation
            && p.generation == self.workspace_generation()
            && self
                .workspace
                .workspace_control_at(self.viewport(), p.bounds.x, p.bounds.y)
                == Some((p.control, p.bounds)))
        .then_some(p.bounds)
    }
    fn capture_workspace_event(&mut self, event: &Event, now: Instant) -> bool {
        if self.workspace_press.is_some() {
            let valid = self.valid_workspace_press().is_some();
            let p = self.workspace_press.take().unwrap();
            self.dirty = true;
            if let Event::Mouse(m) = event {
                let same = valid
                    && self
                        .workspace
                        .workspace_control_at(self.viewport(), m.column, m.row)
                        == Some((p.control, p.bounds));
                if m.kind == MouseEventKind::Up(MouseButton::Left) && same {
                    self.workspace
                        .activate_control(p.control, now, p.generation);
                    self.apply_workspace_feedback();
                } else if same
                    && matches!(
                        m.kind,
                        MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                    )
                {
                    self.workspace_press = Some(p);
                }
                return true;
            }
            if matches!(event, Event::Key(k) if k.code == KeyCode::Esc) {
                return true;
            }
        }
        if self.workspace.tab_dialog.is_some() || self.workspace.picker_open {
            if let Event::Mouse(m) = event {
                if m.kind == MouseEventKind::Down(MouseButton::Left) {
                    if let Some((control, bounds)) =
                        self.workspace
                            .workspace_control_at(self.viewport(), m.column, m.row)
                    {
                        self.workspace_press = Some(WorkspacePress {
                            control,
                            bounds,
                            stamp: self.workspace.control_stamp(),
                            host: self.host,
                            focus: self.focus_generation,
                            generation: self.workspace_generation(),
                        });
                        self.dirty = true;
                    }
                }
                return true;
            }
        }
        false
    }

    fn capture_strip_event(&mut self, event: &Event, now: Instant) -> Option<Flow> {
        let available = self.surface_capture_available();
        if let Some(mut press) = self.strip_press.take() {
            let valid = available
                && self.host == press.host
                && self.focus_generation == press.focus_generation
                && self.workspace.view().current_route == press.route
                && self.workspace.view().workspace_revision == press.catalog_revision
                && self.tabs_layout() == press.layout
                && self.layout.map(|l| l.revision) == press.revision
                && self.surface_at(press.bounds.x, press.bounds.y)
                    == Some((press.action, press.bounds));
            self.dirty = true;
            if let Event::Mouse(mouse) = event {
                let same = valid
                    && self.surface_at(mouse.column, mouse.row)
                        == Some((press.action, press.bounds));
                if mouse.kind == MouseEventKind::Up(press.button) {
                    if same && !press.canceled {
                        match press.action {
                            SurfaceAction::Strip(_) => {
                                self.handle_header_event(
                                    &Event::Mouse(crossterm_input::MouseEvent {
                                        kind: MouseEventKind::Down(press.button),
                                        ..*mouse
                                    }),
                                    now,
                                );
                            }
                            SurfaceAction::History(target) => self.return_to_live(target),
                            SurfaceAction::NewTab => self.request_empty_create(now),
                            SurfaceAction::Detach => return Some(Flow::Quit),
                        }
                    }
                    return Some(Flow::Continue);
                }
                press.canceled |= !same
                    || !matches!(mouse.kind, MouseEventKind::Moved)
                        && mouse.kind != MouseEventKind::Drag(press.button);
                self.strip_press = Some(press);
                return Some(Flow::Continue);
            }
            if matches!(event, Event::Key(key) if key.code == KeyCode::Esc) {
                return Some(Flow::Continue);
            }
        }
        if available {
            if let Event::Mouse(mouse) = event {
                if let MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) =
                    mouse.kind
                {
                    if let Some((action, bounds)) = self.surface_at(mouse.column, mouse.row) {
                        if button == MouseButton::Left || matches!(action, SurfaceAction::Strip(_))
                        {
                            self.cancel_selection();
                            self.strip_press = Some(StripPress {
                                layout: self.tabs_layout(),
                                bounds,
                                revision: self.layout.map(|l| l.revision),
                                action,
                                button,
                                route: self.workspace.view().current_route,
                                catalog_revision: self.workspace.view().workspace_revision,
                                host: self.host,
                                focus_generation: self.focus_generation,
                                canceled: false,
                            });
                            self.dirty = true;
                            return Some(Flow::Continue);
                        }
                    }
                }
            }
        }
        None
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
                (MouseEventKind::Down(MouseButton::Left), Some(TabAction::Close(route))) => {
                    self.begin_tab_close(route)
                }
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

    #[cfg(test)]
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

    #[cfg(test)]
    fn picker_routes(&self) -> Vec<WorkspaceRouteId> {
        self.workspace.picker_routes()
    }

    #[cfg(test)]
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

    #[cfg(test)]
    fn picker_screen_geometry(&self) -> Option<(u16, u16)> {
        self.workspace.picker_screen_geometry(self.viewport())
    }

    #[cfg(test)]
    fn picker_visible_route_count(&self) -> usize {
        self.workspace.picker_visible_route_count(self.viewport())
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
        self.interaction.record_menu_revision(layout.revision);
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
        self.interaction.set_prefix(None);
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
                    let mode = self.interaction.take_mode();
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
                    self.interaction.replace_mode(None);
                    self.dirty = true;
                }
                Event::Resize(_, _) | Event::FocusLost => {
                    self.pointer_divider = None;
                    self.interaction.replace_mode(None);
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
        self.interaction.replace_mode(Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected: 0,
            first_cells: info.first_span,
        }));
        self.cancel_selection();
        self.interaction.set_prefix(None);
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
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        if !self
            .interaction
            .preview_pointer_divider(context, capture, x, screen_y)
        {
            self.pointer_divider = None;
        }
        self.apply_interaction_feedback();
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
            .interaction
            .open_menu_at_pointer(local, x, y, rows, columns, recovery_availability)
            .is_ok()
        {
            self.interaction.compact_menu_reasons();
            self.interaction.record_menu_revision(layout.revision);
            if let Some(target) = self.interaction.view().menu.captured_target() {
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
            .interaction
            .open_menu_for_target(local, target, rows, columns, recovery_availability)
            .is_ok()
        {
            self.interaction.compact_menu_reasons();
            self.interaction.record_menu_revision(revision);
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
        self.interaction.set_prefix(None);
        let Some(model) = self.pane_action_model(target, layout.revision, recovery_availability)
        else {
            self.set_status("pane actions unavailable: layout changed", now);
            return;
        };
        self.interaction.replace_actions(Some(ActionsMenu {
            target,
            revision: layout.revision,
            selected: 0,
            model,
        }));
        self.interaction.record_release(opening_release);
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

    /// Leave room for a title, page navigation and Cancel even at minimum
    /// pane height. Keyboard selection and pointer pages use this same slice.
    #[cfg(test)]
    fn actions_page(&self, menu: ActionsMenu) -> (usize, usize) {
        self.actions_page_geometry(menu)
            .map_or((0, 0), |page| (page.start, page.end))
    }

    fn activate_local_action(
        &mut self,
        menu: ActionsMenu,
        now: Instant,
        recovery_availability: Availability,
        effects: &mut impl Effects,
    ) -> Flow {
        let context = InteractionContext {
            layout: self.layout,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        };
        let action = self.interaction.capture_action(menu, context, now);
        self.apply_interaction_feedback();
        let Some(action) = action else {
            return Flow::Continue;
        };
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
            LocalAction::Help => self.interaction.show_help(true),
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

    /// Presentation only: stale captures never light a replacement control.
    fn valid_header_press(&self) -> Option<(TargetRef, bool)> {
        let press = self.header_press?;
        let view = self.interaction.view();
        if !self.pane_modal_available()
            || press.canceled
            || self.focus_generation != press.focus_generation
            || self.layout.map(|layout| layout.revision) != Some(press.revision)
            || self.workspace.view().current_route != press.route
            || self.host != press.host
            || view.help_open
            || view.confirm.is_open()
            || view.menu.is_open()
            || view.actions_menu.is_some()
            || view.pane_mode.is_some()
            || self.workspace.view().route_switching
            || self.workspace.view().tab_dialog.is_some()
            || self.workspace.view().picker_open
            || self.pointer_seek.is_some()
            || self.pointer_divider.is_some()
        {
            return None;
        }
        self.layout?
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == press.target)
            .then_some((press.target, press.close))
    }

    /// Compose the current frame into `output` (not committed).
    pub(crate) fn render<W: io::Write>(
        &self,
        compositor: &mut Compositor,
        output: &mut W,
        now: Instant,
    ) -> io::Result<()> {
        render::RenderView {
            source: self.copy_context(),
            copy: &self.copy,
            workspace: &self.workspace,
            interaction: &self.interaction,
            local: self.local.as_ref(),
            pane_operations: self.pane_operations,
            viewport: self.viewport(),
            screen_viewport: self.screen_viewport(),
            pointer_seek: self.pointer_seek,
            server_present: self.server_present,
            persistent_status: self.persistent_status,
            header_pressed: self.valid_header_press(),
            modal_pressed: self.visible_modal_press(),
            surface_pressed: self.visible_surface_press(),
            workspace_pressed: self.valid_workspace_press(),
        }
        .render(compositor, output, now)
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
