use super::{
    AsyncSplitAdmission, CloseError, LayoutResizeError, MultiSessionOwner, SplitError, SplitFailure,
};
use crate::control_protocol::{Command, ReadSource, TargetRef};
use crate::managed_launch::LaunchJournalPort;
use crate::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
use crate::pane_layout::{LayoutError, Rect, SplitDirection};
use crate::protocol::{Ledger, OperationKey, OperationStatus, Outcome, ProtocolError};
use crate::response_protocol::RejectionCode;
use crate::server::{EffectSink, ReadResult};
use crate::server_storage::{LaunchRecord, LaunchState};
use crate::session_registry::{RegistryError, SessionState};
use crate::split_protocol::{encode_split_request, SplitRequest, SplitResult};
use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Instant;

const GENERATION: u64 = 73;
const LEASE: u64 = 1;
const NOW: u64 = 2;
const ROOT_ROWS: u16 = 24;
const ROOT_COLUMNS: u16 = 81;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Effect {
    Prepare(u64),
    JournalPreflight,
    Record(LaunchState),
    Spawn(u64),
    Resize {
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
}

impl EffectSink for Sink {
    fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
        Ok(())
    }

    fn read(
        &mut self,
        _source: ReadSource,
        _max_text_bytes: usize,
        _max_lines: u16,
    ) -> Result<ReadResult, Outcome> {
        Ok(ReadResult {
            text: String::new(),
            revision: 1,
            truncated: false,
        })
    }

    fn input(&mut self, _command: Command<'_>) -> Outcome {
        self.log.borrow_mut().push(Effect::Input(self.session));
        Outcome::Applied
    }

    fn poll(&mut self) -> Result<(), Outcome> {
        Ok(())
    }

    fn child_exited(&self) -> bool {
        self.session == u64::MAX
    }

    fn screen_revision(&self) -> Option<u64> {
        Some(1)
    }
}

struct Journal {
    log: Log,
    preflight_failure: bool,
    record_failure: Option<LaunchState>,
}

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _key: OperationKey, _session: u64) -> io::Result<()> {
        self.log.borrow_mut().push(Effect::JournalPreflight);
        if self.preflight_failure {
            Err(io::Error::other("PRIVATE-JOURNAL"))
        } else {
            Ok(())
        }
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        self.log.borrow_mut().push(Effect::Record(record.state));
        if self.record_failure == Some(record.state) {
            Err(io::Error::other("PRIVATE-JOURNAL"))
        } else {
            Ok(())
        }
    }
}

struct Fixture {
    owner: MultiSessionOwner<Sink>,
    log: Log,
    journal: Journal,
    first: TargetRef,
}

fn key(sequence: u64) -> OperationKey {
    OperationKey {
        generation: GENERATION,
        lease: LEASE,
        sequence,
    }
}

fn launch_bytes(key: OperationKey, rows: u16, columns: u16, env_value: &[u8]) -> Vec<u8> {
    let environment = [EnvironmentEntry {
        name: b"SPLIT_TEST_VALUE",
        value: env_value,
    }];
    let mut bytes = Vec::new();
    encode_launch_request(
        &LaunchRequest {
            key,
            rows,
            columns,
            requested_executable: b"/bin/sh",
            requested_working_directory: b"/",
            client_working_directory: b"/",
            arguments: &[],
            environment: &environment,
        },
        &mut bytes,
    )
    .unwrap();
    bytes
}

fn split_bytes(
    key: OperationKey,
    target: TargetRef,
    revision: u64,
    direction: SplitDirection,
    (rows, columns): (u16, u16),
    env_value: &[u8],
) -> Vec<u8> {
    // Split fixtures describe the new pane's outer layout rectangle. The
    // launch request carries only the usable PTY content dimensions.
    let (rows, columns) = (rows.saturating_sub(2), columns.saturating_sub(2));
    let environment = [EnvironmentEntry {
        name: b"SPLIT_TEST_VALUE",
        value: env_value,
    }];
    let launch = LaunchRequest {
        key,
        rows,
        columns,
        requested_executable: b"/bin/sh",
        requested_working_directory: b"/",
        client_working_directory: b"/",
        arguments: &[],
        environment: &environment,
    };
    let mut bytes = Vec::new();
    encode_split_request(
        &SplitRequest {
            key,
            target,
            expected_layout_revision: revision,
            direction,
            launch: &launch,
        },
        &mut bytes,
    )
    .unwrap();
    bytes
}

