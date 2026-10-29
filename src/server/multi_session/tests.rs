use super::MultiSessionOwner;
use crate::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use crate::handshake_protocol::{Hello, CAPABILITY_EVENTS, CAPABILITY_READ};
use crate::managed_launch::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
use crate::managed_launch_protocol::{encode_launch_request, LaunchRequest};
use crate::protocol::{
    Ledger, OperationKey, OperationStatus, Outcome, ProtocolError, LEASE_TTL_SECS,
};
use crate::response_protocol::decode_response_payload;
use crate::response_protocol::{RejectionCode, ResponseBody};
use crate::scoped_watch_protocol::SessionCursor;
use crate::server::SessionReply;
use crate::server::{Connection, EffectSink, ReadResult, ServerError};
use crate::server_storage::{LaunchRecord, LaunchState};
use crate::session_protocol::{SessionHello, SessionInfo, SessionRequest};
use crate::session_registry::SessionState;
use std::cell::Cell;
use std::io;
use std::sync::{Arc, Mutex};

const GENERATION: u64 = 71;
const INTERNAL_LEASE: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Prepare,
    JournalPreflight,
    Intent,
    Spawn,
    Spawned,
    Applied,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct EffectCounts {
    validations: usize,
    reads: usize,
    inputs: usize,
    polls: usize,
    metadata: usize,
}

#[derive(Default)]
struct Sink {
    id: u64,
    validations: Cell<usize>,
    reads: usize,
    inputs: usize,
    polls: usize,
    screen_observations: Cell<usize>,
    fail_poll: bool,
    exited: bool,
    revision: Option<u64>,
    read_text: Option<String>,
    read_revision: Option<u64>,
    read_truncated: bool,
    fail_read: bool,
    read_bounds: Option<(usize, u16)>,
    revision_after_read: Option<u64>,
    shared_counts: Option<Arc<Mutex<EffectCounts>>>,
}

impl Sink {
    fn count(&self, update: impl FnOnce(&mut EffectCounts)) {
        if let Some(counts) = &self.shared_counts {
            update(&mut counts.lock().unwrap());
        }
    }
}

impl EffectSink for Sink {
    fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
        self.validations.set(self.validations.get() + 1);
        self.count(|counts| counts.validations += 1);
        Ok(())
    }

    fn read(
        &mut self,
        _source: ReadSource,
        max_text_bytes: usize,
        max_lines: u16,
    ) -> Result<ReadResult, Outcome> {
        self.reads += 1;
        self.count(|counts| counts.reads += 1);
        self.read_bounds = Some((max_text_bytes, max_lines));
        if self.fail_read {
            return Err(Outcome::Failed);
        }
        let revision = self.read_revision.unwrap_or(1);
        if let Some(after_read) = self.revision_after_read {
            self.revision = Some(after_read);
        }
        Ok(ReadResult {
            text: self
                .read_text
                .clone()
                .unwrap_or_else(|| format!("sink-{}", self.id)),
            revision,
            truncated: self.read_truncated,
        })
    }

    fn input(&mut self, _command: Command<'_>) -> Outcome {
        self.inputs += 1;
        self.count(|counts| counts.inputs += 1);
        Outcome::Applied
    }

    fn poll(&mut self) -> Result<(), Outcome> {
        self.polls += 1;
        self.count(|counts| counts.polls += 1);
        if self.fail_poll {
            Err(Outcome::Failed)
        } else {
            Ok(())
        }
    }

    fn child_exited(&self) -> bool {
        self.exited
    }

    fn screen_revision(&self) -> Option<u64> {
        self.screen_observations
            .set(self.screen_observations.get() + 1);
        self.count(|counts| counts.metadata += 1);
        self.revision
    }
}

struct Journal {
    events: Arc<Mutex<Vec<Event>>>,
    preflight_failure: bool,
    record_failure: Option<LaunchState>,
}

impl Journal {
    fn new(events: &Arc<Mutex<Vec<Event>>>) -> Self {
        Self {
            events: Arc::clone(events),
            preflight_failure: false,
            record_failure: None,
        }
    }
}

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _key: OperationKey, _session: u64) -> io::Result<()> {
        self.events.lock().unwrap().push(Event::JournalPreflight);
        if self.preflight_failure {
            Err(io::Error::other("private journal detail"))
        } else {
            Ok(())
        }
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        let event = match record.state {
            LaunchState::Intent => Event::Intent,
            LaunchState::Spawned => Event::Spawned,
            LaunchState::Applied => Event::Applied,
            LaunchState::Failed => Event::Failed,
            LaunchState::Unknown => Event::Unknown,
        };
        self.events.lock().unwrap().push(event);
        if self.record_failure == Some(record.state) {
            Err(io::Error::other("private journal detail"))
        } else {
            Ok(())
        }
    }
}

fn scoped_wheel_fixture() -> (MultiSessionOwner<Sink>, TargetRef, Connection) {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let mut greeting = hello();
    greeting.hello.optional_capabilities |=
        crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL;
    let (connection, welcome) = owner.connect(target, &greeting, 4).unwrap();
    assert_ne!(
        welcome.selected_capabilities & crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL,
        0
    );
    (owner, target, connection)
}

fn scoped_wheel_request(
    connection: Connection,
    target: TargetRef,
    sequence: u64,
) -> (crate::ui_protocol::ScopedMouseWheelInput, Vec<u8>) {
    let request = crate::ui_protocol::ScopedMouseWheelInput {
        key: key(connection.lease(), sequence),
        target,
        column: 3,
        row: 4,
        up: true,
    };
    let bytes = crate::ui_protocol::encode_scoped_mouse_wheel_input(&request)
        .unwrap()
        .to_vec();
    (request, bytes)
}

#[test]
fn scoped_mouse_wheel_applied_releases_and_duplicate_preserves_later_owner() {
    let (mut owner, target, connection) = scoped_wheel_fixture();
    let (request, bytes) = scoped_wheel_request(connection, target, 1);
    let result = owner
        .handle_scoped_mouse_wheel(
            &connection,
            &request,
            &bytes,
            5,
            |_| Ok(()),
            |sink| {
                sink.inputs += 1;
                Outcome::Applied
            },
        )
        .unwrap();
    assert_eq!(
        response(&result).body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let snapshot = owner
        .launches
        .session_mut(target)
        .unwrap()
        .input_owner
        .snapshot();
    assert_eq!(
        (snapshot.holder, snapshot.pending, snapshot.revision),
        (None, None, 3)
    );
    let retained = owner
        .launches
        .session_mut(target)
        .unwrap()
        .input_owner
        .claim(crate::input_owner::Owner::Human(99), 3, 30, 6)
        .unwrap();
    let duplicate = owner
        .handle_scoped_mouse_wheel(
            &connection,
            &request,
            &bytes,
            6,
            |_| panic!("duplicate revalidated mode"),
            |_| panic!("duplicate resent wheel"),
        )
        .unwrap();
    assert_eq!(duplicate, result);
    assert!(
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .snapshot()
            == retained
    );
    assert_eq!(owner.launches.effects(target).unwrap().effects.inputs, 1);
    let mut changed = request;
    changed.up = false;
    let changed_bytes = crate::ui_protocol::encode_scoped_mouse_wheel_input(&changed).unwrap();
    assert!(owner
        .handle_scoped_mouse_wheel(
            &connection,
            &changed,
            &changed_bytes,
            6,
            |_| panic!("conflict revalidated"),
            |_| panic!("conflict resent")
        )
        .is_err());
    assert!(
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .snapshot()
            == retained
    );
}

#[test]
fn scoped_mouse_wheel_non_applied_retains_scope_without_replaying() {
    for outcome in [
        Outcome::Failed,
        Outcome::Partial,
        Outcome::Unknown,
        Outcome::Accepted,
    ] {
        let (mut owner, target, connection) = scoped_wheel_fixture();
        let (request, bytes) = scoped_wheel_request(connection, target, 1);
        let result = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                5,
                |_| Ok(()),
                |sink| {
                    sink.inputs += 1;
                    outcome
                },
            )
            .unwrap();
        let expected = if outcome == Outcome::Accepted {
            Outcome::Unknown
        } else {
            outcome
        };
        assert_eq!(
            response(&result).body,
            ResponseBody::Status(OperationStatus::Completed(expected))
        );
        let snapshot = owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .snapshot();
        assert_eq!(
            snapshot.holder,
            Some(crate::input_owner::Owner::Client(connection.lease()))
        );
        assert_eq!(
            (snapshot.revision, snapshot.expires_secs, snapshot.pending),
            (2, 35, None)
        );
        let duplicate = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                6,
                |_| panic!("duplicate validated"),
                |_| panic!("duplicate effect"),
            )
            .unwrap();
        assert_eq!(duplicate, result);
        let (next, next_bytes) = scoped_wheel_request(connection, target, 2);
        let busy = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &next,
                &next_bytes,
                6,
                |_| Ok(()),
                |_| panic!("same caller stole retained scope"),
            )
            .unwrap();
        assert_eq!(
            response(&busy).body,
            ResponseBody::Rejected(RejectionCode::InputBusy)
        );
        assert!(
            owner
                .launches
                .session_mut(target)
                .unwrap()
                .input_owner
                .snapshot()
                == snapshot
        );
        assert_eq!(owner.launches.effects(target).unwrap().effects.inputs, 1);
        assert_eq!(
            owner.global.ledger.status(next.key, 6).unwrap(),
            OperationStatus::NotAdmitted
        );
    }
}

