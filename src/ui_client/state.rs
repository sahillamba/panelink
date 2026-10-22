//! Pure presentation state for the attach client: layout, local focus, menu,
//! key batching, transient status and frame building.
//!
//! All network effects go through the [`Effects`] trait so the routing rules
//! can be tested with a recording fake. No host IO happens here.

use std::io;
use std::time::{Duration, Instant};

use crossterm_input::{Event, KeyCode, KeyModifiers};
use panelink::close_protocol::{CloseOutcome, CloseRejection, CloseResult, StopOutcome};
use panelink::control_protocol::TargetRef;
use panelink::pane_layout::{PaneLayout, Rect, SplitDirection};
use panelink::screen_checkpoint::ScreenCheckpoint;
use panelink::split_menu::{
    Availability, CloseConfirm, CloseIntent, ConfirmResponse, MenuResponse, MenuState,
    OverlayStyle as MenuStyle, SplitIntent,
};
use panelink::terminal::compose::{
    ComposeFrame, Compositor, OverlayLine, OverlayStyle, OverlayView, PaneContent, PaneView,
};
use panelink::terminal::{Key, ScreenSnapshot, TerminalError, TerminalSize};
use panelink::ui_protocol::{planned_split_size, LayoutReply, MAX_KEYS};

use super::keys::{confirm_input, map_key, menu_input, mouse_button, KeyAction};
use super::net::NetUpdate;

/// Largest root the client requests (terminal-core and compositor bound).
pub(crate) const MAX_ROOT_ROWS: u16 = 80;
/// Largest root the client requests.
pub(crate) const MAX_ROOT_COLUMNS: u16 = 240;
const MIN_ROOT_ROWS: u16 = 5;
const MIN_ROOT_COLUMNS: u16 = 20;
const STATUS_TTL: Duration = Duration::from_secs(3);
/// Navigation mode ends by itself after this long without a key.
const NAV_TIMEOUT: Duration = Duration::from_secs(4);
/// Bottom banner shown while navigation mode (after Ctrl-B) is active.
pub(crate) const NAV_BANNER: &str =
    " NAV  ? help | s menu | v saved | l r u d split | arrows/o focus | x close | ^X detach | Esc ";
const TOO_SMALL: &str = "Panelink: terminal too small for the pane layout; enlarge it";
const HELP_LINES: [&str; 8] = [
    "Panelink help",
    "Ctrl-B ?: help | Ctrl-B s: pane menu",
    "Ctrl-B l/r/u/d: split | arrows/Tab/o: focus",
    "Ctrl-B x: close pane (confirm y)",
    "Esc: leave navigation or close help",
    "Ctrl-X: detach; running shells continue",
    "Run Panelink again or attach to reconnect",
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
}

