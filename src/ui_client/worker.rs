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
use panelink::terminal::Key;
use panelink::ui_protocol::{
    HISTORY_QUERY_BYTES, LAYOUT_REPLY_BYTES, MAX_HISTORY_REPLY_BYTES, MAX_KEYS,
    MAX_SCREEN_REPLY_BYTES, MOUSE_WHEEL_INPUT_BYTES,
};

use super::net::{Net, NetUpdate, NetView, ScrollUpdate, DETACH_BUDGET};
use super::state::{ClosePlan, Effects, SplitPlan};

const COMMAND_CAPACITY: usize = 32;
const RESULT_CAPACITY: usize = 32;
const OWNED_BYTE_BUDGET: usize = 4 * 1024 * 1024;
const CONTROL_RESULT_RESERVATION: usize = 512;
const INPUT_RESULT_RESERVATION: usize = 128;
const POLL_RESULT_RESERVATION: usize = 2 * MAX_SCREEN_REPLY_BYTES + LAYOUT_REPLY_BYTES + 128;
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
    Split {
        id: u64,
        focus_generation: u64,
        plan: SplitPlan,
    },
    Close {
        id: u64,
        plan: ClosePlan,
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
            Self::Split { .. } => std::mem::size_of::<SplitPlan>(),
            Self::Close { .. } => std::mem::size_of::<ClosePlan>(),
            Self::Poll(_) | Self::Detach => 0,
        }
    }
}

struct Envelope {
    command: Command,
    reservation: Reservation,
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
    Poll(NetUpdate),
    Detached,
}

pub(crate) struct ResultEnvelope {
    pub result: ClientResult,
    reservation: Reservation,
}

pub(crate) struct WorkerHandle {
    commands: Option<SyncSender<Envelope>>,
    results: Receiver<ResultEnvelope>,
    budget: Arc<Mutex<Budget>>,
    thread: Option<JoinHandle<()>>,
    poll_pending: bool,
    scroll_pending: bool,
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
        let envelope = Envelope {
            command,
            reservation: Reservation {
                bytes: actual_reservation,
            },
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
        if self.poll_pending {
            return;
        }
        if self
            .submit_with(0, POLL_RESULT_RESERVATION, || Command::Poll(view))
            .is_ok()
        {
            self.poll_pending = true;
        }
    }

    pub(crate) fn try_result(&mut self) -> Option<ResultEnvelope> {
        match self.results.try_recv() {
            Ok(result) => {
                if matches!(&result.result, ClientResult::Poll(_)) {
                    self.poll_pending = false;
                }
                if matches!(&result.result, ClientResult::Scroll(_)) {
                    self.scroll_pending = false;
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

    /// Called only after `HostGuard::restore`. It never waits for a full
    /// command queue or joins an unfinished worker.
    pub(crate) fn detach_after_restore(mut self) {
        let deadline = Instant::now() + DETACH_BUDGET;
        let mut detach_queued = self.submit_with(0, 32, || Command::Detach).is_ok();
        while Instant::now() < deadline {
            if !detach_queued {
                detach_queued = self.submit_with(0, 32, || Command::Detach).is_ok();
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
            Command::Poll(view) => ClientResult::Poll(self.poll(
                view,
                Instant::now(),
                &std::sync::atomic::AtomicBool::new(false),
            )),
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
        let result = backend.run(envelope.command);
        if results
            .send(ResultEnvelope {
                result,
                reservation: envelope.reservation,
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