#[test]
fn scoped_mouse_wheel_mode_rejection_precedes_retained_ownership() {
    for same_caller in [false, true] {
        let (mut owner, target, connection) = scoped_wheel_fixture();
        let holder = if same_caller {
            crate::input_owner::Owner::Client(connection.lease())
        } else {
            crate::input_owner::Owner::Human(55)
        };
        let snapshot = owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .claim(holder, 1, 30, 5)
            .unwrap();
        let (request, bytes) = scoped_wheel_request(connection, target, 1);
        let result = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                6,
                |_| Err(RejectionCode::Unsupported),
                |_| panic!("unsupported wheel delivered"),
            )
            .unwrap();
        assert_eq!(
            response(&result).body,
            ResponseBody::Rejected(RejectionCode::Unsupported)
        );
        assert!(
            owner
                .launches
                .session_mut(target)
                .unwrap()
                .input_owner
                .snapshot()
                == snapshot
        );
        assert_eq!(
            owner.global.ledger.status(request.key, 6).unwrap(),
            OperationStatus::NotAdmitted
        );
    }
}

#[test]
fn scoped_mouse_wheel_busy_owners_and_pending_are_never_stolen() {
    for case in 0..4 {
        let (mut owner, target, connection) = scoped_wheel_fixture();
        let session = owner.launches.session_mut(target).unwrap();
        let holder = if case == 0 {
            crate::input_owner::Owner::Client(connection.lease())
        } else {
            crate::input_owner::Owner::Human(77)
        };
        let claimed = session.input_owner.claim(holder, 1, 1, 5).unwrap();
        if case == 2 {
            session
                .input_owner
                .begin_input(holder, claimed.revision, key(connection.lease(), 99), 5)
                .unwrap();
        }
        let before = session.input_owner.snapshot();
        let (request, bytes) = scoped_wheel_request(connection, target, 1);
        let now = if case == 3 { 7 } else { 5 };
        let result = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                now,
                |_| Ok(()),
                |_| panic!("busy input delivered"),
            )
            .unwrap();
        assert_eq!(
            response(&result).body,
            ResponseBody::Rejected(RejectionCode::InputBusy)
        );
        assert!(
            owner
                .launches
                .session_mut(target)
                .unwrap()
                .input_owner
                .snapshot()
                == before
        );
    }
}

#[test]
fn scoped_mouse_wheel_missing_capabilities_and_full_ledger_have_no_effect() {
    for missing in [
        crate::handshake_protocol::CAPABILITY_INPUT,
        crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL,
    ] {
        let (mut owner, target, mut connection) = scoped_wheel_fixture();
        connection.selected_capabilities &= !missing;
        let (request, bytes) = scoped_wheel_request(connection, target, 1);
        let result = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                5,
                |_| panic!("unauthorized validation"),
                |_| panic!("unauthorized effect"),
            )
            .unwrap();
        assert_eq!(
            response(&result).body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner
                .launches
                .session_mut(target)
                .unwrap()
                .input_owner
                .snapshot()
                .revision,
            1
        );
    }
    let (mut owner, target, connection) = scoped_wheel_fixture();
    for sequence in 1..=crate::protocol::MAX_UNRECEIPTED_OPERATIONS as u64 {
        let key = key(connection.lease(), sequence);
        owner.global.ledger.admit(key, b"fixture", 5).unwrap();
        owner.global.ledger.complete(key, Outcome::Applied).unwrap();
    }
    let (request, bytes) = scoped_wheel_request(
        connection,
        target,
        crate::protocol::MAX_UNRECEIPTED_OPERATIONS as u64 + 1,
    );
    let result = owner
        .handle_scoped_mouse_wheel(
            &connection,
            &request,
            &bytes,
            5,
            |_| Ok(()),
            |_| panic!("capacity effect"),
        )
        .unwrap();
    assert_eq!(
        response(&result).body,
        ResponseBody::Rejected(RejectionCode::Capacity)
    );
    let snapshot = owner
        .launches
        .session_mut(target)
        .unwrap()
        .input_owner
        .snapshot();
    assert_eq!(
        (snapshot.revision, snapshot.holder, snapshot.pending),
        (1, None, None)
    );
}

#[test]
fn scoped_mouse_wheel_revision_release_capacity_is_prepared_before_delivery() {
    for revision in [u64::MAX - 2, u64::MAX - 1, u64::MAX] {
        let (mut owner, target, connection) = scoped_wheel_fixture();
        owner.launches.session_mut(target).unwrap().input_owner =
            crate::input_owner::InputOwner::vacant_at_revision_for_test(revision);
        let (request, bytes) = scoped_wheel_request(connection, target, 1);
        let calls = Cell::new(0);
        let result = owner
            .handle_scoped_mouse_wheel(
                &connection,
                &request,
                &bytes,
                5,
                |_| Ok(()),
                |_| {
                    calls.set(calls.get() + 1);
                    Outcome::Applied
                },
            )
            .unwrap();
        let snapshot = owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .snapshot();
        assert_eq!((snapshot.holder, snapshot.pending), (None, None));
        if revision == u64::MAX - 2 {
            assert_eq!(
                response(&result).body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
            );
            assert_eq!((snapshot.revision, calls.get()), (u64::MAX, 1));
        } else {
            assert_eq!(
                response(&result).body,
                ResponseBody::Rejected(RejectionCode::InvalidRequest)
            );
            assert_eq!((snapshot.revision, calls.get()), (revision, 0));
            assert_eq!(
                owner.global.ledger.status(request.key, 5).unwrap(),
                OperationStatus::NotAdmitted
            );
        }
    }
}

#[test]
fn scoped_mouse_wheel_grants_require_input_and_resume_never_widens() {
    let (mut owner, target, scoped) = scoped_wheel_fixture();
    let bit = crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL;
    let mut greeting = hello();
    let (old, old_welcome) = owner.connect(target, &greeting, 5).unwrap();
    assert_eq!(old_welcome.selected_capabilities & bit, 0);
    greeting.hello.optional_capabilities |= bit;
    greeting.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: old.lease(),
    });
    let (_, resumed) = owner.connect(target, &greeting, 6).unwrap();
    assert_eq!(resumed.selected_capabilities & bit, 0);
    greeting.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: scoped.lease(),
    });
    let (_, resumed) = owner.connect(target, &greeting, 6).unwrap();
    assert_ne!(resumed.selected_capabilities & bit, 0);
    greeting.resume = None;
    let session = owner.launches.session_mut(target).unwrap();
    let (_, single) = owner.global.connect(session, &greeting, 6).unwrap();
    assert_eq!(single.selected_capabilities & bit, 0);
    owner.global.allow_input = false;
    let (_, denied) = owner.connect(target, &greeting, 6).unwrap();
    assert_eq!(
        denied.selected_capabilities & (bit | crate::handshake_protocol::CAPABILITY_INPUT),
        0
    );
}

fn owner() -> MultiSessionOwner<Sink> {
    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(INTERNAL_LEASE, 1).unwrap();
    MultiSessionOwner::with_ledger(ledger, true).unwrap()
}

fn key(lease: u64, sequence: u64) -> OperationKey {
    OperationKey {
        generation: GENERATION,
        lease,
        sequence,
    }
}

fn launch_payload(key: OperationKey, executable: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    encode_launch_request(
        &LaunchRequest {
            key,
            rows: 24,
            columns: 80,
            requested_executable: executable,
            requested_working_directory: b"/",
            client_working_directory: b"/",
            arguments: &[],
            environment: &[],
        },
        &mut payload,
    )
    .unwrap();
    payload
}

fn launch(
    owner: &mut MultiSessionOwner<Sink>,
    sequence: u64,
    journal: &mut Journal,
    events: &Arc<Mutex<Vec<Event>>>,
) -> Result<crate::managed_launch_protocol::LaunchResult, ManagedLaunchError> {
    owner.execute_launch(
        &launch_payload(key(INTERNAL_LEASE, sequence), b"/bin/sh"),
        2,
        journal,
        |_, target| {
            events.lock().unwrap().push(Event::Prepare);
            Ok::<_, ()>(target.session)
        },
        |id| {
            events.lock().unwrap().push(Event::Spawn);
            Ok::<_, ()>(Sink {
                id,
                revision: Some(1),
                ..Sink::default()
            })
        },
    )
}

fn hello() -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce: 17,
            required_capabilities: crate::handshake_protocol::CAPABILITY_READ,
            optional_capabilities: crate::handshake_protocol::CAPABILITY_INPUT | CAPABILITY_EVENTS,
        },
        resume: None,
    }
}

fn control_payload(key: OperationKey, target: TargetRef, command: Command<'_>) -> Vec<u8> {
    let request = ControlRequest {
        key,
        target,
        command,
    };
    let mut bytes = Vec::new();
    encode_request_payload(&request, &mut bytes).unwrap();
    bytes
}

fn response(payload: &[u8]) -> crate::response_protocol::Response<'_> {
    decode_response_payload(payload).unwrap()
}

fn session_payload(request: SessionRequest) -> Vec<u8> {
    crate::session_protocol::encode_session_request(&request).unwrap()
}

fn session_result(reply: SessionReply) -> Vec<u8> {
    match reply {
        SessionReply::Payload(payload) => payload,
        other => panic!("expected encoded operation result, got {other:?}"),
    }
}

fn session_info(reply: SessionReply) -> SessionInfo {
    match reply {
        SessionReply::Info(info) => info,
        other => panic!("expected session info, got {other:?}"),
    }
}

fn workspace_hello() -> SessionHello {
    SessionHello {
        hello: Hello {
            required_capabilities: CAPABILITY_READ
                | crate::handshake_protocol::CAPABILITY_WORKSPACE_EVENTS,
            optional_capabilities: 0,
            ..hello().hello
        },
        resume: None,
    }
}

