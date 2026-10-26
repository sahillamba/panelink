//! Pure presentation state for the attach client: layout, local focus, menu,
//! key batching, transient status and frame building.
//!
//! All network effects go through the [`Effects`] trait so the routing rules
//! can be tested with a recording fake. No host IO happens here.

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
    OverlayStyle as MenuStyle, SplitIntent,
};
use panelink::terminal::compose::{
    ComposeFrame, Compositor, CopyCaretOverlay, OverlayLine, OverlayStyle, OverlayView,
    PaneContent, PaneView, SelectionOverlay,
};
use panelink::terminal::{Key, ScreenSnapshot, TerminalError, TerminalSize};
use panelink::terminal_selection::{Point, SelectionError, ViewIdentity};
use panelink::ui_protocol::{planned_split_size, LayoutReply, MAX_KEYS};
use panelink::workspace_index::WorkspaceIndex;
use panelink::workspace_tabs::{self, TabAction, TabNavigation, TabsLayout, WorkspaceRouteId};

use super::keys::{
    confirm_input, copy_mode_key, map_key, menu_input, mouse_button, mouse_wheel,
    pane_operation_key, workspace_navigation, CopyModeKey, KeyAction, PaneOperationKey,
};
use super::net::{NetUpdate, ScrollResult, ScrollUpdate};
use super::pane_operations::PaneAction;
use super::selection::{capture, reanchor, FrozenDrag};

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
pub(crate) const NAV_BANNER: &str =
    " NAV  ? help | s menu | c copy | z zoom | m swap | e divider | v saved | l r u d split | arrows/o focus | x close | ^X detach | Esc ";
const WORKSPACE_NAV_BANNER: &str =
    " NAV  ? help | c copy | z zoom | m swap | e divider | w workspaces | n/p tabs | [/] projects | s menu | v saved | l r u d split | arrows/o focus | x close | ^X detach | Esc ";
const TOO_SMALL: &str = "Panelink: terminal too small for the pane layout; enlarge it";
const HELP_LINES: [&str; 8] = [
    "Panelink help",
    "Ctrl-B ?: help | s: menu | c: copy | z: zoom | m: swap | e: divider",
    "Ctrl-B l/r/u/d: split | arrows/Tab/o: focus",
    "Ctrl-B x: close pane (confirm y)",
    "Esc: leave navigation or close help",
    "Ctrl-X: detach; running shells continue",
    "Run Panelink again or attach to reconnect",
    "Press Esc or ? to close help",
];
const WORKSPACE_HELP_LINES: [&str; 8] = [
    "Panelink help",
    "Ctrl-B ?: help | s: menu | c: copy | z: zoom | m: swap | e: divider",
    "Ctrl-B l/r/u/d: split | arrows/Tab/o: focus",
    "Ctrl-B n/p: next/previous tab | [/]: previous/next project",
    "Ctrl-B x: close pane (confirm y)",
    "Esc: leave navigation or close help",
    "Ctrl-X: detach; running shells continue",
    "Press Esc or ? to close help",
];

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

