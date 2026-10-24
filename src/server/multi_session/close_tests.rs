//! Deterministic coverage of the explicit close transaction, automatic
//! collapse on child exit, slot retirement and re-split with fresh identities.

use super::{CloseError, MultiSessionOwner, MAX_RETAINED_CLOSE_RESULTS};
use crate::close_protocol::{
    encode_close_request, CloseOutcome, CloseRejection, CloseRequest, CloseResult, StopOutcome,
};
use crate::control_protocol::{Command, ReadSource, TargetRef};
use crate::handshake_protocol::{Hello, CAPABILITY_READ};
use crate::managed_launch::LaunchJournalPort;
use crate::managed_launch_protocol::{encode_launch_request, LaunchRequest};
use crate::pane_layout::{Rect, SplitDirection};
use crate::protocol::{Ledger, OperationKey, OperationStatus, Outcome, ProtocolError};
use crate::response_protocol::RejectionCode;
use crate::server::{Connection, EffectSink, ReadResult, ServerError};
use crate::server_storage::{LaunchRecord, LaunchState};
use crate::session_protocol::SessionHello;
use crate::session_registry::SessionState;
use crate::split_protocol::{encode_split_request, SplitRequest};
use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

const GENERATION: u64 = 77;
const LEASE: u64 = 1;
const NOW: u64 = 3;
// Layout geometry is the pane's outer rectangle; PTY launch/resize sizes are
// the inset content rectangle used by the compositor.
const ROWS: u16 = 24;
const COLUMNS: u16 = 81;
const CONTENT_ROWS: u16 = ROWS - 2;
const CONTENT_COLUMNS: u16 = COLUMNS - 2;
const SPLIT_CONTENT_COLUMNS: u16 = (COLUMNS - 1) / 2 - 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Effect {
    Stop(u64),
    BeginStop(u64),
    StopPoll {
        session: u64,
        kill: bool,
    },
    Resize {
        session: u64,
        rows: u16,
        columns: u16,
    },
    ApplySize {
        session: u64,
        rows: u16,
        columns: u16,
    },
    Input(u64),
}

type Log = Rc<RefCell<Vec<Effect>>>;

struct Sink {
    session: u64,
    log: Log,
    exited: Rc<RefCell<HashSet<u64>>>,
    stop: StopOutcome,
}

impl EffectSink for Sink {
    fn validate(&self, _: Command<'_>) -> Result<(), RejectionCode> {
        Ok(())
    }

    fn read(&mut self, _: ReadSource, _: usize, _: u16) -> Result<ReadResult, Outcome> {
        Ok(ReadResult {
            text: String::new(),
            revision: 1,
            truncated: false,
        })
    }

    fn input(&mut self, _: Command<'_>) -> Outcome {
        self.log.borrow_mut().push(Effect::Input(self.session));
        Outcome::Applied
    }

    fn poll(&mut self) -> Result<(), Outcome> {
        Ok(())
    }

    fn child_exited(&self) -> bool {
        self.exited.borrow().contains(&self.session)
    }

    fn screen_revision(&self) -> Option<u64> {
        Some(1)
    }

    fn stop(&mut self) -> StopOutcome {
        self.log.borrow_mut().push(Effect::Stop(self.session));
        if self.stop != StopOutcome::Unknown {
            self.exited.borrow_mut().insert(self.session);
        }
        self.stop
    }

    fn begin_stop(&mut self) -> Result<Option<StopOutcome>, StopOutcome> {
        self.log.borrow_mut().push(Effect::BeginStop(self.session));
        Ok(None)
    }

    fn advance_stop(&mut self, kill: bool) -> Result<Option<StopOutcome>, StopOutcome> {
        self.log.borrow_mut().push(Effect::StopPoll {
            session: self.session,
            kill,
        });
        match (self.stop, kill) {
            (StopOutcome::Exited, false) => {
                self.exited.borrow_mut().insert(self.session);
                Ok(Some(StopOutcome::Exited))
            }
            (StopOutcome::Escalated, true) => {
                self.exited.borrow_mut().insert(self.session);
                Ok(Some(StopOutcome::Escalated))
            }
            _ => Ok(None),
        }
    }