#[test]
fn workspace_snapshot_cursor_tracks_commits_and_terminal_empty_without_effect_reads() {
    use crate::watch_protocol::WatchStatus;
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &workspace_hello(), 3).unwrap();
    let state = owner
        .reconcile_workspace(&connection, GENERATION, target.workspace, 3)
        .unwrap();
    assert_eq!((state.cursor.sequence, state.revision), (1, 1));
    assert_eq!(state.layout.unwrap().panes[0].unwrap().target, target);
    let before = owner.launches.session_mut(target).unwrap().effects.polls;
    owner
        .resize_layout(1, 24, 80, |_: &mut Sink, _, _| Ok::<_, ()>(()))
        .unwrap();
    assert!(owner
        .resize_layout(1, 1, 1, |_: &mut Sink, _, _| Ok::<_, ()>(()))
        .is_err());
    assert_eq!(
        owner
            .read_workspace_events(&connection, state.cursor, 64, 3)
            .unwrap()
            .status,
        WatchStatus::TimedOut
    );
    owner
        .resize_layout(1, 25, 80, |_: &mut Sink, _, _| Ok::<_, ()>(()))
        .unwrap();
    let page = owner
        .read_workspace_events(&connection, state.cursor, 64, 3)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(
        (
            page.events[0].sequence,
            page.events[0].revision,
            page.events[0].empty
        ),
        (2, 2, false)
    );
    let updated = owner
        .reconcile_workspace(&connection, GENERATION, target.workspace, 3)
        .unwrap();
    assert_eq!(updated.cursor, page.cursor);
    assert_eq!(updated.layout.unwrap().revision, page.events[0].revision);
    assert_eq!(
        owner.launches.session_mut(target).unwrap().effects.polls,
        before
    );
    assert_eq!(
        owner.launches.session_mut(target).unwrap().effects.inputs,
        0
    );
    owner.launches.session_mut(target).unwrap().effects.exited = true;
    owner.poll(4).unwrap();
    let terminal = owner
        .reconcile_workspace(&connection, GENERATION, target.workspace, 4)
        .unwrap();
    assert!(terminal.layout.is_none());
    assert_eq!((terminal.revision, terminal.cursor.sequence), (2, 3));
    owner.poll(4).unwrap();
    assert_eq!(
        owner
            .read_workspace_events(&connection, terminal.cursor, 64, 4)
            .unwrap()
            .status,
        WatchStatus::TimedOut
    );
}

#[test]
fn workspace_grants_are_explicit_scoped_and_resume_never_widens_them() {
    use crate::handshake_protocol::CAPABILITY_WORKSPACE_EVENTS;
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();
    let (old, _) = owner.connect(target, &hello(), 3).unwrap();
    assert!(matches!(
        owner.reconcile_workspace(&old, GENERATION, target.workspace, 3),
        Err(ServerError::Unauthorized)
    ));
    let mut resume = hello();
    resume.hello.optional_capabilities |= CAPABILITY_WORKSPACE_EVENTS;
    resume.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: old.lease(),
    });
    let (_, welcome) = owner.connect(target, &resume, 4).unwrap();
    assert_eq!(
        welcome.selected_capabilities & CAPABILITY_WORKSPACE_EVENTS,
        0
    );
    let (connection, _) = owner.connect(target, &workspace_hello(), 4).unwrap();
    assert!(matches!(
        owner.reconcile_workspace(&connection, GENERATION + 1, target.workspace, 4),
        Err(ServerError::WrongGeneration)
    ));
    assert!(matches!(
        owner.reconcile_workspace(&connection, GENERATION, target.workspace + 1, 4),
        Err(ServerError::Unauthorized)
    ));
    assert!(matches!(
        owner.reconcile_workspace(
            &connection,
            GENERATION,
            target.workspace,
            4 + LEASE_TTL_SECS
        ),
        Err(ServerError::UnknownContext)
    ));
}

#[test]
fn retired_pane_workspace_observer_renews_same_lease_across_two_ttl_cycles() {
    use crate::handshake_protocol::CAPABILITY_WORKSPACE_EVENTS;
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();
    let (original, welcome) = owner.connect(target, &workspace_hello(), 3).unwrap();
    assert_eq!(
        welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_WORKSPACE_EVENTS
    );

    owner.launches.session_mut(target).unwrap().effects.exited = true;
    owner.poll(4).unwrap();
    assert!(owner.layout_terminal_empty());
    drop(owner.launches.retire(target).unwrap());

    let resume = |lease| SessionHello {
        resume: Some(crate::session_protocol::LeaseRef {
            generation: GENERATION,
            lease,
        }),
        ..workspace_hello()
    };
    let (first, first_welcome) = owner
        .connect(target, &resume(original.lease()), 1_802)
        .unwrap();
    assert_eq!(first.lease(), original.lease());
    assert_eq!(
        first.selected_capabilities(),
        CAPABILITY_READ | CAPABILITY_WORKSPACE_EVENTS
    );
    assert_eq!(
        first_welcome.selected_capabilities,
        first.selected_capabilities()
    );
    assert!(owner
        .reconcile_workspace(&first, GENERATION, target.workspace, 1_802)
        .is_ok());

    let (second, second_welcome) = owner
        .connect(target, &resume(original.lease()), 3_601)
        .unwrap();
    assert_eq!(second.lease(), original.lease());
    assert_eq!(
        second.selected_capabilities(),
        first.selected_capabilities()
    );
    assert_eq!(second_welcome.lease, original.lease());
    assert!(owner
        .reconcile_workspace(&second, GENERATION, target.workspace, 3_601)
        .is_ok());
    assert_eq!(owner.lease_target(original.lease()), Some(target));

    assert!(matches!(
        owner.connect(target, &resume(original.lease()), 5_401),
        Err(ServerError::UnknownContext)
    ));
    let wrong_generation = SessionHello {
        resume: Some(crate::session_protocol::LeaseRef {
            generation: GENERATION + 1,
            lease: original.lease(),
        }),
        ..workspace_hello()
    };
    assert!(matches!(
        owner.connect(target, &wrong_generation, 4_000),
        Err(ServerError::UnknownContext)
    ));
    let widened = SessionHello {
        hello: Hello {
            optional_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
            ..workspace_hello().hello
        },
        resume: Some(crate::session_protocol::LeaseRef {
            generation: GENERATION,
            lease: original.lease(),
        }),
    };
    assert!(matches!(
        owner.connect(target, &widened, 4_000),
        Err(ServerError::UnknownContext)
    ));
}

#[test]
fn server_identity_requires_negotiated_read_only_capability() {
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();

    let (legacy, _) = owner.connect(target, &hello(), 3).unwrap();
    assert!(matches!(
        owner.build_identity(&legacy, 3),
        Err(ServerError::Unauthorized)
    ));

    let mut identity_hello = hello();
    identity_hello.hello.optional_capabilities |=
        crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY;
    let (connection, welcome) = owner.connect(target, &identity_hello, 4).unwrap();
    assert_ne!(
        welcome.selected_capabilities & crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY,
        0
    );
    let identity = owner.build_identity(&connection, 4).unwrap();
    assert_eq!(identity.generation, GENERATION);
    assert_eq!(
        identity.selected_capabilities,
        welcome.selected_capabilities
    );
    assert!(identity.to_json().contains("\"build_identity\""));
}

#[test]
fn metadata_query_requires_exact_read_capability_and_has_no_child_effects() {
    use crate::handshake_protocol::CAPABILITY_PANE_METADATA;
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();
    let read_only = hello();
    let (read_connection, _) = owner.connect(target, &read_only, 3).unwrap();
    let payload = crate::pane_metadata_protocol::encode_query(target).unwrap();
    assert!(matches!(
        owner.authorize_metadata_query(&read_connection, &payload, 3),
        Err(ServerError::Unauthorized)
    ));

    let metadata_hello = SessionHello {
        hello: Hello {
            optional_capabilities: CAPABILITY_PANE_METADATA,
            ..hello().hello
        },
        resume: None,
    };
    let (metadata_connection, welcome) = owner.connect(target, &metadata_hello, 4).unwrap();
    assert_ne!(welcome.selected_capabilities & CAPABILITY_PANE_METADATA, 0);
    let before = {
        let sink = &owner.launches.session_mut(target).unwrap().effects;
        (
            sink.reads,
            sink.inputs,
            sink.polls,
            sink.screen_observations.get(),
        )
    };
    let (observed_target, identity, status) = owner
        .authorize_metadata_query(&metadata_connection, &payload, 4)
        .unwrap();
    assert_eq!(observed_target, target);
    assert_eq!(identity, None);
    assert_eq!(
        status,
        crate::pane_metadata_protocol::PathStatus::Unsupported
    );
    if let Some(other_identity) = crate::process_metadata::capture_identity(std::process::id()) {
        assert!(!owner.metadata_publish_allowed(&metadata_connection, target, other_identity, 4,));
    }

    let mut other_target = target;
    other_target.pane += 1;
    let wrong_payload = crate::pane_metadata_protocol::encode_query(other_target).unwrap();
    assert!(matches!(
        owner.authorize_metadata_query(&metadata_connection, &wrong_payload, 4),
        Err(ServerError::Unauthorized)
    ));
    let sink = &owner.launches.session_mut(target).unwrap().effects;
    assert_eq!(
        before,
        (
            sink.reads,
            sink.inputs,
            sink.polls,
            sink.screen_observations.get()
        )
    );
    owner.launches.session_mut(target).unwrap().effects.exited = true;
    assert_eq!(
        owner
            .authorize_metadata_query(&metadata_connection, &payload, 5)
            .unwrap()
            .2,
        crate::pane_metadata_protocol::PathStatus::Exited
    );
}