fn fixture() -> Fixture {
    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(LEASE, 1).unwrap();
    let mut owner = MultiSessionOwner::with_ledger(ledger, true).unwrap();
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let mut journal = Journal {
        log: Rc::clone(&log),
        preflight_failure: false,
        record_failure: None,
    };
    let spawn_log = Rc::clone(&log);
    let result = owner
        .execute_launch(
            &launch_bytes(key(1), ROOT_ROWS, ROOT_COLUMNS, b"first"),
            NOW,
            &mut journal,
            |_, target| Ok::<_, ()>(target.session),
            move |session| {
                Ok::<_, ()>(Sink {
                    session,
                    log: spawn_log,
                })
            },
        )
        .unwrap();
    assert_eq!(result.outcome, Outcome::Applied);
    owner
        .resize_layout(1, ROOT_ROWS, ROOT_COLUMNS, |_: &mut Sink, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap();
    log.borrow_mut().clear();
    Fixture {
        owner,
        log,
        journal,
        first: result.target.unwrap(),
    }
}

fn planned_size(direction: SplitDirection) -> (u16, u16) {
    match direction {
        SplitDirection::Left | SplitDirection::Right => (24, 40),
        SplitDirection::Up => (11, 81),
        SplitDirection::Down => (12, 81),
    }
}

fn rect(x: u16, y: u16, rows: u16, columns: u16) -> Rect {
    Rect {
        x,
        y,
        rows,
        columns,
    }
}

/// Run one split with fakes; `spawn_ok`/`resize_ok` inject known failures.
fn split(
    fixture: &mut Fixture,
    payload: &[u8],
    spawn_ok: bool,
    resize_ok: bool,
) -> Result<SplitResult, SplitError> {
    let prepare_log = Rc::clone(&fixture.log);
    let spawn_log = Rc::clone(&fixture.log);
    let resize_log = Rc::clone(&fixture.log);
    fixture.owner.execute_split(
        payload,
        NOW,
        &mut fixture.journal,
        move |_, target| {
            prepare_log
                .borrow_mut()
                .push(Effect::Prepare(target.session));
            Ok::<_, ()>(target.session)
        },
        move |session| {
            spawn_log.borrow_mut().push(Effect::Spawn(session));
            if spawn_ok {
                Ok(Sink {
                    session,
                    log: Rc::clone(&spawn_log),
                })
            } else {
                Err("PRIVATE-SPAWN")
            }
        },
        move |sink: &mut Sink, rows, columns| {
            resize_log.borrow_mut().push(Effect::Resize {
                session: sink.session,
                rows,
                columns,
            });
            if resize_ok {
                Ok(())
            } else {
                Err("PRIVATE-RESIZE")
            }
        },
    )
}

fn no_effects_split(fixture: &mut Fixture, payload: &[u8]) -> Result<SplitResult, SplitError> {
    let full = fixture
        .owner
        .layout_snapshot()
        .is_some_and(|layout| layout.pane_count() >= crate::pane_layout::MAX_PANES);
    fixture.owner.execute_split(
        payload,
        NOW,
        &mut fixture.journal,
        move |_, target| {
            if full {
                panic!("must not prepare")
            } else {
                Ok::<u64, ()>(target.session)
            }
        },
        move |session| {
            if full {
                panic!("must not spawn")
            } else {
                Ok::<Sink, ()>(Sink {
                    session,
                    log: Rc::new(RefCell::new(Vec::new())),
                })
            }
        },
        move |_: &mut Sink, _, _| -> Result<(), ()> {
            if full {
                panic!("must not resize")
            } else {
                Ok(())
            }
        },
    )
}

fn ledger_status(fixture: &Fixture, sequence: u64) -> OperationStatus {
    fixture
        .owner
        .global
        .ledger
        .status(key(sequence), NOW)
        .unwrap()
}

fn assert_no_input(fixture: &Fixture) {
    assert!(!fixture
        .log
        .borrow()
        .iter()
        .any(|effect| matches!(effect, Effect::Input(_))));
}

#[test]
fn layout_is_established_by_the_first_applied_launch() {
    let fixture = fixture();
    let snapshot = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(snapshot.revision, 1);
    assert_eq!((snapshot.rows, snapshot.columns), (ROOT_ROWS, ROOT_COLUMNS));
    assert_eq!(snapshot.pane_count(), 1);
    assert_eq!(snapshot.panes[0].unwrap().target, fixture.first);
}

#[test]
fn applied_split_in_each_direction_spawns_once_and_resizes_existing_once() {
    let cases = [
        (
            SplitDirection::Left,
            rect(41, 0, 24, 40),
            rect(0, 0, 24, 40),
        ),
        (
            SplitDirection::Right,
            rect(0, 0, 24, 40),
            rect(41, 0, 24, 40),
        ),
        (SplitDirection::Up, rect(0, 12, 12, 81), rect(0, 0, 11, 81)),
        (
            SplitDirection::Down,
            rect(0, 0, 11, 81),
            rect(0, 12, 12, 81),
        ),
    ];
    for (direction, existing_rect, new_rect) in cases {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            direction,
            planned_size(direction),
            b"env",
        );
        let result = split(&mut fixture, &payload, true, true).unwrap();
        assert_eq!(result.outcome, Outcome::Applied);
        assert_eq!(result.committed_layout_revision, 2);
        assert_eq!(result.existing_target, fixture.first);
        let new_target = result.new_target.unwrap();
        assert_eq!(new_target.session, 2);
        assert_eq!(
            *fixture.log.borrow(),
            [
                Effect::Prepare(2),
                Effect::JournalPreflight,
                Effect::Record(LaunchState::Intent),
                Effect::Spawn(2),
                Effect::Record(LaunchState::Spawned),
                Effect::Record(LaunchState::Applied),
                Effect::Resize {
                    session: 1,
                    rows: existing_rect.rows - 2,
                    columns: existing_rect.columns - 2,
                },
            ],
            "{direction:?}"
        );
        let snapshot = fixture.owner.layout_snapshot().unwrap();
        assert_eq!(snapshot.revision, 2);
        let rect_of = |target| {
            snapshot
                .panes
                .into_iter()
                .flatten()
                .find(|pane| pane.target == target)
                .unwrap()
                .rect
        };
        assert_eq!(rect_of(fixture.first), existing_rect);
        assert_eq!(rect_of(new_target), new_rect);
        assert_eq!((new_rect.rows, new_rect.columns), planned_size(direction));
        assert_eq!(
            fixture.owner.launches.state(new_target),
            Ok(SessionState::Applied)
        );
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Applied)
        );
        assert_no_input(&fixture);
    }
}

