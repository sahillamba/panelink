//! Server-side effects for the attach client: one [`PaneConn`] per pane,
//! layout polling, change-driven screen queries, root resize and splits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use panelink::close_protocol::{
    decode_close_result, encode_close_request, CloseOutcome, CloseRequest, CloseResult, StopOutcome,
};
use panelink::control_protocol::TargetRef;
use panelink::launch::{Environment, LaunchContext};
use panelink::managed_launch_protocol::LaunchRequest;
use panelink::pane_operations_protocol::{
    self as pane_ops, PaneOperationsView, Request as PaneOperationRequest,
};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
use panelink::terminal::Key;
use panelink::ui_protocol::{
    decode_history_reply_with_wraps, decode_layout_reply, decode_resize_reply,
    decode_screen_reply_with_wraps, encode_history_query, encode_layout_query, encode_root_resize,
    encode_screen_query, HistoryReplyView, LayoutReply, RootResize, MAX_HISTORY_REPLY_BYTES,
};

use super::conn::{
    connect, context_path, ensure_context_dir, handshake, parse_context_name, Bootstrap,
    InputResult, LeaseAction, MouseInputResult, PaneConn, Reply, ResumeError, CLOSE_DEADLINE,
    DEADLINE, OP_DEADLINE, SPLIT_DEADLINE,
};
use super::pane_operations::{PaneAction, PaneOperationUpdate};
use super::state::{App, ClosePlan, SplitPlan};

const LAYOUT_INTERVAL: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(30);
const SCREEN_FALLBACK_INTERVAL: Duration = Duration::from_millis(100);
/// Total wall-clock budget for detach work after the host is restored.
pub(crate) const DETACH_BUDGET: Duration = Duration::from_millis(1_500);
const MAX_ENV_ENTRIES: usize = 1_024;
const MAX_ENV_BYTES: usize = 48 * 1024;

/// All pane connections plus the server directory and context directory.
pub(crate) struct Net {
    directory: PathBuf,
    context_dir: PathBuf,
    panes: Vec<PaneConn>,
    next_layout: Instant,
    resize_sent: Option<(u64, u16, u16)>,
    /// Device/inode of the control socket observed at attach.
    endpoint_identity: Option<(u64, u64)>,
    layout: Option<LayoutReply>,
    pane_operations: Option<PaneOperationsView>,
    pane_operations_pending: bool,
    screen_revisions: [(Option<TargetRef>, u64); panelink::pane_layout::MAX_PANES],
    history_extents: [(Option<TargetRef>, u64, u16); panelink::pane_layout::MAX_PANES],
}

/// Host-owned view inputs needed by one network poll.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NetView {
    pub focus: Option<TargetRef>,
    pub desired_root: Option<(u16, u16)>,
}

/// One validated checkpoint update returned to the host thread.
pub(crate) struct ScreenUpdate {
    pub target: TargetRef,
    pub revision: u64,
    pub checkpoint: Vec<u8>,
    pub row_wraps: Option<Vec<bool>>,
}

/// Result of one wheel action. History checkpoints remain independent of the
/// live screen cache and are applied only while the captured view epoch holds.
pub(crate) struct ScrollUpdate {
    pub target: TargetRef,
    pub epoch: u64,
    pub result: ScrollResult,
}

pub(crate) enum ScrollResult {
    History {
        revision: u64,
        offset_rows: u16,
        total_rows: u16,
        checkpoint: Vec<u8>,
        row_wraps: Option<Vec<bool>>,
    },
    MouseDelivered,
    AlternateScreenNoMouse,
    Failed(&'static str),
}

/// Bounded view delta from one worker poll.
pub(crate) struct NetUpdate {
    pub layout: Option<LayoutReply>,
    pub pane_operations: Option<PaneOperationsView>,
    pub screens: Vec<ScreenUpdate>,
    pub status: Option<&'static str>,
    pub server_present: bool,
}

impl Net {
    /// Private server directory this client attached to (read-only use).
    pub(crate) fn server_directory(&self) -> &std::path::Path {
        &self.directory
    }

    /// Connect and load the initial layout.
    ///
    /// Discovery first tries to resume an existing context under
    /// `context_dir` (so reattaching never creates a new lease). Only if none
    /// resumes does it open a fresh, trailer-less Hello; that lease is then
    /// adopted as the first pane's new context, never leaked.
    pub(crate) fn connect(
        directory: PathBuf,
        context_dir: PathBuf,
        app: &mut App,
        now: Instant,
    ) -> Result<Self, &'static str> {
        ensure_context_dir(&context_dir)?;
        let endpoint_identity = {
            use std::os::unix::fs::MetadataExt;
            std::fs::symlink_metadata(directory.join("control.sock"))
                .ok()
                .map(|metadata| (metadata.dev(), metadata.ino()))
        };
        let mut net = Self {
            directory,
            context_dir,
            panes: Vec::with_capacity(2),
            next_layout: now,
            resize_sent: None,
            endpoint_identity,
            layout: None,
            pane_operations: None,
            pane_operations_pending: false,
            screen_revisions: [(None, 0); panelink::pane_layout::MAX_PANES],
            history_extents: [(None, 0, 0); panelink::pane_layout::MAX_PANES],
        };
        let (layout, pane_operations) = match net.resume_any_context(now)? {
            Some(state) => state,
            None => {
                let started = Instant::now();
                let mut stream = connect(&net.directory, DEADLINE)?;
                let welcome = handshake(&mut stream, None, None, DEADLINE)?;
                let mut probe = Probe { stream };
                let layout = probe.layout()?;
                let first = layout.panes[0].ok_or("server reported no panes")?.target;
                let pane = PaneConn::attach(
                    &net.directory,
                    &net.context_dir,
                    first,
                    Some(Bootstrap {
                        stream: probe.stream,
                        welcome,
                        used: 1,
                        started,
                    }),
                    now,
                    DEADLINE,
                )?;
                net.panes.push(pane);
                let first_pane = net.panes.first_mut().ok_or("server unavailable")?;
                let (layout, pane_operations) = query_layout_state(first_pane)?;
                (layout, pane_operations)
            }
        };
        net.layout = Some(layout);
        net.pane_operations = pane_operations;
        net.pane_operations_pending = pane_operations.is_some();
        app.set_layout(layout);
        let mut ignored_status = None;
        net.sync_panes(layout, now, DEADLINE, &mut ignored_status);
        for pane in &mut net.panes {
            pane.set_deadline(OP_DEADLINE);
        }
        Ok(net)
    }

