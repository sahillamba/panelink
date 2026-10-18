//! Bounded bridge from authenticated connection workers to the single owner.
//!
//! Connection loss never owns or cancels the process sink. The owner finishes
//! each admitted operation even when its response receiver has disappeared.

use crate::server::{EffectSink, SessionReply, StateOwner};
use crate::server_io::{
    OwnerEnvelope, OwnerReply, OwnerRequest, StartError, WorkerHub, OWNER_QUEUE_CAPACITY,
};
use crate::session_protocol::encode_session_info;
use crate::transport::AuthenticatedStream;
use crate::watch_protocol::{
    decode_watch_request, encode_reconciliation, Reconciliation, WatchEnd, WatchRequest,
    WatchStatus,
};
use std::time::{Duration, Instant};

const MAX_PENDING_WATCHES: usize = 8;

/// A single state owner with a bounded, independently owned connection hub.
pub struct ServerDriver<E: EffectSink> {
    state: StateOwner<E>,
    hub: WorkerHub,
    pending_watches: Vec<PendingWatch>,
}

impl<E: EffectSink> Drop for ServerDriver<E> {
    fn drop(&mut self) {
        // Stop socket workers before fields drop the process-owning state.
        self.hub.shutdown();
        self.settle_pending_watches();
    }
}

impl<E: EffectSink> ServerDriver<E> {
    /// Construct a driver; no listener, process or thread is created here.
    pub fn new(state: StateOwner<E>) -> Self {
        Self {
            state,
            hub: WorkerHub::new(),
            pending_watches: Vec::with_capacity(MAX_PENDING_WATCHES),
        }
    }

    /// Admit one already kernel-authenticated connection, within the hub cap.
    pub fn attach(&mut self, stream: AuthenticatedStream) -> Result<(), StartError> {
        self.hub.start(stream)
    }

    /// Make bounded progress for processes and at most eight queued requests.
    ///
    /// A slow socket writer never runs on this state-owner thread. The supplied
    /// clock must be monotonic; it is read afresh before each request.
    pub fn tick(&mut self, clock: impl Fn() -> u64) -> Result<(), crate::server::ServerError> {
        let progress = self.state.poll(clock());
        self.process_pending(&clock);
        self.process_pending_watches(&clock);
        self.hub.reap_finished();
        progress
    }

    /// Borrow authoritative state for parent adapters and test observations.
    pub const fn state(&self) -> &StateOwner<E> {
        &self.state
    }

    /// Number of currently admitted connection workers, never more than eight.
    pub fn active_connections(&self) -> usize {
        self.hub.active_connections()
    }

    /// Number of finite watches retained for a later owner-loop tick.
    ///
    /// This read-only bounded observation is intended for integration
    /// coordination and does not grant authority or affect scheduling.
    pub fn pending_watches(&self) -> usize {
        self.pending_watches.len()
    }

    /// Stop connections, then settle queued requests without replaying any input.
    /// This leaves the process sink alive until the driver itself is dropped.
    pub fn stop_connections(&mut self, clock: impl Fn() -> u64) {
        self.hub.shutdown();
        self.process_pending(&clock);
        self.settle_pending_watches();
    }

    fn process_pending(&mut self, clock: &impl Fn() -> u64) {
        for _ in 0..OWNER_QUEUE_CAPACITY {
            let Ok(envelope) = self.hub.owner_receiver().try_recv() else {
                break;
            };
            match dispatch(
                &mut self.state,
                &envelope,
                clock(),
                self.pending_watches.len() < MAX_PENDING_WATCHES,
            ) {
                DispatchResult::Reply(reply) => {
                    // Lost acknowledgments do not undo or resend admitted effects.
                    let _ = envelope.try_reply(reply);
                }
                DispatchResult::Pending { request, deadline } => {
                    self.pending_watches.push(PendingWatch {
                        envelope,
                        request,
                        deadline,
                    });
                }
            }
        }
    }

    fn process_pending_watches(&mut self, clock: &impl Fn() -> u64) {
        let mut index = 0;
        while index < self.pending_watches.len() && index < MAX_PENDING_WATCHES {
            let reply =
                resolve_pending_watch(&mut self.state, &self.pending_watches[index], clock());
            if let Some(reply) = reply {
                let pending = self.pending_watches.swap_remove(index);
                let _ = pending.envelope.try_reply(reply);
            } else {
                index += 1;
            }
        }
    }

    fn settle_pending_watches(&mut self) {
        for pending in self.pending_watches.drain(..) {
            let _ = pending
                .envelope
                .try_reply(OwnerReply::Error("server_stopping"));
        }
    }
}

