//! Bounded owner for all attach-client socket and context-file work.
//!
//! The host thread submits owned, pinned commands and applies typed results;
//! this worker is the only owner of `Net` and `PaneConn` values.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use panelink::close_protocol::CloseResult;
use panelink::control_protocol::{TargetRef, MAX_INPUT_TEXT_BYTES};
use panelink::pane_operations_protocol::PANE_OPERATIONS_VIEW_BYTES;
use panelink::terminal::Key;
use panelink::ui_protocol::{
    HISTORY_QUERY_BYTES, LAYOUT_REPLY_BYTES, MAX_HISTORY_REPLY_BYTES, MAX_KEYS,
    MAX_SCREEN_REPLY_BYTES, MOUSE_WHEEL_INPUT_BYTES,
};

use super::net::{Net, NetUpdate, NetView, ScrollUpdate, DETACH_BUDGET};
use super::pane_operations::{PaneAction, PaneOperationUpdate};
use super::state::{ClosePlan, Effects, SplitPlan};

use panelink::diagnostics::{self, StageEvent, ENVELOPE_TRACE_BYTES, RESULT_TRACE_BYTES};

const COMMAND_CAPACITY: usize = 32;
const RESULT_CAPACITY: usize = 32;
const OWNED_BYTE_BUDGET: usize = 4 * 1024 * 1024;
/// Additive trace reservation carried inside each submitted command/result
/// pair's existing reservation: the per-envelope span handle and monotonic
/// enqueue instant plus the result-side completion instant. Diagnostics
/// therefore never grow the bounded queues beyond the accounted budget.
const TRACE_RESERVATION_BYTES: usize = ENVELOPE_TRACE_BYTES + RESULT_TRACE_BYTES;
const CONTROL_RESULT_RESERVATION: usize = 512 + TRACE_RESERVATION_BYTES;
const INPUT_RESULT_RESERVATION: usize = 128 + TRACE_RESERVATION_BYTES;
const POLL_RESULT_RESERVATION: usize = 2 * MAX_SCREEN_REPLY_BYTES
    + LAYOUT_REPLY_BYTES
    + PANE_OPERATIONS_VIEW_BYTES
    + 128
    + TRACE_RESERVATION_BYTES;
const PANE_OPERATION_RESULT_RESERVATION: usize =
    PANE_OPERATIONS_VIEW_BYTES + CONTROL_RESULT_RESERVATION;
/// Bounded result reservation for the detach acknowledgment itself; its
/// trace reservation rides on the same submit as every other command.
const DETACH_RESULT_RESERVATION: usize = 32 + TRACE_RESERVATION_BYTES;
const SHUTDOWN_POLL: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Default)]
struct Budget {
    count: usize,
    bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Reservation {
    bytes: usize,
}

enum Command {
    Keys {
        target: TargetRef,
        keys: Vec<Key>,
    },
    Paste {
        target: TargetRef,
        text: String,
    },
    Scroll {
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
        column: u16,
        row: u16,
        up: bool,
    },
    AlternateWheel {
        target: TargetRef,
        epoch: u64,
        column: u16,
        row: u16,
        up: bool,
    },
    SeekHistory {
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
    },
    Split {
        id: u64,
        focus_generation: u64,
        plan: SplitPlan,
    },
    Close {
        id: u64,
        plan: ClosePlan,
    },
    PaneOperation {
        id: u64,
        action: PaneAction,
    },
    Poll(NetView),
    Detach,
}

impl Command {
    fn owned_bytes(&self) -> usize {
        match self {
            Self::Keys { keys, .. } => keys.capacity() * std::mem::size_of::<Key>(),
            Self::Paste { text, .. } => text.capacity(),
            Self::Scroll { .. } => HISTORY_QUERY_BYTES + MOUSE_WHEEL_INPUT_BYTES,
            Self::AlternateWheel { .. } => MOUSE_WHEEL_INPUT_BYTES,
            Self::SeekHistory { .. } => HISTORY_QUERY_BYTES,
            Self::Split { .. } => std::mem::size_of::<SplitPlan>(),
            Self::Close { .. } => std::mem::size_of::<ClosePlan>(),
            Self::PaneOperation { action, .. } => std::mem::size_of_val(action),
            Self::Poll(_) | Self::Detach => 0,
        }
    }
}

struct Envelope {
    command: Command,
    reservation: Reservation,
    /// Monotonic enqueue instant; the monotonic queue wait is measured
    /// inside this process when the worker dequeues the envelope.
    submitted: Instant,
    /// Correlation span admitted at submission and entered only while this
    /// one command is processed by the worker (a shared global subscriber
    /// does not propagate current spans between threads).
    span: tracing::Span,
}

impl Command {
    /// Fixed operation label for this command ("only approved labels").
    fn op_label(&self) -> &'static str {
        match self {
            Self::Keys { .. } => "keys",
            Self::Paste { .. } => "paste",
            Self::Scroll { .. } => "scroll",
            Self::AlternateWheel { .. } => "alternate_wheel",
            Self::SeekHistory { .. } => "seek_history",
            Self::Split { .. } => "split",
            Self::Close { .. } => "close",
            Self::PaneOperation { .. } => "pane_operation",
            Self::Poll(_) => "poll",
            Self::Detach => "detach",
        }
    }