    /// Resume the first existing context whose pinned pane is still in the
    /// layout; returns that layout. Dead settled contexts are removed.
    fn resume_any_context(
        &mut self,
        now: Instant,
    ) -> Result<Option<(LayoutReply, Option<PaneOperationsView>)>, &'static str> {
        let Ok(entries) = std::fs::read_dir(&self.context_dir) else {
            return Ok(None);
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| parse_context_name(name).is_some())
            .take(64)
            .collect();
        names.sort();
        for name in names {
            let path = self.context_dir.join(&name);
            let mut pane = match PaneConn::resume(&self.directory, &path, None, now, DEADLINE) {
                Ok(pane) => pane,
                Err(ResumeError::Removed | ResumeError::Other(_)) => continue,
            };
            let (layout, pane_operations) = match query_layout_state(&mut pane) {
                Ok(state) => state,
                Err(label) if pane.pane_operations => return Err(label),
                Err(_) => continue,
            };
            let in_layout = layout
                .panes
                .iter()
                .flatten()
                .any(|entry| entry.target == pane.target);
            // Only the canonical name for that pane is adopted, so later
            // `attach` calls find exactly this context again.
            if in_layout && path == context_path(&self.context_dir, pane.target, 0) {
                self.panes.push(pane);
                return Ok(Some((layout, pane_operations)));
            }
            pane.detach();
        }
        Ok(None)
    }

    fn pane_mut(&mut self, target: TargetRef) -> Option<&mut PaneConn> {
        self.panes.iter_mut().find(|pane| pane.target == target)
    }

    /// Inspect current terminal modes with READ only, then either deliver one
    /// typed wheel operation under the existing owner/dedup path or return a
    /// pure retained-history checkpoint. Never falls back after uncertain
    /// input delivery.
    pub(crate) fn scroll(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
        column: u16,
        row: u16,
        up: bool,
    ) -> ScrollUpdate {
        self.scroll_inner(target, epoch, offset_rows, Some((column, row, up)))
    }

    /// Read retained history without ever claiming input or forwarding a wheel
    /// event, even if the server observes a newly entered alternate screen.
    pub(crate) fn seek_history(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
    ) -> ScrollUpdate {
        self.scroll_inner(target, epoch, offset_rows, None)
    }

    fn scroll_inner(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
        wheel: Option<(u16, u16, bool)>,
    ) -> ScrollUpdate {
        let extent = self
            .history_extents
            .iter()
            .find(|entry| entry.0 == Some(target))
            .copied();
        let result = (|| {
            let pane = self.pane_mut(target).ok_or("pane not connected")?;
            let query =
                encode_history_query(target, offset_rows).map_err(|_| "history request invalid")?;
            let bytes = match pane.request(&query)? {
                Reply::Snapshot(bytes) => bytes,
                Reply::Error(label) => return Err(label),
                Reply::Response(_) => return Err("history reply invalid"),
            };
            let view = decode_history_reply_with_wraps(&bytes, pane.screen_row_wraps)
                .map_err(|_| "history reply invalid")?;
            if view.target() != target {
                return Err("history reply invalid");
            }
            validate_history_checkpoint(&view)?;
            if let Some((column, row, up)) =
                wheel.filter(|_| view.alternate_screen() && view.mouse_reporting())
            {
                if pane.owner_revision.is_none() {
                    let _ = pane.claim(Instant::now());
                }
                return match pane.send_mouse_wheel(column, row, up)? {
                    MouseInputResult::Applied => Ok(ScrollResult::MouseDelivered),
                    MouseInputResult::Unsupported => {
                        // The server explicitly rejected before admission; a
                        // fresh READ decides whether scrollback is now valid.
                        let retry = encode_history_query(target, offset_rows)
                            .map_err(|_| "history request invalid")?;
                        let retry = match pane.request(&retry)? {
                            Reply::Snapshot(bytes) => bytes,
                            Reply::Error(label) => return Err(label),
                            Reply::Response(_) => return Err("history reply invalid"),
                        };
                        let retry = decode_history_reply_with_wraps(&retry, pane.screen_row_wraps)
                            .map_err(|_| "history reply invalid")?;
                        if retry.target() != target {
                            return Err("history reply invalid");
                        }
                        validate_history_checkpoint(&retry)?;
                        history_result(pane, retry, extent)
                    }
                    MouseInputResult::NotOwner => {
                        Err("input is owned by another client; wheel not sent")
                    }
                    MouseInputResult::Failed(label) => Err(label),
                };
            }
            history_result(pane, view, extent)
        })();
        if let Ok(ScrollResult::History {
            revision,
            offset_rows,
            total_rows,
            ..
        }) = &result
        {
            let index = self
                .history_extents
                .iter()
                .position(|entry| entry.0 == Some(target))
                .or_else(|| {
                    self.history_extents.iter().position(|entry| {
                        !self.panes.iter().any(|pane| Some(pane.target) == entry.0)
                    })
                })
                .unwrap_or(0);
            if total_rows > offset_rows {
                self.history_extents[index] = (Some(target), *revision, *total_rows);
            }
        }
        ScrollUpdate {
            target,
            epoch,
            result: result.unwrap_or_else(ScrollResult::Failed),
        }
    }

    /// Execute one captured keyed mutation and reconcile its authoritative view.
    /// An uncertain key is settled by `PaneConn::keyed`; this method never
    /// creates a replacement key for a mutation whose result is unknown.
    pub(crate) fn pane_operation(
        &mut self,
        action: PaneAction,
    ) -> Result<PaneOperationUpdate, &'static str> {
        let target = action.captured_target();
        let (view, outcome) = {
            let pane = self.pane_mut(target).ok_or("pane not connected")?;
            if !pane.pane_operations {
                return Err("pane operations are not supported by this server");
            }
            let mutation = pane.keyed(|key| {
                pane_ops::encode_request(&action.request(key)).map_err(|_| "request_invalid")
            });
            let (key, reply) = match mutation {
                Ok(result) => result,
                Err(label) => {
                    if !pane.connected() {
                        pane.disconnect(Instant::now());
                    }
                    return Err(label);
                }
            };
            let bytes = match reply {
                Reply::Response(bytes) => bytes,
                Reply::Error(label) => return Err(label),
                Reply::Snapshot(_) => return Err("pane operation reply invalid"),
            };
            let response =
                decode_response_payload(&bytes).map_err(|_| "pane operation reply invalid")?;
            if response.key != key {
                return Err("pane operation reply invalid");
            }
            let outcome = match response.body {
                ResponseBody::Rejected(_) => {
                    pane.ack(key)?;
                    return Err("pane operation rejected");
                }
                ResponseBody::Status(OperationStatus::NotAdmitted) => {
                    pane.ack(key)?;
                    return Err("pane operation was not admitted");
                }
                ResponseBody::Status(OperationStatus::InProgress) => {
                    return Err("pane operation outcome pending");
                }
                ResponseBody::Status(OperationStatus::Completed(outcome)) => {
                    pane.ack_admitted(key)?;
                    outcome
                }
                ResponseBody::Read { .. } => return Err("pane operation reply invalid"),
            };
            if !matches!(outcome, Outcome::Applied | Outcome::Partial) {
                return Err(match outcome {
                    Outcome::Failed => "pane operation failed",
                    Outcome::Unknown => "pane operation outcome unknown",
                    Outcome::Accepted => "pane operation outcome pending",
                    Outcome::Applied | Outcome::Partial => unreachable!(),
                });
            }
            let view = match query_pane_operations_view(pane) {
                Ok(view) => view,
                Err(label) => {
                    if !pane.connected() {
                        pane.disconnect(Instant::now());
                    }
                    return Err(label);
                }
            };
            (view, outcome)
        };

        self.layout = Some(view.layout);
        self.pane_operations = Some(view);
        self.pane_operations_pending = true;
        for pane in &mut self.panes {
            pane.needs_screen = true;
        }
        let mut status = None;
        self.sync_panes(view.layout, Instant::now(), OP_DEADLINE, &mut status);
        Ok(PaneOperationUpdate { view, outcome })
    }

    fn any_connected(&mut self) -> Option<&mut PaneConn> {
        self.panes.iter_mut().find(|pane| pane.connected())
    }

    /// Connect to any layout pane we lack; drop panes that left the layout.
    fn sync_panes(
        &mut self,
        layout: LayoutReply,
        now: Instant,
        deadline: Duration,
        status: &mut Option<&'static str>,
    ) {
        retain_layout_panes(&mut self.panes, layout, |pane| pane.target);
        for entry in layout.panes.iter().flatten() {
            if self.panes.iter().any(|pane| pane.target == entry.target) {
                continue;
            }
            match PaneConn::attach(
                &self.directory,
                &self.context_dir,
                entry.target,
                None,
                now,
                deadline,
            ) {
                Ok(mut pane) => {
                    pane.set_deadline(OP_DEADLINE);
                    self.panes.push(pane);
                }
                Err(label) => *status = Some(label),
            }
        }
    }

    /// Lease maintenance for one pane: proactive resume before expiry,
    /// reconnect after loss, and re-attach after a proven lease death.
    /// Never resends input; uncertainty is settled by status only.
    fn maintain(&mut self, index: usize, now: Instant) -> Option<&'static str> {
        let pane = &mut self.panes[index];
        if pane.lease_dead {
            let target = pane.target;
            // Drop (and unlock) the dead context, then re-attach: a settled
            // dead context is replaced with a fresh lease; an unsettled one
            // is kept and reported.
            let placeholder = self.panes.remove(index);
            drop(placeholder);
            match PaneConn::attach(
                &self.directory,
                &self.context_dir,
                target,
                None,
                now,
                OP_DEADLINE,
            ) {
                Ok(pane) => self.panes.insert(index, pane),
                Err(label) => {
                    self.sync_later(now);
                    return Some(label);
                }
            }
            return None;
        }
        if !pane.connected() {
            if now >= pane.next_retry && pane.reconnect(&self.directory, now).is_err() {
                pane.disconnect(now);
            }
            return None;
        }
        if pane.lease_schedule.action(now) == LeaseAction::Renew {
            // Proactive resume Hello on a new stream renews the lease; the
            // old stream is replaced only if the new handshake succeeds.
            match pane.reconnect(&self.directory, now) {
                Ok(()) => {}
                Err(label) => {
                    pane.lease_schedule.failed(now);
                    if pane.lease_schedule.past_ttl(now) {
                        pane.lease_dead = true;
                    }
                    return Some(label);
                }
            }
        }
        None
    }

    /// A pane could not be re-attached now; layout polling retries it.
    fn sync_later(&mut self, now: Instant) {
        self.next_layout = now + LAYOUT_INTERVAL;
    }

    /// One bounded poll step: layout, resize, reconnect, claims, screens.
    ///
    /// Every exchange is bounded by `OP_DEADLINE`; the stop flag is checked
    /// between panes so a stalled server cannot delay Ctrl-C/SIGTERM handling
    /// by more than about one exchange per pane.
    /// Whether the server this client attached to still owns the endpoint.
    ///
    /// A stopped server removes its socket (`NotFound`); a restarted server
    /// binds a new socket inode. Either means the attached workspace ended.
    /// Any other metadata error is treated as transient, not as closure.
    pub(crate) fn server_endpoint_present(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(self.directory.join("control.sock")) {
            Ok(metadata) => self
                .endpoint_identity
                .is_none_or(|identity| identity == (metadata.dev(), metadata.ino())),
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    pub(crate) fn poll(&mut self, view: NetView, now: Instant, stop: &AtomicBool) -> NetUpdate {
        let mut update = NetUpdate {
            layout: None,
            pane_operations: None,
            screens: Vec::with_capacity(2),
            status: None,
            server_present: self.server_endpoint_present(),
        };
        let mut index = 0;
        while index < self.panes.len() {
            let before = self.panes.len();
            update.status = self.maintain(index, now).or(update.status);
            if self.panes.len() == before {
                index += 1;
            }
        }
        if now >= self.next_layout {
            self.next_layout = now + LAYOUT_INTERVAL;
            self.refresh_layout(now, &mut update);
            self.maybe_resize(view.desired_root, now, &mut update);
        }
        for index in 0..self.panes.len() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let target = self.panes[index].target;
            let known_revision = self.screen_revision(target);
            let mut remember_revision = None;
            let pane = &mut self.panes[index];
            if !pane.connected() {
                continue;
            }
            if pane.owner_revision.is_none() && now >= pane.next_claim {
                match pane.claim(now) {
                    Ok(true) => {}
                    Ok(false) if Some(pane.target) == view.focus => {
                        update.status = Some("pane input is owned by another client")
                    }
                    Ok(false) => {}
                    Err(label) => {
                        update.status = Some(label);
                        pane.disconnect(now);
                        continue;
                    }
                }
            } else if pane.owner_revision.is_some() && now >= pane.next_claim {
                // Renew before the 30 s owner TTL elapses.
                if let Err(label) = pane.claim(now) {
                    update.status = Some(label);
                }
            }
            if now < pane.next_watch {
                continue;
            }
            let changed = if pane.events {
                pane.next_watch = now + POLL_INTERVAL;
                match pane.watch() {
                    Ok(changed) => changed,
                    Err(label) => {
                        update.status = Some(label);
                        pane.disconnect(now);
                        continue;
                    }
                }
            } else {
                pane.next_watch = now + SCREEN_FALLBACK_INTERVAL;
                true
            };
            if changed || pane.needs_screen {
                match fetch_screen(pane, target, known_revision) {
                    Ok(Some(screen)) => {
                        remember_revision = Some(screen.revision);
                        update.screens.push(screen);
                        pane.needs_screen = false;
                    }
                    Ok(None) => pane.needs_screen = false,
                    Err(label) => {
                        update.status = Some(label);
                        pane.needs_screen = true;
                    }
                }
            }
            if let Some(revision) = remember_revision {
                self.remember_screen(target, revision);
            }
        }
        update
    }

    fn refresh_layout(&mut self, now: Instant, update: &mut NetUpdate) {
        let Some(pane) = self.any_connected() else {
            return;
        };
        let state = match query_layout_state(pane) {
            Ok(state) => Ok(state),
            Err(label) => {
                if !pane.connected() {
                    pane.disconnect(now);
                }
                Err(label)
            }
        };
        match state {
            Ok((layout, pane_operations)) => {
                let before = self.layout;
                let previous_operations = self.pane_operations;
                self.layout = Some(layout);
                self.pane_operations = pane_operations;
                if pane_operations.is_some() && previous_operations != pane_operations {
                    self.pane_operations_pending = true;
                }
                if before != Some(layout) {
                    for pane in &mut self.panes {
                        pane.needs_screen = true;
                    }
                    for (target, revision) in &mut self.screen_revisions {
                        if target.is_some_and(|target| {
                            !layout
                                .panes
                                .iter()
                                .flatten()
                                .any(|pane| pane.target == target)
                        }) {
                            *target = None;
                            *revision = 0;
                        }
                    }
                    update.layout = Some(layout);
                }
                if self.pane_operations_pending {
                    update.pane_operations = pane_operations;
                    self.pane_operations_pending = false;
                }
                // Also re-attaches panes whose earlier attach failed.
                self.sync_panes(layout, now, OP_DEADLINE, &mut update.status);
            }
            Err(label) => update.status = Some(label),
        }
    }

    /// Send one RootResize when the host size differs from the layout root.
    /// Below the minimum the last accepted layout is kept untouched.
    fn maybe_resize(
        &mut self,
        desired_root: Option<(u16, u16)>,
        now: Instant,
        update: &mut NetUpdate,
    ) {
        let (Some(layout), Some((rows, columns))) = (self.layout, desired_root) else {
            return;
        };
        if (layout.rows, layout.columns) == (rows, columns)
            || self.resize_sent == Some((layout.revision, rows, columns))
        {
            return;
        }
        if !self.panes.iter().any(PaneConn::connected) {
            return;
        }
        // One attempt per (layout revision, size); never re-sent blindly.
        self.resize_sent = Some((layout.revision, rows, columns));
        let Some(pane) = self.any_connected() else {
            return;
        };
        let result = pane.keyed(|key| {
            encode_root_resize(&RootResize {
                key,
                expected_layout_revision: layout.revision,
                rows,
                columns,
            })
            .map(|bytes| bytes.to_vec())
            .map_err(|_| "request_invalid")
        });
        match result {
            Ok((key, Reply::Response(bytes))) => {
                if let Ok(reply) = decode_resize_reply(&bytes) {
                    if reply.key == key {
                        let _ = pane.ack_admitted(key);
                        self.next_layout = now;
                        return;
                    }
                }
                // A rejection/status response is still terminal for this key.
                match decode_response_payload(&bytes) {
                    Ok(response) if response.key == key => match response.body {
                        ResponseBody::Status(OperationStatus::InProgress) => {}
                        ResponseBody::Status(OperationStatus::Completed(_)) => {
                            let _ = pane.ack_admitted(key);
                        }
                        _ => {
                            let _ = pane.ack(key);
                        }
                    },
                    _ => {}
                }
                update.status = Some("resize was not applied");
            }
            Ok((_, Reply::Error(label))) => {
                update.status = Some(label);
                self.next_layout = now;
            }
            Ok((_, Reply::Snapshot(_))) => update.status = Some("resize reply invalid"),
            Err(label) => update.status = Some(label),
        }
    }

    fn screen_revision(&self, target: TargetRef) -> Option<u64> {
        self.screen_revisions
            .iter()
            .find_map(|(known, revision)| (*known == Some(target)).then_some(*revision))
    }

    fn remember_screen(&mut self, target: TargetRef, revision: u64) {
        let index = self
            .screen_revisions
            .iter()
            .position(|(known, _)| *known == Some(target))
            .or_else(|| {
                self.screen_revisions
                    .iter()
                    .position(|(known, _)| known.is_none())
            });
        if let Some(index) = index {
            self.screen_revisions[index] = (Some(target), revision);
        }
    }

    /// Bounded detach: settle, release claims and retire results on each
    /// connected pane (each exchange ≤ `DETACH_DEADLINE`, stopping at the
    /// first stall), then drop connections. Children keep running.
    pub(crate) fn detach(&mut self) {
        let budget = Instant::now() + DETACH_BUDGET;
        for pane in &mut self.panes {
            if Instant::now() >= budget {
                break;
            }
            pane.detach();
        }
        self.panes.clear();
    }
}