    fn apply_size(&mut self, rows: u16, columns: u16) -> Result<(), Outcome> {
        self.log.borrow_mut().push(Effect::ApplySize {
            session: self.session,
            rows,
            columns,
        });
        Ok(())
    }
}

#[derive(Default)]
struct Journal {
    sessions: Vec<u64>,
}

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _: OperationKey, session: u64) -> io::Result<()> {
        // Mirror LaunchJournal: a session ID may only be journaled once.
        if self.sessions.contains(&session) {
            return Err(io::Error::other("PRIVATE-JOURNAL"));
        }
        Ok(())
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        if record.state == LaunchState::Intent {
            if self.sessions.contains(&record.session) {
                return Err(io::Error::other("PRIVATE-JOURNAL"));
            }
            self.sessions.push(record.session);
        }
        Ok(())
    }
}

struct Fixture {
    owner: MultiSessionOwner<Sink>,
    log: Log,
    exited: Rc<RefCell<HashSet<u64>>>,
    journal: Journal,
    first: TargetRef,
    sequence: u64,
    /// Stop behavior given to children spawned from now on.
    next_stop: StopOutcome,
}

fn key(sequence: u64) -> OperationKey {
    OperationKey {
        generation: GENERATION,
        lease: LEASE,
        sequence,
    }
}

fn launch_request(key: OperationKey, (rows, columns): (u16, u16)) -> LaunchRequest<'static> {
    LaunchRequest {
        key,
        rows,
        columns,
        requested_executable: b"/bin/sh",
        requested_working_directory: b"/",
        client_working_directory: b"/",
        arguments: &[],
        environment: &[],
    }
}

impl Fixture {
    fn new() -> Self {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(LEASE, 1).unwrap();
        let mut fixture = Self {
            owner: MultiSessionOwner::with_ledger(ledger, true).unwrap(),
            log: Rc::new(RefCell::new(Vec::new())),
            exited: Rc::new(RefCell::new(HashSet::new())),
            journal: Journal::default(),
            first: TargetRef {
                generation: GENERATION,
                workspace: 1,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            },
            sequence: 0,
            next_stop: StopOutcome::Exited,
        };
        let launch_key = fixture.next_key();
        let mut bytes = Vec::new();
        encode_launch_request(&launch_request(launch_key, (ROWS, COLUMNS)), &mut bytes).unwrap();
        let sink = fixture.sink_factory();
        let result = fixture
            .owner
            .execute_launch(
                &bytes,
                NOW,
                &mut fixture.journal,
                |_, target| Ok::<_, ()>(target.session),
                move |session| Ok::<_, ()>(sink(session)),
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Applied);
        fixture.first = result.target.unwrap();
        fixture
    }

    fn next_key(&mut self) -> OperationKey {
        self.sequence += 1;
        key(self.sequence)
    }

    fn sink_factory(&self) -> impl Fn(u64) -> Sink {
        let log = Rc::clone(&self.log);
        let exited = Rc::clone(&self.exited);
        let stop = self.next_stop;
        move |session| Sink {
            session,
            log: Rc::clone(&log),
            exited: Rc::clone(&exited),
            stop,
        }
    }

    fn revision(&self) -> u64 {
        self.owner.layout_snapshot().unwrap().revision
    }

    /// Split `target` to the right and return the new target.
    fn split_right(&mut self, target: TargetRef) -> TargetRef {
        let split_key = self.next_key();
        let launch = launch_request(split_key, (CONTENT_ROWS, SPLIT_CONTENT_COLUMNS));
        let mut bytes = Vec::new();
        encode_split_request(
            &SplitRequest {
                key: split_key,
                target,
                expected_layout_revision: self.revision(),
                direction: SplitDirection::Right,
                launch: &launch,
            },
            &mut bytes,
        )
        .unwrap();
        let sink = self.sink_factory();
        let log = Rc::clone(&self.log);
        let result = self
            .owner
            .execute_split(
                &bytes,
                NOW,
                &mut self.journal,
                |_, target| Ok::<_, ()>(target.session),
                move |session| Ok::<_, ()>(sink(session)),
                move |sink: &mut Sink, rows, columns| {
                    log.borrow_mut().push(Effect::Resize {
                        session: sink.session,
                        rows,
                        columns,
                    });
                    Ok::<_, ()>(())
                },
            )
            .expect("split succeeds");
        assert_eq!(result.outcome, Outcome::Applied);
        result.new_target.unwrap()
    }