#[test]
fn row_wrap_capability_is_optional_and_resume_cannot_widen_an_old_grant() {
    use crate::handshake_protocol::CAPABILITY_SCREEN_ROW_WRAPS;

    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();

    let legacy = owner.connect(target, &hello(), 3).unwrap().0;
    let mut widened_resume = hello();
    widened_resume.hello.optional_capabilities |= CAPABILITY_SCREEN_ROW_WRAPS;
    widened_resume.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: legacy.lease(),
    });
    let (resumed, resumed_welcome) = owner.connect(target, &widened_resume, 4).unwrap();
    assert_eq!(resumed.lease(), legacy.lease());
    assert_eq!(
        resumed_welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
        0
    );

    let mut fresh = hello();
    fresh.hello.optional_capabilities |= CAPABILITY_SCREEN_ROW_WRAPS;
    let (fresh_connection, fresh_welcome) = owner.connect(target, &fresh, 5).unwrap();
    assert_ne!(
        fresh_welcome.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
        0
    );
    let mut fresh_resume = fresh;
    fresh_resume.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: fresh_connection.lease(),
    });
    let (_, resumed_fresh) = owner.connect(target, &fresh_resume, 6).unwrap();
    assert_ne!(
        resumed_fresh.selected_capabilities & CAPABILITY_SCREEN_ROW_WRAPS,
        0
    );
}

#[test]
fn history_cursor_capability_is_optional_and_resume_cannot_widen_an_old_grant() {
    use crate::handshake_protocol::CAPABILITY_HISTORY_CURSOR;

    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();

    let legacy = owner.connect(target, &hello(), 3).unwrap().0;
    let mut widened_resume = hello();
    widened_resume.hello.optional_capabilities |= CAPABILITY_HISTORY_CURSOR;
    widened_resume.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: legacy.lease(),
    });
    let (resumed, resumed_welcome) = owner.connect(target, &widened_resume, 4).unwrap();
    assert_eq!(resumed.lease(), legacy.lease());
    assert_eq!(
        resumed_welcome.selected_capabilities & CAPABILITY_HISTORY_CURSOR,
        0
    );

    let mut fresh = hello();
    fresh.hello.optional_capabilities |= CAPABILITY_HISTORY_CURSOR;
    let (fresh_connection, fresh_welcome) = owner.connect(target, &fresh, 5).unwrap();
    assert_ne!(
        fresh_welcome.selected_capabilities & CAPABILITY_HISTORY_CURSOR,
        0
    );
    let mut fresh_resume = fresh;
    fresh_resume.resume = Some(crate::session_protocol::LeaseRef {
        generation: GENERATION,
        lease: fresh_connection.lease(),
    });
    let (_, resumed_fresh) = owner.connect(target, &fresh_resume, 6).unwrap();
    assert_ne!(
        resumed_fresh.selected_capabilities & CAPABILITY_HISTORY_CURSOR,
        0
    );
}

#[test]
fn workspace_rollover_requires_snapshot_and_future_cursor_is_refused() {
    use crate::watch_protocol::WatchStatus;
    let mut owner = owner();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&effects);
    let target = launch(&mut owner, 1, &mut journal, &effects)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &workspace_hello(), 3).unwrap();
    let initial = owner
        .reconcile_workspace(&connection, GENERATION, target.workspace, 3)
        .unwrap();
    for offset in 0..66 {
        let revision = owner.layout_snapshot().unwrap().revision;
        owner
            .resize_layout(revision, 25 + offset % 2, 80, |_: &mut Sink, _, _| {
                Ok::<_, ()>(())
            })
            .unwrap();
    }
    let expired = owner
        .read_workspace_events(&connection, initial.cursor, 64, 3)
        .unwrap();
    assert_eq!(expired.status, WatchStatus::ResyncRequired);
    assert!(expired.events.is_empty());
    let snapshot = owner
        .reconcile_workspace(&connection, GENERATION, target.workspace, 3)
        .unwrap();
    assert_eq!(snapshot.cursor, expired.cursor);
    assert_eq!(snapshot.layout.unwrap().revision, snapshot.revision);
    let future = crate::workspace_watch_protocol::WorkspaceCursor {
        sequence: snapshot.cursor.sequence + 1,
        ..snapshot.cursor
    };
    assert!(matches!(
        owner.read_workspace_events(&connection, future, 64, 3),
        Err(ServerError::CursorAhead)
    ));
    assert!(matches!(
        owner.read_workspace_events(&connection, snapshot.cursor, 0, 3),
        Err(ServerError::InvalidWatchRequest)
    ));
    owner.workspace_events_failed = true;
    assert!(matches!(
        owner.reconcile_workspace(&connection, GENERATION, target.workspace, 3),
        Err(ServerError::NotificationsUnavailable)
    ));
    assert!(owner.layout_view(&connection, 3).unwrap().is_some());
    assert!(!owner.all_sessions_ended());
}

#[test]
fn preparation_failures_do_not_spend_identity_but_intent_failure_does() {
    let mut first_owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let failed_prepare = first_owner.execute_launch(
        &launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh"),
        2,
        &mut journal,
        |_, _| Err::<u64, _>("private prepare detail"),
        |_| Ok::<_, ()>(Sink::default()),
    );
    assert!(matches!(
        failed_prepare,
        Err(ManagedLaunchError::PreparationFailed)
    ));
    assert!(first_owner.launches.records().next().is_none());

    journal.preflight_failure = true;
    assert!(matches!(
        launch(&mut first_owner, 1, &mut journal, &events),
        Err(ManagedLaunchError::JournalPreflightFailed)
    ));
    assert!(first_owner.launches.records().next().is_none());

    journal.preflight_failure = false;
    journal.record_failure = Some(LaunchState::Intent);
    let result = launch(&mut first_owner, 1, &mut journal, &events).unwrap();
    let target = result.target.unwrap();
    assert_eq!(target.session, 1);
    assert_eq!(result.outcome, Outcome::Failed);
    assert_eq!(first_owner.launches.state(target), Ok(SessionState::Failed));
    assert_eq!(first_owner.launches.records().count(), 1);

    // A journal failure after admission spends the identity and fails
    // closed; it does not permit a later launch through this coordinator.
    assert!(matches!(
        launch(&mut first_owner, 2, &mut journal, &events),
        Err(ManagedLaunchError::Unhealthy)
    ));

    // A spawn failure also spends its slot, but remains recoverable when
    // the journal itself is healthy.
    let mut spawn_owner = owner();
    journal.record_failure = None;
    let spawn_failed = spawn_owner
        .execute_launch(
            &launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh"),
            2,
            &mut journal,
            |_, target| Ok::<_, ()>(target.session),
            |_| Err::<Sink, _>("private spawn detail"),
        )
        .unwrap();
    assert_eq!(spawn_failed.outcome, Outcome::Failed);
    assert_eq!(spawn_failed.target.unwrap().session, 1);
    journal.record_failure = None;
    let second = launch(&mut spawn_owner, 2, &mut journal, &events).unwrap();
    assert_eq!(second.target.unwrap().session, 2);
    for sequence in 3..=crate::session_registry::SESSION_CAPACITY as u64 {
        let result = launch(&mut spawn_owner, sequence, &mut journal, &events).unwrap();
        assert_eq!(result.target.unwrap().session, sequence);
    }
    assert!(matches!(
        launch(
            &mut spawn_owner,
            crate::session_registry::SESSION_CAPACITY as u64 + 1,
            &mut journal,
            &events
        ),
        Err(ManagedLaunchError::Registry(
            crate::session_registry::RegistryError::Capacity
        ))
    ));
    assert_eq!(
        spawn_owner.launches.records().count(),
        crate::session_registry::SESSION_CAPACITY
    );
}

#[test]
fn launch_orders_seed_prepare_spawn_and_retains_handles_after_unknown_result() {
    for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        journal.record_failure = Some(failed_state);
        let result = launch(&mut owner, 1, &mut journal, &events).unwrap();
        let target = result.target.unwrap();
        assert_eq!(result.outcome, Outcome::Unknown);
        let mut expected = vec![
            Event::Prepare,
            Event::JournalPreflight,
            Event::Intent,
            Event::Spawn,
            Event::Spawned,
        ];
        if failed_state == LaunchState::Applied {
            expected.push(Event::Applied);
        }
        assert_eq!(*events.lock().unwrap(), expected);
        assert_eq!(owner.launches.state(target), Ok(SessionState::Unknown));
        let session = owner.launches.effects(target).unwrap();
        assert_eq!(session.effects.id, target.session);
        assert_eq!(session.effects.polls, 0);

        // The owner can continue polling and route a fresh public read to
        // the sink retained before either post-spawn journal fault.
        owner.poll(3).unwrap();
        assert_eq!(owner.launches.effects(target).unwrap().effects.polls, 1);
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let read = control_payload(
            key(connection.lease(), 1),
            target,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert!(matches!(
            response(&owner.handle_control(&connection, &read, 5).unwrap()).body,
            ResponseBody::Read { target: read_target, .. } if read_target == target
        ));
        assert_eq!(owner.launches.effects(target).unwrap().effects.reads, 1);
        assert!(matches!(
            format!("{owner:?}"),
            rendered if !rendered.contains("/bin/sh") && !rendered.contains("private journal")
        ));
    }
}