#[test]
fn duplicate_after_commit_returns_original_result_despite_stale_revision() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    let first = split(&mut fixture, &payload, true, true).unwrap();
    // Advance the layout so the original expected revision is stale.
    let resized = fixture
        .owner
        .resize_layout(2, 30, 101, |_: &mut Sink, _, _| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(resized.revision, 3);
    fixture.log.borrow_mut().clear();

    let before_retry = fixture
        .owner
        .workspace_events
        .as_ref()
        .unwrap()
        .current_cursor();

    let retry = no_effects_split(&mut fixture, &payload).unwrap();
    assert_eq!(retry, first);
    let mut first_bytes = Vec::new();
    let mut retry_bytes = Vec::new();
    crate::split_protocol::encode_split_result(&first, &mut first_bytes).unwrap();
    crate::split_protocol::encode_split_result(&retry, &mut retry_bytes).unwrap();
    assert_eq!(first_bytes, retry_bytes);
    assert!(fixture.log.borrow().is_empty());
    assert_eq!(fixture.owner.launches.records().count(), 2);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 3);
    assert_eq!(
        fixture
            .owner
            .workspace_events
            .as_ref()
            .unwrap()
            .current_cursor(),
        before_retry
    );
}

#[test]
fn changed_payloads_conflict_before_effects_even_at_capacity() {
    let mut fixture = fixture();
    let original = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    split(&mut fixture, &original, true, true).unwrap();
    let third_payload = split_bytes(
        key(3),
        fixture.first,
        2,
        SplitDirection::Down,
        (12, 40),
        b"env",
    );
    split(&mut fixture, &third_payload, true, true).unwrap();
    let third = fixture
        .owner
        .layout_snapshot()
        .unwrap()
        .panes
        .iter()
        .flatten()
        .last()
        .unwrap()
        .target;
    let fourth_payload = split_bytes(key(4), third, 3, SplitDirection::Down, (12, 40), b"env");
    split(&mut fixture, &fourth_payload, true, true).unwrap();
    assert!(matches!(
        fixture.owner.launches.next_target(),
        Err(RegistryError::Capacity)
    ));
    fixture.log.borrow_mut().clear();
    let stale_target = TargetRef {
        occupant_revision: 2,
        ..fixture.first
    };
    let changed = [
        split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Left,
            (24, 40),
            b"env",
        ),
        split_bytes(
            key(2),
            stale_target,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        ),
        split_bytes(
            key(2),
            fixture.first,
            2,
            SplitDirection::Right,
            (24, 40),
            b"env",
        ),
        split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"enw",
        ),
    ];
    for payload in changed {
        assert_ne!(payload, original);
        let error = no_effects_split(&mut fixture, &payload).unwrap_err();
        assert_eq!(
            error.ledger_error(),
            Some(&ProtocolError::OperationConflict)
        );
    }
    assert!(fixture.log.borrow().is_empty());
    assert_eq!(
        ledger_status(&fixture, 2),
        OperationStatus::Completed(Outcome::Applied)
    );
}

#[test]
fn rejected_new_splits_record_failed_without_ids_layout_or_effects() {
    let mut fixture = fixture();
    let stale_target = TargetRef {
        occupant_revision: 9,
        ..fixture.first
    };
    let cases = [
        (
            split_bytes(
                key(2),
                fixture.first,
                7,
                SplitDirection::Right,
                (24, 40),
                b"env",
            ),
            SplitFailure::StaleLayout,
        ),
        (
            split_bytes(
                key(3),
                stale_target,
                1,
                SplitDirection::Right,
                (24, 40),
                b"env",
            ),
            SplitFailure::StaleTarget,
        ),
        (
            split_bytes(
                key(4),
                fixture.first,
                1,
                SplitDirection::Right,
                (24, 41),
                b"env",
            ),
            SplitFailure::LaunchSizeMismatch,
        ),
    ];
    for (payload, expected) in cases {
        let next_before = fixture.owner.launches.next_target();
        let error = no_effects_split(&mut fixture, &payload).unwrap_err();
        let (reason, result) = error.failure().unwrap();
        assert_eq!(reason, expected);
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(result.committed_layout_revision, 0);
        assert_eq!(result.new_target, None);
        assert_eq!(
            ledger_status(&fixture, result.key.sequence),
            OperationStatus::Completed(Outcome::Failed)
        );
        assert_eq!(fixture.owner.launches.next_target(), next_before);
        assert_eq!(fixture.owner.launches.records().count(), 1);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
        // The exact retry reports the same recorded result, still with no effects.
        let retry = no_effects_split(&mut fixture, &payload).unwrap_err();
        assert_eq!(retry.failure().unwrap().1, result);
    }
    assert!(fixture.log.borrow().is_empty());

    // A geometry rejection from the layout is also Failed before effects.
    let mut narrow = narrow_fixture();
    let payload = split_bytes(
        key(2),
        narrow.first,
        1,
        SplitDirection::Left,
        (24, 22),
        b"env",
    );
    let error = no_effects_split(&mut narrow, &payload).unwrap_err();
    assert_eq!(
        error.failure().unwrap().0,
        SplitFailure::Layout(LayoutError::LayoutTooSmall)
    );
    assert_eq!(narrow.owner.launches.records().count(), 1);
}