struct PendingWatch {
    envelope: OwnerEnvelope,
    request: WatchRequest,
    deadline: Instant,
}

enum DispatchResult {
    Reply(OwnerReply),
    Pending {
        request: WatchRequest,
        deadline: Instant,
    },
}

fn dispatch<E: EffectSink>(
    state: &mut StateOwner<E>,
    envelope: &OwnerEnvelope,
    now: u64,
    pending_capacity: bool,
) -> DispatchResult {
    match &envelope.request {
        OwnerRequest::Hello(hello) => match state.connect(hello, now) {
            Ok((connection, welcome)) => DispatchResult::Reply(OwnerReply::Welcome {
                connection,
                welcome,
            }),
            Err(error) => DispatchResult::Reply(OwnerReply::Error(error.code())),
        },
        OwnerRequest::Request {
            connection,
            payload,
        } => match payload.get(..2) {
            Some([0, 1]) => match state.handle_control(connection, payload, now) {
                Ok(bytes) => DispatchResult::Reply(OwnerReply::Response(bytes)),
                Err(error) => DispatchResult::Reply(OwnerReply::Error(error.code())),
            },
            Some([0, 2]) => match state.handle_session(connection, payload, now) {
                Ok(SessionReply::Info(info)) => match encode_session_info(&info) {
                    Ok(bytes) => DispatchResult::Reply(OwnerReply::Response(bytes.to_vec())),
                    Err(_) => DispatchResult::Reply(OwnerReply::Error("invalid_session_info")),
                },
                Ok(SessionReply::Payload(bytes)) => {
                    DispatchResult::Reply(OwnerReply::Response(bytes))
                }
                Ok(SessionReply::Detached) => DispatchResult::Reply(OwnerReply::Detached),
                Err(error) => DispatchResult::Reply(OwnerReply::Error(error.code())),
            },
            Some([0, 4]) => match decode_watch_request(payload) {
                Ok(request) => dispatch_watch(state, connection, request, now, pending_capacity),
                Err(_) => DispatchResult::Reply(OwnerReply::Error("invalid_watch_request")),
            },
            _ => DispatchResult::Reply(OwnerReply::Error("unsupported_request_schema")),
        },
    }
}

fn dispatch_watch<E: EffectSink>(
    state: &mut StateOwner<E>,
    connection: &crate::server::Connection,
    request: WatchRequest,
    now_secs: u64,
    pending_capacity: bool,
) -> DispatchResult {
    match request {
        WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        } => match state.reconcile(connection, target, max_bytes, max_lines, now_secs) {
            Ok(snapshot) => {
                let response = Reconciliation {
                    info: snapshot.info,
                    cursor: snapshot.cursor,
                    screen_revision: snapshot.screen_revision,
                    truncated: snapshot.truncated,
                    text: &snapshot.text,
                };
                match encode_reconciliation(&response) {
                    Ok(bytes) => DispatchResult::Reply(OwnerReply::Response(bytes)),
                    Err(_) => DispatchResult::Reply(OwnerReply::Error("invalid_reconciliation")),
                }
            }
            Err(error) => DispatchResult::Reply(OwnerReply::Error(error.code())),
        },
        WatchRequest::Watch {
            target,
            cursor,
            limit,
            wait_ms,
        } => {
            let deadline = Instant::now().checked_add(Duration::from_millis(u64::from(wait_ms)));
            match state.read_events(connection, target, cursor, limit, now_secs) {
                Ok(page) if page.resync_required => DispatchResult::Reply(OwnerReply::Watch {
                    events: page.events,
                    end: WatchEnd {
                        status: WatchStatus::ResyncRequired,
                        cursor: page.cursor,
                        event_count: 0,
                    },
                }),
                Ok(page) if !page.events.is_empty() => {
                    let event_count = u16::try_from(page.events.len()).unwrap_or(u16::MAX);
                    DispatchResult::Reply(OwnerReply::Watch {
                        events: page.events,
                        end: WatchEnd {
                            status: WatchStatus::Changed,
                            cursor: page.cursor,
                            event_count,
                        },
                    })
                }
                Ok(page) if wait_ms == 0 => DispatchResult::Reply(OwnerReply::Watch {
                    events: page.events,
                    end: WatchEnd {
                        status: WatchStatus::TimedOut,
                        cursor: page.cursor,
                        event_count: 0,
                    },
                }),
                Ok(_) if !pending_capacity => {
                    DispatchResult::Reply(OwnerReply::Error("watch_capacity"))
                }
                Ok(_) => match deadline {
                    Some(deadline) => DispatchResult::Pending { request, deadline },
                    None => DispatchResult::Reply(OwnerReply::Error("watch_deadline")),
                },
                Err(error) => DispatchResult::Reply(OwnerReply::Error(error.code())),
            }
        }
    }
}