    fn close_bytes(&mut self, target: TargetRef, revision: u64) -> Vec<u8> {
        let request = CloseRequest {
            key: self.next_key(),
            target,
            expected_layout_revision: revision,
        };
        let mut bytes = Vec::new();
        encode_close_request(&request, &mut bytes).unwrap();
        bytes
    }

    fn close(&mut self, bytes: &[u8]) -> Result<CloseResult, CloseError> {
        let log = Rc::clone(&self.log);
        self.owner
            .execute_close(bytes, NOW, move |sink: &mut Sink, rows, columns| {
                log.borrow_mut().push(Effect::Resize {
                    session: sink.session,
                    rows,
                    columns,
                });
                Ok::<_, ()>(())
            })
    }

    fn close_no_effects(&mut self, bytes: &[u8]) -> Result<CloseResult, CloseError> {
        self.owner
            .execute_close(bytes, NOW, |_: &mut Sink, _, _| -> Result<(), ()> {
                panic!("must not resize")
            })
    }

    fn ledger_status(&self, sequence: u64) -> OperationStatus {
        self.owner.global.ledger.status(key(sequence), NOW).unwrap()
    }

    fn take_log(&self) -> Vec<Effect> {
        std::mem::take(&mut *self.log.borrow_mut())
    }

    fn connect(&mut self, target: TargetRef) -> Connection {
        let hello = SessionHello {
            hello: Hello {
                nonce: 41,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: 0,
            },
            resume: None,
        };
        self.owner.connect(target, &hello, NOW).unwrap().0
    }
}

fn root() -> Rect {
    Rect {
        x: 0,
        y: 0,
        rows: ROWS,
        columns: COLUMNS,
    }
}

#[test]
fn closing_a_live_sibling_stops_it_collapses_and_resizes_the_survivor() {
    for close_new in [true, false] {
        let mut fixture = Fixture::new();
        let first = fixture.first;
        let second = fixture.split_right(first);
        fixture.take_log();
        let (closed, survivor) = if close_new {
            (second, first)
        } else {
            (first, second)
        };
        let bytes = fixture.close_bytes(closed, 2);
        let result = fixture.close(&bytes).unwrap();
        assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
        assert_eq!(result.target, Some(closed));
        assert_eq!(result.layout_revision, 3);
        assert!(!result.survivor_resize_pending);
        assert_eq!(
            fixture.take_log(),
            [
                Effect::Stop(closed.session),
                Effect::Resize {
                    session: survivor.session,
                    rows: CONTENT_ROWS,
                    columns: CONTENT_COLUMNS,
                },
            ]
        );
        let layout = fixture.owner.layout_snapshot().unwrap();
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.divider, None);
        let pane = layout.panes[0].unwrap();
        assert_eq!((pane.target, pane.rect), (survivor, root()));
        assert!(fixture.owner.launches.is_retired(closed));
        assert_eq!(fixture.owner.launches.records().count(), 1);
        assert!(!fixture.owner.all_sessions_ended());
        assert_eq!(
            fixture.ledger_status(result.key.sequence),
            OperationStatus::Completed(Outcome::Applied)
        );
        let statuses = fixture.owner.pane_statuses();
        let status = statuses[0].unwrap();
        assert_eq!(status.geometry.target, survivor);
        assert!(status.live);
        assert_eq!(status.state, SessionState::Applied);
        assert!(statuses[1].is_none());

        // Exact retry returns the recorded result with no effect.
        assert_eq!(fixture.close_no_effects(&bytes).unwrap(), result);
        assert!(fixture.take_log().is_empty());
    }
}

