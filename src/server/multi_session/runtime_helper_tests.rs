//! Deterministic coverage of the owner helpers used by the multi-session
//! runtime: typed-key input, root resize, split authorization, scoped
//! observation and launch status.
use super::{MultiSessionOwner, RootResizeReply};
use crate::control_protocol::{Command, ReadSource, TargetRef};
use crate::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use crate::managed_launch::LaunchJournalPort;
use crate::managed_launch_protocol::{encode_launch_request, LaunchRequest};
use crate::pane_layout::SplitDirection;
use crate::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use crate::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use crate::server::{Connection, EffectSink, ReadResult, ServerError, SessionReply};
use crate::server_storage::LaunchRecord;
use crate::session_protocol::{encode_session_request, SessionHello, SessionRequest};
use crate::split_protocol::{decode_split_request, encode_split_request, SplitRequest};
use crate::terminal::Key;
use crate::ui_protocol::{decode_ui_request, encode_key_input, KeyInput, RootResize, UiRequest};
use std::io;

const GENERATION: u64 = 91;
const INTERNAL: u64 = 1;
const NOW: u64 = 5;

#[derive(Default)]
struct Sink {
    delivered: Vec<Key>,
    resizes: Vec<(u16, u16)>,
    fail_resize: bool,
    exited: bool,
}

impl EffectSink for Sink {
    fn validate(&self, _: Command<'_>) -> Result<(), RejectionCode> {
        Ok(())
    }
    fn read(&mut self, _: ReadSource, _: usize, _: u16) -> Result<ReadResult, Outcome> {
        Err(Outcome::Failed)
    }
    fn input(&mut self, _: Command<'_>) -> Outcome {
        Outcome::Applied
    }
    fn poll(&mut self) -> Result<(), Outcome> {
        Ok(())
    }
    fn child_exited(&self) -> bool {
        self.exited
    }
    fn screen_revision(&self) -> Option<u64> {
        Some(1)
    }
}

struct Journal;

impl LaunchJournalPort for Journal {
    fn preflight_launch(&self, _: OperationKey, _: u64) -> io::Result<()> {
        Ok(())
    }
    fn record(&mut self, _: LaunchRecord) -> io::Result<()> {
        Ok(())
    }
}

fn launch(key: OperationKey, rows: u16, columns: u16) -> Vec<u8> {
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
            environment: &[],
        },
        &mut bytes,
    )
    .unwrap();
    bytes
}

fn owner_with_first(allow_input: bool) -> (MultiSessionOwner<Sink>, TargetRef) {
    let mut ledger = Ledger::new(GENERATION).unwrap();
    ledger.open_lease(INTERNAL, 0).unwrap();
    let mut owner = MultiSessionOwner::with_ledger(ledger, allow_input).unwrap();
    let key = OperationKey {
        generation: GENERATION,
        lease: INTERNAL,
        sequence: 1,
    };
    let result = owner
        .execute_launch(
            &launch(key, 24, 81),
            NOW,
            &mut Journal,
            |_, _| Ok::<_, ()>(()),
            |()| Ok::<_, ()>(Sink::default()),
        )
        .unwrap();
    owner
        .resize_layout(1, 24, 81, |sink: &mut Sink, rows, columns| {
            sink.resizes.push((rows, columns));
            Ok::<_, ()>(())
        })
        .unwrap();
    (owner, result.target.unwrap())
}

fn connect(owner: &mut MultiSessionOwner<Sink>, target: TargetRef, nonce: u64) -> Connection {
    let hello = SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ,
            optional_capabilities: CAPABILITY_INPUT,
        },
        resume: None,
    };
    owner.connect(target, &hello, NOW).unwrap().0
}

fn select_pane_operations_for_test(
    owner: &mut MultiSessionOwner<Sink>,
    connection: &mut Connection,
) {
    let capability = crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS;
    connection.selected_capabilities |= capability;
    owner
        .global
        .grants
        .get_mut(&connection.lease())
        .expect("live connection grant")
        .capabilities |= capability;
}