/// Read schema 13 only when this exact lease selected its capability. A
/// malformed selected view is an error; it never falls back to legacy layout.
fn query_layout_state(
    pane: &mut PaneConn,
) -> Result<(LayoutReply, Option<PaneOperationsView>), &'static str> {
    if pane.pane_operations {
        let view = query_pane_operations_view(pane)?;
        return Ok((view.layout, Some(view)));
    }
    let bytes = match pane.request(&encode_layout_query())? {
        Reply::Response(bytes) => bytes,
        Reply::Error(label) => return Err(label),
        Reply::Snapshot(_) => return Err("layout invalid"),
    };
    let layout = decode_layout_reply(&bytes).map_err(|_| "layout invalid")?;
    Ok((layout, None))
}

fn query_pane_operations_view(pane: &mut PaneConn) -> Result<PaneOperationsView, &'static str> {
    let query = pane_ops::encode_request(&PaneOperationRequest::ViewQuery)
        .map_err(|_| "pane operations request invalid")?;
    let bytes = match pane.request(&query)? {
        Reply::Response(bytes) => bytes,
        Reply::Error(label) => return Err(label),
        Reply::Snapshot(_) => return Err("pane operations view invalid"),
    };
    let view = pane_ops::decode_view(&bytes).map_err(|_| "pane operations view invalid")?;
    validate_pane_operations_scope(&view, pane.target)?;
    Ok(view)
}