/// Client presentation state; PTYs and sessions stay server-owned.
pub(crate) struct App {
    layout: Option<LayoutReply>,
    local: Option<PaneLayout>,
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
    screens: [ScreenSlot; 2],
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
            })
        };
        Ok(Self {
            layout: None,
            local: None,
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
            screens: [slot()?, slot()?],
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

    #[cfg(test)]
    pub(crate) fn menu_open(&self) -> bool {
        self.menu.is_open()
    }

    pub(crate) fn needs_recovery_availability(&self, event: &Event) -> bool {
        if self.confirm.is_open() || self.menu.is_open() {
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
        (
            self.host.0.min(MAX_ROOT_ROWS),
            self.host.1.min(MAX_ROOT_COLUMNS),
        )
    }

    /// Root size to request from the server, or `None` below the minimum.
    pub(crate) fn desired_root(&self) -> Option<(u16, u16)> {
        let (rows, columns) = self.viewport();
        (rows >= MIN_ROOT_ROWS && columns >= MIN_ROOT_COLUMNS).then_some((rows, columns))
    }

    pub(crate) fn set_host(&mut self, rows: u16, columns: u16) {
        if self.host != (rows, columns) {
            self.host = (rows, columns);
            let (rows, columns) = self.viewport();
            self.menu.set_viewport(rows, columns);
            self.dirty = true;
        }
    }

    fn fits(&self) -> bool {
        self.layout
            .is_some_and(|layout| layout.rows <= self.host.0 && layout.columns <= self.host.1)
    }

    /// Adopt one server layout observation. Focus is kept when its pane is
    /// still present, otherwise it moves to the first pane. An open menu
    /// keeps its captured target and revision; the server rejects stale use.
    ///
    /// When a pane leaves the layout (explicit close or its shell exited and
    /// the server collapsed it), batched keys typed for it are dropped (never
    /// redirected to another pane), an open close prompt for it is cancelled,
    /// and its screen cache is released. The compositor repaints fully
    /// because pane rectangles and the divider change.
    pub(crate) fn set_layout(&mut self, reply: LayoutReply) {
        if let Some(current) = self.layout {
            let current_generation = current
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| pane.target.generation);
            let reply_generation = reply
                .panes
                .iter()
                .flatten()
                .next()
                .map(|pane| pane.target.generation);
            if current_generation == reply_generation && reply.revision < current.revision {
                return;
            }
        }
        if self.layout == Some(reply) {
            return;
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
            if slot.target.is_some_and(|target| !present(target)) {
                slot.target = None;
                slot.revision = 0;
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
        if let Some(layout) = update.layout {
            self.set_layout(layout);
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
                    if self
                        .load_screen(screen.target, screen.revision, &checkpoint)
                        .is_err()
                    {
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
        let index = self
            .screens
            .iter()
            .position(|slot| slot.target == Some(target))
            .or_else(|| self.screens.iter().position(|slot| slot.target.is_none()))
            .ok_or(TerminalError::InvalidSize)?;
        let slot = &mut self.screens[index];
        slot.snapshot.load_checkpoint(checkpoint)?;
        slot.target = Some(target);
        slot.revision = revision;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn set_status(&mut self, label: &'static str, now: Instant) {
        self.status = Some((label, now));
        self.dirty = true;
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
            if (key.is_press() || key.is_repeat()) && map_key(*key) == KeyAction::Quit {
                self.help_open = false;
                self.prefix = None;
                self.flush(now, effects);
                return Flow::Quit;
            }
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
            let response = self.menu.handle(menu_input(event));
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
                    return self.navigate(
                        key.code,
                        key.modifiers,
                        now,
                        recovery_availability,
                        effects,
                    );
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
                        if let Err(label) = effects.send_paste(target, text) {
                            self.set_status(label, now);
                        }
                    }
                    None => self.set_status("no focused pane", now),
                }
            }
            Event::Mouse(mouse) => match mouse_button(mouse) {
                Some((true, false)) => {
                    if let Some(target) = self.pane_at(mouse.column, mouse.row) {
                        self.flush(now, effects);
                        self.focus_target(target);
                    }
                }
                Some((true, true)) => {
                    self.flush(now, effects);
                    self.open_at_pointer(mouse.column, mouse.row, now, recovery_availability);
                }
                // Mouse events never reach children in this phase.
                _ => {}
            },
            Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => {}
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
            KeyCode::Char('s' | 'S') if plain => self.open_for_focused(now, recovery_availability),
            KeyCode::Char('v' | 'V') if plain => {
                if recovery_availability == Availability::Enabled {
                    return Flow::ViewRecovery;
                }
                self.set_status("No previous layout saved", now);
            }
            KeyCode::Char('x' | 'X') if plain => self.confirm_close_focused(now),
            // Ctrl-X detaches, also from navigation mode.
            KeyCode::Char('x' | 'X') if modifiers == KeyModifiers::CONTROL => {
                self.flush(now, effects);
                return Flow::Quit;
            }
            // Esc, Ctrl-B again, or anything else just leaves the mode.
            _ => {}
        }
        Flow::Continue
    }

    /// Split the focused pane directly in `direction` (same checks and
    /// captured-revision rules as the menu).
    fn split_focused(
        &mut self,
        direction: SplitDirection,
        now: Instant,
        effects: &mut impl Effects,
    ) {
        let (Some(local), Some(layout), Some(focus), true) =
            (self.local.as_ref(), self.layout, self.focus, self.fits())
        else {
            self.set_status("split unavailable: terminal too small", now);
            return;
        };
        if local.can_split(focus, direction).is_err() {
            let label = if layout.panes.iter().flatten().count() >= 2 {
                "split unavailable: pane limit reached (2)"
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
            position: if index == 0 { 1 } else { 2 },
            last_pane: layout.panes.iter().flatten().count() <= 1,
        });
        self.dirty = true;
    }

    /// Send one confirmed close (one fresh key; never retried) and report
    /// the server's result. After a confirmed stop, local focus and batched
    /// keys leave the closed pane at once; the authoritative collapsed layout
    /// arrives with the next layout poll.
    fn close_pane(&mut self, intent: CloseIntent, now: Instant, effects: &mut impl Effects) {
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
        let Some(layout) = self.layout else { return };
        let panes: Vec<_> = layout.panes.iter().flatten().copied().collect();
        if panes.len() < 2 {
            return;
        }
        let Some(current) = panes
            .iter()
            .position(|pane| Some(pane.target) == self.focus)
        else {
            self.focus = Some(panes[0].target);
            return;
        };
        let other = panes[1 - current];
        let here = panes[current].rect;
        let there = other.rect;
        let allowed = match movement {
            FocusMove::Next => true,
            FocusMove::Left => there.x < here.x,
            FocusMove::Right => there.x > here.x,
            FocusMove::Up => there.y < here.y,
            FocusMove::Down => there.y > here.y,
        };
        if allowed {
            self.set_focus(Some(other.target));
        }
    }

    /// Send any batched keys to the pane they were typed into.
    pub(crate) fn flush(&mut self, now: Instant, effects: &mut impl Effects) {
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
        self.layout?
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.rect.contains(x, y))
            .map(|pane| pane.target)
    }

    fn open_at_pointer(
        &mut self,
        x: u16,
        y: u16,
        now: Instant,
        recovery_availability: Availability,
    ) {
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
        let (host_rows, host_columns) = self.host;
        let mut panes = [None, None];
        let mut divider = None;
        let mut focused = None;
        let fits = self.fits();
        if let (Some(layout), true) = (self.layout, fits) {
            for (index, pane) in layout.panes.iter().enumerate() {
                let Some(pane) = pane else { continue };
                let screen = self
                    .screens
                    .iter()
                    .find(|slot| slot.target == Some(pane.target));
                let content = match (pane.pending_confirmed, screen) {
                    (Some(_), _) => PaneContent::Placeholder("resize pending"),
                    (None, Some(slot))
                        if slot.snapshot.rows() == pane.rect.rows
                            && slot.snapshot.columns() == pane.rect.columns =>
                    {
                        PaneContent::Snapshot(&slot.snapshot)
                    }
                    (None, Some(_)) => PaneContent::Placeholder("resizing..."),
                    (None, None) => PaneContent::Placeholder("connecting..."),
                };
                panes[index] = Some(PaneView {
                    rect: pane.rect,
                    content,
                });
                if Some(pane.target) == self.focus {
                    focused = Some(index);
                }
            }
            divider = layout.divider;
        }

        let (view_rows, view_columns) = self.viewport();
        let menu_lines = self.menu.overlay_lines();
        let confirm_line = self.confirm.overlay_line(view_columns);
        let mut lines = [OverlayLine {
            text: "",
            style: OverlayStyle::Normal,
        }; panelink::split_menu::MAX_OVERLAY_ROWS];
        let overlay = if self.help_open && view_rows > 0 && view_columns > 0 {
            let rows = view_rows.min(HELP_LINES.len() as u16);
            let columns = view_columns.min(72);
            let x = view_columns.saturating_sub(columns) / 2;
            let y = view_rows.saturating_sub(rows) / 2;
            for (slot, text) in lines
                .iter_mut()
                .zip(HELP_LINES.iter())
                .take(usize::from(rows))
            {
                *slot = OverlayLine {
                    text,
                    style: if text == &HELP_LINES[0] {
                        OverlayStyle::Selected
                    } else if text == &HELP_LINES[HELP_LINES.len() - 1] {
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
        } else {
            let notice = if !fits && self.layout.is_some() {
                Some((TOO_SMALL, 0))
            } else if self.prefix.is_some() {
                Some((NAV_BANNER, view_rows.saturating_sub(1)))
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
            focused,
            overlay: overlay.map(|(rect, count)| OverlayView {
                rect,
                lines: &lines[..count],
            }),
        };
        compositor.render(&frame, output)
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
    let first = reply.panes[0]?;
    let mut local = PaneLayout::new(first.target, reply.rows, reply.columns).ok()?;
    if let (Some(second), Some(divider)) = (reply.panes[1], reply.divider) {
        let direction = if divider.columns == 1 && divider.rows != 1 {
            SplitDirection::Right
        } else if divider.rows == 1 && divider.columns != 1 {
            SplitDirection::Down
        } else if second.rect.x > first.rect.x {
            SplitDirection::Right
        } else {
            SplitDirection::Down
        };
        let preview = local
            .preview_split(first.target, 1, direction, second.target)
            .ok()?;
        local.commit_split(preview).ok()?;
        let snapshot = local.snapshot();
        let matches = snapshot.panes.iter().zip(reply.panes.iter()).all(|(a, b)| {
            a.map(|pane| (pane.target, pane.rect)) == b.map(|pane| (pane.target, pane.rect))
        });
        if !matches {
            return None;
        }
    }
    Some(local)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