fn narrow_fixture() -> Fixture {
    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(LEASE, 1).unwrap();
    let mut owner = MultiSessionOwner::with_ledger(ledger, true).unwrap();
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let mut journal = Journal {
        log: Rc::clone(&log),
        preflight_failure: false,
        record_failure: None,
    };
    let spawn_log = Rc::clone(&log);
    let result = owner
        .execute_launch(
            &launch_bytes(key(1), 24, 40, b"narrow"),
            NOW,
            &mut journal,
            |_, target| Ok::<_, ()>(target.session),
            move |session| {
                Ok::<_, ()>(Sink {
                    session,
                    log: spawn_log,
                })
            },
        )
        .unwrap();
    owner
        .resize_layout(1, 24, 40, |_: &mut Sink, _, _| Ok::<_, ()>(()))
        .unwrap();
    log.borrow_mut().clear();
    Fixture {
        owner,
        log,
        journal,
        first: result.target.unwrap(),
    }
}

#[test]
fn third_session_and_missing_layout_fail_before_effects() {
    let mut fixture = fixture();
    let spawn_log = Rc::clone(&fixture.log);
    let second = fixture
        .owner
        .execute_launch(
            &launch_bytes(key(2), 10, 40, b"second"),
            NOW,
            &mut fixture.journal,
            |_, target| Ok::<_, ()>(target.session),
            move |session| {
                Ok::<_, ()>(Sink {
                    session,
                    log: spawn_log,
                })
            },
        )
        .unwrap();
    assert_eq!(second.outcome, Outcome::Applied);
    // The layout belongs to the first launch only.
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
    fixture.log.borrow_mut().clear();
    let payload = split_bytes(
        key(3),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    let result = split(&mut fixture, &payload, true, true).unwrap();
    assert_eq!(result.outcome, Outcome::Applied);
    assert_eq!(fixture.owner.launches.records().count(), 3);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);

    let mut three = self::fixture();
    let mut sequence = 2;
    let mut split_leaf = |fixture: &mut Fixture, target: TargetRef, direction: SplitDirection| {
        let layout = fixture.owner.layout_snapshot().unwrap();
        let rect = layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == target)
            .unwrap()
            .rect;
        let size = match crate::pane_layout::split_geometry(rect, direction) {
            Ok(size) => size,
            Err(_) => return target,
        };
        let payload = split_bytes(
            key(sequence),
            target,
            layout.revision,
            direction,
            size,
            b"env",
        );
        sequence += 1;
        split(fixture, &payload, true, true)
            .unwrap()
            .new_target
            .unwrap()
    };
    let first = three.first;
    let second = split_leaf(&mut three, first, SplitDirection::Right);
    let _third = split_leaf(&mut three, first, SplitDirection::Down);
    let fourth = split_leaf(&mut three, second, SplitDirection::Down);
    let layout = three.owner.layout_snapshot().unwrap();
    let target = layout
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == fourth)
        .unwrap()
        .target;
    let mut full = three;
    let fifth_payload = split_bytes(
        key(sequence),
        target,
        layout.revision,
        SplitDirection::Left,
        (7, 40),
        b"env",
    );
    let error = no_effects_split(&mut full, &fifth_payload).unwrap_err();
    assert_eq!(error.failure().unwrap().0, SplitFailure::Capacity);
    assert_eq!(
        full.owner.layout_snapshot().unwrap().pane_count(),
        crate::pane_layout::MAX_PANES
    );

    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(LEASE, 1).unwrap();
    let owner = MultiSessionOwner::<Sink>::with_ledger(ledger, true).unwrap();
    let mut empty = Fixture {
        owner,
        log: Rc::new(RefCell::new(Vec::new())),
        journal: Journal {
            log: Rc::new(RefCell::new(Vec::new())),
            preflight_failure: false,
            record_failure: None,
        },
        first: fixture.first,
    };
    assert!(empty.owner.layout_snapshot().is_none());
    let no_layout_payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    let error = no_effects_split(&mut empty, &no_layout_payload).unwrap_err();
    assert_eq!(error.failure().unwrap().0, SplitFailure::NoLayout);
    assert_eq!(empty.owner.launches.records().count(), 0);
}

#[test]
fn malformed_and_wrong_generation_payloads_reject_before_ledger() {
    let mut fixture = fixture();
    let mut payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    payload[80] = 1; // nonzero reserved byte
    assert!(matches!(
        no_effects_split(&mut fixture, &payload),
        Err(SplitError::InvalidPayload(_))
    ));
    assert_eq!(ledger_status(&fixture, 2), OperationStatus::NotAdmitted);
    let wrong = OperationKey {
        generation: GENERATION + 1,
        ..key(2)
    };
    let wrong_target = TargetRef {
        generation: GENERATION + 1,
        ..fixture.first
    };
    let payload = split_bytes(
        wrong,
        wrong_target,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    assert!(matches!(
        no_effects_split(&mut fixture, &payload),
        Err(SplitError::GenerationMismatch)
    ));
    assert_eq!(ledger_status(&fixture, 2), OperationStatus::NotAdmitted);
}

#[test]
fn preparation_and_journal_preflight_failures_record_failed_without_reservation() {
    for preflight in [false, true] {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        fixture.journal.preflight_failure = preflight;
        let error = fixture
            .owner
            .execute_split(
                &payload,
                NOW,
                &mut fixture.journal,
                |_, target| {
                    if preflight {
                        Ok(target.session)
                    } else {
                        Err("PRIVATE-PREPARE")
                    }
                },
                |_| -> Result<Sink, ()> { panic!("must not spawn") },
                |_: &mut Sink, _, _| -> Result<(), ()> { panic!("must not resize") },
            )
            .unwrap_err();
        let expected = if preflight {
            SplitFailure::JournalPreflightFailed
        } else {
            SplitFailure::PreparationFailed
        };
        assert_eq!(error.failure().unwrap().0, expected);
        assert_eq!(fixture.owner.launches.records().count(), 1);
        assert_eq!(fixture.owner.launches.next_target().unwrap().session, 2);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Failed)
        );
        let rendered = format!("{error:?} {error} {:?}", fixture.owner);
        assert!(!rendered.contains("PRIVATE"));
    }
}