#[test]
fn scoped_events_are_advertised_only_when_available_and_never_added_to_resumed_grants() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    {
        let session = owner.launches.session_mut(target).unwrap();
        session.notifications_available = false;
        session.effects.revision = None;
    }

    let optional_events = SessionHello {
        hello: Hello {
            nonce: 18,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: crate::handshake_protocol::CAPABILITY_INPUT | CAPABILITY_EVENTS,
        },
        resume: None,
    };
    let (connection, welcome) = owner.connect(target, &optional_events, 4).unwrap();
    assert_eq!(connection.lease(), 2);
    assert_eq!(
        welcome.selected_capabilities,
        CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_INPUT
    );

    let required_events = SessionHello {
        hello: Hello {
            nonce: 19,
            required_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
            optional_capabilities: 0,
        },
        resume: None,
    };
    assert!(matches!(
        owner.connect(target, &required_events, 4),
        Err(ServerError::Handshake(_))
    ));
    assert_eq!(owner.global.grants.len(), 1);

    {
        let session = owner.launches.session_mut(target).unwrap();
        session.notifications_available = true;
        session.effects.revision = Some(1);
    }
    let resume = SessionHello {
        resume: Some(crate::session_protocol::LeaseRef {
            generation: GENERATION,
            lease: connection.lease(),
        }),
        ..optional_events
    };
    let (_, resumed_welcome) = owner.connect(target, &resume, 5).unwrap();
    assert_eq!(
        resumed_welcome.selected_capabilities,
        CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_INPUT
    );
    let (event_connection, event_welcome) = owner
        .connect(
            target,
            &SessionHello {
                hello: Hello {
                    nonce: 21,
                    required_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
                    optional_capabilities: 0,
                },
                resume: None,
            },
            6,
        )
        .unwrap();
    assert_eq!(event_connection.lease(), 3);
    assert_eq!(
        event_welcome.selected_capabilities,
        CAPABILITY_READ | CAPABILITY_EVENTS
    );
    assert_eq!(owner.global.grants.len(), 2);
}