fn add_right_pane(
    owner: &mut MultiSessionOwner<Sink>,
    source: TargetRef,
    key: OperationKey,
) -> TargetRef {
    let snapshot = owner.layout_snapshot().unwrap();
    let destination = owner.launches.next_target().unwrap();
    let preview = owner
        .layout
        .as_ref()
        .unwrap()
        .preview_split(
            source,
            snapshot.revision,
            SplitDirection::Right,
            destination,
        )
        .unwrap();
    let planned = preview
        .planned_snapshot()
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == destination)
        .copied()
        .unwrap();
    let content = crate::pane_layout::content_rect(planned.rect).unwrap();
    let launch = LaunchRequest {
        key,
        rows: content.rows,
        columns: content.columns,
        requested_executable: b"/bin/sh",
        requested_working_directory: b"/",
        client_working_directory: b"/",
        arguments: &[],
        environment: &[],
    };
    let mut payload = Vec::new();
    encode_split_request(
        &SplitRequest {
            key,
            target: source,
            expected_layout_revision: snapshot.revision,
            direction: SplitDirection::Right,
            launch: &launch,
        },
        &mut payload,
    )
    .unwrap();
    let result = owner
        .execute_split(
            &payload,
            NOW,
            &mut Journal,
            |_, target| Ok::<_, ()>(target.session),
            |_| Ok::<_, ()>(Sink::default()),
            |sink: &mut Sink, rows, columns| {
                sink.resizes.push((rows, columns));
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    assert_eq!(result.new_target, Some(destination));
    destination
}

fn key(connection: &Connection, sequence: u64) -> OperationKey {
    OperationKey {
        generation: GENERATION,
        lease: connection.lease(),
        sequence,
    }
}

fn claim(owner: &mut MultiSessionOwner<Sink>, connection: &Connection, target: TargetRef) {
    let payload = encode_session_request(&SessionRequest::Claim {
        key: key(connection, 1),
        target,
        expected_owner_revision: 1,
        ttl_seconds: 30,
    })
    .unwrap();
    let SessionReply::Payload(reply) = owner.handle_session(connection, &payload, NOW).unwrap()
    else {
        panic!("claim reply");
    };
    assert_eq!(
        decode_response_payload(&reply).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
}

fn keys(
    owner: &mut MultiSessionOwner<Sink>,
    connection: &Connection,
    payload: &[u8],
) -> ResponseBody<'static> {
    let UiRequest::KeyInput(view) = decode_ui_request(payload).unwrap() else {
        panic!("key input");
    };
    let bytes = owner
        .handle_key_input(
            connection,
            &view,
            payload,
            NOW,
            |_| Ok(()),
            |sink| {
                sink.delivered.extend(view.keys());
                Outcome::Applied
            },
        )
        .unwrap();
    match decode_response_payload(&bytes).unwrap().body {
        ResponseBody::Status(status) => ResponseBody::Status(status),
        ResponseBody::Rejected(code) => ResponseBody::Rejected(code),
        ResponseBody::Read { .. } => panic!("unexpected read"),
    }
}

fn key_payload(key: OperationKey, target: TargetRef, revision: u64) -> Vec<u8> {
    encode_key_input(&KeyInput {
        key,
        target,
        expected_owner_revision: revision,
        keys: &[Key::Char('a'), Key::Enter],
    })
    .unwrap()
}

#[test]
fn guarded_agent_input_rejects_stale_observations_and_duplicates_never_revalidate() {
    use crate::agent_protocol::{AgentIdentity, AgentKind, GuardedSubmit};
    let (mut owner, target) = owner_with_first(true);
    let mut connection = connect(&mut owner, target, 77);
    let capability = crate::handshake_protocol::CAPABILITY_AGENT_OBSERVATION;
    connection.selected_capabilities |= capability;
    owner
        .global
        .grants
        .get_mut(&connection.lease())
        .unwrap()
        .capabilities |= capability;
    claim(&mut owner, &connection, target);
    let request = GuardedSubmit {
        key: key(&connection, 2),
        target,
        owner_revision: 2,
        identity: AgentIdentity {
            pid: 42,
            start_seconds: 100,
            start_microseconds: 1,
            kind: AgentKind::Codex,
        },
        screen_revision: 1,
        text: "review the diff",
    };
    let payload = crate::agent_protocol::encode_submit(&request).unwrap();
    let rejected = owner
        .handle_guarded_agent_submit(&connection, &request, &payload, NOW, |_| {
            Err(RejectionCode::StaleTarget)
        })
        .unwrap();
    assert_eq!(
        decode_response_payload(&rejected).unwrap().body,
        ResponseBody::Rejected(RejectionCode::StaleTarget)
    );
    let applied = owner
        .handle_guarded_agent_submit(&connection, &request, &payload, NOW, |_| Ok(()))
        .unwrap();
    assert_eq!(
        decode_response_payload(&applied).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let duplicate = owner
        .handle_guarded_agent_submit(&connection, &request, &payload, NOW, |_| {
            panic!("duplicate must not revalidate or resend")
        })
        .unwrap();
    assert_eq!(duplicate, applied);
    let changed = GuardedSubmit {
        text: "different",
        ..request
    };
    let changed_payload = crate::agent_protocol::encode_submit(&changed).unwrap();
    assert!(owner
        .handle_guarded_agent_submit(&connection, &changed, &changed_payload, NOW, |_| panic!(
            "payload mismatch must not reach validation"
        ))
        .is_err());
    let no_owner = GuardedSubmit {
        key: key(&connection, 3),
        owner_revision: 99,
        ..changed
    };
    let payload = crate::agent_protocol::encode_submit(&no_owner).unwrap();
    let refused = owner
        .handle_guarded_agent_submit(&connection, &no_owner, &payload, NOW, |_| {
            panic!("owner check must precede agent validation")
        })
        .unwrap();
    assert!(matches!(
        decode_response_payload(&refused).unwrap().body,
        ResponseBody::Rejected(_)
    ));
}

#[test]
fn key_input_requires_input_capability_ownership_and_never_replays() {
    let (mut owner, first) = owner_with_first(true);
    let connection = connect(&mut owner, first, 1);
    // No ownership yet: rejected before admission, nothing delivered.
    let early = key_payload(key(&connection, 2), first, 1);
    assert_eq!(
        keys(&mut owner, &connection, &early),
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    claim(&mut owner, &connection, first);
    let payload = key_payload(key(&connection, 3), first, 2);
    assert_eq!(
        keys(&mut owner, &connection, &payload),
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    // Exact duplicate returns the recorded status without re-delivery.
    assert_eq!(
        keys(&mut owner, &connection, &payload),
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    let delivered = owner
        .launches
        .effects(first)
        .unwrap()
        .effects
        .delivered
        .clone();
    assert_eq!(delivered, vec![Key::Char('a'), Key::Enter]);
    // Another lease's key is unauthorized.
    let foreign = key_payload(
        OperationKey {
            lease: connection.lease() + 7,
            ..key(&connection, 4)
        },
        first,
        2,
    );
    assert_eq!(
        keys(&mut owner, &connection, &foreign),
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
}

#[test]
fn key_input_without_server_input_or_outside_pinned_scope_is_rejected() {
    let (mut owner, first) = owner_with_first(false);
    let read_only = connect(&mut owner, first, 1);
    assert_eq!(read_only.selected_capabilities() & CAPABILITY_INPUT, 0);
    let payload = key_payload(key(&read_only, 1), first, 1);
    assert_eq!(
        keys(&mut owner, &read_only, &payload),
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );

    let (mut owner, first) = owner_with_first(true);
    let connection = connect(&mut owner, first, 1);
    let other = TargetRef {
        pane: 9,
        session: 9,
        ..first
    };
    let payload = key_payload(key(&connection, 1), other, 1);
    assert_eq!(
        keys(&mut owner, &connection, &payload),
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert!(owner
        .launches
        .effects(first)
        .unwrap()
        .effects
        .delivered
        .is_empty());
}

#[test]
fn root_resize_records_outcomes_and_duplicates_do_not_resize_again() {
    let (mut owner, first) = owner_with_first(true);
    let connection = connect(&mut owner, first, 1);
    let request = RootResize {
        key: key(&connection, 1),
        expected_layout_revision: 1,
        rows: 30,
        columns: 100,
    };
    let payload = crate::ui_protocol::encode_root_resize(&request).unwrap();
    let resize = |sink: &mut Sink, rows, columns| {
        sink.resizes.push((rows, columns));
        if sink.fail_resize {
            Err(())
        } else {
            Ok(())
        }
    };
    let RootResizeReply::Resized { snapshot, pending } = owner
        .handle_root_resize(&connection, request, &payload, NOW, resize)
        .unwrap()
    else {
        panic!("resized");
    };
    assert_eq!(
        (snapshot.revision, snapshot.rows, snapshot.columns),
        (2, 30, 100)
    );
    assert_eq!(pending, [false, false, false, false]);
    let RootResizeReply::Response(duplicate) = owner
        .handle_root_resize(&connection, request, &payload, NOW, resize)
        .unwrap()
    else {
        panic!("duplicate");
    };
    assert_eq!(
        decode_response_payload(&duplicate).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    );
    assert_eq!(
        owner.launches.effects(first).unwrap().effects.resizes,
        vec![(22, 79), (28, 98)]
    );

    // Stale revision: Failed, no PTY effect.
    let stale = RootResize {
        key: key(&connection, 2),
        ..request
    };
    let payload = crate::ui_protocol::encode_root_resize(&stale).unwrap();
    assert!(matches!(
        owner
            .handle_root_resize(&connection, stale, &payload, NOW, resize)
            .unwrap(),
        RootResizeReply::LayoutRejected(_)
    ));
    assert_eq!(
        owner.launches.effects(first).unwrap().effects.resizes.len(),
        2
    );

    // A failing PTY resize is Partial with the pane marked pending.
    owner
        .launches
        .session_mut(first)
        .unwrap()
        .effects
        .fail_resize = true;
    let grow = RootResize {
        key: key(&connection, 3),
        expected_layout_revision: 2,
        rows: 31,
        columns: 100,
    };
    let payload = crate::ui_protocol::encode_root_resize(&grow).unwrap();
    let RootResizeReply::Resized { pending, .. } = owner
        .handle_root_resize(&connection, grow, &payload, NOW, resize)
        .unwrap()
    else {
        panic!("pending");
    };
    assert_eq!(pending, [true, false, false, false]);
    let RootResizeReply::Response(again) = owner
        .handle_root_resize(&connection, grow, &payload, NOW, resize)
        .unwrap()
    else {
        panic!("duplicate");
    };
    assert_eq!(
        decode_response_payload(&again).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Partial))
    );
}

#[test]
fn pane_zoom_commits_underlying_revision_and_exact_duplicate_has_no_effect() {
    use crate::pane_operations_protocol::{
        encode_request, encode_view, PaneZoomSet, Request as PaneOperationRequest,
    };

    let (mut owner, target) = owner_with_first(true);
    let mut connection = connect(&mut owner, target, 41);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let request = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 9),
        expected_layout_revision: 1,
        target,
        zoomed: true,
    });
    let payload = encode_request(&request).unwrap();
    let mut effects = Vec::new();
    let first = owner
        .handle_pane_operation(&connection, request, &payload, NOW, |_, rows, columns| {
            effects.push((rows, columns));
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(
        effects.is_empty(),
        "single-pane zoom already has the effective size"
    );
    assert!(matches!(
        decode_response_payload(&first).unwrap().body,
        ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
    ));
    assert_eq!(owner.layout_snapshot().unwrap().revision, 2);
    assert_eq!(owner.zoomed_target, Some(target));

    let duplicate = owner
        .handle_pane_operation(
            &connection,
            request,
            &payload,
            NOW,
            |_, _, _| -> Result<(), ()> { panic!("exact duplicate must not resize") },
        )
        .unwrap();
    assert_eq!(duplicate, first);
    let view = owner
        .pane_operations_view(&connection, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(view.zoomed_target, Some(target));
    assert_eq!(view.confirmed_sizes[0], Some((22, 79)));
    assert_eq!(view.layout.revision, 2);
    assert_eq!(view.layout.panes[0].unwrap().pending_confirmed, None);
    assert!(encode_view(&view).is_ok());
}

#[test]
fn spontaneous_exit_of_zoom_target_clears_projection_for_multiple_and_last_pane() {
    use crate::pane_operations_protocol::{
        encode_request, PaneZoomSet, Request as PaneOperationRequest,
    };

    let (mut owner, source) = owner_with_first(true);
    let mut connection = connect(&mut owner, source, 47);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let zoom_target = add_right_pane(&mut owner, source, key(&connection, 2));
    let zoom = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 3),
        expected_layout_revision: 2,
        target: zoom_target,
        zoomed: true,
    });
    let payload = encode_request(&zoom).unwrap();
    owner
        .handle_pane_operation(&connection, zoom, &payload, NOW, |_, _, _| Ok::<_, ()>(()))
        .unwrap();
    owner.for_each_effects_mut(|target, sink| {
        if target == zoom_target {
            sink.exited = true;
        }
    });
    owner.poll(NOW).unwrap();
    let after_multiple = owner.layout_snapshot().unwrap();
    assert_eq!(owner.zoomed_target, None);
    assert_eq!(after_multiple.pane_count(), 1);
    assert!(!after_multiple
        .panes
        .iter()
        .flatten()
        .any(|pane| pane.target == zoom_target));

    let (mut solo, target) = owner_with_first(true);
    let mut solo_connection = connect(&mut solo, target, 48);
    select_pane_operations_for_test(&mut solo, &mut solo_connection);
    let zoom = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&solo_connection, 9),
        expected_layout_revision: 1,
        target,
        zoomed: true,
    });
    let payload = encode_request(&zoom).unwrap();
    solo.handle_pane_operation(&solo_connection, zoom, &payload, NOW, |_, _, _| {
        Ok::<_, ()>(())
    })
    .unwrap();
    solo.for_each_effects_mut(|_, sink| sink.exited = true);
    solo.poll(NOW).unwrap();
    let after_last = solo.layout_snapshot().unwrap();
    assert_eq!(solo.zoomed_target, None);
    assert_eq!(after_last.revision, 3);
    assert_eq!(
        after_last.pane_count(),
        1,
        "the exited target remains a tombstone"
    );
}