    /// Whether this command's stage record should be emitted: per-poll
    /// screen fetches are excluded to bound record volume.
    fn is_traceable(&self) -> bool {
        !matches!(self, Self::Poll(_))
    }

    fn approved_counts(&self) -> (&'static str, u64, &'static str, u64) {
        match self {
            Self::Keys { keys, .. } => ("keys", keys.len() as u64, "", 0),
            Self::Paste { text, .. } => ("text_bytes", text.len() as u64, "", 0),
            Self::Scroll {
                epoch, offset_rows, ..
            } => ("epoch", *epoch, "offset_rows", u64::from(*offset_rows)),
            Self::AlternateWheel { epoch, .. } => ("epoch", *epoch, "", 0),
            Self::SeekHistory {
                epoch, offset_rows, ..
            } => ("epoch", *epoch, "offset_rows", u64::from(*offset_rows)),
            _ => ("", 0, "", 0),
        }
    }

    /// A per-command tracing span admitted at submission time.
    fn trace_span(&self) -> tracing::Span {
        tracing::span!(
            target: panelink::diagnostics::TARGET,
            tracing::Level::INFO,
            "client_command",
            op = self.op_label(),
        )
    }
}

pub(crate) enum ClientResult {
    Keys(Result<(), &'static str>),
    Paste(Result<(), &'static str>),
    Scroll(ScrollUpdate),
    Split {
        id: u64,
        focus_generation: u64,
        target: TargetRef,
        result: Result<TargetRef, &'static str>,
    },
    Close {
        id: u64,
        target: TargetRef,
        result: Result<CloseResult, &'static str>,
    },
    PaneOperation {
        id: u64,
        action: PaneAction,
        result: Result<Box<PaneOperationUpdate>, &'static str>,
    },
    Poll(Box<NetUpdate>),
    Detached,
}

pub(crate) struct ResultEnvelope {
    pub result: ClientResult,
    reservation: Reservation,
    /// Fixed operation label of the completed command.
    pub(crate) op: &'static str,
    /// Monotonic instant at which the backend finished this command.
    pub(crate) completed_at: Instant,
}

impl ClientResult {
    /// Fixed outcome label for this result.
    fn outcome_label(&self) -> &'static str {
        match self {
            Self::Keys(result) | Self::Paste(result) => match result {
                Ok(()) => "ok",
                Err(_) => "failed",
            },
            Self::Scroll(_) | Self::Poll(_) => "reply",
            Self::Split { result, .. } => match result {
                Ok(_) => "reply",
                Err(_) => "failed",
            },
            Self::Close { result, .. } => match result {
                Ok(_) => "reply",
                Err(_) => "failed",
            },
            Self::PaneOperation { result, .. } => match result {
                Ok(_) => "reply",
                Err(_) => "failed",
            },
            Self::Detached => "detached",
        }
    }
}

pub(crate) struct WorkerHandle {
    commands: Option<SyncSender<Envelope>>,
    results: Receiver<ResultEnvelope>,
    budget: Arc<Mutex<Budget>>,
    thread: Option<JoinHandle<()>>,
    poll_pending: bool,
    scroll_pending: bool,
    alternate_wheel_pending: bool,
    server_directory: PathBuf,
}

impl WorkerHandle {
    pub(crate) fn start(net: Net) -> Self {
        let directory = net.server_directory().to_path_buf();
        let mut handle = Self::start_with(net);
        handle.server_directory = directory;
        handle
    }

    pub(crate) fn server_directory(&self) -> &Path {
        &self.server_directory
    }