#[test]
fn known_spawn_failure_before_commit_is_failed_and_keeps_layout() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Down,
        (12, 81),
        b"env",
    );
    let error = split(&mut fixture, &payload, false, true).unwrap_err();
    let (reason, result) = error.failure().unwrap();
    assert_eq!(reason, SplitFailure::LaunchFailed);
    assert_eq!(result.new_target, None);
    assert_eq!(
        *fixture.log.borrow(),
        [
            Effect::Prepare(2),
            Effect::JournalPreflight,
            Effect::Record(LaunchState::Intent),
            Effect::Spawn(2),
            Effect::Record(LaunchState::Failed),
        ]
    );
    // The identity is spent by the known launch failure, as for launches.
    let spent = fixture.owner.launches.records().nth(1).unwrap();
    assert_eq!(spent.state, SessionState::Failed);
    assert!(!spent.has_effects);
    let snapshot = fixture.owner.layout_snapshot().unwrap();
    assert_eq!((snapshot.revision, snapshot.pane_count()), (1, 1));
    assert_eq!(
        ledger_status(&fixture, 2),
        OperationStatus::Completed(Outcome::Failed)
    );
    fixture.log.borrow_mut().clear();
    let retry = no_effects_split(&mut fixture, &payload).unwrap_err();
    assert_eq!(retry.failure().unwrap().1, result);
}

#[test]
fn post_spawn_journal_fault_commits_unknown_retains_child_and_never_respawns() {
    for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
        let mut fixture = fixture();
        fixture.journal.record_failure = Some(failed_state);
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        let result = split(&mut fixture, &payload, true, true).unwrap();
        assert_eq!(result.outcome, Outcome::Unknown);
        assert_eq!(result.committed_layout_revision, 2);
        let new_target = result.new_target.unwrap();
        assert_eq!(
            fixture.owner.launches.state(new_target),
            Ok(SessionState::Unknown)
        );
        assert!(fixture.owner.launches.effects(new_target).is_ok());
        assert!(fixture.owner.launches.effects(fixture.first).is_ok());
        assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
        assert!(fixture.log.borrow().contains(&Effect::Resize {
            session: 1,
            rows: 22,
            columns: 38
        }));
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Unknown)
        );
        fixture.log.borrow_mut().clear();
        let retry = no_effects_split(&mut fixture, &payload).unwrap();
        assert_eq!(retry, result);
        assert!(fixture.log.borrow().is_empty());
        assert_no_input(&fixture);
    }
}

#[test]
fn existing_session_resize_failure_is_partial_and_layout_stays_committed() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Up,
        (11, 81),
        b"env",
    );
    let result = split(&mut fixture, &payload, true, false).unwrap();
    assert_eq!(result.outcome, Outcome::Partial);
    assert_eq!(result.committed_layout_revision, 2);
    let new_target = result.new_target.unwrap();
    assert_eq!(
        fixture.owner.launches.state(new_target),
        Ok(SessionState::Applied)
    );
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
    assert_eq!(
        ledger_status(&fixture, 2),
        OperationStatus::Completed(Outcome::Partial)
    );
    fixture.log.borrow_mut().clear();
    assert_eq!(no_effects_split(&mut fixture, &payload).unwrap(), result);
    assert!(fixture.log.borrow().is_empty());

    // The existing pane is pending at its desired split rectangle; the
    // new pane was spawned at its planned size and is not pending.
    let pending = fixture.owner.pending_resizes();
    assert_eq!(pending.iter().flatten().count(), 1);
    let entry = pending.into_iter().flatten().next().unwrap();
    assert_eq!(entry.target, fixture.first);
    assert_eq!(entry.desired, (10, 79));
    assert_eq!(entry.confirmed, (ROOT_ROWS - 2, ROOT_COLUMNS - 2));
    assert!(!format!("{entry:?}").contains(&GENERATION.to_string()));
    assert_no_input(&fixture);
}