#[test]
fn zoom_resize_failure_keeps_absolute_pending_size_for_new_key_retry() {
    use crate::pane_operations_protocol::{
        encode_request, PaneZoomSet, Request as PaneOperationRequest,
    };

    let (mut owner, source) = owner_with_first(true);
    let mut connection = connect(&mut owner, source, 42);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let target = add_right_pane(&mut owner, source, key(&connection, 2));
    let current_revision = owner.layout_snapshot().unwrap().revision;
    let request = |sequence| {
        PaneOperationRequest::ZoomSet(PaneZoomSet {
            key: key(&connection, sequence),
            expected_layout_revision: if sequence == 3 {
                current_revision
            } else {
                current_revision + 1
            },
            target,
            zoomed: true,
        })
    };
    let first = request(3);
    let first_payload = encode_request(&first).unwrap();
    owner
        .handle_pane_operation(&connection, first, &first_payload, NOW, |_, _, _| {
            Err::<(), _>(())
        })
        .unwrap();
    assert_eq!(owner.pane_state(target).unwrap().desired, Some((22, 79)));
    let retry = request(4);
    let retry_payload = encode_request(&retry).unwrap();
    let mut calls = Vec::new();
    owner
        .handle_pane_operation(
            &connection,
            retry,
            &retry_payload,
            NOW,
            |_, rows, columns| {
                calls.push((rows, columns));
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    assert_eq!(calls, vec![(22, 79)]);
    assert_eq!(owner.pane_state(target).unwrap().desired, None);
    let view = owner
        .pane_operations_view(&connection, NOW)
        .unwrap()
        .unwrap();
    let target_index = view
        .layout
        .panes
        .iter()
        .position(|pane| pane.is_some_and(|pane| pane.target == target))
        .unwrap();
    assert_eq!(view.confirmed_sizes[target_index], Some((22, 79)));
    assert_eq!(
        view.layout.panes[target_index].unwrap().pending_confirmed,
        Some((22, 79))
    );
}

#[test]
fn root_resize_during_zoom_keeps_projection_and_unzoom_restores_tree() {
    use crate::pane_operations_protocol::{
        encode_request, PaneZoomSet, Request as PaneOperationRequest,
    };
    use crate::ui_protocol::RootResize;

    let (mut owner, target) = owner_with_first(true);
    let mut connection = connect(&mut owner, target, 43);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let zoom = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 20),
        expected_layout_revision: 1,
        target,
        zoomed: true,
    });
    let zoom_payload = encode_request(&zoom).unwrap();
    owner
        .handle_pane_operation(&connection, zoom, &zoom_payload, NOW, |_, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap();

    let root_resize = RootResize {
        key: key(&connection, 21),
        expected_layout_revision: 2,
        rows: 30,
        columns: 100,
    };
    let root_payload = crate::ui_protocol::encode_root_resize(&root_resize).unwrap();
    owner
        .handle_root_resize(
            &connection,
            root_resize,
            &root_payload,
            NOW,
            |_, rows, columns| {
                assert_eq!((rows, columns), (28, 98));
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    let resized = owner.layout_snapshot().unwrap();
    assert_eq!(
        (resized.rows, resized.columns, resized.revision),
        (30, 100, 3)
    );
    assert_eq!(owner.zoomed_target, Some(target));

    let unzoom = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 22),
        expected_layout_revision: 3,
        target,
        zoomed: false,
    });
    let unzoom_payload = encode_request(&unzoom).unwrap();
    owner
        .handle_pane_operation(
            &connection,
            unzoom,
            &unzoom_payload,
            NOW,
            |_, _, _| -> Result<(), ()> {
                panic!("unzoom should not resize a pane already at the restored size")
            },
        )
        .unwrap();
    let restored = owner.layout_snapshot().unwrap();
    assert_eq!(restored.panes, resized.panes);
    assert_eq!(restored.dividers, resized.dividers);
    assert_eq!(restored.revision, 4);
    assert_eq!(owner.zoomed_target, None);
}