#[test]
fn escalated_and_unknown_stops_are_reported_and_unknown_keeps_the_pane() {
    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Escalated;
    let second = fixture.split_right(fixture.first);
    let bytes = fixture.close_bytes(second, 2);
    let result = fixture.close(&bytes).unwrap();
    assert_eq!(
        result.outcome,
        CloseOutcome::Stopped(StopOutcome::Escalated)
    );
    assert_eq!(result.layout_revision, 3);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);

    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Unknown;
    let second = fixture.split_right(fixture.first);
    fixture.take_log();
    let bytes = fixture.close_bytes(second, 2);
    let result = fixture.close(&bytes).unwrap();
    assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Unknown));
    assert_eq!(result.layout_revision, 2);
    assert_eq!(fixture.take_log(), [Effect::Stop(second.session)]);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    assert_eq!(
        fixture.owner.launches.state(second),
        Ok(SessionState::Applied)
    );
    assert_eq!(
        fixture.ledger_status(result.key.sequence),
        OperationStatus::Completed(Outcome::Unknown)
    );
    // A later observed exit collapses through poll.
    fixture.exited.borrow_mut().insert(second.session);
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
    assert!(fixture.owner.launches.is_retired(second));
}

#[test]
fn phased_close_keeps_ledger_pending_until_tick_driven_completion() {
    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Escalated;
    let first = fixture.first;
    let second = fixture.split_right(first);
    fixture.take_log();
    let bytes = fixture.close_bytes(second, 2);
    let started = Instant::now();
    let status = fixture.owner.begin_close(&bytes, NOW, started).unwrap();
    assert!(matches!(status, super::CloseStatus::InProgress));
    let key = crate::close_protocol::decode_close_request(&bytes)
        .unwrap()
        .key;
    assert_eq!(
        fixture.ledger_status(key.sequence),
        OperationStatus::InProgress
    );
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    assert!(matches!(
        fixture.owner.begin_close(&bytes, NOW, started).unwrap(),
        super::CloseStatus::InProgress
    ));
    assert_eq!(fixture.take_log(), [Effect::BeginStop(second.session)]);

    fixture.owner.advance_closes_at(started).unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    fixture
        .owner
        .advance_closes_at(started + Duration::from_secs(1))
        .unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
    assert_eq!(
        fixture.ledger_status(key.sequence),
        OperationStatus::Completed(Outcome::Applied)
    );
    assert_eq!(
        fixture.owner.close_results.last().unwrap().outcome,
        CloseOutcome::Stopped(StopOutcome::Escalated)
    );
    assert_eq!(
        fixture.take_log(),
        [
            Effect::StopPoll {
                session: second.session,
                kill: false,
            },
            Effect::StopPoll {
                session: second.session,
                kill: true,
            },
            Effect::ApplySize {
                session: first.session,
                rows: CONTENT_ROWS,
                columns: CONTENT_COLUMNS,
            },
        ]
    );
    fixture
        .owner
        .advance_closes_at(started + Duration::from_secs(3))
        .unwrap();
    assert_eq!(fixture.owner.close_results.len(), 1);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
}

#[test]
fn legacy_close_does_not_stop_a_target_owned_by_an_active_close() {
    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Escalated;
    let first = fixture.first;
    let second = fixture.split_right(first);
    fixture.take_log();
    let phased = fixture.close_bytes(second, 2);
    let start = Instant::now();
    assert!(matches!(
        fixture.owner.begin_close(&phased, NOW, start).unwrap(),
        super::CloseStatus::InProgress
    ));

    let legacy = fixture.close_bytes(second, 2);
    let rejected = fixture.close_no_effects(&legacy).unwrap();
    assert_eq!(
        rejected.outcome,
        CloseOutcome::Rejected(CloseRejection::AlreadyClosed)
    );
    assert_eq!(
        fixture.ledger_status(rejected.key.sequence),
        OperationStatus::Completed(Outcome::Failed)
    );
    assert_eq!(fixture.take_log(), [Effect::BeginStop(second.session)]);
    assert_eq!(fixture.owner.active_closes.len(), 1);
    fixture
        .owner
        .advance_closes_at(start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
}

#[test]
fn uncertain_last_pane_close_is_not_proof_of_empty_layout() {
    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Unknown;
    let first = fixture.first;
    let before = fixture.owner.layout_snapshot().unwrap();
    let bytes = fixture.close_bytes(first, before.revision);
    let start = Instant::now();
    assert!(matches!(
        fixture.owner.begin_close(&bytes, NOW, start).unwrap(),
        super::CloseStatus::InProgress
    ));
    assert!(!fixture.owner.layout_terminal_empty());
    fixture
        .owner
        .advance_closes_at(start + Duration::from_secs(2))
        .unwrap();
    assert_eq!(fixture.owner.layout_snapshot(), Some(before));
    assert!(!fixture.owner.layout_terminal_empty());
    assert_eq!(
        fixture.owner.launches.state(first),
        Ok(SessionState::Applied)
    );

    // An actual subsequent exit supplies the missing positive evidence.
    fixture.exited.borrow_mut().insert(first.session);
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.owner.layout_snapshot(), Some(before));
    assert!(fixture.owner.layout_terminal_empty());
}

