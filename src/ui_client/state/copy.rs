//! Frozen selection, copy-mode input and source-scoped clipboard completion.
use super::super::{
    clipboard::ClipboardStatus,
    keys::{copy_mode_key, CopyModeKey},
    selection::{capture, reanchor, FrozenDrag},
};
use super::viewport::Viewports;
use super::{is_clipboard_failure_label, Flow};
use crossterm_input::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use panelink::terminal::compose::{pointer_content_point, CopyCaretOverlay, SelectionOverlay};
use panelink::{
    control_protocol::TargetRef,
    pane_layout::Rect,
    terminal::ScreenSnapshot,
    terminal_selection::{Point, SelectionError, ViewIdentity},
    ui_protocol::LayoutReply,
};
use std::time::{Duration, Instant};
pub(super) const COPY_FEEDBACK_TTL: Duration = Duration::from_millis(1_500);
#[derive(Default)]
pub(super) struct CopyFeedback {
    pub dirty: bool,
    pub clear_prefix: bool,
    pub clear_clipboard_failure: bool,
    pub status: Option<(&'static str, Instant)>,
}
pub(super) struct CopyState {
    pub(super) selection: Option<FrozenDrag>,
    pub(super) copy_mode: bool,
    pub(super) copy_anchor: Option<Point>,
    pub(super) copy_exit_key: Option<(KeyCode, KeyModifiers)>,
    pub(super) pending_copy: Option<String>,
    pub(super) pending_copy_target: Option<TargetRef>,
    pub(super) pending_copy_id: Option<u64>,
    pub(super) next_copy_id: u64,
    pub(super) latest_copy_attempt: [Option<(TargetRef, u64)>; crate::pane_layout::MAX_PANES],
    pub(super) copied_feedback: [Option<(TargetRef, Instant)>; crate::pane_layout::MAX_PANES],
    feedback: CopyFeedback,
}
pub(super) struct CopyContext<'a> {
    pub layout: Option<LayoutReply>,
    pub display_layout: Option<LayoutReply>,
    pub fits: bool,
    pub screens: &'a Viewports,
    pub focus: Option<TargetRef>,
    pub status: Option<(&'static str, Instant)>,
}
impl<'a> CopyContext<'a> {
    pub(super) fn visible_source(
        &self,
        target: TargetRef,
    ) -> Option<(usize, Rect, &'a ScreenSnapshot, bool, u64)> {
        if !self.fits {
            return None;
        }
        let layout = self.display_layout?;
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
    pub(super) fn current_selection_identity(&self, drag: &FrozenDrag) -> Option<ViewIdentity> {
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
}
impl CopyState {
    pub(super) fn new() -> Self {
        Self {
            selection: None,
            copy_mode: false,
            copy_anchor: None,
            copy_exit_key: None,
            pending_copy: None,
            pending_copy_target: None,
            pending_copy_id: None,
            next_copy_id: 1,
            latest_copy_attempt: [None; crate::pane_layout::MAX_PANES],
            copied_feedback: [None; crate::pane_layout::MAX_PANES],
            feedback: CopyFeedback::default(),
        }
    }
    pub(super) fn view(&self) -> &Self {
        self
    }
    pub(super) fn take_feedback(&mut self) -> CopyFeedback {
        std::mem::take(&mut self.feedback)
    }
    fn set_status(&mut self, label: &'static str, now: Instant) {
        self.feedback.status = Some((label, now));
        self.feedback.dirty = true;
    }
    pub(super) fn reset_route(&mut self) {
        self.pending_copy = None;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        self.latest_copy_attempt = [None; crate::pane_layout::MAX_PANES];
        self.copied_feedback = [None; crate::pane_layout::MAX_PANES];
    }
    pub(super) fn reconcile_geometry(&mut self, reply: &LayoutReply) {
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
    }
    pub(super) fn reconcile_targets(&mut self, reply: &LayoutReply) {
        let present = |target: TargetRef| {
            reply
                .panes
                .iter()
                .flatten()
                .any(|pane| pane.target == target)
        };
        for attempt in &mut self.latest_copy_attempt {
            if attempt.is_some_and(|(target, _)| !present(target)) {
                *attempt = None;
            }
        }
        for feedback in &mut self.copied_feedback {
            if feedback.is_some_and(|(target, _)| !present(target)) {
                *feedback = None;
            }
        }
    }
    pub(super) fn expire_feedback(&mut self, now: Instant) {
        for feedback in &mut self.copied_feedback {
            if feedback.is_some_and(|(_, expires)| now >= expires) {
                *feedback = None;
                self.feedback.dirty = true;
            }
        }
    }
    pub(super) fn consume_exit_key(&mut self, key: KeyEvent) -> bool {
        if let Some((code, modifiers)) = self.copy_exit_key {
            match key.kind {
                KeyEventKind::Repeat if key.code == code && key.modifiers == modifiers => {
                    return true
                }
                KeyEventKind::Release if key.code == code && key.modifiers == modifiers => {
                    self.copy_exit_key = None;
                    return true;
                }
                KeyEventKind::Press => self.copy_exit_key = None,
                _ => {}
            }
        }
        false
    }
    #[cfg(test)]
    pub(super) fn take_copy_text(&mut self) -> Option<String> {
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        self.pending_copy.take()
    }
    pub(super) fn take_copy_request(&mut self) -> Option<(TargetRef, u64, String)> {
        let target = self.pending_copy_target?;
        let request_id = self.pending_copy_id?;
        let text = self.pending_copy.take()?;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
        Some((target, request_id, text))
    }
    pub(super) fn report_clipboard_unavailable(&mut self, context: &CopyContext<'_>, now: Instant) {
        self.report_clipboard_status(context, None, None, ClipboardStatus::Unavailable, now);
    }
    pub(super) fn report_clipboard_status(
        &mut self,
        context: &CopyContext<'_>,
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
            let Some(index) = context.layout.and_then(|layout| {
                layout
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == target))
            }) else {
                return;
            };
            self.copied_feedback[index] = Some((target, now + COPY_FEEDBACK_TTL));
            if context
                .status
                .is_some_and(|(label, _)| is_clipboard_failure_label(label))
            {
                self.feedback.clear_clipboard_failure = true;
            }
            self.feedback.dirty = true;
            return;
        }