#[test]
fn stale_pane_operation_is_admitted_failed_without_resize_effects() {
    use crate::pane_operations_protocol::{
        encode_request, PaneZoomSet, Request as PaneOperationRequest,
    };

    let (mut owner, target) = owner_with_first(true);
    let mut connection = connect(&mut owner, target, 44);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let request = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 30),
        expected_layout_revision: 99,
        target,
        zoomed: true,
    });
    let payload = encode_request(&request).unwrap();
    let reply = owner
        .handle_pane_operation(
            &connection,
            request,
            &payload,
            NOW,
            |_, _, _| -> Result<(), ()> { panic!("stale request must not resize") },
        )
        .unwrap();
    assert!(matches!(
        decode_response_payload(&reply).unwrap().body,
        ResponseBody::Rejected(RejectionCode::StaleOwner)
    ));
    assert_eq!(owner.layout_snapshot().unwrap().revision, 1);
    assert_eq!(owner.zoomed_target, None);
}

#[test]
fn zoom_resizes_only_selected_pane_and_reports_hidden_confirmed_size() {
    use crate::pane_operations_protocol::{
        encode_request, encode_view, PaneZoomSet, Request as PaneOperationRequest,
    };

    let (mut owner, source) = owner_with_first(true);
    let mut connection = connect(&mut owner, source, 45);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let zoomed = add_right_pane(&mut owner, source, key(&connection, 2));
    let before = owner.layout_snapshot().unwrap();
    assert_eq!(before.revision, 2);
    let hidden_before = owner.pane_state(source).unwrap().confirmed;
    let zoom = PaneOperationRequest::ZoomSet(PaneZoomSet {
        key: key(&connection, 3),
        expected_layout_revision: before.revision,
        target: zoomed,
        zoomed: true,
    });
    let payload = encode_request(&zoom).unwrap();
    let mut calls = Vec::new();
    owner
        .handle_pane_operation(&connection, zoom, &payload, NOW, |_, rows, columns| {
            calls.push((rows, columns));
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(calls, vec![(22, 79)]);
    let view = owner
        .pane_operations_view(&connection, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(view.zoomed_target, Some(zoomed));
    assert_eq!(view.layout.revision, before.revision + 1);
    for geometry in before.panes.iter().flatten() {
        assert!(view
            .layout
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == geometry.target && pane.rect == geometry.rect));
    }
    let hidden_index = view
        .layout
        .panes
        .iter()
        .position(|pane| pane.is_some_and(|pane| pane.target == source))
        .unwrap();
    let zoom_index = view
        .layout
        .panes
        .iter()
        .position(|pane| pane.is_some_and(|pane| pane.target == zoomed))
        .unwrap();
    assert_eq!(view.confirmed_sizes[hidden_index], Some(hidden_before));
    assert_eq!(view.confirmed_sizes[zoom_index], Some((22, 79)));
    assert!(view.layout.panes[zoom_index]
        .unwrap()
        .pending_confirmed
        .is_some());
    assert!(encode_view(&view).is_ok());
}

#[test]
fn swap_and_absolute_divider_commit_once_and_resize_affected_targets() {
    use crate::pane_operations_protocol::{
        encode_request, PaneDividerSet, PaneSwap, Request as PaneOperationRequest,
    };

    let (mut owner, source) = owner_with_first(true);
    let mut connection = connect(&mut owner, source, 46);
    select_pane_operations_for_test(&mut owner, &mut connection);
    let destination = add_right_pane(&mut owner, source, key(&connection, 2));
    // Unequal actual sizes make a location swap require PTY resizes.
    owner
        .layout
        .as_mut()
        .unwrap()
        .set_divider(source, 2, 0, 39)
        .unwrap();
    owner
        .resize_layout(3, 24, 82, |_, _, _| Ok::<_, ()>(()))
        .unwrap();
    let before = owner.layout_snapshot().unwrap();
    let source_rect = before
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == source)
        .unwrap()
        .rect;
    let destination_rect = before
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == destination)
        .unwrap()
        .rect;
    let swap = PaneOperationRequest::Swap(PaneSwap {
        key: key(&connection, 3),
        expected_layout_revision: before.revision,
        source,
        destination,
    });
    let swap_payload = encode_request(&swap).unwrap();
    let mut swap_sizes = Vec::new();
    owner
        .handle_pane_operation(&connection, swap, &swap_payload, NOW, |_, rows, columns| {
            swap_sizes.push((rows, columns));
            Ok::<_, ()>(())
        })
        .unwrap();
    let swapped = owner.layout_snapshot().unwrap();
    assert_eq!(swapped.revision, before.revision + 1);
    assert_eq!(
        swapped
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == source)
            .unwrap()
            .rect,
        destination_rect
    );
    assert_eq!(
        swapped
            .panes
            .iter()
            .flatten()
            .find(|pane| pane.target == destination)
            .unwrap()
            .rect,
        source_rect
    );
    assert_eq!(swap_sizes.len(), 2);

    let info = owner
        .layout
        .as_ref()
        .unwrap()
        .divider_info(source, swapped.revision, 0)
        .unwrap();
    let divider = PaneOperationRequest::DividerSet(PaneDividerSet {
        key: key(&connection, 4),
        expected_layout_revision: swapped.revision,
        target: source,
        divider_index: 0,
        first_cells: info.first_span + 1,
    });
    let divider_payload = encode_request(&divider).unwrap();
    let mut divider_sizes = Vec::new();
    owner
        .handle_pane_operation(
            &connection,
            divider,
            &divider_payload,
            NOW,
            |_, rows, columns| {
                divider_sizes.push((rows, columns));
                Ok::<_, ()>(())
            },
        )
        .unwrap();
    let adjusted = owner.layout_snapshot().unwrap();
    assert_eq!(adjusted.revision, swapped.revision + 1);
    assert_eq!(divider_sizes.len(), 2);
    assert!(divider_sizes.iter().any(|(_, columns)| *columns != 38));
}