#[test]
fn phased_close_timeout_retains_closing_target_without_reenabling_it() {
    let mut fixture = Fixture::new();
    fixture.next_stop = StopOutcome::Unknown;
    let first = fixture.first;
    let second = fixture.split_right(first);
    let bytes = fixture.close_bytes(second, 2);
    let start = Instant::now();
    let key = crate::close_protocol::decode_close_request(&bytes)
        .unwrap()
        .key;
    assert!(matches!(
        fixture.owner.begin_close(&bytes, NOW, start).unwrap(),
        super::CloseStatus::InProgress
    ));
    fixture.owner.advance_closes_at(start).unwrap();
    fixture
        .owner
        .advance_closes_at(start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        fixture.ledger_status(key.sequence),
        OperationStatus::InProgress
    );
    fixture
        .owner
        .advance_closes_at(start + Duration::from_secs(2))
        .unwrap();
    assert_eq!(
        fixture.ledger_status(key.sequence),
        OperationStatus::Completed(Outcome::Unknown)
    );
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    fixture.exited.borrow_mut().insert(second.session);
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
    assert!(fixture.owner.launches.is_retired(second));
}

#[test]
fn shell_exit_with_live_sibling_collapses_automatically_in_poll() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let second = fixture.split_right(first);
    fixture.take_log();
    fixture.exited.borrow_mut().insert(first.session);
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(
        fixture.take_log(),
        [Effect::ApplySize {
            session: second.session,
            rows: CONTENT_ROWS,
            columns: CONTENT_COLUMNS,
        }]
    );
    let layout = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(layout.revision, 3);
    assert_eq!(
        layout.panes,
        [
            Some(crate::pane_layout::PaneGeometry {
                target: second,
                rect: root()
            }),
            None,
            None,
            None
        ]
    );
    assert!(fixture.owner.launches.is_retired(first));
    assert!(!fixture.owner.all_sessions_ended());
    assert!(fixture.owner.pending_resizes().iter().all(Option::is_none));

    // A stale close for the collapsed pane is rejected without effects.
    let bytes = fixture.close_bytes(first, 3);
    let result = fixture.close_no_effects(&bytes).unwrap();
    assert_eq!(
        result.outcome,
        CloseOutcome::Rejected(CloseRejection::AlreadyClosed)
    );
    assert_eq!(result.layout_revision, 3);
    assert_eq!(
        fixture.ledger_status(result.key.sequence),
        OperationStatus::Completed(Outcome::Failed)
    );
    assert!(fixture.take_log().is_empty());

    // Polling again changes nothing.
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.revision(), 3);
}

#[test]
fn last_shell_exit_keeps_tombstone_and_ends_the_workspace() {
    let mut fixture = Fixture::new();
    let second = fixture.split_right(fixture.first);
    fixture.exited.borrow_mut().insert(second.session);
    fixture.owner.poll(NOW).unwrap();
    assert!(!fixture.owner.all_sessions_ended());
    fixture.exited.borrow_mut().insert(fixture.first.session);
    fixture.owner.poll(NOW).unwrap();
    assert!(fixture.owner.all_sessions_ended());
    let layout = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.panes[0].unwrap().target, fixture.first);
    let status = fixture.owner.pane_statuses()[0].unwrap();
    assert!(!status.live);
    assert_eq!(status.state, SessionState::Exited);
}

#[test]
fn both_shells_exiting_in_one_poll_collapse_without_resizing_and_end_the_workspace() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let second = fixture.split_right(first);
    fixture.take_log();
    fixture.exited.borrow_mut().insert(first.session);
    fixture.exited.borrow_mut().insert(second.session);
    fixture.owner.poll(NOW).unwrap();
    assert!(
        fixture.take_log().is_empty(),
        "no resize of an exited survivor"
    );
    assert!(fixture.owner.all_sessions_ended());
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
    assert_eq!(fixture.owner.launches.records().count(), 1);
}