fn recording_resize(log: &Log, fail: bool) -> impl FnMut(&mut Sink, u16, u16) -> Result<(), ()> {
    let log = Rc::clone(log);
    move |sink: &mut Sink, rows, columns| {
        log.borrow_mut().push(Effect::Resize {
            session: sink.session,
            rows,
            columns,
        });
        if fail {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[test]
fn same_size_resize_layout_retries_only_pending_split_resize_without_new_revision() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Up,
        (11, 81),
        b"env",
    );
    assert_eq!(
        split(&mut fixture, &payload, true, false).unwrap().outcome,
        Outcome::Partial
    );
    fixture.log.borrow_mut().clear();

    // Still failing: ResizePending again, same revision, only the pending pane.
    let error = fixture
        .owner
        .resize_layout(
            2,
            ROOT_ROWS,
            ROOT_COLUMNS,
            recording_resize(&fixture.log, true),
        )
        .unwrap_err();
    match error {
        LayoutResizeError::ResizePending { snapshot, failed } => {
            assert_eq!(snapshot.revision, 2);
            // Up places the existing pane second (bottom).
            assert_eq!(failed, [false, true, false, false]);
        }
        other => panic!("expected pending resize, got {other:?}"),
    }
    assert_eq!(
        *fixture.log.borrow(),
        [Effect::Resize {
            session: 1,
            rows: 10,
            columns: 79
        }]
    );
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
    assert_eq!(fixture.owner.pending_resizes().iter().flatten().count(), 1);
    fixture.log.borrow_mut().clear();

    // Succeeding: Ok, same revision, pending cleared, exactly one call.
    let snapshot = fixture
        .owner
        .resize_layout(
            2,
            ROOT_ROWS,
            ROOT_COLUMNS,
            recording_resize(&fixture.log, false),
        )
        .unwrap();
    assert_eq!(snapshot.revision, 2);
    assert_eq!(
        *fixture.log.borrow(),
        [Effect::Resize {
            session: 1,
            rows: 10,
            columns: 79
        }]
    );
    assert!(fixture.owner.pending_resizes().iter().all(Option::is_none));
    fixture.log.borrow_mut().clear();

    // Nothing pending: a same-size call has no PTY effect.
    fixture
        .owner
        .resize_layout(
            2,
            ROOT_ROWS,
            ROOT_COLUMNS,
            recording_resize(&fixture.log, false),
        )
        .unwrap();
    assert!(fixture.log.borrow().is_empty());
    assert_no_input(&fixture);
}

#[test]
fn same_size_resize_layout_retries_failed_root_resize_panes_only() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    split(&mut fixture, &payload, true, true).unwrap();
    fixture.log.borrow_mut().clear();
    let log = Rc::clone(&fixture.log);
    let error = fixture
        .owner
        .resize_layout(2, 30, 101, move |sink: &mut Sink, rows, columns| {
            log.borrow_mut().push(Effect::Resize {
                session: sink.session,
                rows,
                columns,
            });
            if sink.session == 2 {
                Err(())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(matches!(
        error,
        LayoutResizeError::ResizePending { ref snapshot, failed }
            if snapshot.revision == 3 && failed == [false, true, false, false]
    ));
    assert_eq!(fixture.log.borrow().len(), 2);
    let pending = fixture.owner.pending_resizes();
    let entry = pending.into_iter().flatten().next().unwrap();
    assert_eq!(pending.iter().flatten().count(), 1);
    assert_eq!(entry.target.session, 2);
    assert_eq!((entry.desired, entry.confirmed), ((28, 48), (22, 38)));
    fixture.log.borrow_mut().clear();

    let error = fixture
        .owner
        .resize_layout(3, 30, 101, recording_resize(&fixture.log, true))
        .unwrap_err();
    assert!(matches!(
        error,
        LayoutResizeError::ResizePending { ref snapshot, failed }
            if snapshot.revision == 3 && failed == [false, true, false, false]
    ));
    let only_second = [Effect::Resize {
        session: 2,
        rows: 28,
        columns: 48,
    }];
    assert_eq!(*fixture.log.borrow(), only_second);
    fixture.log.borrow_mut().clear();

    let snapshot = fixture
        .owner
        .resize_layout(3, 30, 101, recording_resize(&fixture.log, false))
        .unwrap();
    assert_eq!(snapshot.revision, 3);
    assert_eq!(*fixture.log.borrow(), only_second);
    assert!(fixture.owner.pending_resizes().iter().all(Option::is_none));
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 3);
    assert_no_input(&fixture);
}

#[test]
fn resize_layout_applies_absolute_sizes_rejects_small_and_reports_pending() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        (24, 40),
        b"env",
    );
    split(&mut fixture, &payload, true, true).unwrap();
    fixture.log.borrow_mut().clear();
    let log = Rc::clone(&fixture.log);
    let mut record = move |sink: &mut Sink, rows: u16, columns: u16| {
        log.borrow_mut().push(Effect::Resize {
            session: sink.session,
            rows,
            columns,
        });
        Ok::<_, ()>(())
    };

    // Same size: no revision change and no PTY effect.
    let same = fixture.owner.resize_layout(2, 24, 81, &mut record).unwrap();
    assert_eq!(same.revision, 2);
    assert!(fixture.log.borrow().is_empty());

    // Too small for two 20-column panes, and stale: no change, no effect.
    assert!(matches!(
        fixture.owner.resize_layout(2, 24, 40, &mut record),
        Err(LayoutResizeError::Layout(LayoutError::LayoutTooSmall))
    ));
    assert!(matches!(
        fixture.owner.resize_layout(1, 30, 101, &mut record),
        Err(LayoutResizeError::Layout(LayoutError::StaleRevision))
    ));
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
    assert!(fixture.log.borrow().is_empty());

    let grown = fixture
        .owner
        .resize_layout(2, 30, 101, &mut record)
        .unwrap();
    assert_eq!(grown.revision, 3);
    assert_eq!(
        *fixture.log.borrow(),
        [
            Effect::Resize {
                session: 1,
                rows: 28,
                columns: 48
            },
            Effect::Resize {
                session: 2,
                rows: 28,
                columns: 48
            },
        ]
    );

    let error = fixture
        .owner
        .resize_layout(3, 20, 61, |sink: &mut Sink, _, _| {
            if sink.session == 2 {
                Err("PRIVATE-RESIZE")
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    match error {
        LayoutResizeError::ResizePending { snapshot, failed } => {
            assert_eq!(snapshot.revision, 4);
            assert_eq!(failed, [false, true, false, false]);
        }
        other => panic!("expected pending resize, got {other:?}"),
    }
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 4);
    assert_no_input(&fixture);
}

#[test]
fn async_split_admission_deduplicates_before_busy_checks_and_has_no_io() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async",
    );
    let start = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap();
    let AsyncSplitAdmission::Start {
        key: started,
        target,
    } = start
    else {
        panic!("expected asynchronous start");
    };
    assert_eq!(started, key(2));
    assert!(fixture.log.borrow().is_empty());
    assert!(matches!(
        fixture.owner.begin_async_split(&payload, NOW, false),
        Ok(AsyncSplitAdmission::Pending)
    ));

    let changed = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"changed",
    );
    assert!(matches!(
        fixture.owner.begin_async_split(&changed, NOW, true),
        Err(SplitError::Ledger(ProtocolError::OperationConflict))
    ));
    assert!(matches!(
        fixture.owner.begin_async_split(
            &split_bytes(
                key(3),
                fixture.first,
                1,
                SplitDirection::Right,
                planned_size(SplitDirection::Right),
                b"competing",
            ),
            NOW,
            true,
        ),
        Err(SplitError::InProgress)
    ));
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
    assert!(matches!(
        fixture
            .owner
            .resize_layout(1, ROOT_ROWS, ROOT_COLUMNS, |_, _, _| Ok::<_, ()>(())),
        Err(LayoutResizeError::InProgress)
    ));
    assert_eq!(fixture.owner.async_split_key(), Some(key(2)));
    assert_eq!(fixture.owner.async_split_result(key(2)), None);
    assert!(fixture.owner.launches.state(target).is_ok());
}