    fn start_with<B: Backend>(backend: B) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(RESULT_CAPACITY);
        let budget = Arc::new(Mutex::new(Budget::default()));
        let thread = thread::Builder::new()
            .name("panelink-ui-client-io".into())
            .spawn(move || worker_loop(backend, command_rx, result_tx))
            .expect("client worker thread creation failed");
        Self {
            commands: Some(command_tx),
            results: result_rx,
            budget,
            thread: Some(thread),
            poll_pending: false,
            scroll_pending: false,
            alternate_wheel_pending: false,
            server_directory: PathBuf::new(),
        }
    }

    fn submit_with(
        &mut self,
        command_bytes: usize,
        result_bytes: usize,
        build: impl FnOnce() -> Command,
    ) -> Result<(), &'static str> {
        let reservation_bytes = command_bytes
            .checked_add(result_bytes)
            .filter(|bytes| *bytes <= OWNED_BYTE_BUDGET)
            .ok_or("client request exceeds queue budget")?;
        let mut budget = self.budget.lock().map_err(|_| "client queue unavailable")?;
        if budget.count == COMMAND_CAPACITY || budget.bytes + reservation_bytes > OWNED_BYTE_BUDGET
        {
            return Err("client busy: request queue full");
        }
        budget.count += 1;
        budget.bytes += reservation_bytes;
        let command = build();
        let actual_bytes = command.owned_bytes();
        let Some(actual_reservation) = actual_bytes.checked_add(result_bytes) else {
            budget.count -= 1;
            budget.bytes -= reservation_bytes;
            return Err("client request exceeds queue budget");
        };
        if actual_reservation > reservation_bytes {
            let extra = actual_reservation - reservation_bytes;
            if budget.bytes + extra > OWNED_BYTE_BUDGET {
                budget.count -= 1;
                budget.bytes -= reservation_bytes;
                return Err("client busy: request queue full");
            }
            budget.bytes += extra;
        } else {
            budget.bytes -= reservation_bytes - actual_reservation;
        }
        let span = if command.is_traceable() {
            command.trace_span()
        } else {
            tracing::Span::none()
        };
        let envelope = Envelope {
            command,
            reservation: Reservation {
                bytes: actual_reservation,
            },
            submitted: Instant::now(),
            span,
        };
        let Some(sender) = self.commands.as_ref() else {
            budget.count -= 1;
            budget.bytes -= actual_reservation;
            return Err("client worker unavailable");
        };
        match sender.try_send(envelope) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                budget.count -= 1;
                budget.bytes -= actual_reservation;
                Err("client busy: request queue full")
            }
            Err(TrySendError::Disconnected(_)) => {
                budget.count -= 1;
                budget.bytes -= actual_reservation;
                Err("client worker unavailable")
            }
        }
    }

    pub(crate) fn poll(&mut self, view: NetView) {
        if self.poll_pending || (self.scroll_pending && !self.alternate_wheel_pending) {
            return;
        }
        if self
            .submit_with(0, POLL_RESULT_RESERVATION, || Command::Poll(view))
            .is_ok()
        {
            self.poll_pending = true;
        }
    }

    /// Submit one captured pane operation without waiting for network IO.
    pub(crate) fn pane_operation(
        &mut self,
        id: u64,
        action: PaneAction,
    ) -> Result<(), &'static str> {
        self.submit_with(
            std::mem::size_of::<PaneAction>(),
            PANE_OPERATION_RESULT_RESERVATION,
            || Command::PaneOperation { id, action },
        )
    }

    pub(crate) fn try_result(&mut self) -> Option<ResultEnvelope> {
        match self.results.try_recv() {
            Ok(result) => {
                if matches!(&result.result, ClientResult::Poll(_)) {
                    self.poll_pending = false;
                }
                if matches!(&result.result, ClientResult::Scroll(_)) {
                    self.scroll_pending = false;
                    self.alternate_wheel_pending = false;
                }
                Some(result)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub(crate) fn release_result(&self, result: ResultEnvelope) -> ClientResult {
        if let Ok(mut budget) = self.budget.lock() {
            budget.count = budget.count.saturating_sub(1);
            budget.bytes = budget.bytes.saturating_sub(result.reservation.bytes);
        }
        result.result
    }

    /// Best-effort detach admission after host restoration, without waiting
    /// for either queue capacity or the shared accounting lock.
    pub(crate) fn try_detach_nonblocking(&mut self) -> bool {
        let Ok(mut budget) = self.budget.try_lock() else {
            return false;
        };
        let Some(sender) = self.commands.as_ref() else {
            return false;
        };
        const RESULT_BYTES: usize = DETACH_RESULT_RESERVATION;
        if budget.count >= COMMAND_CAPACITY || budget.bytes + RESULT_BYTES > OWNED_BYTE_BUDGET {
            return false;
        }
        let command = Command::Detach;
        let span = if command.is_traceable() {
            command.trace_span()
        } else {
            tracing::Span::none()
        };
        let envelope = Envelope {
            command,
            reservation: Reservation {
                bytes: RESULT_BYTES,
            },
            submitted: Instant::now(),
            span,
        };
        match sender.try_send(envelope) {
            Ok(()) => {
                budget.count += 1;
                budget.bytes += RESULT_BYTES;
                true
            }
            Err(_) => false,
        }
    }

    /// Called only after `HostGuard::restore`. It never waits for a full
    /// command queue or joins an unfinished worker.
    pub(crate) fn detach_after_restore(mut self) {
        let deadline = Instant::now() + DETACH_BUDGET;
        let mut detach_queued = self
            .submit_with(0, DETACH_RESULT_RESERVATION, || Command::Detach)
            .is_ok();
        while Instant::now() < deadline {
            if !detach_queued {
                detach_queued = self
                    .submit_with(0, DETACH_RESULT_RESERVATION, || Command::Detach)
                    .is_ok();
            }
            let timeout = (deadline - Instant::now()).min(SHUTDOWN_POLL);
            match self.results.recv_timeout(timeout) {
                Ok(result) => {
                    let detached = matches!(result.result, ClientResult::Detached);
                    self.release_result(result);
                    if detached {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.commands.take();
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.commands.take();
        // Receiver drop wakes a worker blocked publishing a bounded result.
        // Never join here: this path can run before terminal restoration.
    }
}

impl Effects for WorkerHandle {
    fn send_keys(&mut self, target: TargetRef, keys: &[Key]) -> Result<(), &'static str> {
        if keys.is_empty() || keys.len() > MAX_KEYS {
            return Err("key batch exceeds the input limit");
        }
        self.submit_with(
            MAX_KEYS * std::mem::size_of::<Key>(),
            INPUT_RESULT_RESERVATION,
            || Command::Keys {
                target,
                keys: keys.to_vec(),
            },
        )
    }

    fn send_paste(&mut self, target: TargetRef, text: &str) -> Result<(), &'static str> {
        if text.len() > MAX_INPUT_TEXT_BYTES {
            return Err("paste exceeds the input limit");
        }
        self.submit_with(MAX_INPUT_TEXT_BYTES, INPUT_RESULT_RESERVATION, || {
            Command::Paste {
                target,
                text: text.to_owned(),
            }
        })
    }

    fn scroll(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
        column: u16,
        row: u16,
        up: bool,
    ) -> Result<(), &'static str> {
        if self.scroll_pending {
            return Err("client busy: wheel request pending");
        }
        self.submit_with(
            HISTORY_QUERY_BYTES + MOUSE_WHEEL_INPUT_BYTES,
            MAX_HISTORY_REPLY_BYTES + INPUT_RESULT_RESERVATION,
            || Command::Scroll {
                target,
                epoch,
                offset_rows,
                column,
                row,
                up,
            },
        )?;
        self.scroll_pending = true;
        Ok(())
    }

    fn seek_history(
        &mut self,
        target: TargetRef,
        epoch: u64,
        offset_rows: u16,
    ) -> Result<(), &'static str> {
        if self.scroll_pending {
            return Err("client busy: history request pending");
        }
        self.submit_with(
            HISTORY_QUERY_BYTES,
            MAX_HISTORY_REPLY_BYTES + TRACE_RESERVATION_BYTES,
            || Command::SeekHistory {
                target,
                epoch,
                offset_rows,
            },
        )?;
        self.scroll_pending = true;
        Ok(())
    }

    fn alternate_wheel(
        &mut self,
        target: TargetRef,
        epoch: u64,
        column: u16,
        row: u16,
        up: bool,
    ) -> Result<(), &'static str> {
        if self.scroll_pending {
            return Err("client busy: wheel request pending");
        }
        self.submit_with(MOUSE_WHEEL_INPUT_BYTES, INPUT_RESULT_RESERVATION, || {
            Command::AlternateWheel {
                target,
                epoch,
                column,
                row,
                up,
            }
        })?;
        self.scroll_pending = true;
        self.alternate_wheel_pending = true;
        Ok(())
    }

    fn split(
        &mut self,
        plan: SplitPlan,
        request_id: u64,
        focus_generation: u64,
    ) -> Result<Option<TargetRef>, &'static str> {
        self.submit_with(
            std::mem::size_of::<SplitPlan>(),
            CONTROL_RESULT_RESERVATION,
            || Command::Split {
                id: request_id,
                focus_generation,
                plan,
            },
        )?;
        Ok(None)
    }

    fn close(
        &mut self,
        plan: ClosePlan,
        request_id: u64,
    ) -> Result<Option<CloseResult>, &'static str> {
        self.submit_with(
            std::mem::size_of::<ClosePlan>(),
            CONTROL_RESULT_RESERVATION,
            || Command::Close {
                id: request_id,
                plan,
            },
        )?;
        Ok(None)
    }

    fn pane_operation(&mut self, id: u64, action: PaneAction) -> Result<(), &'static str> {
        WorkerHandle::pane_operation(self, id, action)
    }
}