#[test]
fn survivor_resize_failure_is_partial_and_pending() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let second = fixture.split_right(first);
    let bytes = fixture.close_bytes(second, 2);
    let result = fixture
        .owner
        .execute_close(&bytes, NOW, |_: &mut Sink, _, _| {
            Err::<(), _>("PRIVATE-RESIZE")
        })
        .unwrap();
    assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
    assert!(result.survivor_resize_pending);
    assert_eq!(
        fixture.ledger_status(result.key.sequence),
        OperationStatus::Completed(Outcome::Partial)
    );
    let pending = fixture.owner.pending_resizes();
    let entry = pending.into_iter().flatten().next().unwrap();
    assert_eq!(entry.target, first);
    assert_eq!(entry.desired, (CONTENT_ROWS, CONTENT_COLUMNS));
    assert_eq!(entry.confirmed, (CONTENT_ROWS, SPLIT_CONTENT_COLUMNS));
    assert!(fixture.owner.pane_statuses()[0].unwrap().resize_pending);
}

#[test]
fn closing_the_only_pane_stops_it_and_ends_the_workspace() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let bytes = fixture.close_bytes(first, 1);
    let result = fixture.close_no_effects(&bytes).unwrap();
    assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
    assert_eq!(result.layout_revision, 1);
    assert_eq!(fixture.take_log(), [Effect::Stop(first.session)]);
    assert!(fixture.owner.all_sessions_ended());
    assert_eq!(
        fixture.owner.launches.state(first),
        Ok(SessionState::Exited)
    );

    // A second, different close of the ended pane does not stop again.
    let again = fixture.close_bytes(first, 1);
    let rejected = fixture.close_no_effects(&again).unwrap();
    assert_eq!(
        rejected.outcome,
        CloseOutcome::Rejected(CloseRejection::AlreadyClosed)
    );
    assert!(fixture.take_log().is_empty());
}

#[test]
fn resplit_after_close_gets_fresh_ids_and_old_targets_stay_spent() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let mut seen = vec![first];
    // Repeated close/re-split cycles never reuse a session or pane ID.
    for cycle in 0..4 {
        let new = fixture.split_right(first);
        assert!(!seen.contains(&new), "cycle {cycle} reused an identity");
        assert!(seen
            .iter()
            .all(|old| old.session != new.session && old.pane != new.pane));
        seen.push(new);
        let revision = fixture.revision();
        let bytes = fixture.close_bytes(new, revision);
        let result = fixture.close(&bytes).unwrap();
        assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
        assert_eq!(fixture.owner.launches.records().count(), 1);
    }
    assert_eq!(fixture.owner.launches.retired_count(), 4);
    assert_eq!(
        seen.iter().map(|target| target.session).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    // Every spent target is rejected as already closed.
    for old in &seen[1..] {
        let revision = fixture.revision();
        let bytes = fixture.close_bytes(*old, revision);
        assert_eq!(
            fixture.close_no_effects(&bytes).unwrap().outcome,
            CloseOutcome::Rejected(CloseRejection::AlreadyClosed)
        );
    }
    // The survivor can also be closed after re-splitting, leaving the newest.
    let newest = fixture.split_right(first);
    assert_eq!(newest.session, 6);
    let bytes = fixture.close_bytes(first, fixture.revision());
    fixture.close(&bytes).unwrap();
    let layout = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(layout.panes[0].unwrap().target, newest);
    assert_eq!(layout.panes[0].unwrap().rect, root());
    let again = fixture.split_right(newest);
    assert_eq!(again.session, 7);
}