#[test]
fn async_split_worker_failure_without_child_releases_topology_slot() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async",
    );
    let AsyncSplitAdmission::Start { key, target } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous start");
    };
    let AsyncSplitAdmission::Terminal(result) = fixture
        .owner
        .fail_async_split(key, target, Outcome::Failed, false)
        .unwrap()
    else {
        panic!("expected terminal failure");
    };
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(result.new_target, None);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
    assert_eq!(fixture.owner.async_split_key(), None);
    assert_eq!(fixture.owner.async_split_result(key), Some(result));
    assert!(matches!(
        fixture.owner.begin_async_split(&payload, NOW, false),
        Ok(AsyncSplitAdmission::Terminal(duplicate)) if duplicate == result
    ));
}

#[test]
fn async_split_no_child_unknown_latches_uncertainty_without_layout_commit() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async",
    );
    let AsyncSplitAdmission::Start { key, target } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous start");
    };
    assert!(matches!(
        fixture
            .owner
            .fail_async_split(key, target, Outcome::Unknown, true),
        Ok(AsyncSplitAdmission::Uncertain)
    ));
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
    assert_eq!(fixture.owner.async_split_key(), None);
    assert_eq!(fixture.owner.async_split_result(key), None);
    assert!(matches!(
        fixture.owner.begin_async_split(&payload, NOW, true),
        Ok(AsyncSplitAdmission::Uncertain)
    ));
    assert!(!fixture.owner.all_sessions_ended());
}

#[test]
fn async_split_attached_unknown_commits_and_keeps_child_owned() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async",
    );
    let AsyncSplitAdmission::Start { key, target } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous start");
    };
    fixture
        .owner
        .attach_async_split(
            key,
            target,
            Sink {
                session: target.session,
                log: Rc::clone(&fixture.log),
            },
        )
        .unwrap();
    let AsyncSplitAdmission::Terminal(result) = fixture
        .owner
        .finish_async_split(key, target, Outcome::Unknown, true, |_, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap()
    else {
        panic!("expected terminal split result");
    };
    assert_eq!(result.outcome, Outcome::Unknown);
    assert_eq!(result.committed_layout_revision, 2);
    assert_eq!(result.new_target, Some(target));
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
    assert_eq!(fixture.owner.async_split_result(key), Some(result));
    assert_eq!(
        fixture.owner.pane_statuses()[1].unwrap().state,
        SessionState::Unknown
    );
}

#[test]
fn exited_children_do_not_end_workspace_before_async_split_settles() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async-exit",
    );
    let AsyncSplitAdmission::Start { key, target } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous start");
    };
    fixture
        .owner
        .attach_async_split(
            key,
            target,
            Sink {
                session: target.session,
                log: Rc::clone(&fixture.log),
            },
        )
        .unwrap();
    fixture
        .owner
        .for_each_effects_mut(|_, sink| sink.session = u64::MAX);
    fixture.owner.poll(NOW).unwrap();
    assert!(!fixture.owner.all_sessions_ended());
    assert!(!fixture.owner.layout_terminal_empty());
    let AsyncSplitAdmission::Terminal(result) = fixture
        .owner
        .finish_async_split(key, target, Outcome::Applied, false, |_, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap()
    else {
        panic!("expected terminal split result");
    };
    assert_eq!(result.outcome, Outcome::Applied);
    assert!(fixture.owner.all_sessions_ended());
    assert!(fixture.owner.layout_terminal_empty());
    assert!(fixture
        .owner
        .pane_statuses()
        .into_iter()
        .flatten()
        .all(|pane| pane.state == SessionState::Exited));
}

#[test]
fn existing_pane_exit_during_async_split_keeps_preview_until_finish() {
    let mut fixture = fixture();
    let payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"async",
    );
    let AsyncSplitAdmission::Start { key, target } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous start");
    };
    fixture.owner.for_each_effects_mut(|pane, sink| {
        if pane == fixture.first {
            sink.session = u64::MAX;
        }
    });
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
    fixture
        .owner
        .attach_async_split(
            key,
            target,
            Sink {
                session: target.session,
                log: Rc::clone(&fixture.log),
            },
        )
        .unwrap();
    let AsyncSplitAdmission::Terminal(result) = fixture
        .owner
        .finish_async_split(key, target, Outcome::Applied, false, |_, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap()
    else {
        panic!("expected terminal split result");
    };
    assert_eq!(result.outcome, Outcome::Applied);
    assert_eq!(result.committed_layout_revision, 2);
    let layout = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(layout.revision, 3);
    assert_eq!(layout.pane_count(), 1);
    assert_eq!(layout.panes[0].unwrap().target, target);
    assert!(!fixture.owner.all_sessions_ended());
}

