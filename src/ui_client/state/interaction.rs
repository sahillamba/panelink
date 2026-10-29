//! Captured pane modes, modal event consumption and menu/confirmation identity.
use super::super::keys::{confirm_input, map_key, menu_input, KeyAction};
use super::super::pane_operations::PaneAction;
use super::Flow;
use super::{local_layout, PointerDivider, HELP_LINES};
use crossterm_input::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use panelink::split_menu::{
    Availability, CloseConfirm, CloseIntent, ConfirmResponse, MenuResponse, MenuState,
    PaneActionModel, PaneMenuAction, SplitIntent,
};
use panelink::{
    control_protocol::TargetRef,
    pane_layout::{DividerOrientation, PaneLayout, Rect, SplitDirection},
    pane_operations_protocol::PaneOperationsView,
    ui_protocol::LayoutReply,
};
use std::time::Instant;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ConfirmUiGeometry {
    pub(super) overlay: Rect,
    pub(super) close: Rect,
    pub(super) cancel: Rect,
    pub(super) position: u8,
    pub(super) consequence: &'static str,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneMode {
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
pub(super) struct ActionsMenu {
    pub(super) target: TargetRef,
    pub(super) revision: u64,
    pub(super) selected: usize,
    pub(super) model: PaneActionModel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActionsMenuPage {
    pub(super) rect: Rect,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) action_row: usize,
    pub(super) reason_row: usize,
    pub(super) navigation_row: usize,
    pub(super) cancel_row: usize,
    pub(super) line_count: usize,
    pub(super) back_column: Option<u16>,
    pub(super) more_column: Option<u16>,
    pub(super) cancel_column: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LocalAction {
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
pub(super) const LOCAL_ACTIONS: [LocalAction; panelink::split_menu::PANE_ACTION_COUNT] = [
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
#[derive(Default)]
pub(super) struct InteractionFeedback {
    pub dirty: bool,
    pub cancel_selection: bool,
    pub status: Option<(&'static str, Instant)>,
}
pub(super) struct Interaction {
    pub(super) pane_mode: Option<PaneMode>,
    pub(super) menu: MenuState,
    pub(super) menu_revision: u64,
    pub(super) confirm: CloseConfirm,
    pub(super) confirm_close_selected: bool,
    pub(super) confirm_open_key: Option<(KeyCode, KeyModifiers)>,
    pub(super) actions_menu: Option<ActionsMenu>,
    pub(super) menu_release_latch: Option<MouseButton>,
    pub(super) help_open: bool,
    pub(super) prefix: Option<Instant>,
    pub(super) pane_mode_exit_key: Option<(KeyCode, KeyModifiers)>,
    feedback: InteractionFeedback,
    operation: Option<PaneAction>,
    flush: Option<Instant>,
}
#[derive(Clone, Copy)]
pub(super) struct InteractionContext<'a> {
    pub layout: Option<LayoutReply>,
    pub local: Option<&'a PaneLayout>,
    pub pane_operations: Option<PaneOperationsView>,
    pub viewport: (u16, u16),
    pub header_offset: u16,
}
impl InteractionContext<'_> {
    pub(super) fn divider_first_span(&self, target: TargetRef, revision: u64, index: u8) -> u16 {
        self.local
            .and_then(|layout| layout.divider_info(target, revision, index).ok())
            .map_or(0, |info| info.first_span)
    }
    pub(super) fn adjust_divider_preview(
        &self,
        target: TargetRef,
        revision: u64,
        index: u8,
        key: crossterm_input::KeyEvent,
        first_cells: &mut u16,
    ) {
        let Some(info) = self
            .local
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
    pub(super) fn clamp_divider_span(
        &self,
        target: TargetRef,
        revision: u64,
        index: u8,
        requested: u16,
    ) -> Option<u16> {
        let info = self.local?.divider_info(target, revision, index).ok()?;
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
    pub(super) fn divider_value_valid(
        &self,
        target: TargetRef,
        revision: u64,
        index: u8,
        value: u16,
    ) -> bool {
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
}
// Keep the bounded captured action model on the stack, as in the original
// event handler; boxing would add an allocation to menu activation.
#[allow(clippy::large_enum_variant)]
pub(super) enum MenuDispatch {
    Continue,
    Activate(ActionsMenu),
    Split(SplitIntent),
    ViewRecovery,
}
impl Interaction {
    pub(super) fn new() -> Self {
        Self {
            pane_mode: None,
            menu: MenuState::new(),
            menu_revision: 0,
            confirm: CloseConfirm::new(),
            confirm_close_selected: false,
            confirm_open_key: None,
            actions_menu: None,
            menu_release_latch: None,
            help_open: false,
            prefix: None,
            pane_mode_exit_key: None,
            feedback: InteractionFeedback::default(),
            operation: None,
            flush: None,
        }
    }
    pub(super) const fn view(&self) -> &Self {
        self
    }
    pub(super) fn take_feedback(&mut self) -> InteractionFeedback {
        std::mem::take(&mut self.feedback)
    }
    pub(super) fn take_operation(&mut self) -> Option<PaneAction> {
        self.operation.take()
    }
    pub(super) fn take_flush(&mut self) -> Option<Instant> {
        self.flush.take()
    }
    fn request_pane_operation(&mut self, action: PaneAction, _now: Instant) {
        self.operation = Some(action);
    }
    fn set_status(&mut self, label: &'static str, now: Instant) {
        self.feedback.status = Some((label, now));
        self.feedback.dirty = true;
    }
    pub(super) fn consume_release(&mut self, event: &Event) -> bool {
        if let (Some(button), Event::Mouse(mouse)) = (self.menu_release_latch, event) {
            if mouse.kind == MouseEventKind::Up(button) {
                self.menu_release_latch = None;
                return true;
            }
        }
        if let Event::Key(key) = event {
            if let Some((code, modifiers)) = self.pane_mode_exit_key {
                match key.kind {
                    KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                        return true;
                    }
                    KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                        self.pane_mode_exit_key = None;
                        return true;
                    }
                    KeyEventKind::Press => self.pane_mode_exit_key = None,
                    _ => {}
                }
            }
        }
        false
    }
    pub(super) fn handle_help_event(
        &mut self,
        event: &Event,
        viewport: (u16, u16),
        header_offset: u16,
    ) -> Flow {
        if let Event::Mouse(mouse) = event {
            let (rows, columns) = viewport;
            let height = rows.min(HELP_LINES.len() as u16);
            let width = columns.min(72);
            let left = columns.saturating_sub(width) / 2;
            let top = rows.saturating_sub(height) / 2 + header_offset;
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && mouse.row == top
                && mouse.column >= left
                && mouse.column < left.saturating_add(width)
            {
                self.help_open = false;
                self.feedback.dirty = true;
            }
        }
        if let Event::Key(key) = event {
            match map_key(*key) {
                KeyAction::Quit => {
                    self.help_open = false;
                    self.feedback.dirty = true;
                    return Flow::Quit;
                }
                _ if key.is_press() || key.is_repeat() => {
                    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                    if plain && (key.code == KeyCode::Esc || key.code == KeyCode::Char('?')) {
                        self.help_open = false;
                        self.feedback.dirty = true;
                    }
                }
                _ => {}
            }
        }
        // Help owns keys, paste and mouse. Resize is observed by the host
        // loop, which updates `host` through `set_host`.
        Flow::Continue
    }
    pub(super) fn handle_confirm_event(
        &mut self,
        event: &Event,
        now: Instant,
        viewport: (u16, u16),
        header_offset: u16,
    ) -> Option<CloseIntent> {
        // The prompt owns every event; none reaches a pane.
        self.feedback.dirty = true;
        if let (Some((code, modifiers)), Event::Key(key)) = (self.confirm_open_key, event) {
            match key.kind {
                KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                    return None;
                }
                KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                    self.confirm_open_key = None;
                    return None;
                }
                KeyEventKind::Press => self.confirm_open_key = None,
                _ => {}
            }
        }
        if let Event::Mouse(mouse) = event {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                let intent = self.confirm.intent()?;
                let (rows, columns) = viewport;
                let geometry = self.confirm_ui_geometry(rows, columns, header_offset);
                if geometry.is_some_and(|geometry| geometry.close.contains(mouse.column, mouse.row))
                {
                    self.dismiss_close_confirmation();
                    return Some(intent);
                } else if geometry
                    .is_some_and(|geometry| geometry.cancel.contains(mouse.column, mouse.row))
                {
                    self.dismiss_close_confirmation();
                    self.set_status("close cancelled", now);
                    return None;
                }
            }
        }
        match self.confirm.handle(confirm_input(event)) {
            ConfirmResponse::Confirmed(intent) => {
                self.dismiss_close_confirmation();
                return Some(intent);
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
                        return Some(intent);
                    }
                } else {
                    self.dismiss_close_confirmation();
                    self.set_status("close cancelled", now);
                }
            }
            _ => {}
        }
        None
    }
    pub(super) fn handle_split_menu_event(
        &mut self,
        event: &Event,
        header_offset: u16,
    ) -> MenuResponse {
        if let Event::Mouse(mouse) = event {
            if let MouseEventKind::Down(button @ (MouseButton::Left | MouseButton::Right)) =
                mouse.kind
            {
                self.menu_release_latch = Some(button);
            }
        }
        let mut input =
            if header_offset > 0 && matches!(event, Event::Mouse(mouse) if mouse.row == 0) {
                panelink::split_menu::MenuInput::Other
            } else {
                menu_input(event)
            };
        if header_offset > 0 {
            match &mut input {
                panelink::split_menu::MenuInput::MousePress { y, .. }
                | panelink::split_menu::MenuInput::MouseRelease { y, .. } => {
                    *y = y.saturating_sub(1);
                }
                _ => {}
            }
        }
        let response = self.menu.handle(input);
        self.feedback.dirty = true;
        match response {
            MenuResponse::Activated(intent) => MenuResponse::Activated(SplitIntent {
                expected_layout_revision: self.menu_revision,
                ..intent
            }),
            MenuResponse::CloseRequested(intent) => MenuResponse::CloseRequested(CloseIntent {
                expected_layout_revision: self.menu_revision,
                ..intent
            }),
            other => other,
        }
    }
    pub(super) fn capture_action(
        &mut self,
        menu: ActionsMenu,
        context: InteractionContext<'_>,
        now: Instant,
    ) -> Option<LocalAction> {
        let Some(layout) = context.layout else {
            self.actions_menu = None;
            self.menu.close();
            return None;
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
            return None;
        }
        let Some(action) = LOCAL_ACTIONS.get(menu.selected).copied() else {
            self.actions_menu = None;
            self.menu.close();
            self.feedback.dirty = true;
            return None;
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
            return None;
        }
        self.actions_menu = None;
        if !matches!(action, LocalAction::Split(_)) {
            self.menu.close();
        }
        Some(action)
    }
    pub(super) fn open_close_confirmation(&mut self, intent: CloseIntent) {
        self.confirm.open(intent);
        self.confirm_close_selected = false;
        self.confirm_open_key = None;
        self.feedback.dirty = true;
    }
    pub(super) fn dismiss_close_confirmation(&mut self) {
        self.confirm = CloseConfirm::new();
        self.confirm_close_selected = false;
        self.confirm_open_key = None;
        self.feedback.dirty = true;
    }
    pub(super) fn confirm_ui_geometry(
        &self,
        rows: u16,
        columns: u16,
        header_offset: u16,
    ) -> Option<ConfirmUiGeometry> {
        let intent = self.confirm.intent()?;
        if rows == 0 || columns == 0 {
            return None;
        }
        let width = columns.min(48);
        let left = columns.saturating_sub(width) / 2;
        let overlay_rows = rows.min(2);
        let top = rows.saturating_sub(overlay_rows);
        let button_y = top.saturating_add(overlay_rows.saturating_sub(1));
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
    pub(super) fn handle_pane_mode_event(
        &mut self,
        context: InteractionContext<'_>,
        event: &Event,
        now: Instant,
    ) {
        if let Event::Mouse(mouse) = event {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.handle_pane_mode_pointer(context, mouse.column, mouse.row, now);
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
                self.feedback.cancel_selection = true;
                self.flush = Some(now);
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
                            self.request_pane_operation(
                                PaneAction::Swap {
                                    source,
                                    destination,
                                    expected_layout_revision: revision,
                                },
                                now,
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
                                context.layout.is_some_and(|layout| {
                                    layout.revision == revision
                                        && layout
                                            .panes
                                            .iter()
                                            .flatten()
                                            .any(|pane| pane.target == target)
                                }) && context.divider_value_valid(
                                    target,
                                    revision,
                                    *divider_index,
                                    first_cells,
                                )
                            })
                        {
                            self.request_pane_operation(
                                PaneAction::DividerSet {
                                    target,
                                    expected_layout_revision: revision,
                                    divider_index,
                                    first_cells,
                                },
                                now,
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
                        first_cells = context.divider_first_span(
                            target,
                            revision,
                            indices[selected].unwrap_or(0),
                        );
                    } else if let Some(index) = indices[selected] {
                        context.adjust_divider_preview(
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
            self.feedback.dirty = true;
        }
        // Paste, mouse, focus, and resize are owned by the mode and never
        // become child input. Resize cancellation is handled by set_host.
    }
    pub(super) fn handle_pane_mode_pointer(
        &mut self,
        context: InteractionContext<'_>,
        x: u16,
        y: u16,
        now: Instant,
    ) {
        let Some(mode) = self.pane_mode else { return };
        let (view_rows, view_columns) = context.viewport;
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
        let offset = context.header_offset;
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
                self.handle_pane_mode_event(context, &key, now);
            }
            return;
        }
        if row == apply_row {
            let enter = Event::Key(crossterm_input::KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ));
            self.handle_pane_mode_event(context, &enter, now);
            return;
        }
        if row == cancel_row {
            let escape = Event::Key(crossterm_input::KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            ));
            self.handle_pane_mode_event(context, &escape, now);
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
                let first_cells = indices[selected].map_or(0, |index| {
                    context.divider_first_span(target, revision, index)
                });
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
        self.feedback.dirty = true;
    }
    pub(super) fn open_swap_mode_for_target(
        &mut self,
        context: InteractionContext<'_>,
        source: TargetRef,
        revision: u64,
        now: Instant,
    ) {
        let Some(view) = context.pane_operations else {
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
        self.feedback.cancel_selection = true;
        self.pane_mode = Some(PaneMode::Swap {
            source,
            revision,
            choices,
            selected: 0,
        });
        self.prefix = None;
        self.feedback.dirty = true;
    }
    pub(super) fn open_divider_mode_for_target(
        &mut self,
        context: InteractionContext<'_>,
        target: TargetRef,
        revision: u64,
        now: Instant,
    ) {
        let (Some(view), Some(local)) = (context.pane_operations, context.local) else {
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
        self.feedback.cancel_selection = true;
        self.pane_mode = Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected,
            first_cells,
        });
        self.prefix = None;
        self.feedback.dirty = true;
    }
    pub(super) fn actions_menu_rect(&self, context: InteractionContext<'_>) -> Option<Rect> {
        let (root_rows, root_columns) = context.viewport;
        let rows = root_rows.min(panelink::split_menu::MAX_OVERLAY_ROWS as u16);
        let columns = root_columns.min(42);
        (rows > 0 && columns > 0).then_some(Rect {
            x: root_columns.saturating_sub(columns) / 2,
            y: root_rows.saturating_sub(rows) / 2,
            rows,
            columns,
        })
    }
    pub(super) fn local_action_item(
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
    pub(super) fn actions_page_geometry(
        &self,
        context: InteractionContext<'_>,
        menu: ActionsMenu,
    ) -> Option<ActionsMenuPage> {
        let rect = self.actions_menu_rect(context)?;
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
    pub(super) fn handle_actions_menu_event(
        &mut self,
        context: InteractionContext<'_>,
        event: &Event,
        now: Instant,
        recovery_availability: Availability,
    ) -> MenuDispatch {
        let Some(mut menu) = self.actions_menu else {
            return MenuDispatch::Continue;
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
                    return MenuDispatch::Activate(menu);
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
                        return MenuDispatch::Split(SplitIntent {
                            target: menu.target,
                            expected_layout_revision: menu.revision,
                            direction,
                        });
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
                        return MenuDispatch::ViewRecovery;
                    }
                    self.set_status("No previous layout saved", now);
                }
                self.feedback.dirty = true;
            }
            Event::Mouse(mouse)
                if matches!(
                    mouse.kind,
                    MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
                ) =>
            {
                if let Some(page) = self.actions_page_geometry(context, menu) {
                    let rect = page.rect;
                    let offset = context.header_offset;
                    let inside = mouse.column >= rect.x
                        && mouse.column < rect.x.saturating_add(rect.columns)
                        && mouse.row >= rect.y.saturating_add(offset)
                        && mouse.row < rect.y.saturating_add(rect.rows + offset);
                    if !inside {
                        self.actions_menu = None;
                        self.menu.close();
                        self.feedback.dirty = true;
                        return MenuDispatch::Continue;
                    }
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                        && mouse.row >= rect.y.saturating_add(1 + offset)
                    {
                        let row = usize::from(mouse.row - rect.y - offset);
                        if (page.action_row..page.reason_row).contains(&row) {
                            menu.selected = page.start + row - page.action_row;
                            return MenuDispatch::Activate(menu);
                        } else if row == page.navigation_row {
                            let column = mouse.column.saturating_sub(rect.x);
                            if column >= page.cancel_column
                                && column < page.cancel_column.saturating_add(8)
                            {
                                self.actions_menu = None;
                                self.menu.close();
                                self.feedback.dirty = true;
                            } else if page.back_column.is_some_and(|start| {
                                column >= start && column < start.saturating_add(6)
                            }) {
                                menu.selected = page.start.saturating_sub(1);
                                self.actions_menu = Some(menu);
                                self.feedback.dirty = true;
                            } else if page.more_column.is_some_and(|start| {
                                column >= start && column < start.saturating_add(6)
                            }) {
                                menu.selected = page.end;
                                self.actions_menu = Some(menu);
                                self.feedback.dirty = true;
                            }
                        }
                    }
                }
                // The menu owns every pointer event, including outside clicks.
            }
            _ => {}
        }
        MenuDispatch::Continue
    }
    pub(super) fn reconcile_modals(&mut self, reply: &LayoutReply) {
        let present = |target: TargetRef| {
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
                    .is_some_and(|target| !present(target)))
        {
            self.cancel_menu();
            self.set_status("menu cancelled: layout changed", Instant::now());
        }
        if self.confirm.intent().is_some_and(|intent| {
            intent.expected_layout_revision != reply.revision || !present(intent.target)
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
    }
    pub(super) fn preview_pointer_divider(
        &mut self,
        context: InteractionContext<'_>,
        capture: PointerDivider,
        x: u16,
        screen_y: u16,
    ) -> bool {
        let Some(PaneMode::Divider {
            target,
            revision,
            indices,
            selected,
            ..
        }) = self.pane_mode
        else {
            return true;
        };
        if revision != capture.revision
            || context
                .layout
                .is_none_or(|layout| layout.revision != capture.revision)
        {
            self.pane_mode = None;
            return false;
        }
        let root_y = screen_y.saturating_sub(context.header_offset);
        let requested = match capture.orientation {
            DividerOrientation::Horizontal => x.saturating_sub(capture.region.x),
            DividerOrientation::Vertical => root_y.saturating_sub(capture.region.y),
        };
        if let Some(candidate) = context.clamp_divider_span(
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
            self.feedback.dirty = true;
        }
        true
    }
    pub(super) fn replace_mode(&mut self, mode: Option<PaneMode>) {
        self.pane_mode = mode;
    }
    pub(super) fn take_mode(&mut self) -> Option<PaneMode> {
        self.pane_mode.take()
    }
    pub(super) fn record_mode_exit(&mut self, key: Option<(KeyCode, KeyModifiers)>) {
        self.pane_mode_exit_key = key;
    }
    pub(super) fn replace_actions(&mut self, menu: Option<ActionsMenu>) {
        self.actions_menu = menu;
    }
    pub(super) fn record_release(&mut self, button: Option<MouseButton>) {
        self.menu_release_latch = button;
    }
    pub(super) fn show_help(&mut self, show: bool) {
        self.help_open = show;
    }
    pub(super) fn set_prefix(&mut self, at: Option<Instant>) {
        self.prefix = at;
    }
    pub(super) fn record_menu_revision(&mut self, revision: u64) {
        self.menu_revision = revision;
    }
    pub(super) fn record_confirm_open_key(&mut self, key: Option<(KeyCode, KeyModifiers)>) {
        self.confirm_open_key = key;
    }
    pub(super) fn cancel_menu(&mut self) {
        self.menu.handle(panelink::split_menu::MenuInput::Cancel);
    }
    pub(super) fn close_menu(&mut self) {
        self.menu.close();
    }
    pub(super) fn set_menu_viewport(&mut self, rows: u16, columns: u16) {
        self.menu.set_viewport(rows, columns);
    }
    pub(super) fn compact_menu_reasons(&mut self) {
        self.menu.set_compact_reasons(true);
    }
    pub(super) fn open_menu_at_pointer(
        &mut self,
        local: &PaneLayout,
        x: u16,
        y: u16,
        rows: u16,
        columns: u16,
        recovery: Availability,
    ) -> Result<(), panelink::split_menu::MenuOpenError> {
        self.menu
            .open_at_pointer_with_recovery(local, x, y, rows, columns, recovery)
    }
    #[cfg(test)]
    pub(super) fn open_menu_for_target(
        &mut self,
        local: &PaneLayout,
        target: TargetRef,
        rows: u16,
        columns: u16,
        recovery: Availability,
    ) -> Result<(), panelink::split_menu::MenuOpenError> {
        self.menu
            .open_for_focused_with_recovery(local, target, rows, columns, recovery)
    }
}

impl Interaction {
    pub(super) fn format_mode(
        &self,
        context: InteractionContext<'_>,
        pane_mode_text: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
    ) -> usize {
        match self.pane_mode {
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
                    let pane_number = context
                        .layout
                        .and_then(|layout| {
                            layout
                                .panes
                                .iter()
                                .position(|pane| pane.is_some_and(|pane| pane.target == *candidate))
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
                let axis = context
                    .local
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
        }
    }
    pub(super) fn format_actions(
        &self,
        context: InteractionContext<'_>,
        menu: ActionsMenu,
        action_text: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
    ) -> Option<ActionsMenuPage> {
        let page = self.actions_page_geometry(context, menu)?;
        let pane = context
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
        Some(page)
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