#[test]
fn split_duplicates_survive_close_and_resplit() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let split_key = fixture.next_key();
    let launch = launch_request(split_key, (CONTENT_ROWS, SPLIT_CONTENT_COLUMNS));
    let mut bytes = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: split_key,
            target: first,
            expected_layout_revision: 1,
            direction: SplitDirection::Right,
            launch: &launch,
        },
        &mut bytes,
    )
    .unwrap();
    let sink = fixture.sink_factory();
    let original = fixture
        .owner
        .execute_split(
            &bytes,
            NOW,
            &mut fixture.journal,
            |_, target| Ok::<_, ()>(target.session),
            move |session| Ok::<_, ()>(sink(session)),
            |_: &mut Sink, _, _| Ok::<_, ()>(()),
        )
        .unwrap();
    let close = fixture.close_bytes(original.new_target.unwrap(), 2);
    fixture.close(&close).unwrap();
    fixture.split_right(first);
    let retry = fixture
        .owner
        .execute_split(
            &bytes,
            NOW,
            &mut fixture.journal,
            |_, _| -> Result<u64, ()> { panic!("must not prepare") },
            |_| -> Result<Sink, ()> { panic!("must not spawn") },
            |_: &mut Sink, _, _| -> Result<(), ()> { panic!("must not resize") },
        )
        .unwrap();
    assert_eq!(retry, original);
}

#[test]
fn stale_layout_stale_target_and_no_layout_are_recorded_failed_without_effects() {
    let mut fixture = Fixture::new();
    let second = fixture.split_right(fixture.first);
    fixture.take_log();
    let stale_layout = fixture.close_bytes(second, 1);
    let result = fixture.close_no_effects(&stale_layout).unwrap();
    assert_eq!(
        result.outcome,
        CloseOutcome::Rejected(CloseRejection::StaleLayout)
    );
    assert_eq!(result.layout_revision, 2);
    let unknown_target = TargetRef {
        occupant_revision: 2,
        ..second
    };
    let stale_target = fixture.close_bytes(unknown_target, 2);
    assert_eq!(
        fixture.close_no_effects(&stale_target).unwrap().outcome,
        CloseOutcome::Rejected(CloseRejection::StaleTarget)
    );
    assert!(fixture.take_log().is_empty());
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    // Exact retry of a rejection returns it unchanged.
    assert_eq!(fixture.close_no_effects(&stale_layout).unwrap(), result);

    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(LEASE, 1).unwrap();
    let mut empty = MultiSessionOwner::<Sink>::with_ledger(ledger, true).unwrap();
    let mut bytes = Vec::new();
    encode_close_request(
        &CloseRequest {
            key: key(1),
            target: fixture.first,
            expected_layout_revision: 1,
        },
        &mut bytes,
    )
    .unwrap();
    let result = empty
        .execute_close(&bytes, NOW, |_: &mut Sink, _, _| -> Result<(), ()> {
            panic!("must not resize")
        })
        .unwrap();
    assert_eq!(
        result.outcome,
        CloseOutcome::Rejected(CloseRejection::NoLayout)
    );
    assert_eq!(result.layout_revision, 0);
}

#[test]
fn changed_payload_conflicts_and_malformed_or_foreign_generation_are_not_admitted() {
    let mut fixture = Fixture::new();
    let second = fixture.split_right(fixture.first);
    let bytes = fixture.close_bytes(second, 2);
    fixture.close(&bytes).unwrap();
    fixture.take_log();
    let mut changed = bytes.clone();
    changed[75] = 9; // different expected revision under the same key
    let error = fixture.close_no_effects(&changed).unwrap_err();
    assert_eq!(
        error.ledger_error(),
        Some(&ProtocolError::OperationConflict)
    );
    assert!(fixture.take_log().is_empty());

    let mut malformed = fixture.close_bytes(fixture.first, 3);
    let sequence = fixture.sequence;
    malformed[79] = 1;
    assert!(matches!(
        fixture.close_no_effects(&malformed),
        Err(CloseError::InvalidPayload(_))
    ));
    assert_eq!(
        fixture.ledger_status(sequence),
        OperationStatus::NotAdmitted
    );

    let foreign = OperationKey {
        generation: GENERATION + 1,
        ..key(50)
    };
    let mut bytes = Vec::new();
    encode_close_request(
        &CloseRequest {
            key: foreign,
            target: TargetRef {
                generation: GENERATION + 1,
                ..fixture.first
            },
            expected_layout_revision: 3,
        },
        &mut bytes,
    )
    .unwrap();
    assert!(matches!(
        fixture.close_no_effects(&bytes),
        Err(CloseError::GenerationMismatch)
    ));
    let rendered = format!("{error:?} {error} {:?}", fixture.owner);
    assert!(!rendered.contains("PRIVATE"));
}

