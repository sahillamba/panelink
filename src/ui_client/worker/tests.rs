use super::*;
use crossterm_input::Decoder;
use panelink::split_menu::{Availability, DisabledReason};
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

enum GateCommand {
    Keys(usize),
    Paste(usize),
    Poll,
}

struct SlowBackend {
    entered: Sender<GateCommand>,
    release: Receiver<()>,
    finished: Sender<()>,
}

impl Backend for SlowBackend {
    fn run(&mut self, command: Command) -> ClientResult {
        match command {
            Command::Keys { keys, .. } => {
                let _ = self.entered.send(GateCommand::Keys(keys.len()));
                let _ = self.release.recv();
                let _ = self.finished.send(());
                ClientResult::Keys(Err("input result unknown"))
            }
            Command::Paste { text, .. } => {
                let _ = self.entered.send(GateCommand::Paste(text.len()));
                let _ = self.release.recv();
                ClientResult::Paste(Ok(()))
            }
            Command::Poll(_) => {
                let _ = self.entered.send(GateCommand::Poll);
                let _ = self.release.recv();
                ClientResult::Poll(NetUpdate {
                    layout: None,
                    screens: Vec::new(),
                    status: None,
                    server_present: true,
                })
            }
            Command::Split {
                id,
                focus_generation,
                plan,
            } => ClientResult::Split {
                id,
                focus_generation,
                target: plan.target,
                result: Err("split delayed"),
            },
            Command::Close { id, plan } => ClientResult::Close {
                id,
                target: plan.target,
                result: Err("close delayed"),
            },
            Command::Detach => ClientResult::Detached,
        }
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: 1,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    }
}

fn slow_worker() -> (
    WorkerHandle,
    Receiver<GateCommand>,
    Sender<()>,
    Receiver<()>,
) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let backend = SlowBackend {
        entered: entered_tx,
        release: release_rx,
        finished: finished_tx,
    };
    (
        WorkerHandle::start_with(backend),
        entered_rx,
        release_tx,
        finished_rx,
    )
}

#[test]
fn stalled_poll_does_not_block_host_side_effect_admission_or_result_polling() {
    let (mut worker, entered, release, _) = slow_worker();
    worker.poll(NetView::default());
    assert!(matches!(
        entered.recv_timeout(Duration::from_secs(1)),
        Ok(GateCommand::Poll)
    ));
    let started = Instant::now();
    worker
        .send_keys(target(), &[Key::Char('x')])
        .expect("bounded command enqueue does not wait for stalled socket IO");
    assert!(started.elapsed() < Duration::from_millis(50));
    assert!(worker.try_result().is_none());
    release.send(()).unwrap();
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut saw_poll = false;
    let mut saw_keys = false;
    while Instant::now() < deadline && !(saw_poll && saw_keys) {
        if let Some(envelope) = worker.try_result() {
            match worker.release_result(envelope) {
                ClientResult::Poll(_) => saw_poll = true,
                ClientResult::Keys(Err("input result unknown")) => saw_keys = true,
                _ => {}
            }
        } else {
            thread::yield_now();
        }
    }
    assert!(saw_poll && saw_keys);
}

#[test]
fn input_unknown_is_reported_once_and_never_replayed_by_the_worker() {
    let (mut worker, entered, release, finished) = slow_worker();
    worker
        .send_keys(target(), &[Key::Char('a'), Key::Char('b')])
        .unwrap();
    assert!(matches!(
        entered.recv_timeout(Duration::from_secs(1)),
        Ok(GateCommand::Keys(2))
    ));
    release.send(()).unwrap();
    finished.recv_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut results = 0;
    while Instant::now() < deadline && results == 0 {
        if let Some(envelope) = worker.try_result() {
            if matches!(
                worker.release_result(envelope),
                ClientResult::Keys(Err("input result unknown"))
            ) {
                results += 1;
            }
        } else {
            thread::yield_now();
        }
    }
    assert_eq!(results, 1);
    assert!(worker.try_result().is_none());
}

struct OrderedDetachBackend {
    restored: Arc<std::sync::atomic::AtomicBool>,
    observed: Sender<bool>,
}

impl Backend for OrderedDetachBackend {
    fn run(&mut self, command: Command) -> ClientResult {
        if matches!(command, Command::Detach) {
            let _ = self.observed.send(self.restored.load(Ordering::SeqCst));
            ClientResult::Detached
        } else {
            panic!("unexpected command in detach-order test");
        }
    }
}

#[test]
fn restored_terminal_precedes_worker_detach_command() {
    use super::super::host::restore_then_detach;

    let restored = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (observed_tx, observed_rx) = mpsc::channel();
    let backend = OrderedDetachBackend {
        restored: Arc::clone(&restored),
        observed: observed_tx,
    };
    let worker = WorkerHandle::start_with(backend);
    restore_then_detach(
        || restored.store(true, Ordering::SeqCst),
        || worker.detach_after_restore(),
    );
    assert_eq!(observed_rx.recv_timeout(Duration::from_secs(1)), Ok(true));
}