#[test]
fn async_splits_grow_two_to_three_and_three_to_four_panes() {
    let mut fixture = fixture();
    let mut sequence = 2;
    let mut split_leaf = |fixture: &mut Fixture, target: TargetRef, direction| {
        let layout = fixture.owner.layout_snapshot().unwrap();
        let rect = layout
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == target)
            .unwrap()
            .rect;
        let size = crate::pane_layout::split_geometry(rect, direction).unwrap();
        let payload = split_bytes(
            key(sequence),
            target,
            layout.revision,
            direction,
            size,
            b"async-four-pane",
        );
        sequence += 1;
        let AsyncSplitAdmission::Start {
            key,
            target: new_target,
        } = fixture
            .owner
            .begin_async_split(&payload, NOW, true)
            .unwrap()
        else {
            panic!("expected asynchronous split start");
        };
        fixture
            .owner
            .attach_async_split(
                key,
                new_target,
                Sink {
                    session: new_target.session,
                    log: Rc::clone(&fixture.log),
                },
            )
            .unwrap();
        let AsyncSplitAdmission::Terminal(result) = fixture
            .owner
            .finish_async_split(key, new_target, Outcome::Applied, false, |_, _, _| {
                Ok::<_, ()>(())
            })
            .unwrap()
        else {
            panic!("expected terminal split result");
        };
        assert_eq!(result.outcome, Outcome::Applied);
        assert!(result.committed_layout_revision > layout.revision);
        result.new_target.unwrap()
    };

    let first = fixture.first;
    let second = split_leaf(&mut fixture, first, SplitDirection::Right);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    let third = split_leaf(&mut fixture, first, SplitDirection::Down);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 3);
    let fourth = split_leaf(&mut fixture, second, SplitDirection::Down);
    let snapshot = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(snapshot.pane_count(), 4);
    assert!([first, second, third, fourth].iter().all(|target| {
        snapshot
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == *target)
    }));
    assert!(fixture.owner.pending_resizes().iter().all(Option::is_none));
}

#[test]
fn multiple_exits_during_async_split_wait_for_preview_then_collapse() {
    let mut fixture = fixture();
    let second_payload = split_bytes(
        key(2),
        fixture.first,
        1,
        SplitDirection::Right,
        planned_size(SplitDirection::Right),
        b"second",
    );
    let second = split(&mut fixture, &second_payload, true, true)
        .unwrap()
        .new_target
        .unwrap();
    let payload = split_bytes(
        key(3),
        fixture.first,
        2,
        SplitDirection::Down,
        crate::pane_layout::split_geometry(
            fixture
                .owner
                .layout_snapshot()
                .unwrap()
                .panes
                .iter()
                .flatten()
                .find(|pane| pane.target == fixture.first)
                .unwrap()
                .rect,
            SplitDirection::Down,
        )
        .unwrap(),
        b"third",
    );
    let AsyncSplitAdmission::Start {
        key: split_key,
        target,
    } = fixture
        .owner
        .begin_async_split(&payload, NOW, true)
        .unwrap()
    else {
        panic!("expected asynchronous split start");
    };
    fixture.owner.for_each_effects_mut(|pane, sink| {
        if pane == fixture.first || pane == second {
            sink.session = u64::MAX;
        }
    });
    fixture.owner.poll(NOW).unwrap();
    assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
    assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
    assert!(matches!(
        fixture
            .owner
            .resize_layout(2, 30, 100, |_, _, _| Ok::<_, ()>(())),
        Err(LayoutResizeError::InProgress)
    ));
    let close_payload = {
        let request = crate::close_protocol::CloseRequest {
            key: key(4),
            target: fixture.first,
            expected_layout_revision: 2,
        };
        let mut bytes = Vec::new();
        crate::close_protocol::encode_close_request(&request, &mut bytes).unwrap();
        bytes
    };
    assert!(matches!(
        fixture
            .owner
            .begin_close(&close_payload, NOW, Instant::now()),
        Err(CloseError::InProgress)
    ));
    fixture
        .owner
        .attach_async_split(
            split_key,
            target,
            Sink {
                session: target.session,
                log: Rc::clone(&fixture.log),
            },
        )
        .unwrap();
    let AsyncSplitAdmission::Terminal(result) = fixture
        .owner
        .finish_async_split(split_key, target, Outcome::Applied, false, |_, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap()
    else {
        panic!("expected terminal split result");
    };
    assert_eq!(result.committed_layout_revision, 3);
    let snapshot = fixture.owner.layout_snapshot().unwrap();
    assert_eq!(snapshot.pane_count(), 1);
    assert_eq!(snapshot.panes[0].unwrap().target, target);
    assert_eq!(snapshot.revision, 5);
    assert_eq!(fixture.owner.async_split_result(split_key), Some(result));
    let journal = fixture.owner.workspace_events.as_ref().unwrap();
    let cursor = crate::workspace_watch_protocol::WorkspaceCursor {
        sequence: 0,
        ..journal.current_cursor()
    };
    let changes = journal.read_after(cursor, 64).unwrap();
    assert_eq!(
        changes
            .iter()
            .map(|event| event.revision)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert!(changes.iter().all(|event| !event.empty));
}