        if let Some((target, _request_id)) = current_attempt {
            if let Some(index) = context.layout.and_then(|layout| {
                layout
                    .panes
                    .iter()
                    .position(|pane| pane.is_some_and(|pane| pane.target == target))
            }) {
                self.copied_feedback[index] = None;
                self.feedback.dirty = true;
            }
        }
        self.set_status(status.label(), now);
    }
    pub(super) fn cancel_for_detach(&mut self) {
        self.cancel_selection();
        self.pending_copy = None;
        self.pending_copy_target = None;
        self.pending_copy_id = None;
    }
    pub(super) fn cancel_selection(&mut self) {
        let active =
            self.selection.take().is_some() || self.copy_mode || self.copy_anchor.is_some();
        self.copy_mode = false;
        self.copy_anchor = None;
        if active {
            self.feedback.dirty = true;
        }
    }
    pub(super) fn queue_copy_text(
        &mut self,
        context: &CopyContext<'_>,
        target: TargetRef,
        text: String,
        now: Instant,
    ) {
        let request_id = self.mark_copy_attempt(context, target);
        if self.pending_copy.is_some() {
            self.report_clipboard_status(
                context,
                Some(target),
                Some(request_id),
                ClipboardStatus::Busy,
                now,
            );
        } else {
            self.pending_copy = Some(text);
            self.pending_copy_target = Some(target);
            self.pending_copy_id = Some(request_id);
            self.feedback.dirty = true;
        }
    }
    pub(super) fn mark_copy_attempt(
        &mut self,
        context: &CopyContext<'_>,
        target: TargetRef,
    ) -> u64 {
        let request_id = self.next_copy_id;
        self.next_copy_id = self.next_copy_id.wrapping_add(1).max(1);
        if let Some(index) = context.layout.and_then(|layout| {
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
            self.feedback.dirty = true;
        }
        request_id
    }
    pub(super) fn fail_copy_attempt(
        &mut self,
        context: &CopyContext<'_>,
        target: TargetRef,
        label: &'static str,
        now: Instant,
    ) {
        self.mark_copy_attempt(context, target);
        self.set_status(label, now);
    }
    pub(super) fn begin_copy_mode(&mut self, context: &CopyContext<'_>, now: Instant) {
        let Some(target) = context.focus else {
            self.set_status("Copy unavailable", now);
            return;
        };
        self.begin_copy_mode_for_target(context, target, now);
    }
    pub(super) fn begin_copy_mode_for_target(
        &mut self,
        context: &CopyContext<'_>,
        target: TargetRef,
        now: Instant,
    ) {
        let Some((slot, content, snapshot, source_history, epoch)) = context.visible_source(target)
        else {
            self.fail_copy_attempt(context, target, "Copy unavailable", now);
            return;
        };
        if snapshot.row_wraps().is_none() || content.rows == 0 || content.columns == 0 {
            self.fail_copy_attempt(context, target, "Copy unavailable", now);
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
                self.feedback.clear_prefix = true;
                self.feedback.dirty = true;
            }
            Err(_) => self.fail_copy_attempt(context, target, "Copy unavailable", now),
        }
    }
    pub(super) fn handle_copy_mode_event(
        &mut self,
        context: &CopyContext<'_>,
        event: &Event,
        now: Instant,
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
                self.feedback.dirty = true;
            }
            CopyModeKey::Copy => {
                if let Event::Key(key) = event {
                    self.copy_exit_key = Some((key.code, key.modifiers));
                }
                self.finish_copy_mode(context, now);
            }
            CopyModeKey::Cancel => self.cancel_selection(),
            CopyModeKey::Detach => {
                self.cancel_selection();
                return Flow::Quit;
            }
            CopyModeKey::Ignore => {}
        }
        Flow::Continue
    }
    pub(super) fn move_copy_caret(&mut self, dx: i8, dy: i8) {
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
        self.feedback.dirty = true;
    }
    pub(super) fn finish_copy_mode(&mut self, context: &CopyContext<'_>, now: Instant) {
        let anchor = self.copy_anchor;
        let drag = self.selection.take();
        self.copy_mode = false;
        self.copy_anchor = None;
        self.feedback.dirty = true;
        let Some(drag) = drag else { return };
        if anchor.is_none() || anchor == Some(drag.end) {
            return;
        }
        let result = context
            .current_selection_identity(&drag)
            .ok_or(SelectionError::StaleView)
            .and_then(|identity| drag.selection.finish(identity));
        match result {
            Ok(text) => self.queue_copy_text(context, drag.target, text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => {
                self.fail_copy_attempt(context, drag.target, "Copy unavailable", now)
            }
            Err(SelectionError::TooLarge) => {
                self.fail_copy_attempt(context, drag.target, "Copy too large", now)
            }
            Err(_) => self.fail_copy_attempt(context, drag.target, "Copy cancelled", now),
        }
    }
    pub(super) fn begin_selection(
        &mut self,
        context: &CopyContext<'_>,
        target: TargetRef,
        x: u16,
        y: u16,
        now: Instant,
    ) {
        let Some((slot, content, snapshot, source_history, epoch)) = context.visible_source(target)
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
                self.feedback.dirty = true;
            }
            Err(SelectionError::TooLarge) => {
                self.fail_copy_attempt(context, target, "Copy too large", now)
            }
            Err(_) => self.set_status("Copy unavailable", now),
        }
    }
    pub(super) fn selection_point(drag: &FrozenDrag, x: u16, y: u16) -> Point {
        pointer_content_point(drag.content_rect, x, y)
    }
    pub(super) fn extend_selection(&mut self, x: u16, y: u16) {
        if let Some(drag) = self.selection.as_mut() {
            let point = Self::selection_point(drag, x, y);
            drag.end = point;
            drag.selection.extend(point);
            self.feedback.dirty = true;
        }
    }
    pub(super) fn finish_selection(&mut self, context: &CopyContext<'_>, now: Instant) {
        let Some(drag) = self.selection.take() else {
            return;
        };
        self.feedback.dirty = true;
        let result = context
            .current_selection_identity(&drag)
            .ok_or(SelectionError::StaleView)
            .and_then(|identity| drag.selection.finish(identity));
        match result {
            Ok(text) => self.queue_copy_text(context, drag.target, text, now),
            Err(SelectionError::Empty) => {}
            Err(SelectionError::UnknownWrap) => {
                self.fail_copy_attempt(context, drag.target, "Copy unavailable", now)
            }
            Err(SelectionError::TooLarge) => {
                self.fail_copy_attempt(context, drag.target, "Copy too large", now)
            }
            Err(_) => self.fail_copy_attempt(context, drag.target, "Copy cancelled", now),
        }
    }
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

impl CopyState {
    pub(super) fn selection_overlay(
        &self,
        context: &CopyContext<'_>,
    ) -> Option<SelectionOverlay<'_>> {
        self.selection.as_ref().and_then(|drag| {
            let identity = context.current_selection_identity(drag)?;
            if identity != drag.identity || (self.copy_mode && self.copy_anchor.is_none()) {
                return None;
            }
            let slot = context.display_layout?.panes.iter().position(|pane| {
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
        })
    }
    pub(super) fn caret_overlay(
        &self,
        context: &CopyContext<'_>,
        visible: bool,
    ) -> Option<CopyCaretOverlay<'_>> {
        if self.copy_mode && visible {
            self.selection.as_ref().and_then(|drag| {
                if context.current_selection_identity(drag)? != drag.identity {
                    return None;
                }
                let slot = context
                    .display_layout?
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
        }
    }
}