#[test]
fn global_launch_dedup_connect_scope_and_control_dispatch_share_authority() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let first_payload = launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh");
    let first = owner
        .execute_launch(
            &first_payload,
            2,
            &mut journal,
            |_, target| Ok::<_, ()>(target.session),
            |id| {
                Ok::<_, ()>(Sink {
                    id,
                    revision: Some(1),
                    ..Sink::default()
                })
            },
        )
        .unwrap();
    let target_one = first.target.unwrap();
    let events_before_duplicate = events.lock().unwrap().len();
    let duplicate = owner
        .execute_launch(
            &first_payload,
            3,
            &mut journal,
            |_, _| -> Result<u64, ()> { panic!("exact launch retry must not prepare") },
            |_: u64| -> Result<Sink, ()> { panic!("exact launch retry must not spawn") },
        )
        .unwrap();
    assert_eq!(duplicate, first);
    assert_eq!(events.lock().unwrap().len(), events_before_duplicate);

    let changed = launch_payload(key(INTERNAL_LEASE, 1), b"/bin/zsh");
    assert!(matches!(
        owner.execute_launch(
            &changed,
            3,
            &mut journal,
            |_, _| Ok::<_, ()>(()),
            |_| Ok::<_, ()>(Sink::default()),
        ),
        Err(ManagedLaunchError::Ledger(ProtocolError::OperationConflict))
    ));

    // A second key is a distinct target and lease while the ledger remains shared.
    let second = owner
        .execute_launch(
            &launch_payload(key(INTERNAL_LEASE, 2), b"/bin/sh"),
            3,
            &mut journal,
            |_, target| Ok::<_, ()>(target.session),
            |id| {
                Ok::<_, ()>(Sink {
                    id,
                    revision: Some(1),
                    ..Sink::default()
                })
            },
        )
        .unwrap();
    let target_two = second.target.unwrap();
    assert_eq!(target_one.session, 1);
    assert_eq!(target_two.session, 2);
    assert_ne!(target_one, target_two);

    let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
    let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();
    assert_eq!(connection_two.lease(), connection_one.lease() + 1);
    let original_expiry = owner.global.grants[&connection_one.lease()].expires_secs;
    let resumed_other = SessionHello {
        resume: Some(crate::session_protocol::LeaseRef {
            generation: GENERATION,
            lease: connection_one.lease(),
        }),
        ..hello()
    };
    assert!(matches!(
        owner.connect(target_two, &resumed_other, 5),
        Err(ServerError::UnknownContext)
    ));
    assert_eq!(
        owner.global.grants[&connection_one.lease()].expires_secs,
        original_expiry
    );

    let denied_key = key(connection_one.lease(), 1);
    let denied = control_payload(
        denied_key,
        target_two,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    assert_eq!(
        response(&owner.handle_control(&connection_one, &denied, 5).unwrap()).body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(
        owner.global.ledger.status(denied_key, 5).unwrap(),
        OperationStatus::NotAdmitted
    );
    assert_eq!(owner.launches.effects(target_two).unwrap().effects.reads, 0);

    // Input ownership is staged only inside this trusted fixture; the new
    // owner intentionally has no public Claim/Release/Handoff route yet.
    let session_one = owner.launches.session_mut(target_one).unwrap();
    session_one
        .input_owner
        .claim(
            crate::input_owner::Owner::Client(connection_one.lease()),
            1,
            30,
            5,
        )
        .unwrap();
    let read_key = key(connection_one.lease(), 2);
    let read = control_payload(
        read_key,
        target_one,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    assert!(matches!(
        response(&owner.handle_control(&connection_one, &read, 6).unwrap()).body,
        ResponseBody::Read { target, .. } if target == target_one
    ));
    let input_key = key(connection_one.lease(), 3);
    let input = control_payload(
        input_key,
        target_one,
        Command::Submit {
            owner_revision: owner
                .launches
                .session_mut(target_one)
                .unwrap()
                .input_owner
                .snapshot()
                .revision,
            text: "accepted",
        },
    );
    assert_eq!(
        response(&owner.handle_control(&connection_one, &input, 6).unwrap()).body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        owner.launches.effects(target_one).unwrap().effects.inputs,
        1
    );
    assert_eq!(
        owner.launches.effects(target_two).unwrap().effects.inputs,
        0
    );
}

#[test]
fn global_control_preflight_short_circuits_before_session_lookup() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_one = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_two = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
    let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();

    let screen_before = owner
        .launches
        .effects(target_one)
        .unwrap()
        .effects
        .screen_observations
        .get();
    let wrong_lease = control_payload(
        key(connection_two.lease(), 9),
        target_one,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    assert_eq!(
        response(
            &owner
                .handle_control(&connection_one, &wrong_lease, 5)
                .unwrap()
        )
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(
        owner
            .launches
            .effects(target_one)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        screen_before
    );

    let global_key = key(connection_one.lease(), 10);
    let exact_payload = control_payload(
        global_key,
        target_one,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    owner
        .handle_control(&connection_one, &exact_payload, 5)
        .unwrap();
    let reads = owner.launches.effects(target_one).unwrap().effects.reads;
    let screens = owner
        .launches
        .effects(target_one)
        .unwrap()
        .effects
        .screen_observations
        .get();
    assert_eq!(
        response(
            &owner
                .handle_control(&connection_one, &exact_payload, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        owner.launches.effects(target_one).unwrap().effects.reads,
        reads
    );
    assert_eq!(
        owner
            .launches
            .effects(target_one)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        screens
    );

    let changed_payload = control_payload(
        global_key,
        target_two,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 257,
            max_lines: 1,
        },
    );
    assert!(matches!(
        owner.handle_control(&connection_one, &changed_payload, 6),
        Err(ServerError::Protocol(ProtocolError::OperationConflict))
    ));
    assert_eq!(owner.launches.effects(target_two).unwrap().effects.reads, 0);
    assert_eq!(
        owner
            .launches
            .effects(target_two)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        1,
        "the target seed is the only metadata sample before lookup"
    );
}

#[test]
fn cached_control_results_and_conflicts_precede_an_empty_target_registry() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    let completed_key = key(connection.lease(), 1);
    let exact_payload = control_payload(
        completed_key,
        target,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    owner
        .handle_control(&connection, &exact_payload, 5)
        .unwrap();

    // Simulate losing every in-memory target record while the one global
    // ledger remains authoritative for cached operation identities.
    owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
    assert_eq!(
        response(
            &owner
                .handle_control(&connection, &exact_payload, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );

    let changed_target = TargetRef {
        pane: target.pane + 1,
        ..target
    };
    let changed_payload = control_payload(
        completed_key,
        changed_target,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 257,
            max_lines: 1,
        },
    );
    assert!(matches!(
        owner.handle_control(&connection, &changed_payload, 6),
        Err(ServerError::Protocol(ProtocolError::OperationConflict))
    ));

    let fresh_key = key(connection.lease(), 2);
    let fresh_payload = control_payload(
        fresh_key,
        target,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    assert_eq!(
        response(
            &owner
                .handle_control(&connection, &fresh_payload, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Rejected(RejectionCode::StaleTarget)
    );
    assert_eq!(
        owner.global.ledger.status(fresh_key, 6).unwrap(),
        OperationStatus::NotAdmitted
    );
}

#[test]
fn global_status_receipt_and_polling_never_select_the_wrong_target() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let first = launch(&mut owner, 1, &mut journal, &events).unwrap();
    let second = launch(&mut owner, 2, &mut journal, &events).unwrap();
    let target_one = first.target.unwrap();
    let target_two = second.target.unwrap();
    let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
    let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();

    let first_key = key(connection_one.lease(), 1);
    let first_read = control_payload(
        first_key,
        target_one,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    owner
        .handle_control(&connection_one, &first_read, 5)
        .unwrap();
    let second_key = key(connection_two.lease(), 1);
    let second_read = control_payload(
        second_key,
        target_two,
        Command::Read {
            source: ReadSource::Screen,
            max_bytes: 256,
            max_lines: 1,
        },
    );
    owner
        .handle_control(&connection_two, &second_read, 5)
        .unwrap();

    assert_eq!(
        response(
            &owner
                .operation_status(&connection_one, first_key, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        response(
            &owner
                .operation_status(&connection_two, second_key, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        response(
            &owner
                .operation_status(&connection_one, second_key, 6)
                .unwrap()
        )
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert!(matches!(
        owner.receipt(&connection_one, GENERATION, connection_two.lease(), 1, 6,),
        Err(ServerError::Unauthorized)
    ));
    owner
        .receipt(&connection_one, GENERATION, connection_one.lease(), 1, 6)
        .unwrap();
    assert_eq!(
        owner.global.ledger.status(first_key, 6),
        Err(ProtocolError::ResultExpired)
    );
    assert_eq!(
        owner.global.ledger.status(second_key, 6),
        Ok(OperationStatus::Completed(Outcome::Applied))
    );

    owner
        .launches
        .session_mut(target_one)
        .unwrap()
        .input_owner
        .claim(
            crate::input_owner::Owner::Client(connection_one.lease()),
            1,
            1,
            6,
        )
        .unwrap();
    owner
        .launches
        .session_mut(target_two)
        .unwrap()
        .input_owner
        .claim(
            crate::input_owner::Owner::Client(connection_two.lease()),
            1,
            1,
            6,
        )
        .unwrap();
    owner
        .launches
        .session_mut(target_one)
        .unwrap()
        .effects
        .fail_poll = true;
    owner
        .launches
        .session_mut(target_one)
        .unwrap()
        .effects
        .exited = true;
    owner
        .launches
        .session_mut(target_two)
        .unwrap()
        .effects
        .exited = true;

    assert_eq!(
        owner.poll(LEASE_TTL_SECS + 10),
        Err(ServerError::EffectPollFailed)
    );
    assert_eq!(owner.launches.effects(target_one).unwrap().effects.polls, 1);
    assert_eq!(owner.launches.effects(target_two).unwrap().effects.polls, 1);
    assert_eq!(owner.launches.state(target_one), Ok(SessionState::Exited));
    assert_eq!(owner.launches.state(target_two), Ok(SessionState::Exited));
    assert_eq!(
        owner.launches.records().filter(|r| r.has_effects).count(),
        2
    );
    assert_eq!(owner.global.grants.len(), 0);
    assert_eq!(owner.global.ledger.usage().active_leases, 0);
}

#[test]
fn public_session_claim_then_input_is_independent_on_both_targets() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_a = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_b = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
    let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
    let initial_a = session_info(
        owner
            .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 5)
            .unwrap(),
    );
    let initial_b = session_info(
        owner
            .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 5)
            .unwrap(),
    );
    assert_eq!(initial_a.target, target_a);
    assert_eq!(initial_b.target, target_b);
    assert_eq!(initial_a.owner_lease, 0);
    assert_eq!(initial_b.owner_lease, 0);

    let claim_a_key = key(connection_a.lease(), 1);
    let claim_a = session_payload(SessionRequest::Claim {
        key: claim_a_key,
        target: target_a,
        expected_owner_revision: initial_a.owner_revision,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection_a, &claim_a, 6).unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let after_a_claim_b = session_info(
        owner
            .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 6)
            .unwrap(),
    );
    assert_eq!(after_a_claim_b, initial_b);

    let claim_b_key = key(connection_b.lease(), 1);
    let claim_b = session_payload(SessionRequest::Claim {
        key: claim_b_key,
        target: target_b,
        expected_owner_revision: initial_b.owner_revision,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection_b, &claim_b, 6).unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let owner_a = session_info(
        owner
            .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 7)
            .unwrap(),
    );
    let owner_b = session_info(
        owner
            .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 7)
            .unwrap(),
    );
    assert_eq!(owner_a.owner_lease, connection_a.lease());
    assert_eq!(owner_b.owner_lease, connection_b.lease());

    let submit_a = control_payload(
        key(connection_a.lease(), 2),
        target_a,
        Command::Submit {
            owner_revision: owner_a.owner_revision,
            text: "from-a",
        },
    );
    let submit_b = control_payload(
        key(connection_b.lease(), 2),
        target_b,
        Command::Submit {
            owner_revision: owner_b.owner_revision,
            text: "from-b",
        },
    );
    for (connection, payload, target) in [
        (&connection_a, &submit_a, target_a),
        (&connection_b, &submit_b, target_b),
    ] {
        assert_eq!(
            response(&owner.handle_control(connection, payload, 8).unwrap()).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.launches.effects(target).unwrap().effects.inputs, 1);
    }
    assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);
    assert_eq!(owner.launches.effects(target_b).unwrap().effects.inputs, 1);
}

#[test]
fn public_release_handoff_and_preflight_denials_preserve_target_owners() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_a = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_b = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
    let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
    let read_only_hello = SessionHello {
        hello: Hello {
            nonce: 33,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let (read_only_recipient, _) = owner.connect(target_a, &read_only_hello, 4).unwrap();

    let counts_a = Arc::new(Mutex::new(EffectCounts::default()));
    let counts_b = Arc::new(Mutex::new(EffectCounts::default()));
    owner
        .launches
        .session_mut(target_a)
        .unwrap()
        .effects
        .shared_counts = Some(Arc::clone(&counts_a));
    owner
        .launches
        .session_mut(target_b)
        .unwrap()
        .effects
        .shared_counts = Some(Arc::clone(&counts_b));

    for (connection, target) in [(&connection_a, target_a), (&connection_b, target_b)] {
        let key = key(connection.lease(), 1);
        let request = session_payload(SessionRequest::Claim {
            key,
            target,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(connection, &request, 5).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
    }
    let info_a = session_info(
        owner
            .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 6)
            .unwrap(),
    );
    let info_b = session_info(
        owner
            .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 6)
            .unwrap(),
    );

    let release = session_payload(SessionRequest::Release {
        key: key(connection_a.lease(), 2),
        target: target_a,
        expected_owner_revision: info_a.owner_revision,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection_a, &release, 7).unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let after_release_a = session_info(
        owner
            .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 8)
            .unwrap(),
    );
    let after_release_b = session_info(
        owner
            .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 8)
            .unwrap(),
    );
    assert_eq!(after_release_a.owner_lease, 0);
    assert_eq!(after_release_b, info_b);

    // Reclaim A, then try handing it to B's live input grant. The grant is
    // valid but pinned to the wrong target, so no admission or transition.
    let reclaim_key = key(connection_a.lease(), 3);
    let reclaim = session_payload(SessionRequest::Claim {
        key: reclaim_key,
        target: target_a,
        expected_owner_revision: after_release_a.owner_revision,
        ttl_seconds: 30,
    });
    owner.handle_session(&connection_a, &reclaim, 9).unwrap();
    let reclaimed = session_info(
        owner
            .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 9)
            .unwrap(),
    );
    let handoff_key = key(connection_a.lease(), 4);
    let handoff = session_payload(SessionRequest::Handoff {
        key: handoff_key,
        target: target_a,
        expected_owner_revision: reclaimed.owner_revision,
        recipient_lease: connection_b.lease(),
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection_a, &handoff, 10).unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(
        owner.global.ledger.status(handoff_key, 10).unwrap(),
        OperationStatus::NotAdmitted
    );
    assert_eq!(
        session_info(
            owner
                .handle_session(
                    &connection_a,
                    &session_payload(SessionRequest::Describe),
                    10,
                )
                .unwrap(),
        )
        .owner_lease,
        connection_a.lease()
    );

    let missing_recipient_key = key(connection_a.lease(), 5);
    let missing_recipient = session_payload(SessionRequest::Handoff {
        key: missing_recipient_key,
        target: target_a,
        expected_owner_revision: reclaimed.owner_revision,
        recipient_lease: connection_b.lease() + 100,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&connection_a, &missing_recipient, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::UnknownContext)
    );
    assert_eq!(
        owner
            .global
            .ledger
            .status(missing_recipient_key, 10)
            .unwrap(),
        OperationStatus::NotAdmitted
    );

    let no_input_key = key(connection_a.lease(), 6);
    let no_input_handoff = session_payload(SessionRequest::Handoff {
        key: no_input_key,
        target: target_a,
        expected_owner_revision: reclaimed.owner_revision,
        recipient_lease: read_only_recipient.lease(),
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&connection_a, &no_input_handoff, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(
        owner.global.ledger.status(no_input_key, 10).unwrap(),
        OperationStatus::NotAdmitted
    );

    let expired_recipient_key = key(connection_a.lease(), 7);
    owner
        .global
        .grants
        .get_mut(&read_only_recipient.lease())
        .unwrap()
        .expires_secs = 10;
    let expired_recipient = session_payload(SessionRequest::Handoff {
        key: expired_recipient_key,
        target: target_a,
        expected_owner_revision: reclaimed.owner_revision,
        recipient_lease: read_only_recipient.lease(),
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&connection_a, &expired_recipient, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::UnknownContext)
    );
    assert_eq!(
        owner
            .global
            .ledger
            .status(expired_recipient_key, 10)
            .unwrap(),
        OperationStatus::NotAdmitted
    );

    // Wrong target generation and lease are global denials before exact
    // target selection, and therefore do not synchronize metadata.
    let before_a = *counts_a.lock().unwrap();
    let wrong_generation = TargetRef {
        generation: GENERATION + 1,
        ..target_a
    };
    let wrong_generation_payload = session_payload(SessionRequest::Claim {
        key: OperationKey {
            generation: GENERATION + 1,
            lease: connection_a.lease(),
            sequence: 6,
        },
        target: wrong_generation,
        expected_owner_revision: reclaimed.owner_revision,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&connection_a, &wrong_generation_payload, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    let wrong_lease_payload = session_payload(SessionRequest::Claim {
        key: key(connection_b.lease(), 6),
        target: target_a,
        expected_owner_revision: reclaimed.owner_revision,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&connection_a, &wrong_lease_payload, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(*counts_a.lock().unwrap(), before_a);
    assert_eq!(counts_b.lock().unwrap().inputs, 0);
}

#[test]
fn public_same_target_handoff_grants_recipient_input_and_release() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_a = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_b = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (owner_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
    let (sibling_owner, _) = owner.connect(target_b, &hello(), 4).unwrap();
    let (recipient, _) = owner.connect(target_a, &hello(), 4).unwrap();

    for (connection, target) in [(&owner_a, target_a), (&sibling_owner, target_b)] {
        let request = session_payload(SessionRequest::Claim {
            key: key(connection.lease(), 1),
            target,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(connection, &request, 5).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
    }
    let owner_info = session_info(
        owner
            .handle_session(&owner_a, &session_payload(SessionRequest::Describe), 6)
            .unwrap(),
    );
    let sibling_info = session_info(
        owner
            .handle_session(
                &sibling_owner,
                &session_payload(SessionRequest::Describe),
                6,
            )
            .unwrap(),
    );
    let handoff_key = key(owner_a.lease(), 2);
    let handoff = session_payload(SessionRequest::Handoff {
        key: handoff_key,
        target: target_a,
        expected_owner_revision: owner_info.owner_revision,
        recipient_lease: recipient.lease(),
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&owner_a, &handoff, 7).unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let handed_off = session_info(
        owner
            .handle_session(&recipient, &session_payload(SessionRequest::Describe), 8)
            .unwrap(),
    );
    assert_eq!(handed_off.owner_lease, recipient.lease());

    let recipient_submit = control_payload(
        key(recipient.lease(), 1),
        target_a,
        Command::Submit {
            owner_revision: handed_off.owner_revision,
            text: "recipient-input",
        },
    );
    assert_eq!(
        response(
            &owner
                .handle_control(&recipient, &recipient_submit, 9)
                .unwrap()
        )
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);

    // Use the current revision so this denial proves the prior owner no
    // longer holds ownership, rather than merely presenting a stale CAS.
    let previous_owner_submit = control_payload(
        key(owner_a.lease(), 3),
        target_a,
        Command::Submit {
            owner_revision: handed_off.owner_revision,
            text: "old-owner-input",
        },
    );
    assert_eq!(
        response(
            &owner
                .handle_control(&owner_a, &previous_owner_submit, 9)
                .unwrap()
        )
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(
        owner
            .global
            .ledger
            .status(key(owner_a.lease(), 3), 9)
            .unwrap(),
        OperationStatus::NotAdmitted
    );
    assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);
    assert_eq!(
        session_info(
            owner
                .handle_session(
                    &sibling_owner,
                    &session_payload(SessionRequest::Describe),
                    9,
                )
                .unwrap(),
        ),
        sibling_info
    );
    assert_eq!(owner.launches.effects(target_b).unwrap().effects.inputs, 0);

    let recipient_release = session_payload(SessionRequest::Release {
        key: key(recipient.lease(), 2),
        target: target_a,
        expected_owner_revision: handed_off.owner_revision,
    });
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(&recipient, &recipient_release, 10)
                .unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        session_info(
            owner
                .handle_session(&recipient, &session_payload(SessionRequest::Describe), 11,)
                .unwrap(),
        )
        .owner_lease,
        0
    );
    assert_eq!(
        session_info(
            owner
                .handle_session(
                    &sibling_owner,
                    &session_payload(SessionRequest::Describe),
                    11,
                )
                .unwrap(),
        ),
        sibling_info
    );
}

#[test]
fn session_dedup_status_and_stale_results_survive_empty_target_registry() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    let counts = Arc::new(Mutex::new(EffectCounts::default()));
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .shared_counts = Some(Arc::clone(&counts));
    let mutation_key = key(connection.lease(), 1);
    let exact = session_payload(SessionRequest::Claim {
        key: mutation_key,
        target,
        expected_owner_revision: 1,
        ttl_seconds: 30,
    });
    owner.handle_session(&connection, &exact, 5).unwrap();
    let counts_after_admission = *counts.lock().unwrap();

    owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection, &exact, 6).unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let changed_target = TargetRef {
        pane: target.pane + 1,
        ..target
    };
    let changed = session_payload(SessionRequest::Claim {
        key: mutation_key,
        target: changed_target,
        expected_owner_revision: 2,
        ttl_seconds: 30,
    });
    assert!(matches!(
        owner.handle_session(&connection, &changed, 6),
        Err(ServerError::Protocol(ProtocolError::OperationConflict))
    ));

    let stale_key = key(connection.lease(), 2);
    let stale = session_payload(SessionRequest::Claim {
        key: stale_key,
        target,
        expected_owner_revision: 1,
        ttl_seconds: 30,
    });
    assert_eq!(
        response(&session_result(
            owner.handle_session(&connection, &stale, 6).unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::StaleTarget)
    );
    assert_eq!(
        owner.global.ledger.status(stale_key, 6).unwrap(),
        OperationStatus::NotAdmitted
    );
    assert_eq!(*counts.lock().unwrap(), counts_after_admission);
}

#[test]
fn describe_pins_target_and_targetless_requests_need_no_session() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_a = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_b = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
    let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
    assert_eq!(
        session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 5,)
                .unwrap(),
        )
        .target,
        target_a
    );
    assert_eq!(
        session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 5,)
                .unwrap(),
        )
        .target,
        target_b
    );

    let claim_key = key(connection_a.lease(), 1);
    let claim = session_payload(SessionRequest::Claim {
        key: claim_key,
        target: target_a,
        expected_owner_revision: 1,
        ttl_seconds: 30,
    });
    owner.handle_session(&connection_a, &claim, 6).unwrap();
    let no_read_hello = SessionHello {
        hello: Hello {
            nonce: 88,
            required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let (no_read, _) = owner.connect(target_a, &no_read_hello, 6).unwrap();

    let counts_a = Arc::new(Mutex::new(EffectCounts::default()));
    let counts_b = Arc::new(Mutex::new(EffectCounts::default()));
    owner
        .launches
        .session_mut(target_a)
        .unwrap()
        .effects
        .shared_counts = Some(Arc::clone(&counts_a));
    owner
        .launches
        .session_mut(target_b)
        .unwrap()
        .effects
        .shared_counts = Some(Arc::clone(&counts_b));
    let before_a = *counts_a.lock().unwrap();
    let before_b = *counts_b.lock().unwrap();

    owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(
                    &connection_a,
                    &session_payload(SessionRequest::OperationStatus { key: claim_key }),
                    7,
                )
                .unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let wrong_key = key(connection_b.lease(), 1);
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(
                    &connection_a,
                    &session_payload(SessionRequest::OperationStatus { key: wrong_key }),
                    7,
                )
                .unwrap()
        ))
        .body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert!(matches!(
        owner.handle_session(
            &no_read,
            &session_payload(SessionRequest::OperationStatus { key: claim_key }),
            7
        ),
        Err(ServerError::Unauthorized)
    ));
    assert!(matches!(
        owner.handle_session(
            &connection_a,
            &session_payload(SessionRequest::Receipt {
                generation: GENERATION,
                lease: connection_b.lease(),
                through_sequence: 1,
            }),
            7
        ),
        Err(ServerError::Unauthorized)
    ));
    assert!(matches!(
        owner.handle_session(&connection_a, &[0, 0, 0, 0], 7),
        Err(ServerError::Session(_))
    ));
    assert_eq!(
        response(&session_result(
            owner
                .handle_session(
                    &connection_a,
                    &session_payload(SessionRequest::Receipt {
                        generation: GENERATION,
                        lease: connection_a.lease(),
                        through_sequence: 1,
                    }),
                    7,
                )
                .unwrap()
        ))
        .body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert!(matches!(
        owner.handle_session(
            &connection_a,
            &session_payload(SessionRequest::OperationStatus { key: claim_key }),
            8
        ),
        Err(ServerError::Protocol(ProtocolError::ResultExpired))
    ));
    assert!(matches!(
        owner.handle_session(&connection_a, &session_payload(SessionRequest::Detach), 8),
        Ok(SessionReply::Detached)
    ));
    assert_eq!(*counts_a.lock().unwrap(), before_a);
    assert_eq!(*counts_b.lock().unwrap(), before_b);
}

#[test]
fn selected_describe_error_synchronizes_and_expired_detach_fails_closed() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    let no_read_hello = SessionHello {
        hello: Hello {
            nonce: 34,
            required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let (no_read, _) = owner.connect(target, &no_read_hello, 4).unwrap();
    let before_denials = owner
        .launches
        .effects(target)
        .unwrap()
        .effects
        .screen_observations
        .get();
    assert!(matches!(
        owner.handle_session(&no_read, &session_payload(SessionRequest::Describe), 5),
        Err(ServerError::Unauthorized)
    ));
    let missing_context = Connection {
        generation: GENERATION,
        lease: u64::MAX,
        selected_capabilities: CAPABILITY_READ,
    };
    assert!(matches!(
        owner.handle_session(
            &missing_context,
            &session_payload(SessionRequest::Describe),
            5
        ),
        Err(ServerError::UnknownContext)
    ));
    assert!(matches!(
        owner.handle_session(
            &connection,
            &session_payload(SessionRequest::Describe),
            4 + LEASE_TTL_SECS
        ),
        Err(ServerError::UnknownContext)
    ));
    assert_eq!(
        owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        before_denials
    );
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .input_owner
        .claim(crate::input_owner::Owner::Human(7), 1, 30, 5)
        .unwrap();
    let before = owner
        .launches
        .effects(target)
        .unwrap()
        .effects
        .screen_observations
        .get();
    assert!(matches!(
        owner.handle_session(&connection, &session_payload(SessionRequest::Describe), 6),
        Err(ServerError::UnsupportedOwner)
    ));
    assert_eq!(
        owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        before + 1
    );
    assert!(matches!(
        owner.handle_session(
            &connection,
            &session_payload(SessionRequest::Detach),
            4 + LEASE_TTL_SECS
        ),
        Err(ServerError::UnknownContext)
    ));
}

#[test]
fn scoped_reads_keep_equal_sequences_in_separate_targets_and_reject_crossed_cursors() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target_a = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let target_b = launch(&mut owner, 2, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
    let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
    for target in [target_a, target_b] {
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .read_text = Some(String::new());
    }
    let a = owner.reconcile(&connection_a, target_a, 128, 1, 5).unwrap();
    let b = owner.reconcile(&connection_b, target_b, 128, 1, 5).unwrap();
    assert_eq!(a.cursor.session, target_a.session);
    assert_eq!(b.cursor.session, target_b.session);
    assert_eq!(a.cursor.sequence, b.cursor.sequence);
    assert_eq!(
        owner
            .launches
            .effects(target_a)
            .unwrap()
            .effects
            .read_bounds,
        Some((0, 1))
    );
    assert_eq!(
        owner
            .launches
            .effects(target_b)
            .unwrap()
            .effects
            .read_bounds,
        Some((0, 1))
    );

    owner
        .launches
        .session_mut(target_a)
        .unwrap()
        .effects
        .revision = Some(2);
    owner.poll(6).unwrap();
    owner
        .launches
        .session_mut(target_b)
        .unwrap()
        .effects
        .revision = Some(2);
    owner.poll(6).unwrap();
    let initial = |target: TargetRef| SessionCursor {
        generation: GENERATION,
        session: target.session,
        sequence: 0,
    };
    let page_a = owner
        .read_events(&connection_a, target_a, initial(target_a), 1, 7)
        .unwrap();
    let page_b = owner
        .read_events(&connection_b, target_b, initial(target_b), 1, 7)
        .unwrap();
    assert_eq!(page_a.cursor.sequence, page_b.cursor.sequence);
    assert_eq!(page_a.events[0].target, target_a);
    assert_eq!(page_b.events[0].target, target_b);
    let before_b = owner
        .launches
        .effects(target_b)
        .unwrap()
        .effects
        .screen_observations
        .get();
    assert!(matches!(
        owner.read_events(&connection_b, target_b, page_a.cursor, 1, 8),
        Err(ServerError::TargetStale)
    ));
    assert_eq!(
        owner
            .launches
            .effects(target_b)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        before_b
    );
}

#[test]
fn scoped_read_denials_and_invalid_bounds_precede_metadata_and_sink_reads() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    let no_events = SessionHello {
        hello: Hello {
            nonce: 33,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: 0,
        },
        resume: None,
    };
    let (no_events, _) = owner.connect(target, &no_events, 4).unwrap();
    let no_read_hello = SessionHello {
        hello: Hello {
            nonce: 34,
            required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
            optional_capabilities: CAPABILITY_EVENTS,
        },
        resume: None,
    };
    let (no_read, _) = owner.connect(target, &no_read_hello, 4).unwrap();
    let before = owner
        .launches
        .effects(target)
        .unwrap()
        .effects
        .screen_observations
        .get();
    assert!(matches!(
        owner.reconcile(&no_events, target, 128, 1, 5),
        Err(ServerError::Unauthorized)
    ));
    assert!(matches!(
        owner.reconcile(&no_read, target, 128, 1, 5),
        Err(ServerError::Unauthorized)
    ));
    assert!(matches!(
        owner.reconcile(&connection, target, 127, 1, 5),
        Err(ServerError::InvalidWatchRequest)
    ));
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 0, 5),
        Err(ServerError::InvalidWatchRequest)
    ));
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1_025, 5),
        Err(ServerError::InvalidWatchRequest)
    ));
    let cursor = SessionCursor {
        generation: GENERATION,
        session: target.session,
        sequence: 0,
    };
    assert!(matches!(
        owner.read_events(&connection, target, cursor, 0, 5),
        Err(ServerError::InvalidWatchRequest)
    ));
    assert!(matches!(
        owner.read_events(
            &connection,
            target,
            cursor,
            crate::watch_protocol::MAX_WATCH_EVENTS + 1,
            5
        ),
        Err(ServerError::InvalidWatchRequest)
    ));
    assert!(matches!(
        owner.read_events(
            &connection,
            target,
            SessionCursor {
                generation: GENERATION + 1,
                ..cursor
            },
            1,
            5
        ),
        Err(ServerError::WrongGeneration)
    ));
    assert!(matches!(
        owner.read_events(
            &connection,
            target,
            SessionCursor {
                session: target.session + 1,
                ..cursor
            },
            1,
            5
        ),
        Err(ServerError::TargetStale)
    ));
    let missing = Connection {
        generation: GENERATION,
        lease: u64::MAX,
        selected_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
    };
    assert!(matches!(
        owner.reconcile(&missing, target, 128, 1, 5),
        Err(ServerError::UnknownContext)
    ));
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1, 4 + LEASE_TTL_SECS),
        Err(ServerError::UnknownContext)
    ));
    assert_eq!(
        owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get(),
        before
    );
    assert_eq!(owner.launches.effects(target).unwrap().effects.reads, 0);
}

#[test]
fn scoped_reconciliation_preserves_exact_budgets_snapshot_errors_and_debug_redaction() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .read_text = Some("abcd".to_owned());
    let state = owner.reconcile(&connection, target, 132, 1, 5).unwrap();
    assert_eq!(state.text, "abcd");
    assert_eq!(
        owner.launches.effects(target).unwrap().effects.read_bounds,
        Some((4, 1))
    );
    let wire = crate::scoped_watch_protocol::encode_reconciliation(
        &crate::scoped_watch_protocol::Reconciliation {
            info: state.info,
            cursor: state.cursor,
            screen_revision: state.screen_revision,
            truncated: state.truncated,
            text: &state.text,
        },
    )
    .unwrap();
    assert_eq!(wire.len(), 132);
    assert!(matches!(
        owner.reconcile(&connection, target, 65_537, 1, 5),
        Err(ServerError::InvalidWatchRequest)
    ));
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .read_text = Some("a\nb".to_owned());
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1, 6),
        Err(ServerError::InvalidSnapshot)
    ));
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .read_text = Some("x".repeat(65_408));
    let max = owner
        .reconcile(&connection, target, 65_536, 1_024, 7)
        .unwrap();
    assert_eq!(max.text.len(), 65_408);
    assert_eq!(
        owner.launches.effects(target).unwrap().effects.read_bounds,
        Some((65_408, 1_024))
    );
    assert_eq!(
        crate::scoped_watch_protocol::encode_reconciliation(
            &crate::scoped_watch_protocol::Reconciliation {
                info: max.info,
                cursor: max.cursor,
                screen_revision: max.screen_revision,
                truncated: max.truncated,
                text: &max.text,
            }
        )
        .unwrap()
        .len(),
        65_536
    );
    {
        let effects = &mut owner.launches.session_mut(target).unwrap().effects;
        effects.revision_after_read = Some(2);
        effects.read_revision = Some(1);
    }
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1, 8),
        Err(ServerError::SnapshotChanged)
    ));
    {
        let effects = &mut owner.launches.session_mut(target).unwrap().effects;
        effects.revision_after_read = None;
        effects.read_revision = Some(2);
    }
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .fail_read = true;
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1, 9),
        Err(ServerError::SnapshotReadFailed)
    ));
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .fail_read = false;
    owner
        .launches
        .session_mut(target)
        .unwrap()
        .effects
        .read_revision = Some(0);
    assert!(matches!(
        owner.reconcile(&connection, target, 128, 1, 10),
        Err(ServerError::NotificationsUnavailable)
    ));
    let state_debug = format!("{max:?}");
    assert!(state_debug.contains("info: \"<redacted>\""));
    assert!(state_debug.contains("cursor: \"<redacted>\""));
    assert!(state_debug.contains("screen_revision: \"<redacted>\""));
    assert!(!state_debug.contains(&max.text));
}