#[test]
fn root_resize_and_split_require_input_capability_and_own_lease() {
    let (mut owner, first) = owner_with_first(false);
    let connection = connect(&mut owner, first, 1);
    let request = RootResize {
        key: key(&connection, 1),
        expected_layout_revision: 1,
        rows: 30,
        columns: 100,
    };
    let payload = crate::ui_protocol::encode_root_resize(&request).unwrap();
    let RootResizeReply::Response(bytes) = owner
        .handle_root_resize(&connection, request, &payload, NOW, |_: &mut Sink, _, _| {
            Ok::<_, ()>(())
        })
        .unwrap()
    else {
        panic!("rejected");
    };
    assert_eq!(
        decode_response_payload(&bytes).unwrap().body,
        ResponseBody::Rejected(RejectionCode::Unauthorized)
    );
    assert_eq!(owner.layout_snapshot().unwrap().revision, 1);

    let launch_request = LaunchRequest {
        key: key(&connection, 2),
        rows: 24,
        columns: 40,
        requested_executable: b"/bin/zsh",
        requested_working_directory: b"/",
        client_working_directory: b"/",
        arguments: &[],
        environment: &[],
    };
    let mut split = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: key(&connection, 2),
            target: first,
            expected_layout_revision: 1,
            direction: SplitDirection::Right,
            launch: &launch_request,
        },
        &mut split,
    )
    .unwrap();
    let view = decode_split_request(&split).unwrap();
    assert_eq!(
        owner.authorize_split(&connection, &view, NOW),
        Err(ServerError::Unauthorized)
    );

    let (mut owner, first) = owner_with_first(true);
    let connection = connect(&mut owner, first, 1);
    let view = decode_split_request(&split).unwrap();
    // Both owners issue the same first client lease, so the bytes apply.
    assert_eq!(view.key().lease, connection.lease());
    assert_eq!(owner.authorize_split(&connection, &view, NOW), Ok(()));
    let other = TargetRef {
        pane: 5,
        session: 5,
        ..first
    };
    let mut scoped = Vec::new();
    encode_split_request(
        &SplitRequest {
            key: key(&connection, 2),
            target: other,
            expected_layout_revision: 1,
            direction: SplitDirection::Right,
            launch: &launch_request,
        },
        &mut scoped,
    )
    .unwrap();
    assert_eq!(
        owner.authorize_split(&connection, &decode_split_request(&scoped).unwrap(), NOW),
        Err(ServerError::Unauthorized)
    );
}