trait Backend: Send + 'static {
    fn run(&mut self, command: Command) -> ClientResult;
}

impl Backend for Net {
    fn run(&mut self, command: Command) -> ClientResult {
        match command {
            Command::Keys { target, keys } => ClientResult::Keys(self.send_keys(target, &keys)),
            Command::Paste { target, text } => ClientResult::Paste(self.send_paste(target, &text)),
            Command::Scroll {
                target,
                epoch,
                offset_rows,
                column,
                row,
                up,
            } => ClientResult::Scroll(self.scroll(target, epoch, offset_rows, column, row, up)),
            Command::SeekHistory {
                target,
                epoch,
                offset_rows,
            } => ClientResult::Scroll(self.seek_history(target, epoch, offset_rows)),
            Command::AlternateWheel {
                target,
                epoch,
                column,
                row,
                up,
            } => ClientResult::Scroll(self.alternate_wheel(target, epoch, column, row, up)),
            Command::Split {
                id,
                focus_generation,
                plan,
            } => ClientResult::Split {
                id,
                focus_generation,
                target: plan.target,
                result: self.split(plan),
            },
            Command::Close { id, plan } => ClientResult::Close {
                id,
                target: plan.target,
                result: self.close(plan),
            },
            Command::PaneOperation { id, action } => ClientResult::PaneOperation {
                id,
                action,
                result: self.pane_operation(action).map(Box::new),
            },
            Command::Poll(view) => ClientResult::Poll(Box::new(self.poll(
                view,
                Instant::now(),
                &std::sync::atomic::AtomicBool::new(false),
            ))),
            Command::Detach => {
                self.detach();
                ClientResult::Detached
            }
        }
    }
}