#[test]
fn scoped_event_pages_return_target_local_resync_state_and_redacted_debug() {
    let mut owner = owner();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::new(&events);
    let target = launch(&mut owner, 1, &mut journal, &events)
        .unwrap()
        .target
        .unwrap();
    let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
    let cursor = SessionCursor {
        generation: GENERATION,
        session: target.session,
        sequence: 0,
    };
    {
        let session = owner.launches.session_mut(target).unwrap();
        for _ in 0..=crate::event_journal::MAX_RETAINED_EVENTS {
            session
                .journal
                .push(target, crate::event_protocol::EventBody::ChildExited)
                .unwrap();
        }
    }
    let expired = owner
        .read_events(&connection, target, cursor, 64, 5)
        .unwrap();
    assert!(expired.resync_required);
    assert!(expired.events.is_empty());
    assert_eq!(expired.cursor.session, target.session);
    assert_eq!(expired.cursor.sequence, 65);
    let retained = SessionCursor {
        sequence: 1,
        ..cursor
    };
    let current = owner
        .read_events(&connection, target, retained, 64, 6)
        .unwrap();
    assert!(!current.resync_required);
    assert_eq!(current.events.len(), 64);
    assert_eq!(current.cursor.session, target.session);
    let event_debug = format!("{current:?}");
    assert!(event_debug.contains("cursor: \"<redacted>\""));
    assert!(!event_debug.contains("ChildExited"));
}
