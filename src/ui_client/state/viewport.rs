//! Owns bounded pane view caches, row identity and pending history/input motion.
//! Callers inspect immutable slots and request named state transitions.
use super::super::net::{ScrollResult, ScrollUpdate};
use super::{Effects, LayoutReply, MAX_ROOT_COLUMNS, MAX_ROOT_ROWS};
use panelink::control_protocol::TargetRef;
use panelink::screen_checkpoint::ScreenCheckpoint;
use panelink::terminal::{ScreenSnapshot, TerminalError, TerminalSize};
use panelink::ui_protocol::WireHistoryCursor;
use std::time::{Duration, Instant};

// Claim/send/release and screen refreshes can take ~40 ms per report in a
// debug build. Bound retained gestures without expiring an ordinary wheel burst.
pub(super) const ALT_WHEEL_TTL: Duration = Duration::from_secs(1);
pub(super) const ALT_WHEEL_CAPACITY: usize = 32;
fn add_history_rows(offset: u16, delta: u64) -> u16 {
    u64::from(offset)
        .saturating_add(delta)
        .min(u64::from(panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS)) as u16
}
pub(super) struct ScreenSlot {
    pub(super) target: Option<TargetRef>,
    pub(super) revision: u64,
    pub(super) snapshot: ScreenSnapshot,
    pub(super) history: ScreenSnapshot,
    pub(super) history_revision: u64,
    pub(super) history_offset: u16,
    /// Cursor against which `history_offset` is expressed.
    pub(super) history_base: Option<WireHistoryCursor>,
    pub(super) view_epoch: u64,
    pub(super) history_pending: bool,
    pub(super) history_inflight_offset: u16,
    pub(super) history_requested: u16,
    pub(super) history_total: u16,
    pub(super) history_extent_known: bool,
    pub(super) new_output: bool,
    pub(super) history_truncated: bool,
    /// Bounded accumulated normal-history motion; never queued child input.
    pub(super) queued_scroll: Option<(i16, u16, u16)>,
    /// Fresh, unsent gestures only; expiry and view changes discard them.
    pub(super) queued_alternate_wheel: std::collections::VecDeque<AlternateWheelIntent>,
    pub(super) queued_seek: Option<u16>,
}

#[derive(Clone, Copy)]
pub(super) struct AlternateWheelIntent {
    pub(super) target: TargetRef,
    pub(super) epoch: u64,
    pub(super) column: u16,
    pub(super) row: u16,
    pub(super) up: bool,
    pub(super) captured: Instant,
}

#[derive(Default)]
pub(super) struct ViewportChange {
    pub dirty: bool,
    pub status: Option<&'static str>,
    pub finish_extent: bool,
}
impl ViewportChange {
    fn set_status(&mut self, label: &'static str) {
        self.status = Some(label);
        self.dirty = true;
    }
}