fn worker_loop<B: Backend>(
    mut backend: B,
    commands: Receiver<Envelope>,
    results: SyncSender<ResultEnvelope>,
) {
    while let Ok(envelope) = commands.recv() {
        let detached = matches!(&envelope.command, Command::Detach);
        let traceable = envelope.command.is_traceable();
        let op = envelope.command.op_label();
        let bytes = envelope.command.owned_bytes() as u64;
        let (count0_label, count0_value, count1_label, count1_value) =
            envelope.command.approved_counts();
        let wait_us = diagnostics::micros_since(envelope.submitted);
        let span_enter = envelope.span.enter();
        let started = Instant::now();
        let result = backend.run(envelope.command);
        let service_us = diagnostics::micros_since(started);
        drop(span_enter);
        if traceable {
            StageEvent {
                op,
                stage: "client_service",
                outcome: result.outcome_label(),
                wait_us,
                service_us,
                bytes,
                count0: (count0_label, count0_value),
                count1: (count1_label, count1_value),
                ..StageEvent::default()
            }
            .record();
        }
        if results
            .send(ResultEnvelope {
                result,
                reservation: envelope.reservation,
                op,
                completed_at: Instant::now(),
            })
            .is_err()
        {
            return;
        }
        if detached {
            return;
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod diag_tests {
    use super::*;
    use panelink::control_protocol::TargetRef;
    use std::sync::mpsc;
    use std::sync::Mutex;

    /// Targeted capture: records diagnostic events on any thread inside this
    /// test only (a scoped `set_default` subscriber on this test thread).
    struct Capture {
        events: Mutex<Vec<Vec<(String, String)>>>,
        span_names: Mutex<Vec<String>>,
        next_id: std::sync::atomic::AtomicU64,
    }

    impl Capture {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                span_names: Mutex::new(Vec::new()),
                next_id: std::sync::atomic::AtomicU64::new(1),
            }
        }

        fn events(&self) -> Vec<Vec<(String, String)>> {
            self.events.lock().unwrap().clone()
        }
    }

    struct Visitor {
        fields: Vec<(String, String)>,
    }

    impl tracing::field::Visit for Visitor {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.fields
                .push((field.name().to_owned(), value.to_owned()));
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.fields
                .push((field.name().to_owned(), format!("CANARY:{value:?}")));
        }
    }

    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target() == panelink::diagnostics::TARGET
        }

        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::Id {
            self.span_names
                .lock()
                .unwrap()
                .push(span.metadata().name().to_owned());
            tracing::Id::from_u64(
                self.next_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )
        }

        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = Visitor { fields: Vec::new() };
            event.record(&mut visitor);
            self.events.lock().unwrap().push(visitor.fields);
        }

        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }

    fn target() -> TargetRef {
        TargetRef {
            generation: 5,
            workspace: 1,
            pane: 2,
            session: 1,
            occupant_revision: 1,
        }
    }

    fn keys_envelope(span: tracing::Span, submitted: Instant) -> Envelope {
        Envelope {
            command: Command::Keys {
                target: target(),
                keys: vec![panelink::terminal::Key::Enter],
            },
            reservation: Reservation {
                bytes: MAX_KEYS * std::mem::size_of::<panelink::terminal::Key>()
                    + INPUT_RESULT_RESERVATION,
            },
            submitted,
            span,
        }
    }

    #[test]
    fn worker_loop_measures_queue_wait_under_the_admitted_span_and_labels_outcomes() {
        let capture = std::sync::Arc::new(Capture::new());
        let dispatch = tracing::Dispatch::new(capture.clone());
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::sync_channel(2);
        let submitted = Instant::now();
        std::thread::sleep(Duration::from_millis(3));
        let submitted_ref = submitted;
        commands_tx
            .send(keys_envelope(
                tracing::span!(
                    target: panelink::diagnostics::TARGET,
                    tracing::Level::INFO,
                    "client_command",
                    op = "keys"
                ),
                submitted_ref,
            ))
            .expect("loop command capacity");
        drop(commands_tx);
        tracing::subscriber::with_default(capture.clone(), || {
            worker_loop(BackendForTests, commands_rx, results_tx);
        });
        let events = capture.events();
        assert_eq!(events.len(), 1, "one traceable command: {events:?}");
        let lookup = |name: &str| {
            events[0]
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(lookup("op"), "keys");
        assert_eq!(lookup("stage"), "client_service");
        assert_eq!(lookup("outcome"), "ok");
        let wait_us: u64 = lookup("wait_us").parse().unwrap_or(0);
        assert!(wait_us >= 3_000, "queue wait was measured: {wait_us}");
        assert_eq!(lookup("count0_label"), "keys");
        assert_eq!(lookup("count0_value"), "1");
        assert!(capture
            .span_names
            .lock()
            .unwrap()
            .contains(&"client_command".to_owned()));
        let result = results_rx.try_recv().expect("bounded result published");
        assert_eq!(result.op, "keys");
    }

    #[test]
    fn poll_commands_are_not_traced_to_bound_record_volume() {
        let capture = std::sync::Arc::new(Capture::new());
        let dispatch = tracing::Dispatch::new(capture.clone());
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, _results_rx) = mpsc::sync_channel(2);
        let command = Command::Poll(NetView::default());
        let envelope = Envelope {
            command,
            reservation: Reservation {
                bytes: POLL_RESULT_RESERVATION,
            },
            submitted: Instant::now(),
            span: tracing::Span::none(),
        };
        commands_tx.send(envelope).expect("loop command capacity");
        drop(commands_tx);
        tracing::subscriber::with_default(capture.clone(), || {
            worker_loop(BackendForTests, commands_rx, results_tx);
        });
        assert!(capture.events().is_empty(), "poll stays untraced");
    }

    /// Minimal backend that returns immediately for every command shape.
    struct BackendForTests;

    impl Backend for BackendForTests {
        fn run(&mut self, command: Command) -> ClientResult {
            match command {
                Command::Keys { .. } | Command::Paste { .. } => ClientResult::Keys(Ok(())),
                Command::Poll(_) => ClientResult::Poll(Box::new(NetUpdate {
                    layout: None,
                    pane_operations: None,
                    screens: Vec::new(),
                    status: None,
                    server_present: false,
                })),
                _ => ClientResult::Detached,
            }
        }
    }
}