fn resolve_pending_watch<E: EffectSink>(
    state: &mut StateOwner<E>,
    pending: &PendingWatch,
    now_secs: u64,
) -> Option<OwnerReply> {
    let WatchRequest::Watch {
        target,
        cursor,
        limit,
        ..
    } = pending.request
    else {
        return Some(OwnerReply::Error("invalid_watch_request"));
    };
    let OwnerRequest::Request { connection, .. } = &pending.envelope.request else {
        return Some(OwnerReply::Error("invalid_watch_request"));
    };
    match state.read_events(connection, target, cursor, limit, now_secs) {
        Ok(page) if page.resync_required => Some(OwnerReply::Watch {
            events: page.events,
            end: WatchEnd {
                status: WatchStatus::ResyncRequired,
                cursor: page.cursor,
                event_count: 0,
            },
        }),
        Ok(page) if !page.events.is_empty() => {
            let event_count = u16::try_from(page.events.len()).unwrap_or(u16::MAX);
            Some(OwnerReply::Watch {
                events: page.events,
                end: WatchEnd {
                    status: WatchStatus::Changed,
                    cursor: page.cursor,
                    event_count,
                },
            })
        }
        Ok(page) if Instant::now() >= pending.deadline => Some(OwnerReply::Watch {
            events: page.events,
            end: WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: page.cursor,
                event_count: 0,
            },
        }),
        Ok(_) => None,
        Err(error) => Some(OwnerReply::Error(error.code())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::{Command, ReadSource, TargetRef};
    use crate::event_journal::EventCursor;
    use crate::handshake_protocol::{decode_welcome, Hello, CAPABILITY_EVENTS, CAPABILITY_READ};
    use crate::protocol::Outcome;
    use crate::protocol_wire::{Frame, MessageKind};
    use crate::response_protocol::RejectionCode;
    use crate::server::ReadResult;
    use crate::session_protocol::{encode_session_hello, SessionHello};
    use crate::watch_protocol::{
        decode_watch_end, encode_watch_request, WatchRequest, WatchStatus,
    };
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::{Duration, Instant};

    struct FailedPoll;
    impl EffectSink for FailedPoll {
        fn validate(&self, _: Command<'_>) -> Result<(), RejectionCode> {
            Ok(())
        }
        fn read(&mut self, _: ReadSource, _: usize, _: u16) -> Result<ReadResult, Outcome> {
            Err(Outcome::Unknown)
        }
        fn input(&mut self, _: Command<'_>) -> Outcome {
            Outcome::Unknown
        }
        fn poll(&mut self) -> Result<(), Outcome> {
            Err(Outcome::Unknown)
        }
        fn child_exited(&self) -> bool {
            false
        }

        fn screen_revision(&self) -> Option<u64> {
            Some(1)
        }
    }

    #[test]
    fn poll_error_does_not_starve_an_already_queued_owner_message() {
        let target = TargetRef {
            generation: 1,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let state = StateOwner::new(1, target, FailedPoll, false).unwrap();
        let mut driver = ServerDriver::new(state);
        let (server, client) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server).unwrap())
            .unwrap();
        thread::scope(|scope| {
            let client = scope.spawn(move || {
                let mut stream = AuthenticatedStream::new(client).unwrap();
                let payload = encode_session_hello(&SessionHello {
                    hello: Hello {
                        nonce: 1,
                        required_capabilities: CAPABILITY_READ,
                        optional_capabilities: 0,
                    },
                    resume: None,
                })
                .unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &payload).unwrap(),
                        Instant::now() + Duration::from_secs(2),
                    )
                    .unwrap();
                let reply = stream
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(reply.kind(), MessageKind::Welcome);
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while !client.is_finished() {
                assert!(driver.tick(|| 1).is_err());
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            client.join().unwrap();
        });
        driver.stop_connections(|| 1);
    }

    #[test]
    fn positive_watch_is_retained_until_its_finite_deadline() {
        let target = TargetRef {
            generation: 1,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let state = StateOwner::new(1, target, FailedPoll, false).unwrap();
        let mut driver = ServerDriver::new(state);
        let (server, client) = UnixStream::pair().unwrap();
        driver
            .attach(AuthenticatedStream::new(server).unwrap())
            .unwrap();

        thread::scope(|scope| {
            let client = scope.spawn(move || {
                let mut stream = AuthenticatedStream::new(client).unwrap();
                let hello = SessionHello {
                    hello: Hello {
                        nonce: 1,
                        required_capabilities: CAPABILITY_READ,
                        optional_capabilities: CAPABILITY_EVENTS,
                    },
                    resume: None,
                };
                let hello_payload = encode_session_hello(&hello).unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Hello, 0, &hello_payload).unwrap(),
                        Instant::now() + Duration::from_secs(2),
                    )
                    .unwrap();
                let welcome = stream
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(welcome.kind(), MessageKind::Welcome);
                let welcome = decode_welcome(welcome.payload()).unwrap();
                assert_eq!(
                    welcome.selected_capabilities,
                    CAPABILITY_READ | CAPABILITY_EVENTS
                );

                let request_payload = encode_watch_request(&WatchRequest::Watch {
                    target,
                    cursor: EventCursor {
                        generation: target.generation,
                        sequence: 0,
                    },
                    limit: 4,
                    wait_ms: 30,
                })
                .unwrap();
                stream
                    .write_frame(
                        &Frame::new(MessageKind::Request, 1, &request_payload).unwrap(),
                        Instant::now() + Duration::from_secs(2),
                    )
                    .unwrap();
                let terminal = stream
                    .read_frame(Instant::now() + Duration::from_secs(2))
                    .unwrap();
                assert_eq!(terminal.kind(), MessageKind::Response);
                assert_eq!(terminal.request_id(), 1);
                decode_watch_end(terminal.payload()).unwrap()
            });

            let pending_deadline = Instant::now() + Duration::from_secs(2);
            while driver.pending_watches() == 0 {
                assert!(driver.tick(|| 1).is_err());
                assert!(Instant::now() < pending_deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(driver.pending_watches(), 1);

            while !client.is_finished() {
                assert!(driver.tick(|| 1).is_err());
                assert!(Instant::now() < pending_deadline);
                thread::sleep(Duration::from_millis(1));
            }
            let end = client.join().unwrap();
            assert_eq!(end.status, WatchStatus::TimedOut);
            assert_eq!(end.event_count, 0);
            assert_eq!(end.cursor.sequence, 0);
            assert_eq!(driver.pending_watches(), 0);
        });
        driver.stop_connections(|| 1);
    }

    #[test]
    fn eight_pending_watches_are_bounded_and_stop_settles_them() {
        let target = TargetRef {
            generation: 1,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let state = StateOwner::new(1, target, FailedPoll, false).unwrap();
        let mut driver = ServerDriver::new(state);

        thread::scope(|scope| {
            for index in 0..MAX_PENDING_WATCHES {
                let (server, client) = UnixStream::pair().unwrap();
                driver
                    .attach(AuthenticatedStream::new(server).unwrap())
                    .unwrap();
                scope.spawn(move || {
                    let mut stream = AuthenticatedStream::new(client).unwrap();
                    let hello = SessionHello {
                        hello: Hello {
                            nonce: index as u64 + 1,
                            required_capabilities: CAPABILITY_READ,
                            optional_capabilities: CAPABILITY_EVENTS,
                        },
                        resume: None,
                    };
                    let hello_payload = encode_session_hello(&hello).unwrap();
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Hello, 0, &hello_payload).unwrap(),
                            Instant::now() + Duration::from_secs(2),
                        )
                        .unwrap();
                    let welcome = stream
                        .read_frame(Instant::now() + Duration::from_secs(2))
                        .unwrap();
                    assert_eq!(welcome.kind(), MessageKind::Welcome);
                    let request_payload = encode_watch_request(&WatchRequest::Watch {
                        target,
                        cursor: EventCursor {
                            generation: target.generation,
                            sequence: 0,
                        },
                        limit: 1,
                        wait_ms: 1_000,
                    })
                    .unwrap();
                    stream
                        .write_frame(
                            &Frame::new(MessageKind::Request, 1, &request_payload).unwrap(),
                            Instant::now() + Duration::from_secs(2),
                        )
                        .unwrap();
                    assert!(stream
                        .read_frame(Instant::now() + Duration::from_secs(2))
                        .is_err());
                });
            }

            let pending_deadline = Instant::now() + Duration::from_secs(2);
            while driver.pending_watches() < MAX_PENDING_WATCHES {
                assert!(driver.tick(|| 1).is_err());
                assert!(Instant::now() < pending_deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(driver.pending_watches(), MAX_PENDING_WATCHES);
            assert_eq!(driver.active_connections(), MAX_PENDING_WATCHES);
            driver.stop_connections(|| 1);
            assert_eq!(driver.pending_watches(), 0);
        });
    }
}