pub(super) struct Viewports {
    slots: [ScreenSlot; crate::pane_layout::MAX_PANES],
}
impl Viewports {
    pub(super) fn new() -> Result<Self, TerminalError> {
        let size = TerminalSize::new(MAX_ROOT_ROWS, MAX_ROOT_COLUMNS)?;
        let slot = || -> Result<ScreenSlot, TerminalError> {
            Ok(ScreenSlot {
                target: None,
                revision: 0,
                snapshot: ScreenSnapshot::try_new(size)?,
                history: ScreenSnapshot::try_new(size)?,
                history_revision: 0,
                history_offset: 0,
                history_base: None,
                view_epoch: 0,
                history_pending: false,
                history_inflight_offset: 0,
                history_requested: 0,
                history_total: 0,
                history_extent_known: false,
                new_output: false,
                history_truncated: false,
                queued_scroll: None,
                queued_alternate_wheel: std::collections::VecDeque::new(),
                queued_seek: None,
            })
        };
        Ok(Self {
            slots: [slot()?, slot()?, slot()?, slot()?],
        })
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, ScreenSlot> {
        self.slots.iter()
    }
    #[cfg(test)]
    pub(super) fn iter_mut(&mut self) -> std::slice::IterMut<'_, ScreenSlot> {
        self.slots.iter_mut()
    }
    pub(super) fn discard_alternate(&mut self) {
        for slot in &mut self.slots {
            slot.queued_alternate_wheel.clear();
        }
    }
    pub(super) fn invalidate_pending(&mut self) {
        for slot in &mut self.slots {
            slot.history_pending = false;
            slot.queued_scroll = None;
            slot.queued_alternate_wheel.clear();
            slot.queued_seek = None;
            slot.view_epoch = slot.view_epoch.wrapping_add(1);
        }
    }
    pub(super) fn queue_seek(&mut self, target: TargetRef, offset: u16) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.target == Some(target))
        {
            if slot.history_requested != offset {
                slot.queued_seek = Some(offset);
                slot.history_requested = offset;
            }
        }
    }
    pub(super) fn cancel_seek(&mut self, target: TargetRef) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.target == Some(target))
        {
            slot.queued_seek = None;
            slot.history_requested = if slot.history_pending {
                slot.history_inflight_offset
            } else {
                slot.history_offset
            };
        }
    }
    pub(super) fn reconcile_layout(&mut self, old: Option<LayoutReply>, reply: &LayoutReply) {
        for slot in &mut self.slots {
            if let Some(target) = slot.target {
                let new_pane = reply
                    .panes
                    .iter()
                    .flatten()
                    .find(|pane| pane.target == target);
                if let Some(pane) = new_pane {
                    let old_rect = old.and_then(|layout| {
                        layout
                            .panes
                            .iter()
                            .flatten()
                            .find(|old| old.target == target)
                            .map(|old| old.rect)
                    });
                    if old_rect != Some(pane.rect) {
                        slot.history_total = 0;
                        slot.history_extent_known = false;
                        slot.history_offset = 0;
                        slot.history_base = None;
                        slot.new_output = false;
                        slot.history_truncated = false;
                        slot.history_revision = 0;
                        slot.view_epoch = slot.view_epoch.wrapping_add(1);
                        slot.history_pending = false;
                        slot.queued_scroll = None;
                        slot.queued_alternate_wheel.clear();
                        slot.queued_seek = None;
                        slot.history_pending = false;
                    }
                } else {
                    slot.target = None;
                    slot.history_total = 0;
                    slot.history_extent_known = false;
                    slot.revision = 0;
                    slot.history_offset = 0;
                    slot.history_base = None;
                    slot.new_output = false;
                    slot.history_truncated = false;
                    slot.history_revision = 0;
                    slot.view_epoch = slot.view_epoch.wrapping_add(1);
                    slot.history_pending = false;
                    slot.queued_scroll = None;
                    slot.queued_alternate_wheel.clear();
                    slot.queued_seek = None;
                    slot.history_pending = false;
                }
            }
        }
    }
    pub(super) fn apply_scroll_update(&mut self, update: ScrollUpdate) -> ViewportChange {
        let mut change = ViewportChange::default();
        let Some(slot_index) = self
            .slots
            .iter_mut()
            .position(|slot| slot.target == Some(update.target) && slot.view_epoch == update.epoch)
        else {
            if let Some(slot) = self
                .slots
                .iter_mut()
                .find(|slot| slot.target == Some(update.target))
            {
                slot.queued_alternate_wheel.clear();
            }
            return change;
        };
        let slot = &mut self.slots[slot_index];
        slot.history_pending = false;
        match update.result {
            ScrollResult::History {
                revision,
                offset_rows,
                total_rows,
                extent_known,
                checkpoint,
                row_wraps,
                history_cursor,
            } => {
                slot.queued_alternate_wheel.clear();
                slot.history_total = total_rows;
                slot.history_extent_known = extent_known;
                let requested_offset = slot.history_inflight_offset;
                if extent_known && offset_rows < requested_offset {
                    slot.history_truncated = true;
                }
                let mut effective_offset = offset_rows;
                let mut cursor_epoch_changed = false;
                if let (Some(reply_cursor), Some(live_cursor)) = (history_cursor, slot.history_base)
                {
                    if reply_cursor.epoch == live_cursor.epoch {
                        if live_cursor.ordinal > reply_cursor.ordinal {
                            let delta = live_cursor.ordinal - reply_cursor.ordinal;
                            effective_offset = add_history_rows(effective_offset, delta);
                            if extent_known {
                                slot.history_total = add_history_rows(slot.history_total, delta);
                            }
                        }
                        slot.history_base = Some(live_cursor);
                    } else {
                        slot.history_truncated = true;
                        cursor_epoch_changed = true;
                    }
                } else if history_cursor.is_some() {
                    slot.history_base = history_cursor;
                }
                // An older in-flight frame must not flash over the latest drag
                // intent. Keep the last committed history snapshot until the
                // replaceable queued seek completes.
                let superseded = slot
                    .queued_seek
                    .is_some_and(|queued| queued != effective_offset)
                    || cursor_epoch_changed;
                match panelink::screen_checkpoint::decode(&checkpoint) {
                    Ok(snapshot) if offset_rows > 0 => {
                        if !superseded && slot.history.load_checkpoint(&snapshot).is_ok() {
                            if let Some(wraps) = row_wraps.as_deref() {
                                if slot.history.apply_row_wraps(wraps).is_err() {
                                    slot.history.clear_row_wraps();
                                    slot.queued_scroll = None;
                                    slot.queued_alternate_wheel.clear();
                                    slot.queued_seek = None;
                                    change.set_status("history snapshot invalid");
                                    return change;
                                }
                            }
                            slot.history_offset = effective_offset;
                            slot.history_revision = revision;
                            slot.history_requested = effective_offset;
                            change.dirty = true;
                        } else if !superseded {
                            slot.queued_scroll = None;
                            slot.queued_alternate_wheel.clear();
                            slot.queued_seek = None;
                            change.set_status("history snapshot invalid");
                        }
                    }
                    Ok(_) => {
                        if !superseded {
                            slot.history.clear_row_wraps();
                            slot.history_offset = 0;
                            slot.history_revision = revision;
                            slot.history_base = history_cursor;
                            slot.new_output = false;
                            slot.history_truncated = false;
                            slot.history_requested = 0;
                            change.dirty = true;
                        }
                    }
                    Err(_) => {
                        slot.queued_scroll = None;
                        slot.queued_alternate_wheel.clear();
                        slot.queued_seek = None;
                        change.set_status("history snapshot invalid");
                    }
                }
            }
            ScrollResult::MouseDelivered => {
                for intent in &mut slot.queued_alternate_wheel {
                    intent.epoch = slot.view_epoch;
                }
                slot.queued_scroll = None;
                slot.queued_seek = None;
                slot.history_total = 0;
                slot.history_extent_known = false;
                slot.history_offset = 0;
                slot.history_requested = 0;
                slot.history_inflight_offset = 0;
                slot.history_base = None;
                slot.new_output = false;
                slot.history_truncated = false;
                slot.history_revision = 0;
                change.dirty = true;
            }
            ScrollResult::AlternateScreenNoMouse => {
                slot.queued_scroll = None;
                slot.queued_alternate_wheel.clear();
                slot.queued_seek = None;
                slot.history_total = 0;
                slot.history_extent_known = false;
                slot.history_offset = 0;
                slot.history_requested = 0;
                slot.history_inflight_offset = 0;
                slot.history_base = None;
                slot.new_output = false;
                slot.history_truncated = false;
                slot.history_revision = 0;
                change.dirty = true;
                change.set_status("alternate-screen app; use its scroll keys");
            }
            ScrollResult::Failed(label) => {
                slot.queued_scroll = None;
                slot.queued_alternate_wheel.clear();
                slot.queued_seek = None;
                slot.history_requested = slot.history_offset;
                change.set_status(label);
            }
        }
        change.finish_extent = true;
        change
    }
    pub(super) fn return_to_live(&mut self, target: TargetRef) -> bool {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.target == Some(target))
        {
            slot.history_offset = 0;
            slot.history_base = None;
            slot.history_revision = 0;
            slot.history_pending = false;
            slot.new_output = false;
            slot.history_truncated = false;
            slot.history_requested = 0;
            slot.history_inflight_offset = 0;
            slot.queued_scroll = None;
            slot.queued_alternate_wheel.clear();
            slot.queued_seek = None;
            slot.view_epoch = slot.view_epoch.wrapping_add(1);
            return true;
        }
        false
    }
    pub(super) fn seek(
        &mut self,
        target: TargetRef,
        requested: u16,
        refresh_extent: bool,
        effects: &mut impl Effects,
    ) -> Option<&'static str> {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.target == Some(target))?;
        if slot.snapshot.alternate_screen() {
            return None;
        }
        // A wheel's immediate reply may know only its current offset. That
        // lower bound must not cap subsequent paging or oldest-history seeks.
        let maximum = if !slot.history_extent_known {
            panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS
        } else {
            slot.history_total.max(slot.history_offset)
        };
        let offset = requested.min(maximum);
        if slot.history_pending {
            if slot.history_requested != offset {
                slot.queued_seek = Some(offset);
                slot.history_requested = offset;
            }
            return None;
        }
        if offset == slot.history_offset && !refresh_extent {
            return None;
        }
        let epoch = slot.view_epoch.wrapping_add(1);
        slot.history_pending = true;
        slot.history_requested = offset;
        slot.history_inflight_offset = offset;
        slot.queued_scroll = None;
        slot.queued_alternate_wheel.clear();
        match effects.seek_history(target, epoch, offset) {
            Ok(()) => slot.view_epoch = epoch,
            Err(label) => {
                slot.history_pending = false;
                slot.history_requested = slot.history_offset;
                return Some(label);
            }
        }
        None
    }
    pub(super) fn wheel(
        &mut self,
        target: TargetRef,
        column: u16,
        row: u16,
        up: bool,
        now: Instant,
        effects: &mut impl Effects,
    ) -> Option<&'static str> {
        let slot_index = self
            .slots
            .iter()
            .position(|slot| slot.target == Some(target))
            .or_else(|| self.slots.iter().position(|slot| slot.target.is_none()))?;
        let slot = &mut self.slots[slot_index];
        if slot.target != Some(target) {
            slot.queued_alternate_wheel.clear();
            slot.target = Some(target);
            slot.revision = 0;
            slot.history_offset = 0;
            slot.history_base = None;
            slot.history_revision = 0;
            slot.history_total = 0;
            slot.history_extent_known = false;
            slot.new_output = false;
            slot.history_truncated = false;
            slot.history_requested = 0;
            slot.history_inflight_offset = 0;
        }
        if slot.revision > 0 && slot.snapshot.alternate_screen() {
            if slot.history_pending || !slot.queued_alternate_wheel.is_empty() {
                if slot.queued_alternate_wheel.len() < ALT_WHEEL_CAPACITY {
                    slot.queued_alternate_wheel.push_back(AlternateWheelIntent {
                        target,
                        epoch: slot.view_epoch,
                        column,
                        row,
                        up,
                        captured: now,
                    });
                }
                return None;
            }
            let epoch = slot.view_epoch.wrapping_add(1);
            match effects.alternate_wheel(target, epoch, column, row, up) {
                Ok(()) => {
                    slot.view_epoch = epoch;
                    slot.history_pending = true;
                }
                Err(label) => {
                    slot.queued_alternate_wheel.clear();
                    return Some(label);
                }
            }
            return None;
        }
        // Preserve the in-flight epoch and combine only known normal-buffer
        // gestures. Alternate-screen wheel delivery is nonreplayable child input.
        if slot.history_pending || slot.queued_scroll.is_some() {
            if slot.revision > 0 && !slot.snapshot.alternate_screen() {
                let delta = if up { 3 } else { -3 };
                let current = slot.queued_scroll.map_or(0, |queued| queued.0);
                slot.queued_scroll = Some((
                    current.saturating_add(delta).clamp(-8192, 8192),
                    column,
                    row,
                ));
            }
            return None;
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
        slot.history_inflight_offset = requested;
        slot.history_requested = requested;
        match effects.scroll(target, epoch, requested, column, row, up) {
            Ok(()) => slot.view_epoch = epoch,
            Err(label) => {
                slot.history_pending = false;
                slot.queued_scroll = None;
                slot.queued_alternate_wheel.clear();
                return Some(label);
            }
        }
        None
    }
    pub(super) fn flush(
        &mut self,
        now: Instant,
        effects: &mut impl Effects,
    ) -> Option<&'static str> {
        if self.slots.iter().any(|slot| slot.history_pending) {
            return None;
        }
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| !slot.queued_alternate_wheel.is_empty())
        {
            let slot = &mut self.slots[index];
            while slot.queued_alternate_wheel.front().is_some_and(|intent| {
                now.saturating_duration_since(intent.captured) > ALT_WHEEL_TTL
                    || slot.target != Some(intent.target)
                    || slot.view_epoch != intent.epoch
                    || !slot.snapshot.alternate_screen()
                    || slot.revision == 0
            }) {
                slot.queued_alternate_wheel.pop_front();
            }
            let intent = slot.queued_alternate_wheel.pop_front()?;
            let epoch = slot.view_epoch.wrapping_add(1);
            match effects.alternate_wheel(
                intent.target,
                epoch,
                intent.column,
                intent.row,
                intent.up,
            ) {
                Ok(()) => {
                    slot.view_epoch = epoch;
                    slot.history_pending = true;
                }
                Err(label) => {
                    slot.queued_alternate_wheel.clear();
                    return Some(label);
                }
            }
            return None;
        }
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.queued_seek.is_some())
        {
            let slot = &mut self.slots[index];
            let offset = slot.queued_seek.take()?;
            let target = slot.target?;
            if slot.snapshot.alternate_screen() {
                return None;
            }
            let offset = if slot.history_extent_known {
                offset.min(slot.history_total.max(slot.history_offset))
            } else {
                offset
            };
            let epoch = slot.view_epoch.wrapping_add(1);
            match effects.seek_history(target, epoch, offset) {
                Ok(()) => {
                    slot.view_epoch = epoch;
                    slot.history_pending = true;
                    slot.history_inflight_offset = offset;
                    slot.history_requested = offset;
                }
                Err(label) => return Some(label),
            }
            return None;
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.queued_scroll.is_some())?;
        let slot = &mut self.slots[index];
        let (delta, column, row) = slot.queued_scroll.take()?;
        let target = slot.target?;
        if delta == 0 || slot.snapshot.alternate_screen() {
            return None;
        }
        let requested = (i32::from(slot.history_offset) + i32::from(delta))
            .clamp(0, i32::from(panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS))
            as u16;
        if requested == slot.history_offset {
            return None;
        }
        let epoch = slot.view_epoch.wrapping_add(1);
        match effects.scroll(target, epoch, requested, column, row, delta > 0) {
            Ok(()) => {
                slot.view_epoch = epoch;
                slot.history_pending = true;
                slot.history_inflight_offset = requested;
                slot.history_requested = requested;
            }
            Err(label) => return Some(label),
        }
        None
    }
    pub(super) fn load_screen(
        &mut self,
        target: TargetRef,
        revision: u64,
        checkpoint: &ScreenCheckpoint<'_>,
        row_wraps: Option<&[bool]>,
        history_cursor: Option<WireHistoryCursor>,
    ) -> Result<bool, TerminalError> {
        let index = self
            .slots
            .iter()
            .position(|slot| slot.target == Some(target))
            .or_else(|| self.slots.iter().position(|slot| slot.target.is_none()))
            .ok_or(TerminalError::InvalidSize)?;
        let buffer_changed = {
            let slot = &mut self.slots[index];
            let old_buffer =
                (slot.target == Some(target)).then(|| slot.snapshot.alternate_screen());
            let old_revision = slot.revision;
            slot.snapshot.load_checkpoint(checkpoint)?;
            if let Some(wraps) = row_wraps {
                slot.snapshot.apply_row_wraps(wraps)?;
            }
            slot.target = Some(target);
            slot.revision = revision;
            let buffer_changed =
                old_buffer.is_some_and(|old| old != slot.snapshot.alternate_screen());
            if buffer_changed {
                slot.history_total = 0;
                slot.history_extent_known = false;
                slot.history_offset = 0;
                slot.history_base = None;
                slot.new_output = false;
                slot.history_truncated = false;
                slot.history_pending = false;
                slot.queued_scroll = None;
                slot.queued_alternate_wheel.clear();
                slot.queued_seek = None;
                slot.view_epoch = slot.view_epoch.wrapping_add(1);
            } else if slot.history_offset > 0 {
                let revision_advanced = revision > old_revision;
                slot.new_output |= revision_advanced;
                if let Some(cursor) = history_cursor {
                    match slot.history_base {
                        Some(base) if base.epoch == cursor.epoch => {
                            if cursor.ordinal >= base.ordinal {
                                let delta = cursor.ordinal - base.ordinal;
                                if delta > 0 {
                                    slot.new_output = true;
                                    slot.history_offset =
                                        add_history_rows(slot.history_offset, delta);
                                    if slot.history_extent_known {
                                        slot.history_total =
                                            add_history_rows(slot.history_total, delta);
                                    }
                                    slot.history_requested =
                                        add_history_rows(slot.history_requested, delta);
                                    if let Some(queued) = slot.queued_seek.as_mut() {
                                        *queued = add_history_rows(*queued, delta);
                                    }
                                }
                                slot.history_base = Some(cursor);
                            }
                        }
                        Some(_) => {
                            slot.history_truncated = true;
                            slot.history_base = Some(cursor);
                        }
                        None => slot.history_base = Some(cursor),
                    }
                } else {
                    // Older servers lack row identity: revision advances can
                    // signal new output, but cannot compensate the frozen row offset.
                }
            }
            buffer_changed
        };
        Ok(buffer_changed)
    }
}
impl std::ops::Index<usize> for Viewports {
    type Output = ScreenSlot;
    fn index(&self, index: usize) -> &Self::Output {
        &self.slots[index]
    }
}
#[cfg(test)]
impl std::ops::IndexMut<usize> for Viewports {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.slots[index]
    }
}