struct ScreenSlot {
    target: Option<TargetRef>,
    revision: u64,
    snapshot: ScreenSnapshot,
    history: ScreenSnapshot,
    history_revision: u64,
    history_offset: u16,
    view_epoch: u64,
    history_pending: bool,
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
    /// Persistent local help modal, independent of the NAV timeout.
    help_open: bool,
    /// Navigation mode (entered with Ctrl-B): when it started, if active.
    prefix: Option<Instant>,
    pending: [Key; MAX_KEYS],
    pending_len: usize,
    pending_target: Option<TargetRef>,
    pending_blocked: bool,
    pending_split: Option<(u64, u64, TargetRef)>,
    pending_split_focus: Option<(u64, TargetRef)>,
    pending_close: Option<(u64, TargetRef, bool)>,
    next_request_id: u64,
    focus_generation: u64,
    status: Option<(&'static str, Instant)>,
    host: (u16, u16),
    screens: [ScreenSlot; crate::pane_layout::MAX_PANES],
    selection: Option<FrozenDrag>,
    copy_mode: bool,
    copy_anchor: Option<Point>,
    copy_exit_key: Option<(KeyCode, KeyModifiers)>,
    pane_mode_exit_key: Option<(KeyCode, KeyModifiers)>,
    pending_copy: Option<String>,
    workspace_index: Option<WorkspaceIndex>,
    current_route: Option<WorkspaceRouteId>,
    picker_selection: Option<WorkspaceRouteId>,
    catalog_refresh_requested: bool,
    route_request: Option<WorkspaceRouteId>,
    route_switching: bool,
    route_switch_notice: Option<&'static str>,
    dirty: bool,
}

impl App {
    /// Preallocate both screen caches for the largest supported pane.
    pub(crate) fn new(host_rows: u16, host_columns: u16) -> Result<Self, TerminalError> {
        let size = TerminalSize::new(MAX_ROOT_ROWS, MAX_ROOT_COLUMNS)?;
        let slot = || -> Result<ScreenSlot, TerminalError> {
            Ok(ScreenSlot {
                target: None,
                revision: 0,
                snapshot: ScreenSnapshot::try_new(size)?,
                history: ScreenSnapshot::try_new(size)?,
                history_revision: 0,
                history_offset: 0,
                view_epoch: 0,
                history_pending: false,
            })
        };
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
            host: (host_rows, host_columns),
            screens: [slot()?, slot()?, slot()?, slot()?],
            selection: None,
            copy_mode: false,
            copy_anchor: None,
            copy_exit_key: None,
            pane_mode_exit_key: None,
            pending_copy: None,
            workspace_index: None,
            current_route: None,
            picker_selection: None,
            catalog_refresh_requested: false,
            route_request: None,
            route_switching: false,
            route_switch_notice: None,
            dirty: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn layout(&self) -> Option<&LayoutReply> {
        self.layout.as_ref()
    }

    pub(crate) fn focus(&self) -> Option<TargetRef> {
        self.focus
    }

    /// Enable the route strip from an immutable metadata snapshot. The App
    /// keeps only the bounded index model and never opens routes or storage.
    pub(crate) fn install_workspace_tabs(
        &mut self,
        index: WorkspaceIndex,
        current: WorkspaceRouteId,
    ) {
        if workspace_tabs::layout(&index, current, 0, 0).is_err() {
            return;
        }
        if self.current_route != Some(current) {
            self.cancel_selection();
        }
        self.workspace_index = Some(index);
        self.current_route = Some(current);
        self.picker_selection = None;
        self.route_request = None;
        self.route_switching = false;
        self.route_switch_notice = None;
        self.dirty = true;
    }

    /// Replace the immutable catalog without disturbing an in-flight route
    /// request or its input freeze. An invalid active route leaves the last
    /// valid snapshot and chooser untouched.
    pub(crate) fn update_workspace_catalog(&mut self, index: WorkspaceIndex) -> bool {
        let (Some(current), Some(_)) = (self.current_route, self.workspace_index.as_ref()) else {
            return false;
        };
        if workspace_tabs::layout(&index, current, 0, 0).is_err() {
            return false;
        }
        self.workspace_index = Some(index);
        if self
            .picker_selection
            .is_some_and(|selection| !self.route_exists(selection))
        {
            self.picker_selection = Some(current);
        }
        self.dirty = true;
        true
    }

    /// Take the coalesced request to refresh metadata after the chooser opens.
    pub(crate) fn take_catalog_refresh_request(&mut self) -> bool {
        std::mem::replace(&mut self.catalog_refresh_requested, false)
    }

    fn route_exists(&self, route: WorkspaceRouteId) -> bool {
        self.workspace_index
            .as_ref()
            .and_then(|index| index.project(route.project).ok())
            .is_some_and(|project| project.tabs().any(|tab| tab.id == route.tab))
    }

    fn open_workspace_picker(&mut self) {
        self.cancel_selection();
        self.picker_selection = self.current_route;
        self.prefix = None;
        self.catalog_refresh_requested = true;
        self.dirty = true;
    }

    /// Take the one typed destination selected by the user, if any.
    pub(crate) fn take_route_request(&mut self) -> Option<WorkspaceRouteId> {
        self.route_request.take()
    }

    /// Whether the host must keep resize and child-bound work local.
    pub(crate) const fn route_switching(&self) -> bool {
        self.route_switching
    }

    pub(crate) const fn pane_mutations_blocked(&self) -> bool {
        self.pane_mode.is_some() || self.pending_pane_operation.is_some()
    }

    /// Freeze child effects while the host prepares a fresh route attachment.
    pub(crate) fn begin_route_switch(&mut self, now: Instant) {
        if self.route_switching {
            return;
        }
        // An admitted server operation is not rolled back, but its stale UI
        // completion must not be applied to the destination route.
        self.pane_mode = None;
        self.pending_pane_operation = None;
        self.pane_operations = None;
        self.route_switching = true;
        self.prefix = None;
        self.picker_selection = None;
        self.cancel_selection();
        self.pending_copy = None;
        let abandoned = self.abandon_pending_keys();
        let notice = if abandoned {
            "workspace switch preparing; queued keys discarded"
        } else {
            "workspace switch preparing"
        };
        self.route_switch_notice = Some(notice);
        self.set_status(notice, now);
        self.dirty = true;
    }

    /// Resume the active App after preparation failure or successful install.
    pub(crate) fn finish_route_switch(&mut self, failed: bool, now: Instant) {
        self.route_switching = false;
        self.route_switch_notice = None;
        self.route_request = None;
        self.picker_selection = None;
        self.prefix = None;
        if failed {
            self.set_status("workspace switch failed", now);
        }
        self.dirty = true;
    }

    #[cfg(test)]
    pub(crate) fn menu_open(&self) -> bool {
        self.menu.is_open()
    }

    pub(crate) fn needs_recovery_availability(&self, event: &Event) -> bool {
        if self.copy_mode
            || self.route_switching
            || self.pane_mode.is_some()
            || self.pending_pane_operation.is_some()
            || self.help_open
            || self.picker_selection.is_some()
            || (self.workspace_index.is_some()
                && matches!(event, Event::Mouse(mouse) if mouse.row == 0))
            || self.confirm.is_open()
            || self.menu.is_open()
        {
            return false;
        }
        match event {
            Event::Mouse(mouse) => matches!(mouse_button(mouse), Some((true, true))),
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
        } else if self.workspace_index.is_some() {
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
        let (rows, columns) = self.screen_viewport();
        workspace_tabs::layout(
            self.workspace_index.as_ref()?,
            self.current_route?,
            rows,
            columns,
        )
        .ok()
    }

    fn route_at_index(&self, offset: isize, from: WorkspaceRouteId) -> Option<WorkspaceRouteId> {
        let index = self.workspace_index.as_ref()?;
        let routes: Vec<_> = index
            .projects()
            .flat_map(|project| {
                project.tabs().map(move |tab| WorkspaceRouteId {
                    project: project.id(),
                    tab: tab.id,
                })
            })
            .collect();
        let current = routes.iter().position(|route| *route == from)?;
        let len = isize::try_from(routes.len()).ok()?;
        if len == 0 {
            return None;
        }
        let next = (isize::try_from(current).ok()? + offset).rem_euclid(len);
        routes.get(usize::try_from(next).ok()?).copied()
    }

    fn request_route(&mut self, route: WorkspaceRouteId, now: Instant) {
        if self.route_switching {
            return;
        }
        self.cancel_selection();
        self.picker_selection = None;
        self.prefix = None;
        self.dirty = true;
        if self.current_route == Some(route) {
            return;
        }
        self.route_request = Some(route);
        self.begin_route_switch(now);
    }

    fn navigate_workspace(&mut self, action: TabNavigation, now: Instant) {
        let (Some(index), Some(current)) = (&self.workspace_index, self.current_route) else {
            return;
        };
        if let Ok(route) = workspace_tabs::navigate(index, current, action) {
            self.request_route(route, now);
        }
    }

    /// Root size to request from the server, or `None` below the minimum.
    pub(crate) fn desired_root(&self) -> Option<(u16, u16)> {
        let (rows, columns) = self.viewport();
        (rows >= MIN_ROOT_ROWS && columns >= MIN_ROOT_COLUMNS).then_some((rows, columns))
    }

    pub(crate) fn set_host(&mut self, rows: u16, columns: u16) {
        if self.host != (rows, columns) {
            self.pane_mode = None;
            self.cancel_selection();
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
        if self.layout.is_some_and(|layout| {
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
        if !self.focus.is_some_and(present) {
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
            self.confirm = CloseConfirm::new();
            self.status = Some(("pane already closed", Instant::now()));
        }
        for slot in &mut self.screens {
            if let Some(target) = slot.target {
                let new_pane = reply
                    .panes
                    .iter()
                    .flatten()
                    .find(|pane| pane.target == target);
                if let Some(pane) = new_pane {
                    let old_rect = self.layout.and_then(|layout| {
                        layout
                            .panes
                            .iter()
                            .flatten()
                            .find(|old| old.target == target)
                            .map(|old| old.rect)
                    });
                    if old_rect != Some(pane.rect) {
                        slot.history_offset = 0;
                        slot.history_revision = 0;
                        slot.view_epoch = slot.view_epoch.wrapping_add(1);
                        slot.history_pending = false;
                    }
                } else {
                    slot.target = None;
                    slot.revision = 0;
                    slot.history_offset = 0;
                    slot.history_revision = 0;
                    slot.view_epoch = slot.view_epoch.wrapping_add(1);
                    slot.history_pending = false;
                }
            }
        }
        self.layout = Some(reply);
        if let Some((generation, target)) = self.pending_split_focus {
            let present = reply
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target);
            if generation != self.focus_generation {
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
        let Some((active_id, admitted_focus, source)) = self.pending_split else {
            return;
        };
        if active_id != request_id || admitted_focus != focus_generation || source != target {
            return;
        }
        self.pending_split = None;
        match result {
            Ok(new_target) => {
                if self.focus_generation == focus_generation {
                    self.pending_split_focus = Some((focus_generation, new_target));
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
            Err(label) => self.set_status(label, now),
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
                if matches!(
                    result.outcome,
                    CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated)
                ) {
                    self.forget_pane(target);
                }
                self.set_status(close_message(&result, last_pane), now);
            }
            Err(label) => self.set_status(label, now),
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
                    self.set_status("pane operation applied; newer layout retained", now);
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
                self.set_status(
                    if update.outcome == Outcome::Partial {
                        "pane operation applied; resize pending"
                    } else {
                        "pane operation applied"
                    },
                    now,
                );
            }
            Ok(_) => self.set_status("pane view unavailable", now),
            Err(_) => self.set_status("pane operation status unknown; layout unchanged", now),
        }
        self.dirty = true;
    }

    pub(crate) fn report_worker_status(&mut self, label: &'static str, now: Instant) {
        self.set_status(label, now);
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
                        Some(wraps) => self.load_screen_with_wraps(
                            screen.target,
                            screen.revision,
                            &checkpoint,
                            Some(wraps),
                        ),
                        None => self.load_screen(screen.target, screen.revision, &checkpoint),
                    };
                    if loaded.is_err() {
                        self.set_status("screen invalid", now);
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
        let Some(slot) = self
            .screens
            .iter_mut()
            .find(|slot| slot.target == Some(update.target) && slot.view_epoch == update.epoch)
        else {
            return;
        };
        slot.history_pending = false;
        match update.result {
            ScrollResult::History {
                revision,
                offset_rows,
                checkpoint,
                row_wraps,
            } => match panelink::screen_checkpoint::decode(&checkpoint) {
                Ok(snapshot) if offset_rows > 0 => {
                    if slot.history.load_checkpoint(&snapshot).is_ok() {
                        if let Some(wraps) = row_wraps.as_deref() {
                            if slot.history.apply_row_wraps(wraps).is_err() {
                                slot.history.clear_row_wraps();
                                self.set_status("history snapshot invalid", now);
                                return;
                            }
                        }
                        slot.history_offset = offset_rows;
                        slot.history_revision = revision;
                        self.dirty = true;
                    } else {
                        self.set_status("history snapshot invalid", now);
                    }
                }
                Ok(_) => {
                    slot.history.clear_row_wraps();
                    slot.history_offset = 0;
                    slot.history_revision = revision;
                    self.dirty = true;
                }
                Err(_) => self.set_status("history snapshot invalid", now),
            },
            ScrollResult::MouseDelivered => {
                slot.history_offset = 0;
                slot.history_revision = 0;
                self.dirty = true;
            }
            ScrollResult::AlternateScreenNoMouse => {
                slot.history_offset = 0;
                slot.history_revision = 0;
                self.dirty = true;
                self.set_status("alternate-screen app; use its scroll keys", now);
            }
            ScrollResult::Failed(label) => self.set_status(label, now),
        }
    }

    fn return_to_live(&mut self, target: TargetRef) {
        if let Some(slot) = self
            .screens
            .iter_mut()
            .find(|slot| slot.target == Some(target))
        {
            slot.history_offset = 0;
            slot.history_revision = 0;
            slot.history_pending = false;
            slot.view_epoch = slot.view_epoch.wrapping_add(1);
            self.dirty = true;
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
        let column = screen_x
            .saturating_sub(content.x)
            .min(content.columns.saturating_sub(1));
        let row = screen_y
            .saturating_sub(content.y)
            .min(content.rows.saturating_sub(1));
        let Some(slot_index) = self
            .screens
            .iter()
            .position(|slot| slot.target == Some(target))
            .or_else(|| self.screens.iter().position(|slot| slot.target.is_none()))
        else {
            return;
        };
        let slot = &mut self.screens[slot_index];
        if slot.target != Some(target) {
            slot.target = Some(target);
            slot.revision = 0;
            slot.history_offset = 0;
            slot.history_revision = 0;
        }
        // A single large history checkpoint may be in flight. Ignore further
        // wheel ticks until its result is applied so a dropped/coalesced event
        // cannot invalidate the request that is already on the wire.
        if slot.history_pending {
            return;
        }
        let requested = if up {
            slot.history_offset
                .saturating_add(3)
                .min(panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS)
        } else {
            slot.history_offset.saturating_sub(3)
        };
        let epoch = slot.view_epoch.wrapping_add(1);
        slot.history_pending = true;
        self.dirty = true;
        match effects.scroll(target, epoch, requested, column, row, up) {
            Ok(()) => slot.view_epoch = epoch,
            Err(label) => {
                slot.history_pending = false;
                self.set_status(label, now);
            }
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
    pub(crate) fn load_screen(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
    ) -> Result<(), TerminalError> {
        self.load_screen_with_wraps(target, revision, checkpoint, None)
    }

    pub(crate) fn load_screen_with_wraps(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
        row_wraps: Option<&[bool]>,
    ) -> Result<(), TerminalError> {
        if self.selection.as_ref().is_some_and(|drag| {
            drag.target == target
                && (row_wraps.is_none()
                    || (self.copy_mode && row_wraps.is_some_and(|wraps| drag.row_wraps != wraps)))
        }) {
            self.cancel_selection();
        }
        let index = self
            .screens
            .iter()
            .position(|slot| slot.target == Some(target))
            .or_else(|| self.screens.iter().position(|slot| slot.target.is_none()))
            .ok_or(TerminalError::InvalidSize)?;
        let buffer_changed = {
            let slot = &mut self.screens[index];
            let old_buffer =
                (slot.target == Some(target)).then(|| slot.snapshot.alternate_screen());
            slot.snapshot.load_checkpoint(checkpoint)?;
            if let Some(wraps) = row_wraps {
                slot.snapshot.apply_row_wraps(wraps)?;
            }
            slot.target = Some(target);
            slot.revision = revision;
            let buffer_changed =
                old_buffer.is_some_and(|old| old != slot.snapshot.alternate_screen());
            if buffer_changed {
                slot.history_offset = 0;
                slot.history_pending = false;
                slot.view_epoch = slot.view_epoch.wrapping_add(1);
            }
            buffer_changed
        };
        if buffer_changed {
            self.cancel_selection();
        }
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn set_status(&mut self, label: &'static str, now: Instant) {
        self.status = Some((label, now));
        self.dirty = true;
    }

    pub(crate) fn take_copy_text(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    pub(crate) fn report_clipboard_unavailable(&mut self, now: Instant) {
        self.set_status("clipboard unavailable", now);
    }

    pub(crate) fn cancel_for_detach(&mut self) {
        self.cancel_selection();
        self.pending_copy = None;
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

    fn queue_copy_text(&mut self, text: String, now: Instant) {
        if self.pending_copy.is_some() {
            self.set_status("clipboard busy", now);
        } else {
            self.pending_copy = Some(text);
            self.set_status("copying selection", now);
        }
    }

    fn begin_copy_mode(&mut self, now: Instant) {
        let Some(target) = self.focus else {
            self.set_status("copy unavailable", now);
            return;
        };
        let Some((slot, content, snapshot, source_history, epoch)) = self.visible_source(target)
        else {
            self.set_status("copy unavailable", now);
            return;
        };
        if snapshot.row_wraps().is_none() || content.rows == 0 || content.columns == 0 {
            self.set_status("copy unavailable", now);
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
            Err(_) => self.set_status("copy unavailable", now),
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
                        self.set_status("copy unavailable", now);
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
            Ok(text) => self.queue_copy_text(text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => self.set_status("copy unavailable", now),
            Err(SelectionError::TooLarge) => self.set_status("copy too large", now),
            Err(_) => self.set_status("copy cancelled", now),
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
        if self.help_open || self.confirm.is_open() || self.menu.is_open() {
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
                if self.route_switching {
                    return Flow::Quit;
                }
                self.flush(now, effects);
                return Flow::Quit;
            }
        }
        if self.route_switching {
            // Host resize and repaint remain local while a destination is
            // prepared. Every other event is consumed without child effects.
            if matches!(event, Event::Resize(_, _)) {
                self.dirty = true;
            }
            return Flow::Continue;
        }
        if self.pane_mode.is_some() {
            self.handle_pane_mode_event(event, now, effects);
            return Flow::Continue;
        }
        if self.help_open {
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
            match self.confirm.handle(confirm_input(event)) {
                ConfirmResponse::Confirmed(intent) => self.close_pane(intent, now, effects),
                ConfirmResponse::Cancelled => self.set_status("close cancelled", now),
                ConfirmResponse::Consumed | ConfirmResponse::NotOpen => {}
            }
            return Flow::Continue;
        }
        if self.menu.is_open() {
            let mut input = if self.workspace_index.is_some()
                && matches!(event, Event::Mouse(mouse) if mouse.row == 0)
            {
                panelink::split_menu::MenuInput::Other
            } else {
                menu_input(event)
            };
            if self.workspace_index.is_some() {
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
                MenuResponse::Activated(intent) => self.activate(intent, now, effects),
                // The menu's mirror revision is local; use the server
                // revision captured when the menu opened.
                MenuResponse::CloseRequested(intent) => self.confirm.open(CloseIntent {
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
        if self.picker_selection.is_some() {
            self.handle_workspace_picker_event(event, now);
            return Flow::Continue;
        }
        if self.copy_mode {
            return self.handle_copy_mode_event(event, now, effects);
        }
        if self.handle_header_event(event, now) {
            return Flow::Continue;
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
                    let flow =
                        self.navigate(key.code, key.modifiers, now, recovery_availability, effects);
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
                        self.open_at_pointer(mouse.column, row, now, recovery_availability);
                    }
                }
                _ => {
                    if let (Some(up), Some(target)) =
                        (mouse_wheel(mouse), self.pane_at(mouse.column, mouse.row))
                    {
                        self.flush(now, effects);
                        self.focus_target(target);
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
        if self.workspace_index.is_some() && plain && matches!(code, KeyCode::Char('w' | 'W')) {
            self.open_workspace_picker();
            return Flow::Continue;
        }
        if self.workspace_index.is_some() {
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
                    self.open_for_focused(now, recovery_availability);
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
        let zoomed = view.zoomed_target.is_none();
        let target = view.zoomed_target.or(self.focus);
        let Some(target) = target else {
            self.set_status("zoom unavailable: no focused pane", now);
            return;
        };
        self.submit_pane_operation(
            PaneAction::ZoomSet {
                target,
                expected_layout_revision: view.layout.revision,
                zoomed,
            },
            now,
            effects,
        );
    }

    fn open_swap_mode(&mut self, now: Instant, effects: &mut impl Effects) {
        if !self.operation_ready(now, effects) {
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before swapping panes", now);
            return;
        }
        let Some(view) = self.pane_operations else {
            return;
        };
        let Some(source) = self.focus else {
            self.set_status("swap unavailable: no focused pane", now);
            return;
        };
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
            revision: view.layout.revision,
            choices,
            selected: 0,
        });
        self.prefix = None;
        self.dirty = true;
    }

    fn open_divider_mode(&mut self, now: Instant, effects: &mut impl Effects) {
        if !self.operation_ready(now, effects) {
            return;
        }
        if self
            .pane_operations
            .is_some_and(|view| view.zoomed_target.is_some())
        {
            self.set_status("unzoom before adjusting dividers", now);
            return;
        }
        let (Some(view), Some(target), Some(local)) =
            (self.pane_operations, self.focus, self.local.as_ref())
        else {
            self.set_status("divider adjustment unavailable", now);
            return;
        };
        let mut indices = [None; crate::pane_layout::MAX_PANES - 1];
        let mut count = 0;
        let mut initial = None;
        for index in 0..indices.len() {
            let Ok(info) = local.divider_info(target, view.layout.revision, index as u8) else {
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
            revision: view.layout.revision,
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
                self.set_status("pane operation pending", now);
            }
            Err("pane operations unavailable") => {
                self.set_status("pane controls unsupported by this server", now)
            }
            Err(_) => self.set_status("busy: pane operation was not queued", now),
        }
        self.dirty = true;
    }

    fn handle_pane_mode_event(&mut self, event: &Event, now: Instant, effects: &mut impl Effects) {
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
                        if let Some(divider_index) = indices.get(selected).copied().flatten() {
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
        let Some(mut preview) = self.layout.and_then(|layout| local_layout(&layout)) else {
            return;
        };
        if preview
            .set_divider(target, revision, index, candidate)
            .is_ok()
        {
            *first_cells = candidate;
        }
    }

    fn handle_header_event(&mut self, event: &Event, now: Instant) -> bool {
        if self.workspace_index.is_some() {
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
        self.cancel_selection();
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            match layout.hit_test(mouse.column, mouse.row) {
                Some(TabAction::ProjectChooser) => {
                    self.open_workspace_picker();
                }
                Some(TabAction::Select(route)) => self.request_route(route, now),
                Some(TabAction::PreviousTab) => {
                    self.navigate_workspace(TabNavigation::PreviousTab, now)
                }
                Some(TabAction::NextTab) => self.navigate_workspace(TabNavigation::NextTab, now),
                None => {}
            }
        }
        // Every event on row zero is local, including blank strip cells.
        true
    }

    fn handle_workspace_picker_event(&mut self, event: &Event, now: Instant) {
        match event {
            Event::Key(key) if key.is_press() || key.is_repeat() => {
                let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                match (key.code, plain) {
                    (KeyCode::Esc, true) => {
                        self.picker_selection = None;
                        self.dirty = true;
                    }
                    (KeyCode::Enter, true) => {
                        if let Some(route) = self.picker_selection {
                            self.request_route(route, now);
                        }
                    }
                    (KeyCode::Up, true) => self.move_picker_selection(-1),
                    (KeyCode::Down, true) => self.move_picker_selection(1),
                    (KeyCode::Left, true) => self.move_picker_project(-1),
                    (KeyCode::Right, true) => self.move_picker_project(1),
                    _ => {}
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some((top, rows)) = self.picker_screen_geometry() {
                    if mouse.row > top && mouse.row < top.saturating_add(rows) {
                        if let Some(route) = self.picker_route_at_row(mouse.row - top - 1) {
                            self.request_route(route, now);
                        }
                    }
                }
            }
            Event::Resize(_, _) => self.dirty = true,
            _ => {}
        }
    }

    fn picker_routes(&self) -> Vec<WorkspaceRouteId> {
        self.workspace_index
            .as_ref()
            .into_iter()
            .flat_map(|index| index.projects())
            .flat_map(|project| {
                project.tabs().map(move |tab| WorkspaceRouteId {
                    project: project.id(),
                    tab: tab.id,
                })
            })
            .collect()
    }

    fn picker_route_at_row(&self, row: u16) -> Option<WorkspaceRouteId> {
        let offset = self.picker_scroll_offset()?;
        let routes = self.picker_routes();
        routes.get(offset.checked_add(usize::from(row))?).copied()
    }

    fn picker_screen_geometry(&self) -> Option<(u16, u16)> {
        let offset = self.picker_scroll_offset()?;
        let routes = self.picker_routes();
        let (view_rows, columns) = self.viewport();
        if view_rows == 0 || columns == 0 {
            return None;
        }
        let visible_routes = routes
            .len()
            .saturating_sub(offset)
            .min(self.picker_visible_route_count());
        let rows = (visible_routes + 1).min(usize::from(view_rows)) as u16;
        Some((1 + view_rows.saturating_sub(rows) / 2, rows))
    }

    fn picker_visible_route_count(&self) -> usize {
        let (view_rows, _) = self.viewport();
        usize::from(view_rows)
            .min(panelink::split_menu::MAX_OVERLAY_ROWS)
            .saturating_sub(1)
    }

    fn picker_scroll_offset(&self) -> Option<usize> {
        let selection = self.picker_selection?;
        let routes = self.picker_routes();
        let selected = routes.iter().position(|route| *route == selection)?;
        let visible = self.picker_visible_route_count();
        Some(selected.saturating_sub(visible.saturating_sub(1)))
    }

    fn move_picker_selection(&mut self, delta: isize) {
        if let Some(selection) = self.picker_selection {
            self.picker_selection = self.route_at_index(delta, selection);
            self.dirty = true;
        }
    }

    fn move_picker_project(&mut self, delta: isize) {
        let (Some(index), Some(selection)) = (&self.workspace_index, self.picker_selection) else {
            return;
        };
        let action = if delta < 0 {
            TabNavigation::PreviousProject
        } else {
            TabNavigation::NextProject
        };
        if let Ok(route) = workspace_tabs::navigate(index, selection, action) {
            self.picker_selection = Some(route);
            self.dirty = true;
        }
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
        let Some(focus) = self.focus else {
            self.set_status("close unavailable: no focused pane", now);
            return;
        };
        let Some(index) = layout
            .panes
            .iter()
            .position(|pane| pane.is_some_and(|pane| pane.target == focus))
        else {
            self.set_status("close unavailable: no focused pane", now);
            return;
        };
        self.confirm.open(CloseIntent {
            target: focus,
            expected_layout_revision: layout.revision,
            position: u8::try_from(index + 1).unwrap_or(u8::MAX),
            last_pane: layout.panes.iter().flatten().count() <= 1,
        });
        self.dirty = true;
    }

    /// Send one confirmed close (one fresh key; never retried) and report
    /// the server's result. After a confirmed stop, local focus and batched
    /// keys leave the closed pane at once; the authoritative collapsed layout
    /// arrives with the next layout poll.
    fn close_pane(&mut self, intent: CloseIntent, now: Instant, effects: &mut impl Effects) {
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
            Ok(None) => self.set_status("closing pane…", now),
            Err(label) => {
                self.pending_close = None;
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
        if self.route_switching {
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

    fn pane_row(&self, screen_row: u16) -> Option<u16> {
        if self.workspace_index.is_some() {
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
        let point = Point {
            row: y
                .saturating_sub(content.y)
                .min(content.rows.saturating_sub(1)),
            column: x
                .saturating_sub(content.x)
                .min(content.columns.saturating_sub(1)),
        };
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
            Err(SelectionError::TooLarge) => self.set_status("selection too large", now),
            Err(_) => self.set_status("selection unavailable", now),
        }
    }

    fn selection_point(drag: &FrozenDrag, x: u16, y: u16) -> Point {
        Point {
            row: y
                .saturating_sub(drag.content_rect.y)
                .min(drag.content_rect.rows.saturating_sub(1)),
            column: x
                .saturating_sub(drag.content_rect.x)
                .min(drag.content_rect.columns.saturating_sub(1)),
        }
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
            Ok(text) => self.queue_copy_text(text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => {
                self.set_status("selection unavailable: line wrapping unknown", now)
            }
            Err(SelectionError::TooLarge) => self.set_status("selection too large", now),
            Err(_) => self.set_status("selection cancelled", now),
        }
    }

    fn open_at_pointer(
        &mut self,
        x: u16,
        y: u16,
        now: Instant,
        recovery_availability: Availability,
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
            self.menu_revision = layout.revision;
            self.dirty = true;
        }
    }

    fn open_for_focused(&mut self, now: Instant, recovery_availability: Availability) {
        self.cancel_selection();
        let (Some(local), Some(layout), Some(focus), true) =
            (self.local.as_ref(), self.layout, self.focus, self.fits())
        else {
            self.set_status("menu unavailable: terminal too small", now);
            return;
        };
        let (rows, columns) = self.viewport();
        if self
            .menu
            .open_for_focused_with_recovery(local, focus, rows, columns, recovery_availability)
            .is_ok()
        {
            self.menu_revision = layout.revision;
            self.dirty = true;
        }
    }

    /// Turn a captured intent into one split. The local `PaneLayout` mirror
    /// always starts at revision 1, so the server revision captured together
    /// with the menu (`menu_revision`) replaces the mirror's revision.
    fn activate(&mut self, intent: SplitIntent, now: Instant, effects: &mut impl Effects) {
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
        let Ok((rows, columns)) = planned_split_size(&layout, intent.target, intent.direction)
        else {
            self.set_status("split unavailable: layout changed; reopen the menu", now);
            return;
        };
        let plan = SplitPlan {
            target: intent.target,
            expected_layout_revision: self.menu_revision,
            direction: intent.direction,
            rows,
            columns,
        };
        let request_id = self.next_request_id();
        let focus_generation = self.focus_generation;
        self.pending_split = Some((request_id, focus_generation, plan.target));
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
            Ok(None) => self.set_status("split pending", now),
            Err(label) => {
                self.pending_split = None;
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
        let header_enabled = self.workspace_index.is_some();
        let header_offset = if header_enabled { 1 } else { 0 };
        let (host_rows, host_columns) = if header_enabled {
            self.screen_viewport()
        } else {
            self.host
        };
        if header_enabled && (host_rows == 0 || host_columns == 0) {
            return Ok(());
        }
        let mut panes = [None; crate::pane_layout::MAX_PANES];
        let mut divider = None;
        let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
        let mut focused = None;
        let fits = self.fits();
        let display_layout = self.display_layout();
        if let (Some(layout), true) = (display_layout, fits) {
            for (index, pane) in layout.panes.iter().enumerate() {
                let Some(pane) = pane else { continue };
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
                    (false, Some(slot), Some(_)) if slot.history_pending => {
                        PaneContent::Placeholder("loading scrollback...")
                    }
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
        let confirm_line = self.confirm.overlay_line(view_columns);
        let mut lines = [OverlayLine {
            text: "",
            style: OverlayStyle::Normal,
        }; panelink::split_menu::MAX_OVERLAY_ROWS];
        let mut picker_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let picker_count = if self.picker_selection.is_some() {
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
                    == self.picker_selection.as_ref()
                {
                    OverlayStyle::Selected
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
                    "Swap pane: arrows/hjkl choose, Enter confirms, Esc cancels".into();
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
                pane_mode_text[0] = format!("Divider {} ({axis}) first span {first_cells}; Tab changes ancestor, Enter saves, Esc cancels", index + 1);
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
                count
            }
            None => 0,
        };
        let overlay = if self.help_open && view_rows > 0 && view_columns > 0 {
            let help_lines = if header_enabled {
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
        } else if let (Some(line), true) = (confirm_line.as_ref(), view_rows > 0) {
            // The y/n prompt takes precedence over every notice.
            lines[0] = OverlayLine {
                text: line.text(),
                style: compose_style(line.style()),
            };
            Some((
                Rect {
                    x: 0,
                    y: view_rows - 1,
                    rows: 1,
                    columns: view_columns,
                },
                1,
            ))
        } else if let (Some((top, rows)), true) = (
            self.picker_screen_geometry(),
            self.picker_selection.is_some(),
        ) {
            Some((
                Rect {
                    x: 0,
                    y: top.saturating_sub(1),
                    rows,
                    columns: view_columns,
                },
                usize::from(rows).min(picker_count),
            ))
        } else {
            let notice = if self.copy_mode {
                None
            } else if self.route_switching {
                Some((
                    self.route_switch_notice
                        .unwrap_or("workspace switch preparing"),
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
        if let Some(caret) = caret {
            return compositor.render_with_copy_mode(
                &frame,
                selection,
                caret,
                tabs_layout.as_ref().map(|layout| layout.header_cells()),
                output,
            );
        }
        if let Some(tabs_layout) = tabs_layout.as_ref() {
            compositor.render_with_header(
                &frame,
                selection,
                Some(tabs_layout.header_cells()),
                output,
            )
        } else {
            compositor.render_with_selection(&frame, selection, output)
        }
    }

    fn picker_overlay_text(
        &self,
        lines: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
    ) -> usize {
        let routes = self.picker_routes();
        let Some(selected) = self.picker_selection else {
            return 0;
        };
        let Some(offset) = self.picker_scroll_offset() else {
            return 0;
        };
        let Some(index) = self.workspace_index.as_ref() else {
            return 0;
        };
        let Some(project) = index.project(selected.project).ok() else {
            return 0;
        };
        lines[0] = format!("Project: {}  (Esc closes)", project.display_name());
        let visible = self
            .picker_visible_route_count()
            .min(lines.len().saturating_sub(1));
        let mut count = 1;
        for route in routes.iter().skip(offset).take(visible) {
            let Some(project) = index.project(route.project).ok() else {
                continue;
            };
            let Some(tab) = project.tabs().find(|tab| tab.id == route.tab) else {
                continue;
            };
            let active = self.current_route == Some(*route);
            let selected = self.picker_selection == Some(*route);
            lines[count] = format!(
                "{}{} / {}",
                if selected {
                    "> "
                } else if active {
                    "* "
                } else {
                    "  "
                },
                project.display_name(),
                tab.display_name
            );
            count += 1;
        }
        count
    }
}

fn shifted_rect(mut rect: Rect, rows: u16) -> Rect {
    rect.y = rect.y.saturating_add(rows);
    rect
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