/// A live connection authorizes a current workspace view, even when its
/// pinned occupant has exited and is absent from that view. Every returned
/// target must still belong to the exact connection generation and workspace.
fn validate_pane_operations_scope(
    view: &PaneOperationsView,
    querying_target: TargetRef,
) -> Result<(), &'static str> {
    if view.layout.panes.iter().flatten().all(|entry| {
        entry.target.generation == querying_target.generation
            && entry.target.workspace == querying_target.workspace
    }) {
        Ok(())
    } else {
        Err("pane operations view invalid")
    }
}

/// Keep only connections represented by a validated authoritative layout.
/// Kept generic so the stale-target lifecycle can be exercised without a
/// live socket or a fabricated authenticated `PaneConn`.
fn retain_layout_panes<T>(
    panes: &mut Vec<T>,
    layout: LayoutReply,
    target: impl Fn(&T) -> TargetRef,
) {
    panes.retain(|pane| {
        layout
            .panes
            .iter()
            .flatten()
            .any(|entry| entry.target == target(pane))
    });
}

fn validate_history_checkpoint(view: &HistoryReplyView<'_>) -> Result<(), &'static str> {
    if view.checkpoint().len() > MAX_HISTORY_REPLY_BYTES {
        return Err("history reply invalid");
    }
    panelink::screen_checkpoint::decode(view.checkpoint())
        .map(|_| ())
        .map_err(|_| "history checkpoint invalid")
}

fn history_result(
    pane: &mut PaneConn,
    view: HistoryReplyView<'_>,
    cached_extent: Option<(Option<TargetRef>, u64, u16)>,
) -> Result<ScrollResult, &'static str> {
    if view.alternate_screen() {
        return Ok(ScrollResult::AlternateScreenNoMouse);
    }
    let checkpoint = panelink::screen_checkpoint::decode(view.checkpoint())
        .map_err(|_| "history checkpoint invalid")?;
    let row_wraps = row_wrap_flags(checkpoint.rows(), view.wrap_metadata_available(), |row| {
        view.row_wrapped(row)
    });
    // Existing read-only queries clamp to the available history. Probe the
    // oldest viewport once per live revision to size the scrollbar without a
    // wire change. This query never forwards mouse input or claims ownership.
    let total_rows = if let Some((_, _, total)) =
        cached_extent.filter(|(_, revision, _)| *revision == view.screen_revision())
    {
        total.max(view.offset_rows())
    } else {
        // Chrome must not turn an already valid viewport into a failed scroll.
        // A conservative extent is not cached, so a later gesture can retry the
        // read-only probe. Never combine extents from different screen revisions.
        let probed = (|| {
            let query = encode_history_query(
                view.target(),
                panelink::ui_protocol::MAX_HISTORY_OFFSET_ROWS,
            )
            .map_err(|_| "history request invalid")?;
            let bytes = match pane.request(&query)? {
                Reply::Snapshot(bytes) => bytes,
                Reply::Error(label) => return Err(label),
                Reply::Response(_) => return Err("history reply invalid"),
            };
            let oldest = decode_history_reply_with_wraps(&bytes, pane.screen_row_wraps)
                .map_err(|_| "history reply invalid")?;
            validate_history_checkpoint(&oldest)?;
            if oldest.target() != view.target()
                || oldest.screen_revision() != view.screen_revision()
                || oldest.alternate_screen()
                || oldest.offset_rows() < view.offset_rows()
            {
                return Err("history extent changed");
            }
            Ok(oldest.offset_rows())
        })();
        probed.unwrap_or(view.offset_rows())
    };
    Ok(ScrollResult::History {
        revision: view.screen_revision(),
        offset_rows: view.offset_rows(),
        total_rows,
        checkpoint: view.checkpoint().to_vec(),
        row_wraps,
    })
}

