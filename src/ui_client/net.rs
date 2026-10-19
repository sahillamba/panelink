//! Server-side effects for the attach client: one [`PaneConn`] per pane,
//! layout polling, change-driven screen queries, root resize and splits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use panelink::control_protocol::TargetRef;
use panelink::launch::{Environment, LaunchContext};
use panelink::managed_launch_protocol::LaunchRequest;
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, ResponseBody};
use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
use panelink::terminal::Key;
use panelink::ui_protocol::{
    decode_layout_reply, decode_resize_reply, decode_screen_reply, encode_layout_query,
    encode_root_resize, encode_screen_query, LayoutReply, RootResize,
};

use super::conn::{
    connect, context_path, ensure_context_dir, handshake, parse_context_name, Bootstrap,
    InputResult, LeaseAction, PaneConn, Reply, ResumeError, DEADLINE, OP_DEADLINE, SPLIT_DEADLINE,
};
use super::state::{App, Effects, SplitPlan};

const LAYOUT_INTERVAL: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(30);
const SCREEN_FALLBACK_INTERVAL: Duration = Duration::from_millis(100);
/// Total wall-clock budget for detach work after the host is restored.
const DETACH_BUDGET: Duration = Duration::from_millis(1_500);
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
}

impl Net {
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
        };
        let layout = match net.resume_any_context(now)? {
            Some(layout) => layout,
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
                layout
            }
        };
        app.set_layout(layout);
        net.sync_panes(app, now, DEADLINE);
        for pane in &mut net.panes {
            pane.set_deadline(OP_DEADLINE);
        }
        Ok(net)
    }

    /// Resume the first existing context whose pinned pane is still in the
    /// layout; returns that layout. Dead settled contexts are removed.
    fn resume_any_context(&mut self, now: Instant) -> Result<Option<LayoutReply>, &'static str> {
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
            let Ok(Reply::Response(bytes)) = pane.request(&encode_layout_query()) else {
                continue;
            };
            let Ok(layout) = decode_layout_reply(&bytes) else {
                continue;
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
                return Ok(Some(layout));
            }
            pane.detach();
        }
        Ok(None)
    }

    fn pane_mut(&mut self, target: TargetRef) -> Option<&mut PaneConn> {
        self.panes.iter_mut().find(|pane| pane.target == target)
    }

    fn any_connected(&mut self) -> Option<&mut PaneConn> {
        self.panes.iter_mut().find(|pane| pane.connected())
    }

    /// Connect to any layout pane we lack; drop panes that left the layout.
    fn sync_panes(&mut self, app: &mut App, now: Instant, deadline: std::time::Duration) {
        let Some(layout) = app.layout().copied() else {
            return;
        };
        self.panes.retain(|pane| {
            layout
                .panes
                .iter()
                .flatten()
                .any(|entry| entry.target == pane.target)
        });
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
                Err(label) => app.set_status(label, now),
            }
        }
    }

    /// Lease maintenance for one pane: proactive resume before expiry,
    /// reconnect after loss, and re-attach after a proven lease death.
    /// Never resends input; uncertainty is settled by status only.
    fn maintain(&mut self, index: usize, app: &mut App, now: Instant) {
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
                    app.set_status(label, now);
                    self.sync_later(now);
                }
            }
            return;
        }
        if !pane.connected() {
            if now >= pane.next_retry && pane.reconnect(&self.directory, now).is_err() {
                pane.disconnect(now);
            }
            return;
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
                    app.set_status(label, now);
                }
            }
        }
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

    pub(crate) fn poll(&mut self, app: &mut App, now: Instant, stop: &AtomicBool) {
        let mut index = 0;
        while index < self.panes.len() {
            let before = self.panes.len();
            self.maintain(index, app, now);
            if self.panes.len() == before {
                index += 1;
            }
        }
        if now >= self.next_layout {
            self.next_layout = now + LAYOUT_INTERVAL;
            self.refresh_layout(app, now);
            self.maybe_resize(app, now);
        }
        let focus = app.focus();
        for index in 0..self.panes.len() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let pane = &mut self.panes[index];
            if !pane.connected() {
                continue;
            }
            if pane.owner_revision.is_none() && now >= pane.next_claim {
                match pane.claim(now) {
                    Ok(true) => {}
                    Ok(false) if Some(pane.target) == focus => {
                        app.set_status("pane input is owned by another client", now)
                    }
                    Ok(false) => {}
                    Err(label) => {
                        app.set_status(label, now);
                        pane.disconnect(now);
                        continue;
                    }
                }
            } else if pane.owner_revision.is_some() && now >= pane.next_claim {
                // Renew before the 30 s owner TTL elapses.
                if let Err(label) = pane.claim(now) {
                    app.set_status(label, now);
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
                        app.set_status(label, now);
                        pane.disconnect(now);
                        continue;
                    }
                }
            } else {
                pane.next_watch = now + SCREEN_FALLBACK_INTERVAL;
                true
            };
            if changed || pane.needs_screen {
                let target = pane.target;
                match fetch_screen(pane, app, target) {
                    Ok(()) => pane.needs_screen = false,
                    Err(label) => {
                        app.set_status(label, now);
                        pane.needs_screen = true;
                    }
                }
            }
        }
    }

    fn refresh_layout(&mut self, app: &mut App, now: Instant) {
        let Some(pane) = self.any_connected() else {
            return;
        };
        let reply = match pane.request(&encode_layout_query()) {
            Ok(Reply::Response(bytes)) => decode_layout_reply(&bytes).map_err(|_| "layout invalid"),
            Ok(Reply::Error(label)) => Err(label),
            Ok(Reply::Snapshot(_)) => Err("layout invalid"),
            Err(label) => {
                pane.disconnect(now);
                Err(label)
            }
        };
        match reply {
            Ok(layout) => {
                let before = app.layout().copied();
                app.set_layout(layout);
                if before != Some(layout) {
                    for pane in &mut self.panes {
                        pane.needs_screen = true;
                    }
                }
                // Also re-attaches panes whose earlier attach failed.
                self.sync_panes(app, now, OP_DEADLINE);
            }
            Err(label) => app.set_status(label, now),
        }
    }

    /// Send one RootResize when the host size differs from the layout root.
    /// Below the minimum the last accepted layout is kept untouched.
    fn maybe_resize(&mut self, app: &mut App, now: Instant) {
        let (Some(layout), Some((rows, columns))) = (app.layout().copied(), app.desired_root())
        else {
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
                app.set_status("resize was not applied", now);
            }
            Ok((_, Reply::Error(label))) => {
                app.set_status(label, now);
                self.next_layout = now;
            }
            Ok((_, Reply::Snapshot(_))) => app.set_status("resize reply invalid", now),
            Err(label) => app.set_status(label, now),
        }
    }

    /// Bounded detach: settle, release claims and retire results on each
    /// connected pane (each exchange ≤ `DETACH_DEADLINE`, stopping at the
    /// first stall), then drop connections. Children keep running.
    pub(crate) fn detach(mut self) {
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

fn fetch_screen(pane: &mut PaneConn, app: &mut App, target: TargetRef) -> Result<(), &'static str> {
    let query = encode_screen_query(target).map_err(|_| "request_invalid")?;
    let bytes = match pane.request(&query)? {
        Reply::Snapshot(bytes) => bytes,
        Reply::Error(label) => return Err(label),
        Reply::Response(_) => return Err("screen reply invalid"),
    };
    let view = decode_screen_reply(&bytes).map_err(|_| "screen reply invalid")?;
    if view.target() != target {
        return Err("screen reply invalid");
    }
    if app.screen_revision(target) == Some(view.screen_revision()) {
        return Ok(());
    }
    let checkpoint =
        panelink::screen_checkpoint::decode(view.checkpoint()).map_err(|_| "screen invalid")?;
    app.load_screen(target, view.screen_revision(), &checkpoint)
        .map_err(|_| "screen invalid")
}

impl Effects for Net {
    fn send_keys(&mut self, target: TargetRef, keys: &[Key]) -> Result<(), &'static str> {
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

    fn send_paste(&mut self, target: TargetRef, text: &str) -> Result<(), &'static str> {
        let pane = self.pane_mut(target).ok_or("pane not connected")?;
        match pane.send_paste(text)? {
            InputResult::Applied => Ok(()),
            InputResult::NotOwner => Err("input owned by another client; paste not sent"),
            InputResult::Failed(label) => Err(label),
        }
    }

    fn split(&mut self, plan: SplitPlan) -> Result<TargetRef, &'static str> {
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
    use panelink::pane_layout::SplitDirection;
    use panelink::split_protocol::decode_split_request;
    use std::ffi::OsString;

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