#[test]
fn authorize_close_requires_live_own_lease_but_no_input_or_pin() {
    let mut fixture = Fixture::new();
    let second = fixture.split_right(fixture.first);
    // Read-only connection pinned to the first pane may stop the second.
    let connection = fixture.connect(fixture.first);
    let request = CloseRequest {
        key: OperationKey {
            generation: GENERATION,
            lease: connection.lease(),
            sequence: 1,
        },
        target: second,
        expected_layout_revision: 2,
    };
    fixture
        .owner
        .authorize_close(&connection, &request, NOW)
        .unwrap();
    let foreign_lease = CloseRequest {
        key: key(1),
        ..request
    };
    assert_eq!(
        fixture
            .owner
            .authorize_close(&connection, &foreign_lease, NOW)
            .unwrap_err(),
        ServerError::Unauthorized
    );

    let mut bytes = Vec::new();
    encode_close_request(&request, &mut bytes).unwrap();
    let log = Rc::clone(&fixture.log);
    let result = fixture
        .owner
        .execute_close(&bytes, NOW, move |sink: &mut Sink, rows, columns| {
            log.borrow_mut().push(Effect::Resize {
                session: sink.session,
                rows,
                columns,
            });
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(result.outcome, CloseOutcome::Stopped(StopOutcome::Exited));
    assert_eq!(
        fixture
            .owner
            .close_status(&connection, request.key, NOW)
            .unwrap(),
        result
    );
    let never = OperationKey {
        sequence: 99,
        ..request.key
    };
    assert_eq!(
        fixture
            .owner
            .close_status(&connection, never, NOW)
            .unwrap()
            .outcome,
        CloseOutcome::Rejected(CloseRejection::NotAdmitted)
    );
    assert_eq!(
        fixture
            .owner
            .close_status(&connection, key(1), NOW)
            .unwrap_err(),
        ServerError::Unauthorized
    );
    let view = fixture.owner.pane_view(&connection, NOW).unwrap();
    assert_eq!(view[0].unwrap().geometry.target, fixture.first);
    assert!(!fixture.take_log().contains(&Effect::Input(1)));
    assert!(!format!("{:?}", view[0].unwrap()).contains(&GENERATION.to_string()));
}

#[test]
fn failed_split_records_are_reclaimed_so_a_later_split_succeeds() {
    let mut fixture = Fixture::new();
    let first = fixture.first;
    let split_key = fixture.next_key();
    let launch = launch_request(split_key, (CONTENT_ROWS, SPLIT_CONTENT_COLUMNS));
    let mut bytes = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: split_key,
            target: first,
            expected_layout_revision: 1,
            direction: SplitDirection::Right,
            launch: &launch,
        },
        &mut bytes,
    )
    .unwrap();
    let error = fixture
        .owner
        .execute_split(
            &bytes,
            NOW,
            &mut fixture.journal,
            |_, target| Ok::<_, ()>(target.session),
            |_| Err::<Sink, _>("PRIVATE-SPAWN"),
            |_: &mut Sink, _, _| Ok::<_, ()>(()),
        )
        .unwrap_err();
    assert!(error.failure().is_some());
    assert_eq!(fixture.owner.launches.records().count(), 2);
    let second = fixture.split_right(first);
    assert_eq!(second.session, 3, "the failed identity 2 stays spent");
    assert_eq!(fixture.owner.launches.records().count(), 2);
}

#[test]
fn close_result_store_is_bounded_before_admission() {
    let mut fixture = Fixture::new();
    let filler = CloseResult {
        key: key(1),
        outcome: CloseOutcome::Rejected(CloseRejection::StaleTarget),
        target: Some(fixture.first),
        layout_revision: 1,
        survivor_resize_pending: false,
    };
    fixture.owner.close_results = vec![filler; MAX_RETAINED_CLOSE_RESULTS];
    let bytes = fixture.close_bytes(fixture.first, 1);
    let sequence = fixture.sequence;
    assert!(matches!(
        fixture.close_no_effects(&bytes),
        Err(CloseError::Capacity)
    ));
    assert_eq!(
        fixture.ledger_status(sequence),
        OperationStatus::NotAdmitted
    );
    assert!(fixture.take_log().is_empty());
}