fn row_wrap_flags(
    rows: u16,
    available: bool,
    mut row_wrapped: impl FnMut(u16) -> Option<bool>,
) -> Option<Vec<bool>> {
    if !available {
        return None;
    }
    (0..rows).map(&mut row_wrapped).collect()
}

/// How one close reply frame was classified.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CloseReply {
    /// A valid schema-9 result for exactly this key (admitted; terminal).
    Result(CloseResult),
    /// A schema-1 response for this key (for example a pre-admission
    /// rejection): terminal, never admitted.
    Refused,
    /// Anything else; the key stays unacknowledged.
    Invalid,
}

/// Classify a response payload for close `key`.
pub(crate) fn classify_close_reply(key: OperationKey, bytes: &[u8]) -> CloseReply {
    match decode_close_result(bytes) {
        Ok(result) if result.key == key => CloseReply::Result(result),
        Ok(_) => CloseReply::Invalid,
        Err(_) if decode_response_payload(bytes).is_ok_and(|response| response.key == key) => {
            CloseReply::Refused
        }
        Err(_) => CloseReply::Invalid,
    }
}

/// Encode one schema-9 close request for exactly the captured plan.
pub(crate) fn encode_close(key: OperationKey, plan: ClosePlan) -> Result<Vec<u8>, &'static str> {
    let mut payload = Vec::new();
    encode_close_request(
        &CloseRequest {
            key,
            target: plan.target,
            expected_layout_revision: plan.expected_layout_revision,
        },
        &mut payload,
    )
    .map_err(|_| "close request invalid")?;
    Ok(payload)
}

fn fetch_screen(
    pane: &mut PaneConn,
    target: TargetRef,
    known_revision: Option<u64>,
) -> Result<Option<ScreenUpdate>, &'static str> {
    let query = encode_screen_query(target).map_err(|_| "request_invalid")?;
    let bytes = match pane.request(&query)? {
        Reply::Snapshot(bytes) => bytes,
        Reply::Error(label) => return Err(label),
        Reply::Response(_) => return Err("screen reply invalid"),
    };
    let view = decode_screen_reply_with_wraps(&bytes, pane.screen_row_wraps)
        .map_err(|_| "screen reply invalid")?;
    if view.target() != target {
        return Err("screen reply invalid");
    }
    if known_revision == Some(view.screen_revision()) {
        return Ok(None);
    }
    let checkpoint =
        panelink::screen_checkpoint::decode(view.checkpoint()).map_err(|_| "screen invalid")?;
    let row_wraps = row_wrap_flags(checkpoint.rows(), view.wrap_metadata_available(), |row| {
        view.row_wrapped(row)
    });
    Ok(Some(ScreenUpdate {
        target,
        revision: view.screen_revision(),
        checkpoint: view.checkpoint().to_vec(),
        row_wraps,
    }))
}

impl Net {
    pub(crate) fn send_keys(
        &mut self,
        target: TargetRef,
        keys: &[Key],
    ) -> Result<(), &'static str> {
        let pane = self.pane_mut(target).ok_or("pane not connected")?;
        // Not yet owner (for example a freshly split or reconnected pane):
        // try one immediate claim rather than dropping the keys.
        if pane.owner_revision.is_none() {
            let _ = pane.claim(Instant::now());
        }
        match pane.send_keys(keys)? {
            InputResult::Applied => {
                pane.next_watch = Instant::now();
                Ok(())
            }
            InputResult::NotOwner => Err("input owned by another client; keys not sent"),
            InputResult::Failed(label) => Err(label),
        }
    }

    pub(crate) fn send_paste(&mut self, target: TargetRef, text: &str) -> Result<(), &'static str> {
        let pane = self.pane_mut(target).ok_or("pane not connected")?;
        match pane.send_paste(text)? {
            InputResult::Applied => Ok(()),
            InputResult::NotOwner => Err("input owned by another client; paste not sent"),
            InputResult::Failed(label) => Err(label),
        }
    }

    pub(crate) fn split(&mut self, plan: SplitPlan) -> Result<TargetRef, &'static str> {
        let context = capture_launch_context()?;
        let pane = self.pane_mut(plan.target).ok_or("pane not connected")?;
        // The server spawns a child for this request: allow longer.
        pane.set_deadline(SPLIT_DEADLINE);
        let result = pane.keyed(|key| encode_split(key, plan, &context));
        pane.set_deadline(OP_DEADLINE);
        drop(context);
        let (key, reply) = result?;
        let bytes = match reply {
            Reply::Response(bytes) => bytes,
            Reply::Error(label) => return Err(label),
            Reply::Snapshot(_) => return Err("split reply invalid"),
        };
        let result = match decode_split_result(&bytes) {
            Ok(result) if result.key == key => result,
            _ => {
                // A schema-1 rejection (for example unauthorized) is terminal.
                if decode_response_payload(&bytes).is_ok_and(|response| response.key == key) {
                    let _ = pane.ack(key);
                    return Err("split rejected by the server");
                }
                return Err("split reply invalid");
            }
        };
        pane.ack_admitted(key)?;
        let new_target = match (result.outcome, result.new_target) {
            (Outcome::Applied, Some(target)) => target,
            (Outcome::Failed, _) => {
                return Err("split rejected: layout changed or pane limit; reopen the menu")
            }
            (_, Some(_)) => return Err("split committed but not fully applied"),
            (_, None) => return Err("split failed"),
        };
        let now = Instant::now();
        let mut attached = PaneConn::attach(
            &self.directory,
            &self.context_dir,
            new_target,
            None,
            now,
            SPLIT_DEADLINE,
        )?;
        attached.set_deadline(OP_DEADLINE);
        // Claim input now so keys typed right after the split (the new pane
        // is focused) are delivered instead of being refused as not-owner.
        let _ = attached.claim(now);
        self.panes.push(attached);
        self.next_layout = now;
        Ok(new_target)
    }

    pub(crate) fn close(&mut self, plan: ClosePlan) -> Result<CloseResult, &'static str> {
        let index = self
            .close_connection(plan.target)
            .ok_or("close not sent: no connected pane")?;
        let pane = &mut self.panes[index];
        // The server's bounded stop may take about 2 s before it replies.
        // One fresh key; a lost reply is settled by status, never resent.
        pane.set_deadline(CLOSE_DEADLINE);
        let result = pane.keyed(|key| encode_close(key, plan));
        pane.set_deadline(OP_DEADLINE);
        let (key, reply) = result?;
        let bytes = match reply {
            Reply::Response(bytes) => bytes,
            Reply::Error(label) => return Err(label),
            Reply::Snapshot(_) => return Err("close reply invalid"),
        };
        let result = match classify_close_reply(key, &bytes) {
            CloseReply::Result(result) => result,
            CloseReply::Refused => {
                let _ = pane.ack(key);
                return Err("close rejected by the server");
            }
            CloseReply::Invalid => return Err("close reply invalid"),
        };
        // Every schema-9 result for our key (including a stale rejection) was
        // admitted and is terminal.
        let _ = pane.ack_admitted(key);
        if matches!(
            result.outcome,
            CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated)
        ) {
            // The closed pane's connection is dropped now; the collapsed
            // layout is fetched on the next poll (through a survivor).
            self.panes.retain(|pane| pane.target != plan.target);
        }
        self.next_layout = Instant::now();
        Ok(result)
    }
}