#[test]
fn observation_is_scoped_and_layout_is_shared_with_read_connections() {
    let (mut owner, first) = owner_with_first(false);
    let connection = connect(&mut owner, first, 1);
    assert_eq!(
        owner
            .observe_target(&connection, first, NOW, |sink| sink.delivered.len())
            .unwrap(),
        0
    );
    let other = TargetRef {
        pane: 4,
        session: 4,
        ..first
    };
    assert_eq!(
        owner
            .observe_target(&connection, other, NOW, |_| ())
            .unwrap_err(),
        ServerError::Unauthorized
    );
    let (layout, pending) = owner.layout_view(&connection, NOW).unwrap().unwrap();
    assert_eq!(layout.panes[0].unwrap().target, first);
    assert!(pending.iter().all(Option::is_none));
    assert_eq!(owner.first_pane_target(), Some(first));
    assert_eq!(owner.lease_target(connection.lease()), Some(first));
    assert_eq!(
        owner.lease_target(INTERNAL),
        None,
        "internal lease has no grant"
    );
    // Launch status for another lease's key is unauthorized.
    assert_eq!(
        owner
            .launch_status(
                &connection,
                OperationKey {
                    generation: GENERATION,
                    lease: INTERNAL,
                    sequence: 1,
                },
                NOW,
            )
            .unwrap_err(),
        ServerError::Unauthorized
    );
}