#[test]
fn accepted_commands_are_byte_and_count_bounded_including_in_flight_results() {
    let (mut worker, entered, release, _) = slow_worker();
    let max_paste = "x".repeat(MAX_INPUT_TEXT_BYTES);
    worker.send_paste(target(), &max_paste).unwrap();
    assert!(matches!(
        entered.recv_timeout(Duration::from_secs(1)),
        Ok(GateCommand::Paste(MAX_INPUT_TEXT_BYTES))
    ));
    let mut admitted = 1;
    while admitted < COMMAND_CAPACITY {
        match worker.send_paste(target(), "x") {
            Ok(()) => admitted += 1,
            Err(_) => break,
        }
    }
    assert_eq!(admitted, COMMAND_CAPACITY);
    assert_eq!(
        worker.send_paste(target(), "z"),
        Err("client busy: request queue full")
    );
    release.send(()).unwrap();
}

#[test]
fn saturated_worker_does_not_hold_ctrl_x_behind_retained_keys() {
    use super::super::state::{App, Flow};
    use panelink::pane_layout::Rect;
    use panelink::ui_protocol::{LayoutReply, UiPane};

    let (mut worker, entered, release, _) = slow_worker();
    worker.poll(NetView::default());
    assert!(matches!(
        entered.recv_timeout(Duration::from_secs(1)),
        Ok(GateCommand::Poll)
    ));
    for _ in 0..COMMAND_CAPACITY - 1 {
        worker.send_paste(target(), "x").unwrap();
    }

    let mut app = App::new(24, 80).unwrap();
    app.set_layout(LayoutReply {
        revision: 1,
        rows: 24,
        columns: 80,
        panes: [
            Some(UiPane {
                target: target(),
                rect: Rect {
                    x: 0,
                    y: 0,
                    rows: 24,
                    columns: 80,
                },
                pending_confirmed: None,
            }),
            None,
        ],
        divider: None,
    });
    let now = Instant::now();
    let mut decoder = Decoder::new();
    for (index, byte) in b"a".iter().enumerate() {
        if let Some(event) = decoder.advance(*byte, index == 0).unwrap() {
            app.handle_event(
                &event,
                now,
                Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
                &mut worker,
            );
        }
    }
    app.flush(now, &mut worker);
    assert!(
        app.retry_pending_keys(),
        "queue saturation retains the pinned batch"
    );

    let mut decoder = Decoder::new();
    let mut flow = Flow::Continue;
    if let Some(event) = decoder.advance(b'\x18', false).unwrap() {
        flow = app.handle_event(
            &event,
            now,
            Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
            &mut worker,
        );
    }
    assert_eq!(flow, Flow::Quit);
    assert!(app.abandon_pending_keys());
    assert!(matches!(entered.try_recv(), Err(mpsc::TryRecvError::Empty)));

    // Cleanup only after proving Ctrl-X did not need the backend to progress.
    release.send(()).unwrap();
}

#[test]
fn dropping_handle_never_waits_and_unblocks_a_worker_result_send() {
    let (mut worker, entered, release, finished) = slow_worker();
    worker.send_keys(target(), &[Key::Char('q')]).unwrap();
    assert!(matches!(
        entered.recv_timeout(Duration::from_secs(1)),
        Ok(GateCommand::Keys(1))
    ));
    let started = Instant::now();
    drop(worker);
    assert!(started.elapsed() < Duration::from_millis(50));
    release.send(()).unwrap();
    finished.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[cfg(unix)]
struct SocketBackend(std::os::unix::net::UnixStream);

#[cfg(unix)]
impl Backend for SocketBackend {
    fn run(&mut self, command: Command) -> ClientResult {
        match command {
            Command::Poll(_) => {
                self.0.write_all(b"P").unwrap();
                let mut response = [0];
                self.0.read_exact(&mut response).unwrap();
                assert_eq!(&response, b"A");
                ClientResult::Poll(NetUpdate {
                    layout: None,
                    screens: Vec::new(),
                    status: None,
                    server_present: true,
                })
            }
            Command::Keys { .. } => {
                self.0.write_all(b"K").unwrap();
                ClientResult::Keys(Ok(()))
            }
            Command::Paste { .. } => ClientResult::Paste(Ok(())),
            Command::Split {
                id,
                focus_generation,
                plan,
            } => ClientResult::Split {
                id,
                focus_generation,
                target: plan.target,
                result: Err("split delayed"),
            },
            Command::Close { id, plan } => ClientResult::Close {
                id,
                target: plan.target,
                result: Err("close delayed"),
            },
            Command::Detach => ClientResult::Detached,
        }
    }
}

#[cfg(unix)]
#[test]
fn socketpair_stall_keeps_host_submission_responsive_until_peer_replies() {
    use std::os::unix::net::UnixStream;

    let (worker_socket, mut peer) = UnixStream::pair().unwrap();
    let mut worker = WorkerHandle::start_with(SocketBackend(worker_socket));
    worker.poll(NetView::default());
    let mut poll_byte = [0];
    peer.read_exact(&mut poll_byte).unwrap();
    assert_eq!(&poll_byte, b"P");

    let started = Instant::now();
    worker.send_keys(target(), &[Key::Char('z')]).unwrap();
    assert!(started.elapsed() < Duration::from_millis(50));
    peer.write_all(b"A").unwrap();
    let mut key_byte = [0];
    peer.read_exact(&mut key_byte).unwrap();
    assert_eq!(&key_byte, b"K");
}