impl Net {
    /// Index of the connection used for a close: the target's own pinned
    /// connection when connected, otherwise any connected pane. Every same-UID
    /// lease may close any pane; the key always uses that connection's lease.
    fn close_connection(&self, target: TargetRef) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.target == target && pane.connected())
            .or_else(|| self.panes.iter().position(PaneConn::connected))
    }
}

/// Encode a schema-7 split launching exactly `/bin/zsh` with this client's
/// own captured cwd/environment. Minimal copy of the `control split` builder
/// (`p3_cli::ui_split`), kept local by ownership rules.
pub(crate) fn encode_split(
    key: OperationKey,
    plan: SplitPlan,
    context: &LaunchContext,
) -> Result<Vec<u8>, &'static str> {
    use std::os::unix::ffi::OsStrExt;
    let cwd = context.working_directory.as_os_str().as_bytes();
    let environment = panelink::multi_startup::environment_entries(&context.environment);
    let launch = LaunchRequest {
        key,
        rows: plan.rows,
        columns: plan.columns,
        requested_executable: b"/bin/zsh",
        requested_working_directory: cwd,
        client_working_directory: cwd,
        arguments: &[],
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_split_request(
        &SplitRequest {
            key,
            target: plan.target,
            expected_layout_revision: plan.expected_layout_revision,
            direction: plan.direction,
            launch: &launch,
        },
        &mut payload,
    )
    .map_err(|_| "split request invalid (environment too large?)")?;
    Ok(payload)
}

/// This client's own cwd and environment minus `PANELINK_*`, bounded like
/// the CLI (1,024 entries / 48 KiB). Values are never logged.
pub(crate) fn capture_launch_context() -> Result<LaunchContext, &'static str> {
    let working_directory = std::env::current_dir().map_err(|_| "cwd unavailable")?;
    let mut environment = Environment::new();
    let mut total = 0_usize;
    for (name, value) in std::env::vars_os() {
        use std::os::unix::ffi::OsStrExt;
        if name.as_bytes().starts_with(b"PANELINK_") {
            continue;
        }
        total = total
            .checked_add(name.len() + value.len() + 6)
            .filter(|total| *total <= MAX_ENV_BYTES)
            .ok_or("environment too large for a split")?;
        if environment.len() >= MAX_ENV_ENTRIES {
            return Err("environment too large for a split");
        }
        environment.insert(name, value);
    }
    Ok(LaunchContext {
        working_directory,
        environment,
    })
}

/// Bootstrap stream before a context exists.
struct Probe {
    stream: panelink::transport::AuthenticatedStream,
}

impl Probe {
    fn layout(&mut self) -> Result<LayoutReply, &'static str> {
        use panelink::protocol_wire::{Frame, MessageKind};
        let query = encode_layout_query();
        let frame = Frame::new(MessageKind::Request, 1, &query).map_err(|_| "request_invalid")?;
        self.stream
            .write_frame(&frame, Instant::now() + super::conn::DEADLINE)
            .map_err(|_| "connection lost")?;
        let reply = self
            .stream
            .read_frame(Instant::now() + super::conn::DEADLINE)
            .map_err(|_| "connection lost")?;
        if reply.request_id() != 1 {
            return Err("response_invalid");
        }
        match reply.kind() {
            MessageKind::Response => {
                decode_layout_reply(reply.payload()).map_err(|_| "layout invalid")
            }
            MessageKind::Error => Err(super::conn::server_label(reply.payload())),
            _ => Err("response_invalid"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelink::pane_layout::Rect;
    use panelink::pane_layout::SplitDirection;
    use panelink::split_protocol::decode_split_request;
    use panelink::ui_protocol::UiPane;
    use std::ffi::OsString;

    fn target(generation: u64, workspace: u64, pane: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace,
            pane,
            session: pane,
            occupant_revision: 1,
        }
    }

    fn view_with_target(target: TargetRef) -> PaneOperationsView {
        PaneOperationsView {
            layout: LayoutReply {
                revision: 9,
                rows: 24,
                columns: 80,
                panes: [
                    Some(UiPane {
                        target,
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
                dividers: [None; panelink::pane_layout::MAX_PANES - 1],
            },
            zoomed_target: None,
            confirmed_sizes: [Some((22, 78)), None, None, None],
        }
    }

    #[test]
    fn refresh_accepts_survivor_view_over_exited_pane_socket_and_prunes_it() {
        use panelink::handshake_protocol::{Welcome, CAPABILITY_PANE_OPERATIONS, CAPABILITY_READ};
        use panelink::protocol_wire::{Frame, MessageKind};
        use panelink::transport::AuthenticatedStream;
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::net::UnixStream;
        use std::time::{SystemTime, UNIX_EPOCH};

        struct PrivateRoot(PathBuf);
        impl Drop for PrivateRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let serial = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PrivateRoot(PathBuf::from(format!(
            "/private/tmp/panelink-refresh-{}-{serial}",
            std::process::id()
        )));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .unwrap();
        let exited = target(7, 3, 1);
        let survivor = target(7, 3, 2);
        let view = view_with_target(survivor);
        let now = Instant::now();
        let mut panes = Vec::new();
        let mut peers = Vec::new();
        for (index, target) in [exited, survivor].into_iter().enumerate() {
            let (client, server) = UnixStream::pair().unwrap();
            let boot = Bootstrap {
                stream: AuthenticatedStream::new(client).unwrap(),
                welcome: Welcome {
                    nonce: 1,
                    generation: 7,
                    lease: 10 + index as u64,
                    selected_capabilities: CAPABILITY_READ | CAPABILITY_PANE_OPERATIONS,
                    ttl_seconds: 1800,
                },
                used: 0,
                started: now,
            };
            panes.push(
                PaneConn::attach(
                    &root.0,
                    &root.0,
                    target,
                    Some(boot),
                    now,
                    Duration::from_secs(2),
                )
                .unwrap(),
            );
            peers.push(AuthenticatedStream::new(server).unwrap());
        }
        let mut net = Net {
            directory: root.0.clone(),
            context_dir: root.0.clone(),
            panes,
            next_layout: now,
            resize_sent: None,
            endpoint_identity: None,
            layout: Some(view_with_target(exited).layout),
            pane_operations: None,
            pane_operations_pending: false,
            screen_revisions: [(None, 0); panelink::pane_layout::MAX_PANES],
            history_extents: [(None, 0, 0); panelink::pane_layout::MAX_PANES],
        };
        std::thread::scope(|scope| {
            let server = scope.spawn(move || {
                for mut peer in peers {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let query = peer.read_frame(deadline).unwrap();
                    assert_eq!(query.kind(), MessageKind::Request);
                    assert_eq!(
                        pane_ops::decode_request(query.payload()).unwrap(),
                        PaneOperationRequest::ViewQuery
                    );
                    let bytes = pane_ops::encode_view(&view).unwrap();
                    peer.write_frame(
                        &Frame::new(MessageKind::Response, query.request_id(), &bytes).unwrap(),
                        deadline,
                    )
                    .unwrap();
                }
            });
            for _ in 0..2 {
                let mut update = NetUpdate {
                    layout: None,
                    pane_operations: None,
                    screens: Vec::new(),
                    status: None,
                    server_present: true,
                };
                net.refresh_layout(Instant::now(), &mut update);
                assert_eq!(update.status, None);
                assert_eq!(net.layout, Some(view.layout));
                assert_eq!(net.panes.len(), 1);
                assert_eq!(net.panes[0].target, survivor);
            }
            server.join().unwrap();
        });
    }

    #[test]
    fn scrollbar_extent_probe_is_read_only_cached_and_invalidated_by_revision() {
        use panelink::handshake_protocol::{Welcome, CAPABILITY_READ};
        use panelink::protocol_wire::{Frame, MessageKind};
        use panelink::screen_checkpoint::{encode, CheckpointMetadata};
        use panelink::terminal::{ScreenSnapshot, TerminalSize};
        use panelink::transport::AuthenticatedStream;
        use panelink::ui_protocol::{
            decode_ui_request, prefix_history_reply, UiRequest, MAX_HISTORY_OFFSET_ROWS,
        };
        use std::os::unix::{fs::DirBuilderExt, net::UnixStream};
        use std::time::{SystemTime, UNIX_EPOCH};
        struct Root(PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let serial = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = Root(PathBuf::from(format!(
            "/private/tmp/panelink-scroll-{}-{serial}",
            std::process::id()
        )));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .unwrap();
        let target = target(7, 3, 1);
        let now = Instant::now();
        let (client, server) = UnixStream::pair().unwrap();
        let mut client = AuthenticatedStream::new(client).unwrap();
        client.enable_snapshot_reads().unwrap();
        let pane = PaneConn::attach(
            &root.0,
            &root.0,
            target,
            Some(Bootstrap {
                stream: client,
                welcome: Welcome {
                    nonce: 1,
                    generation: 7,
                    lease: 10,
                    selected_capabilities: CAPABILITY_READ,
                    ttl_seconds: 1800,
                },
                used: 0,
                started: now,
            }),
            now,
            Duration::from_secs(2),
        )
        .unwrap();
        let mut net = Net {
            directory: root.0.clone(),
            context_dir: root.0.clone(),
            panes: vec![pane],
            next_layout: now,
            resize_sent: None,
            endpoint_identity: None,
            layout: Some(view_with_target(target).layout),
            pane_operations: None,
            pane_operations_pending: false,
            screen_revisions: [(None, 0); panelink::pane_layout::MAX_PANES],
            history_extents: [(None, 0, 0); panelink::pane_layout::MAX_PANES],
        };
        std::thread::scope(|scope| {
            let peer = scope.spawn(move || {
                let mut peer = AuthenticatedStream::new(server).unwrap();
                peer.enable_snapshot_writes().unwrap();
                // The second wheel reuses the extent. A new revision refreshes
                // it; a later alternate-screen reply must not probe or inject.
                for (requested, revision, available, alternate) in [
                    (3, 1, 100, false),
                    (MAX_HISTORY_OFFSET_ROWS, 1, 100, false),
                    (6, 1, 100, false),
                    (9, 2, 110, false),
                    (MAX_HISTORY_OFFSET_ROWS, 2, 110, false),
                    (12, 4, 100, false),
                    (MAX_HISTORY_OFFSET_ROWS, 5, 120, false),
                    (15, 4, 100, false),
                    (MAX_HISTORY_OFFSET_ROWS, 4, u16::MAX, false),
                    (18, 4, 100, false),
                    (MAX_HISTORY_OFFSET_ROWS, 4, u16::MAX - 1, false),
                    (21, 4, 100, false),
                    (MAX_HISTORY_OFFSET_ROWS, 4, 100, false),
                    (24, 6, 0, true),
                    // A scrollbar request may race an app enabling both
                    // alternate screen and mouse reporting. It stays READ-only.
                    (27, 7, 0, true),
                ] {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let frame = peer.read_frame(deadline).unwrap();
                    assert_eq!(frame.kind(), MessageKind::Request);
                    let UiRequest::HistoryQuery(query) =
                        decode_ui_request(frame.payload()).unwrap()
                    else {
                        panic!("extent inspection sent a mutation or input");
                    };
                    assert_eq!(query.target, target);
                    assert_eq!(query.offset_rows, requested);
                    if available >= u16::MAX - 1 {
                        let kind = if available == u16::MAX {
                            MessageKind::Error
                        } else {
                            MessageKind::Snapshot
                        };
                        peer.write_frame(
                            &Frame::new(kind, frame.request_id(), b"invalid").unwrap(),
                            deadline,
                        )
                        .unwrap();
                        continue;
                    }
                    let snapshot =
                        ScreenSnapshot::try_new(TerminalSize::new(22, 78).unwrap()).unwrap();
                    let mut record = encode(
                        &snapshot,
                        &CheckpointMetadata::new(target, 0, 1, revision).unwrap(),
                    )
                    .unwrap();
                    if alternate {
                        record[80] = 1;
                    }
                    let bytes = prefix_history_reply(
                        target,
                        revision,
                        requested.min(available),
                        alternate,
                        alternate && revision == 7,
                        record,
                    )
                    .unwrap();
                    peer.write_frame(
                        &Frame::new(MessageKind::Snapshot, frame.request_id(), &bytes).unwrap(),
                        deadline,
                    )
                    .unwrap();
                }
            });
            for (epoch, offset, total) in [
                (1, 3, 100),
                (2, 6, 100),
                (3, 9, 110),
                (4, 12, 12),
                (5, 15, 15),
                (6, 18, 18),
                (7, 21, 100),
            ] {
                let update = net.scroll(target, epoch, offset, 0, 0, true);
                let ScrollResult::History {
                    offset_rows,
                    total_rows,
                    ..
                } = update.result
                else {
                    panic!("history unavailable");
                };
                assert_eq!((offset_rows, total_rows), (offset, total));
                assert_eq!(net.panes[0].owner_revision, None);
            }
            let update = net.scroll(target, 8, 24, 0, 0, true);
            assert!(matches!(
                update.result,
                ScrollResult::AlternateScreenNoMouse
            ));
            assert_eq!(net.panes[0].owner_revision, None);
            let update = net.seek_history(target, 9, 27);
            assert!(matches!(
                update.result,
                ScrollResult::AlternateScreenNoMouse
            ));
            assert_eq!(net.panes[0].owner_revision, None);
            peer.join().unwrap();
        });
    }

    #[test]
    fn selected_operations_view_can_retire_its_exited_pinned_target() {
        let exited = target(7, 3, 1);
        let survivor = target(7, 3, 2);
        let view = view_with_target(survivor);
        let encoded = pane_ops::encode_view(&view).unwrap();
        let decoded = pane_ops::decode_view(&encoded).unwrap();

        // A current owner view is scoped to the querying connection's exact
        // server generation and workspace, not to its now-exited occupant.
        assert_eq!(validate_pane_operations_scope(&decoded, exited), Ok(()));

        // refresh_layout's normal sync step then drops the stale connection
        // and retains the survivor connection represented in the view.
        let mut connected = vec![exited, survivor];
        retain_layout_panes(&mut connected, decoded.layout, |pane| *pane);
        assert_eq!(connected, vec![survivor]);
    }

    #[test]
    fn selected_operations_view_rejects_other_workspace_or_generation() {
        let querying = target(7, 3, 1);

        assert_eq!(
            validate_pane_operations_scope(&view_with_target(target(7, 4, 2)), querying),
            Err("pane operations view invalid")
        );
        assert_eq!(
            validate_pane_operations_scope(&view_with_target(target(8, 3, 2)), querying),
            Err("pane operations view invalid")
        );
    }

    #[test]
    fn split_request_carries_captured_target_revision_direction_and_planned_size() {
        let target = TargetRef {
            generation: 9,
            workspace: 1,
            pane: 4,
            session: 5,
            occupant_revision: 1,
        };
        let key = OperationKey {
            generation: 9,
            lease: 3,
            sequence: 7,
        };
        let plan = SplitPlan {
            target,
            expected_layout_revision: 42,
            direction: SplitDirection::Down,
            rows: 11,
            columns: 80,
        };
        let context = LaunchContext {
            working_directory: PathBuf::from("/private/tmp"),
            environment: Environment::from([(OsString::from("A"), OsString::from("B"))]),
        };
        let bytes = encode_split(key, plan, &context).unwrap();
        let view = decode_split_request(&bytes).unwrap();
        assert_eq!(view.key(), key);
        assert_eq!(view.target(), target);
        assert_eq!(view.expected_layout_revision(), 42);
        assert_eq!(view.direction(), SplitDirection::Down);
        assert_eq!(view.launch().dimensions(), (11, 80));
        assert_eq!(view.launch().requested_executable(), b"/bin/zsh");
        assert_eq!(view.launch().client_working_directory(), b"/private/tmp");
        assert_eq!(view.launch().arguments().count(), 0);
        assert_eq!(view.launch().environment().count(), 1);
    }

    fn close_fixture() -> (OperationKey, ClosePlan) {
        let key = OperationKey {
            generation: 9,
            lease: 3,
            sequence: 8,
        };
        let plan = ClosePlan {
            target: TargetRef {
                generation: 9,
                workspace: 1,
                pane: 2,
                session: 2,
                occupant_revision: 1,
            },
            expected_layout_revision: 17,
        };
        (key, plan)
    }

    #[test]
    fn close_request_carries_exact_key_captured_target_and_revision() {
        let (key, plan) = close_fixture();
        let bytes = encode_close(key, plan).unwrap();
        let request = panelink::close_protocol::decode_close_request(&bytes).unwrap();
        assert_eq!(request.key, key);
        assert_eq!(request.target, plan.target);
        assert_eq!(request.expected_layout_revision, 17);
        // A zero revision is refused locally, before anything is sent.
        let stale = ClosePlan {
            expected_layout_revision: 0,
            ..plan
        };
        assert_eq!(encode_close(key, stale), Err("close request invalid"));
    }

    #[test]
    fn close_replies_are_matched_to_the_exact_key() {
        use panelink::close_protocol::{encode_close_result, CloseRejection};
        use panelink::response_protocol::encode_response_payload;
        let (key, plan) = close_fixture();
        let result = CloseResult {
            key,
            outcome: CloseOutcome::Stopped(StopOutcome::Escalated),
            target: Some(plan.target),
            layout_revision: 18,
            survivor_resize_pending: false,
        };
        let mut bytes = Vec::new();
        encode_close_result(&result, &mut bytes).unwrap();
        assert_eq!(
            classify_close_reply(key, &bytes),
            CloseReply::Result(result)
        );

        let other = OperationKey { sequence: 9, ..key };
        assert_eq!(classify_close_reply(other, &bytes), CloseReply::Invalid);
        assert_eq!(classify_close_reply(key, &bytes[..40]), CloseReply::Invalid);

        let rejected = CloseResult {
            outcome: CloseOutcome::Rejected(CloseRejection::StaleLayout),
            layout_revision: 18,
            ..result
        };
        let mut bytes = Vec::new();
        encode_close_result(&rejected, &mut bytes).unwrap();
        assert_eq!(
            classify_close_reply(key, &bytes),
            CloseReply::Result(rejected)
        );

        let refusal = panelink::response_protocol::Response {
            key,
            body: ResponseBody::Rejected(panelink::response_protocol::RejectionCode::Unauthorized),
        };
        let mut bytes = Vec::new();
        encode_response_payload(&refusal, &mut bytes).unwrap();
        assert_eq!(classify_close_reply(key, &bytes), CloseReply::Refused);
        assert_eq!(classify_close_reply(other, &bytes), CloseReply::Invalid);
    }

    #[test]
    fn captured_context_excludes_panelink_names() {
        let context = capture_launch_context().unwrap();
        assert!(context.environment.keys().all(|name| {
            use std::os::unix::ffi::OsStrExt;
            !name.as_bytes().starts_with(b"PANELINK_")
        }));
        assert!(context.working_directory.is_absolute());
    }
}
