use super::*;
use crossterm_input::Decoder;
use crossterm_input::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use panelink::pane_operations_protocol::PaneOperationsView;
use panelink::protocol::OperationKey;
use panelink::screen_checkpoint::{
    decode, encode, CheckpointMetadata, SCREEN_CHECKPOINT_CELL_BYTES,
    SCREEN_CHECKPOINT_HEADER_BYTES,
};
use panelink::split_menu::DisabledReason;
use panelink::ui_protocol::UiPane;
use panelink::workspace_index::{ProjectId, WorkspaceIndex, WorkspaceTabId};
use panelink::workspace_tabs::{TabAction, WorkspaceRouteId};

fn target(pane: u64) -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane,
        session: pane,
        occupant_revision: 1,
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

fn workspace_catalog() -> (WorkspaceIndex, WorkspaceRouteId, WorkspaceRouteId) {
    let project_a = ProjectId::new(11).unwrap();
    let project_b = ProjectId::new(22).unwrap();
    let tab_a = WorkspaceTabId::new(101).unwrap();
    let tab_b = WorkspaceTabId::new(102).unwrap();
    let tab_c = WorkspaceTabId::new(201).unwrap();
    let mut index = WorkspaceIndex::new();
    index
        .add_project(project_a, "Editors", tab_a, "Main")
        .unwrap();
    index.add_tab(project_a, tab_b, "Review").unwrap();
    index
        .add_project(project_b, "Tools", tab_c, "Shell")
        .unwrap();
    (
        index,
        WorkspaceRouteId {
            project: project_a,
            tab: tab_a,
        },
        WorkspaceRouteId {
            project: project_a,
            tab: tab_b,
        },
    )
}

fn one_pane(revision: u64) -> LayoutReply {
    LayoutReply {
        revision,
        rows: 24,
        columns: 81,
        panes: [
            Some(UiPane {
                target: target(1),
                rect: rect(0, 0, 24, 81),
                pending_confirmed: None,
            }),
            None,
            None,
            None,
        ],
        divider: None,
        dividers: [None; panelink::pane_layout::MAX_PANES - 1],
    }
}

fn two_panes() -> LayoutReply {
    LayoutReply {
        revision: 2,
        rows: 24,
        columns: 81,
        panes: [
            Some(UiPane {
                target: target(1),
                rect: rect(0, 0, 24, 40),
                pending_confirmed: None,
            }),
            Some(UiPane {
                target: target(2),
                rect: rect(41, 0, 24, 40),
                pending_confirmed: None,
            }),
            None,
            None,
        ],
        divider: Some(rect(40, 0, 24, 1)),
        dividers: [Some(rect(40, 0, 24, 1)), None, None],
    }
}

fn operations_view(layout: LayoutReply, zoomed: Option<TargetRef>) -> PaneOperationsView {
    let mut view = PaneOperationsView {
        layout,
        zoomed_target: zoomed,
        confirmed_sizes: [None; panelink::pane_layout::MAX_PANES],
    };
    for (index, pane) in layout.panes.iter().enumerate() {
        let Some(pane) = pane else { continue };
        let underlying = (pane.rect.rows - 2, pane.rect.columns - 2);
        let size = if zoomed == Some(pane.target) {
            (layout.rows - 2, layout.columns - 2)
        } else {
            underlying
        };
        if size != underlying {
            view.layout.panes[index].as_mut().unwrap().pending_confirmed = Some(size);
        }
        view.confirmed_sizes[index] = Some(size);
    }
    view
}

#[derive(Default)]
struct Fake {
    keys: Vec<(TargetRef, Vec<Key>)>,
    pastes: Vec<TargetRef>,
    scrolls: Vec<(TargetRef, u64, u16, u16, u16, bool)>,
    splits: Vec<SplitPlan>,
    split_reply: Option<Result<TargetRef, &'static str>>,
    split_async: bool,
    split_request: Option<(u64, u64)>,
    reject_keys: bool,
    closes: Vec<ClosePlan>,
    close_reply: Option<Result<CloseOutcome, &'static str>>,
    recovery_available: bool,
    recovery_checks: usize,
    pane_operations: Vec<PaneAction>,
    reject_pane_operations: bool,
}

impl Effects for Fake {
    fn send_keys(&mut self, target: TargetRef, keys: &[Key]) -> Result<(), &'static str> {
        if self.reject_keys {
            return Err("client busy: request queue full");
        }
        self.keys.push((target, keys.to_vec()));
        Ok(())
    }
    fn send_paste(&mut self, target: TargetRef, _text: &str) -> Result<(), &'static str> {
        self.pastes.push(target);
        Ok(())
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
        self.scrolls
            .push((target, epoch, offset_rows, column, row, up));
        Ok(())
    }
    fn split(
        &mut self,
        plan: SplitPlan,
        request_id: u64,
        focus_generation: u64,
    ) -> Result<Option<TargetRef>, &'static str> {
        self.splits.push(plan);
        self.split_request = Some((request_id, focus_generation));
        if self.split_async {
            Ok(None)
        } else {
            self.split_reply.unwrap_or(Ok(target(2))).map(Some)
        }
    }
    fn close(
        &mut self,
        plan: ClosePlan,
        _request_id: u64,
    ) -> Result<Option<CloseResult>, &'static str> {
        self.closes.push(plan);
        let outcome = self
            .close_reply
            .unwrap_or(Ok(CloseOutcome::Stopped(StopOutcome::Exited)))?;
        Ok(Some(CloseResult {
            key: OperationKey {
                generation: 7,
                lease: 1,
                sequence: self.closes.len() as u64,
            },
            outcome,
            target: Some(plan.target),
            layout_revision: plan.expected_layout_revision + 1,
            survivor_resize_pending: false,
        }))
    }

    fn pane_operation(&mut self, _id: u64, action: PaneAction) -> Result<(), &'static str> {
        if self.reject_pane_operations {
            return Err("client request queue full");
        }
        self.pane_operations.push(action);
        Ok(())
    }
}

fn feed(app: &mut App, fake: &mut Fake, bytes: &[u8]) -> Flow {
    let mut decoder = Decoder::new();
    let now = Instant::now();
    let mut flow = Flow::Continue;
    for (index, byte) in bytes.iter().enumerate() {
        if let Some(event) = decoder.advance(*byte, index + 1 < bytes.len()).unwrap() {
            let needs_check = app.needs_recovery_availability(&event);
            if needs_check {
                fake.recovery_checks += 1;
            }
            let availability = if needs_check && fake.recovery_available {
                Availability::Enabled
            } else {
                Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
            };
            flow = app.handle_event(&event, now, availability, fake);
        }
    }
    if let Some(event) = decoder.finish_escape().unwrap() {
        let needs_check = app.needs_recovery_availability(&event);
        if needs_check {
            fake.recovery_checks += 1;
        }
        let availability = if needs_check && fake.recovery_available {
            Availability::Enabled
        } else {
            Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
        };
        flow = app.handle_event(&event, now, availability, fake);
    }
    flow
}

fn feed_events(app: &mut App, fake: &mut Fake, bytes: &[u8]) -> Vec<Flow> {
    let mut decoder = Decoder::new();
    let now = Instant::now();
    let mut flows = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if let Some(event) = decoder.advance(*byte, index + 1 < bytes.len()).unwrap() {
            let needs_check = app.needs_recovery_availability(&event);
            if needs_check {
                fake.recovery_checks += 1;
            }
            let availability = if needs_check && fake.recovery_available {
                Availability::Enabled
            } else {
                Availability::Disabled(DisabledReason::NoPreviousLayoutSaved)
            };
            flows.push(app.handle_event(&event, now, availability, fake));
        }
    }
    flows
}

#[test]
fn typed_keys_batch_to_the_focused_pane_and_flush_on_focus_change() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    assert_eq!(app.focus(), Some(target(1)));
    feed(&mut app, &mut fake, b"ab");
    assert!(fake.keys.is_empty(), "batched until flush");
    // Left click on pane 2 flushes pane-1 keys, then moves local focus.
    feed(&mut app, &mut fake, b"\x1b[<0;50;3M\x1b[<0;50;3m");
    assert_eq!(app.focus(), Some(target(2)));
    feed(&mut app, &mut fake, b"c\r");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(
        fake.keys,
        vec![
            (target(1), vec![Key::Char('a'), Key::Char('b')]),
            (target(2), vec![Key::Char('c'), Key::Enter]),
        ]
    );
    // A divider click changes nothing.
    feed(&mut app, &mut fake, b"\x1b[<0;41;3M");
    assert_eq!(app.focus(), Some(target(2)));
}

#[test]
fn retained_key_batch_stays_pinned_and_blocks_later_paste_and_mutation() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    feed(&mut app, &mut fake, b"a");
    fake.reject_keys = true;
    app.flush(Instant::now(), &mut fake);
    assert!(app.retry_pending_keys());

    // A focus change does not retarget the retained batch. Paste is held
    // behind it and a split request cannot pass it either.
    app.focus_target(target(2));
    feed(&mut app, &mut fake, b"\x1b[200~paste\x1b[201~");
    assert!(fake.pastes.is_empty());
    feed(&mut app, &mut fake, b"\x02xy");
    assert!(fake.closes.is_empty());

    fake.reject_keys = false;
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('a')])]);
    assert_eq!(app.focus(), Some(target(2)));
}

#[test]
fn stale_split_completion_cannot_override_focus_moved_before_layout_update() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        split_async: true,
        ..Fake::default()
    };
    app.set_layout(one_pane(5));
    feed(&mut app, &mut fake, b"\x1b[<2;10;5M\x1b[<2;10;5m r");
    let (request_id, focus_generation) = fake.split_request.unwrap();
    assert_eq!(app.focus(), Some(target(1)));

    // A deliberate local focus choice can happen before the new layout poll.
    app.focus_target(target(9));
    app.complete_split(
        request_id,
        focus_generation,
        target(1),
        Ok(target(2)),
        Instant::now(),
    );
    assert_eq!(app.focus(), Some(target(9)));
}

#[test]
fn split_completion_waits_for_newer_validated_layout_before_focusing_target() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        split_async: true,
        ..Fake::default()
    };
    app.set_layout(one_pane(5));
    feed(&mut app, &mut fake, b"\x1b[<2;10;5M\x1b[<2;10;5m r");
    let (request_id, focus_generation) = fake.split_request.unwrap();

    app.complete_split(
        request_id,
        focus_generation,
        target(1),
        Ok(target(2)),
        Instant::now(),
    );
    assert_eq!(app.focus(), Some(target(1)));

    // A delayed poll captured before the split cannot erase its focus intent.
    app.set_layout(one_pane(5));
    assert_eq!(app.focus(), Some(target(1)));
    assert_eq!(app.layout().unwrap().revision, 5);

    let mut layout = two_panes();
    layout.revision = 6;
    app.set_layout(layout);
    assert_eq!(app.focus(), Some(target(2)));
    assert_eq!(
        app.status.map(|(message, _)| message),
        Some("split applied; new pane focused")
    );
}

#[test]
fn ctrl_x_quits_even_when_a_key_batch_is_retained() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        reject_keys: true,
        ..Fake::default()
    };
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"a");
    app.flush(Instant::now(), &mut fake);
    assert!(app.retry_pending_keys());

    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(app.abandon_pending_keys());
    assert!(!app.retry_pending_keys());
}

#[test]
fn right_click_opens_menu_for_pane_under_pointer_and_menu_inputs_never_reach_panes() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    app.focus_target(target(2));
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    assert!(app.menu_open());
    assert_eq!(app.menu.captured_target(), Some(target(1)));
    // Every kind of input while open is consumed: keys, Enter on a disabled
    // item (Split Left: a 40-column pane is too narrow), paste, Ctrl-X,
    // mouse, arrows. Home selects the first (disabled) item.
    let flow = feed(
        &mut app,
        &mut fake,
        b"xyz\x1b[H\r\x1b[B\x1b[200~paste\x1b[201~\x18\x03\x1b[<2;5;5m",
    );
    assert_eq!(flow, Flow::Continue);
    assert!(app.menu_open(), "disabled items keep the menu open");
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.splits.is_empty());
    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());
    feed(&mut app, &mut fake, b"\x1b");
    assert!(!app.menu_open());
    app.flush(Instant::now(), &mut fake);
    assert!(
        fake.keys.is_empty(),
        "Esc closing the menu is not forwarded"
    );
    // A left press outside the overlay closes the menu; it is consumed by the
    // menu and does not move focus to the pane under the pointer.
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    assert!(app.menu_open());
    feed(&mut app, &mut fake, b"\x1b[<0;70;20M");
    assert!(!app.menu_open());
    assert_eq!(
        app.focus(),
        Some(target(2)),
        "menu-owned click does not refocus"
    );
}

#[test]
fn recovery_menu_and_navigation_request_viewer_without_forwarding_keys() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        recovery_available: true,
        ..Fake::default()
    };
    app.set_layout(one_pane(1));

    feed(&mut app, &mut fake, b"\x02s");
    assert!(app.menu_open());
    assert_eq!(
        app.menu.recovery_availability(),
        Some(Availability::Enabled)
    );
    let flows = feed_events(&mut app, &mut fake, b"v");
    assert_eq!(flows, vec![Flow::ViewRecovery]);
    assert!(!app.menu_open());
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty());

    let flows = feed_events(&mut app, &mut fake, b"\x02v");
    assert_eq!(flows.last(), Some(&Flow::ViewRecovery));
    app.flush(Instant::now(), &mut fake);
    assert!(
        fake.keys.is_empty(),
        "Ctrl-B v is local and never forwarded"
    );
}

#[test]
fn disabled_recovery_menu_item_reports_status_and_never_forwards_bytes() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let flows = feed_events(&mut app, &mut fake, b"\x1b[<2;5;5M");
    assert!(flows.iter().all(|flow| *flow == Flow::Continue));
    assert!(app.menu_open());
    assert_eq!(
        app.menu.recovery_availability(),
        Some(Availability::Disabled(
            DisabledReason::NoPreviousLayoutSaved
        ))
    );
    feed(&mut app, &mut fake, b"\x1b[B");
    let flows = feed_events(&mut app, &mut fake, b"v");
    assert!(flows.iter().all(|flow| *flow == Flow::Continue));
    assert!(app.menu_open(), "disabled recovery stays in the menu");
    assert_eq!(status(&app), Some("No previous layout saved"));
    let flows = feed_events(&mut app, &mut fake, b"\x02v");
    assert!(flows.last().is_some_and(|flow| *flow == Flow::Continue));
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty());
    assert!(fake.pastes.is_empty());
}

#[test]
fn split_intent_uses_captured_target_and_server_revision_then_focuses_new_pane() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(5));
    feed(&mut app, &mut fake, b"\x1b[<2;10;5M\x1b[<2;10;5m");
    assert!(app.menu_open());
    // Layout changes while the menu is open: the captured revision stays 5.
    app.set_layout(one_pane(6));
    feed(&mut app, &mut fake, b"r");
    assert!(!app.menu_open());
    assert_eq!(
        fake.splits,
        vec![SplitPlan {
            target: target(1),
            expected_layout_revision: 5,
            direction: SplitDirection::Right,
            rows: 22,
            columns: 38,
        }]
    );
    assert_eq!(app.focus(), Some(target(1)));
    let mut split_layout = two_panes();
    split_layout.revision = 7;
    app.set_layout(split_layout);
    assert_eq!(app.focus(), Some(target(2)));
    assert!(fake.keys.is_empty());
}

#[test]
fn keyboard_fallback_ctrl_b_s_opens_for_focused_and_esc_closes_without_bytes() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x02s");
    assert!(app.menu_open());
    assert_eq!(app.menu.captured_target(), Some(target(1)));
    feed(&mut app, &mut fake, b"\x1b");
    assert!(!app.menu_open());
    // Ctrl-B followed by another key is dropped locally.
    feed(&mut app, &mut fake, b"\x02q");
    assert!(!app.menu_open());
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty());
    feed(&mut app, &mut fake, b"\x02s\x1b[B\x1b[B\r");
    assert_eq!(fake.splits.len(), 1);
    assert_eq!(fake.splits[0].direction, SplitDirection::Up);
    assert_eq!(fake.splits[0].rows, 9);
}

#[test]
fn split_failure_shows_status_and_keeps_focus() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        split_reply: Some(Err("split rejected: layout changed")),
        ..Fake::default()
    };
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x02sd");
    assert_eq!(app.focus(), Some(target(1)));
    assert_eq!(
        app.status.map(|s| s.0),
        Some("split rejected: layout changed")
    );
}

#[test]
fn ctrl_x_quits_after_flushing_and_paste_goes_to_focus() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x1b[200~hi\x1b[201~");
    assert_eq!(fake.pastes, vec![target(1)]);
    assert_eq!(feed(&mut app, &mut fake, b"q\x18"), Flow::Quit);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('q')])]);
}

#[test]
fn wheel_routes_to_outer_pane_with_content_local_coordinates_and_typing_invalidates_reply() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let event = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_event(
        &event,
        Instant::now(),
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert_eq!(fake.scrolls.len(), 1);
    let (scroll_target, epoch, offset, column, row, up) = fake.scrolls[0];
    assert_eq!(scroll_target, target(1));
    assert_eq!((offset, column, row, up), (3, 0, 0, true));

    // A reply from the pending query is rejected after typing returns the
    // pane to its live viewport, even if the network response arrives later.
    let checkpoint = blank_checkpoint(24, 81, 1);
    feed(&mut app, &mut fake, b"x");
    app.apply_scroll_update(
        ScrollUpdate {
            target: target(1),
            epoch,
            result: ScrollResult::History {
                revision: 1,
                offset_rows: 3,
                checkpoint,
                row_wraps: None,
            },
        },
        Instant::now(),
    );
    let slot = app
        .screens
        .iter()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    assert_eq!(slot.history_offset, 0);
}

#[test]
fn wheel_down_at_live_offset_still_queries_mode_and_pending_bursts_do_not_invalidate() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let event = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        })
    };
    let availability = Availability::Disabled(DisabledReason::NoPreviousLayoutSaved);
    app.handle_event(
        &event(MouseEventKind::ScrollDown),
        Instant::now(),
        availability,
        &mut fake,
    );
    assert_eq!(fake.scrolls.len(), 1);
    let (_, epoch, offset, _, _, up) = fake.scrolls[0];
    assert_eq!((offset, up), (0, false));

    // The first bounded history request remains current while a second tick
    // arrives before its large snapshot result is applied.
    app.handle_event(
        &event(MouseEventKind::ScrollUp),
        Instant::now(),
        availability,
        &mut fake,
    );
    assert_eq!(fake.scrolls.len(), 1);
    assert_eq!(
        app.screens
            .iter()
            .find(|slot| slot.target == Some(target(1)))
            .unwrap()
            .view_epoch,
        epoch
    );
}

#[test]
fn closing_pane_discards_late_history_reply() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    let event = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_event(
        &event,
        Instant::now(),
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    let (_, epoch, ..) = fake.scrolls[0];
    app.set_layout(survivor_only());
    app.apply_scroll_update(
        ScrollUpdate {
            target: target(1),
            epoch,
            result: ScrollResult::History {
                revision: 1,
                offset_rows: 3,
                checkpoint: blank_checkpoint(24, 40, 1),
                row_wraps: None,
            },
        },
        Instant::now(),
    );
    assert!(app
        .screens
        .iter()
        .all(|slot| slot.target != Some(target(1))));
}

#[test]
fn negotiated_row_wraps_follow_the_exact_live_and_history_checkpoint() {
    let mut app = App::new(24, 81).unwrap();
    app.set_layout(one_pane(1));
    let live_checkpoint = blank_checkpoint(24, 81, 1);
    let live = decode(&live_checkpoint).unwrap();
    let mut live_wraps = vec![false; usize::from(live.rows())];
    live_wraps[3] = true;
    app.load_screen_with_wraps(target(1), 8, &live, Some(&live_wraps))
        .unwrap();
    let slot = app
        .screens
        .iter()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    assert_eq!(slot.snapshot.row_wrapped(3), Some(true));
    assert_eq!(slot.snapshot.row_wrapped(2), Some(false));

    let slot = app
        .screens
        .iter_mut()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    slot.view_epoch = 12;
    slot.history_pending = true;
    let history_checkpoint = blank_checkpoint(24, 81, 2);
    let history = decode(&history_checkpoint).unwrap();
    let mut history_wraps = vec![false; usize::from(history.rows())];
    history_wraps[7] = true;
    app.apply_scroll_update(
        ScrollUpdate {
            target: target(1),
            epoch: 12,
            result: ScrollResult::History {
                revision: 9,
                offset_rows: 3,
                checkpoint: history_checkpoint,
                row_wraps: Some(history_wraps),
            },
        },
        Instant::now(),
    );
    let slot = app
        .screens
        .iter()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    assert_eq!(slot.history.row_wrapped(7), Some(true));
    assert_eq!(slot.history.row_wrapped(6), Some(false));
    assert_eq!(slot.history_revision, 9);
}

#[test]
fn selection_drag_is_frozen_local_and_cancelled_by_layout_change() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = blank_checkpoint(24, 81, 1);
    let decoded = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(decoded.rows())];
    app.load_screen_with_wraps(target(1), 1, &decoded, Some(&wraps))
        .unwrap();

    let event = |kind, column, row| {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    let now = Instant::now();
    app.handle_event(
        &event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.selection.is_some());
    app.handle_event(
        &event(MouseEventKind::Drag(MouseButton::Left), 2, 1),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );

    // A live output revision does not replace the frozen drag source.
    app.load_screen_with_wraps(target(1), 2, &decoded, Some(&wraps))
        .unwrap();
    assert!(app.selection.is_some());
    app.handle_event(
        &event(MouseEventKind::Up(MouseButton::Left), 2, 1),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.selection.is_none());
    assert!(app.take_copy_text().is_none()); // blank cells trim to an empty copy
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.scrolls.is_empty());

    app.handle_event(
        &event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.selection.is_some());
    let mut resized = one_pane(2);
    resized.columns -= 1;
    resized.panes[0].as_mut().unwrap().rect.columns -= 1;
    app.set_layout(resized);
    assert!(app.selection.is_none());

    app.set_layout(one_pane(3));
    app.load_screen_with_wraps(target(1), 3, &decoded, Some(&wraps))
        .unwrap();
    app.handle_event(
        &event(MouseEventKind::Down(MouseButton::Left), 1, 1),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.selection.is_some());
    app.load_screen(target(1), 4, &decoded).unwrap();
    assert!(app.selection.is_none()); // loss of negotiated wrap metadata cancels
}

#[test]
fn keyboard_copy_consumes_input_and_copies_the_frozen_live_view_once() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let original_bytes = text_checkpoint("abc", 1);
    let original = decode(&original_bytes).unwrap();
    let wraps = vec![false; usize::from(original.rows())];
    app.load_screen_with_wraps(target(1), 1, &original, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);

    let now = Instant::now();
    app.handle_event(
        &Event::Paste("paste must stay local".to_owned()),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    app.handle_event(
        &Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 50,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.scrolls.is_empty());

    let revised_bytes = text_checkpoint("XYZ", 1);
    let revised = decode(&revised_bytes).unwrap();
    app.load_screen_with_wraps(target(1), 2, &revised, Some(&wraps))
        .unwrap();
    let visible = screen_text(&app);
    assert!(
        visible.contains("abc"),
        "copy mode should render its frozen source: {visible:?}"
    );
    assert!(
        !visible.contains("XYZ"),
        "live revision leaked into frozen view: {visible:?}"
    );

    feed(&mut app, &mut fake, b" ll");
    let repeated_y = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('y'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    app.handle_event(
        &repeated_y,
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.copy_mode, "repeat must not complete one-shot copy");
    assert!(app.pending_copy.is_none());
    feed(&mut app, &mut fake, b"y");
    assert_eq!(app.take_copy_text().as_deref(), Some("abc"));
    assert!(app.take_copy_text().is_none());
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.scrolls.is_empty());
}

#[test]
fn keyboard_copy_swallows_exit_key_repeat_until_release_but_allows_new_presses() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = text_checkpoint("abc", 1);
    let snapshot = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();

    feed(&mut app, &mut fake, b"\x02c ly");
    assert!(!app.copy_mode);
    assert_eq!(app.take_copy_text().as_deref(), Some("ab"));
    let now = Instant::now();
    for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
        app.handle_event(
            &Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                kind,
            )),
            now,
            Availability::Enabled,
            &mut fake,
        );
    }
    assert!(fake.keys.is_empty(), "copy gesture events must stay local");

    app.handle_event(
        &Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        now,
        Availability::Enabled,
        &mut fake,
    );
    app.flush(now, &mut fake);
    assert_eq!(fake.keys.len(), 1);
    assert_eq!(fake.keys[0].1, vec![Key::Char('z')]);
}

#[test]
fn keyboard_copy_skips_recovery_probe_and_keeps_caret_visible_under_worker_status() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = text_checkpoint("abc", 1);
    let snapshot = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);

    let right_click = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 4,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    if app.needs_recovery_availability(&right_click) {
        fake.recovery_checks += 1;
    }
    app.handle_event(
        &right_click,
        Instant::now(),
        Availability::Enabled,
        &mut fake,
    );
    assert_eq!(fake.recovery_checks, 0);
    assert!(fake.keys.is_empty());

    app.report_worker_status("late worker notice", Instant::now());
    let mut compositor = Compositor::try_new().unwrap();
    let mut bytes = Vec::new();
    app.render(&mut compositor, &mut bytes, Instant::now())
        .unwrap();
    let mut parser = vt100::Parser::new(24, 81, 0);
    parser.process(&bytes);
    assert!(parser.screen().cell(1, 1).unwrap().inverse());
    assert!(!screen_text(&app).contains("late worker notice"));
}

#[test]
fn keyboard_copy_empty_selection_and_escape_queue_nothing() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = text_checkpoint("abc", 1);
    let snapshot = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();

    feed(&mut app, &mut fake, b"\x02c\r");
    assert!(!app.copy_mode);
    assert!(app.take_copy_text().is_none());
    feed(&mut app, &mut fake, b"\x02c\x1b");
    assert!(!app.copy_mode);
    assert!(app.take_copy_text().is_none());
    feed(&mut app, &mut fake, b"\x02c");
    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(!app.copy_mode);
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.scrolls.is_empty());
}

#[test]
fn keyboard_copy_requires_wrap_metadata_and_cancels_on_resize_or_focus_change() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = text_checkpoint("abc", 1);
    let snapshot = decode(&checkpoint).unwrap();
    app.load_screen(target(1), 1, &snapshot).unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(!app.copy_mode);
    assert_eq!(app.status.map(|(label, _)| label), Some("copy unavailable"));

    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 2, &snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    app.set_host(25, 82);
    assert!(!app.copy_mode);
    assert!(app.take_copy_text().is_none());

    app.set_host(24, 81);
    app.set_layout(two_panes());
    let narrow_checkpoint = text_checkpoint_size("abc", 1, 22, 38);
    let narrow = decode(&narrow_checkpoint).unwrap();
    let narrow_wraps = vec![false; usize::from(narrow.rows())];
    app.load_screen_with_wraps(target(1), 3, &narrow, Some(&narrow_wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    app.focus_target(target(2));
    assert!(!app.copy_mode);
    assert!(app.take_copy_text().is_none());
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());

    app.focus_target(target(1));
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    let mut changed_wraps = narrow_wraps.clone();
    changed_wraps[0] = true;
    app.load_screen_with_wraps(target(1), 4, &narrow, Some(&changed_wraps))
        .unwrap();
    assert!(!app.copy_mode);
}

#[test]
fn keyboard_copy_uses_the_visible_history_snapshot() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let live_checkpoint = text_checkpoint("LIVE", 1);
    let live = decode(&live_checkpoint).unwrap();
    let wraps = vec![false; usize::from(live.rows())];
    app.load_screen_with_wraps(target(1), 1, &live, Some(&wraps))
        .unwrap();
    let history_checkpoint = text_checkpoint("HIST", 1);
    let slot = app
        .screens
        .iter_mut()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    slot.view_epoch = 7;
    slot.history_pending = true;
    app.apply_scroll_update(
        ScrollUpdate {
            target: target(1),
            epoch: 7,
            result: ScrollResult::History {
                revision: 4,
                offset_rows: 3,
                checkpoint: history_checkpoint,
                row_wraps: Some(wraps.clone()),
            },
        },
        Instant::now(),
    );
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.selection.as_ref().unwrap().source_history);
    let visible = screen_text(&app);
    assert!(
        visible.contains("HIST"),
        "history source was not rendered: {visible:?}"
    );
    assert!(
        !visible.contains("LIVE"),
        "live source replaced visible history: {visible:?}"
    );
    feed(&mut app, &mut fake, b" llll");
    feed(&mut app, &mut fake, b"y");
    assert_eq!(app.take_copy_text().as_deref(), Some("HIST"));
}

#[test]
fn keyboard_copy_preserves_unicode_and_hard_or_soft_line_boundaries() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let checkpoint = text_checkpoint("界e\u{301}x\r\nZ", 1);
    let snapshot = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c ");
    feed(&mut app, &mut fake, b"lll\x1b[B");
    feed(&mut app, &mut fake, b"y");
    assert_eq!(app.take_copy_text().as_deref(), Some("界e\u{301}x\nZ"));

    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let long = format!("{}Z", "x".repeat(79));
    let checkpoint = text_checkpoint(&long, 1);
    let snapshot = decode(&checkpoint).unwrap();
    let mut wraps = vec![false; usize::from(snapshot.rows())];
    wraps[0] = true;
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c ");
    let rightward = vec![b'l'; 79];
    feed(&mut app, &mut fake, &rightward);
    feed(&mut app, &mut fake, b"y");
    assert_eq!(app.take_copy_text().as_deref(), Some(long.as_str()));
}

#[test]
fn keyboard_copy_caret_respects_the_workspace_header_offset() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    let mut layout = one_pane(1);
    layout.rows = 23;
    layout.panes[0].as_mut().unwrap().rect.rows = 23;
    app.set_layout(layout);
    let (index, route, _) = workspace_catalog();
    app.install_workspace_tabs(index, route);
    let checkpoint = text_checkpoint_size("abc", 1, 21, 79);
    let snapshot = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(snapshot.rows())];
    app.load_screen_with_wraps(target(1), 1, &snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    let mut compositor = Compositor::try_new().unwrap();
    let mut bytes = Vec::new();
    app.render(&mut compositor, &mut bytes, Instant::now())
        .unwrap();
    let mut parser = vt100::Parser::new(24, 81, 0);
    parser.process(&bytes);
    assert!(parser.screen().cell(2, 1).unwrap().inverse());
    // The project control is inverse even outside copy mode. Its blank cell
    // must stay in the header while the copy caret highlights pane content.
    assert_eq!(parser.screen().cell(0, 1).unwrap().contents(), " ");
}

#[test]
fn keyboard_copy_cancels_before_buffer_route_or_modal_identity_changes() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let normal_bytes = text_checkpoint("normal", 1);
    let normal = decode(&normal_bytes).unwrap();
    let wraps = vec![false; usize::from(normal.rows())];
    app.load_screen_with_wraps(target(1), 1, &normal, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    let mut alternate_bytes = text_checkpoint("alternate", 1);
    alternate_bytes[80] = 1;
    let alternate = decode(&alternate_bytes).unwrap();
    app.load_screen_with_wraps(target(1), 2, &alternate, Some(&wraps))
        .unwrap();
    assert!(!app.copy_mode);

    let mut tab_layout = one_pane(3);
    tab_layout.rows = 23;
    tab_layout.panes[0].as_mut().unwrap().rect.rows = 23;
    app.set_layout(tab_layout);
    let tab_checkpoint = text_checkpoint_size("normal", 1, 21, 79);
    let tab_snapshot = decode(&tab_checkpoint).unwrap();
    let tab_wraps = vec![false; usize::from(tab_snapshot.rows())];
    app.load_screen_with_wraps(target(1), 3, &tab_snapshot, Some(&tab_wraps))
        .unwrap();
    let (index, route, _) = workspace_catalog();
    app.install_workspace_tabs(index, route);
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    let (next_index, _, next_route) = workspace_catalog();
    app.install_workspace_tabs(next_index, next_route);
    assert!(!app.copy_mode);

    let (base_index, base_route, _) = workspace_catalog();
    app.install_workspace_tabs(base_index, base_route);
    feed(&mut app, &mut fake, b"\x02c");
    assert!(app.copy_mode);
    app.open_for_focused(
        Instant::now(),
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
    );
    assert!(!app.copy_mode);
    assert!(app.menu_open());
    assert!(app.take_copy_text().is_none());
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());
}

#[test]
fn ordinary_alternate_screen_revision_preserves_frozen_selection_and_view_epoch() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let mut checkpoint = blank_checkpoint(24, 81, 1);
    checkpoint[80] = 1;
    let decoded = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(decoded.rows())];
    app.load_screen_with_wraps(target(1), 1, &decoded, Some(&wraps))
        .unwrap();
    let epoch = app
        .screens
        .iter()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap()
        .view_epoch;

    let now = Instant::now();
    app.handle_event(
        &Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }),
        now,
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(app.selection.is_some());

    app.load_screen_with_wraps(target(1), 2, &decoded, Some(&wraps))
        .unwrap();
    assert!(app.selection.is_some());
    assert_eq!(
        app.screens
            .iter()
            .find(|slot| slot.target == Some(target(1)))
            .unwrap()
            .view_epoch,
        epoch
    );
}

#[test]
fn alternate_live_screen_and_same_target_resize_invalidate_history_replies() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    let event = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_event(
        &event,
        Instant::now(),
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    let (_, pending_epoch, ..) = fake.scrolls[0];

    let mut resized = one_pane(2);
    resized.columns = 80;
    resized.panes[0].as_mut().unwrap().rect.columns = 80;
    app.set_layout(resized);
    app.apply_scroll_update(
        ScrollUpdate {
            target: target(1),
            epoch: pending_epoch,
            result: ScrollResult::History {
                revision: 1,
                offset_rows: 3,
                checkpoint: blank_checkpoint(24, 81, 1),
                row_wraps: None,
            },
        },
        Instant::now(),
    );
    let slot = app
        .screens
        .iter_mut()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    assert_eq!(slot.history_offset, 0);
    assert!(slot.view_epoch != pending_epoch);

    // A live alternate-screen checkpoint retires any normal-screen history
    // view even when the target and geometry have not changed.
    slot.history_offset = 3;
    let epoch = slot.view_epoch;
    let mut alternate = blank_checkpoint(24, 80, 1);
    alternate[80] = 1;
    app.load_screen(target(1), 2, &decode(&alternate).unwrap())
        .unwrap();
    let slot = app
        .screens
        .iter()
        .find(|slot| slot.target == Some(target(1)))
        .unwrap();
    assert_eq!(slot.history_offset, 0);
    assert!(slot.view_epoch > epoch);
}

/// A blank checkpoint record (cell-level conversion is covered by
/// `terminal::checkpoint_load` tests).
fn blank_checkpoint(rows: u16, columns: u16, pane: u64) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(
        TerminalSize::new(rows.saturating_sub(2), columns.saturating_sub(2)).unwrap(),
    )
    .unwrap();
    encode(
        &snapshot,
        &CheckpointMetadata::new(target(pane), 0, 1, 1).unwrap(),
    )
    .unwrap()
}

fn text_checkpoint(text: &str, pane: u64) -> Vec<u8> {
    text_checkpoint_size(text, pane, 22, 79)
}

fn text_checkpoint_size(text: &str, pane: u64, rows: u16, columns: u16) -> Vec<u8> {
    let mut record = blank_checkpoint(rows + 2, columns + 2, pane);
    let mut row = 0_u16;
    let mut column = 0_u16;
    for character in text.chars() {
        if character == '\r' {
            continue;
        }
        if character == '\n' {
            row = row.saturating_add(1).min(rows - 1);
            column = 0;
            continue;
        }
        if character == '\u{301}' {
            if column > 0 {
                append_checkpoint_cell(&mut record, row, column - 1, character);
            }
            continue;
        }
        let wide = character == '界';
        if column >= columns {
            row = row.saturating_add(1).min(rows - 1);
            column = 0;
        }
        write_checkpoint_cell(
            &mut record,
            row,
            column,
            &character.to_string(),
            u8::from(wide),
        );
        if wide && column + 1 < columns {
            write_checkpoint_cell(&mut record, row, column + 1, "", 2);
        }
        column = column.saturating_add(if wide { 2 } else { 1 });
    }
    record
}

fn write_checkpoint_cell(record: &mut [u8], row: u16, column: u16, text: &str, flags: u8) {
    let columns = u16::from_be_bytes([record[74], record[75]]);
    let index = usize::from(row) * usize::from(columns) + usize::from(column);
    let start = SCREEN_CHECKPOINT_HEADER_BYTES + index * SCREEN_CHECKPOINT_CELL_BYTES;
    record[start] = u8::try_from(text.len()).unwrap();
    record[start + 1] = flags;
    let bytes = text.as_bytes();
    record[start + 10..start + 10 + bytes.len()].copy_from_slice(bytes);
}

fn append_checkpoint_cell(record: &mut [u8], row: u16, column: u16, character: char) {
    let columns = u16::from_be_bytes([record[74], record[75]]);
    let index = usize::from(row) * usize::from(columns) + usize::from(column);
    let start = SCREEN_CHECKPOINT_HEADER_BYTES + index * SCREEN_CHECKPOINT_CELL_BYTES;
    let len = usize::from(record[start]);
    let mut encoded = [0; 4];
    let bytes = character.encode_utf8(&mut encoded).as_bytes();
    record[start + 10 + len..start + 10 + len + bytes.len()].copy_from_slice(bytes);
    record[start] = u8::try_from(len + bytes.len()).unwrap();
}

fn render_text(app: &App) -> String {
    let mut compositor = Compositor::try_new().unwrap();
    let mut bytes = Vec::new();
    app.render(&mut compositor, &mut bytes, Instant::now())
        .unwrap();
    let (rows, columns) = app.host;
    let mut parser = vt100::Parser::new(rows, columns, 0);
    parser.process(&bytes);
    parser.screen().contents()
}

fn install_operations_view(app: &mut App, view: PaneOperationsView) {
    app.apply_net_update(
        NetUpdate {
            layout: Some(view.layout),
            pane_operations: Some(view),
            screens: Vec::new(),
            status: None,
            server_present: true,
        },
        Instant::now(),
    );
}

#[test]
fn swap_mode_captures_exact_targets_and_consumes_all_local_input() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    app.focus_target(target(1));

    feed(&mut app, &mut fake, b"\x02m");
    feed(
        &mut app,
        &mut fake,
        b"\x1b[B\x1b[200~private paste\x1b[201~\x1b[<0;10;10M",
    );
    assert!(fake.pane_operations.is_empty());
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.scrolls.is_empty());
    feed(&mut app, &mut fake, b"\r");
    assert_eq!(
        fake.pane_operations,
        vec![PaneAction::Swap {
            source: target(1),
            destination: target(2),
            expected_layout_revision: 2,
        }]
    );
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());
}

#[test]
fn divider_mode_commits_one_absolute_pure_preview() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    app.focus_target(target(1));
    feed(&mut app, &mut fake, b"\x02e\x1b[C");
    assert!(fake.pane_operations.is_empty());
    assert_eq!(
        app.layout().unwrap().panes[0].unwrap().rect,
        rect(0, 0, 24, 40)
    );
    assert_eq!(
        app.display_layout().unwrap().panes[0].unwrap().rect,
        rect(0, 0, 24, 41)
    );
    feed(&mut app, &mut fake, b"\r");
    assert_eq!(
        fake.pane_operations,
        vec![PaneAction::DividerSet {
            target: target(1),
            expected_layout_revision: 2,
            divider_index: 0,
            first_cells: 41,
        }]
    );
    assert_eq!(
        app.layout().unwrap().panes[0].unwrap().rect,
        rect(0, 0, 24, 40)
    );
}

#[test]
fn zoom_projection_keeps_full_target_registry_and_hidden_screen_updates() {
    let mut app = App::new(24, 81).unwrap();
    let view = operations_view(two_panes(), Some(target(1)));
    install_operations_view(&mut app, view);
    assert_eq!(app.focus(), Some(target(1)));
    assert_eq!(app.layout().unwrap().panes.iter().flatten().count(), 2);
    let display = app.display_layout().unwrap();
    assert_eq!(display.panes.iter().flatten().count(), 1);
    assert_eq!(display.panes[0].unwrap().rect, rect(0, 0, 24, 81));
    assert_eq!(display.panes[0].unwrap().target, target(1));

    let hidden = blank_checkpoint(24, 40, 2);
    let checkpoint = decode(&hidden).unwrap();
    app.load_screen(target(2), 9, &checkpoint).unwrap();
    assert_eq!(app.screen_revision(target(2)), Some(9));
    assert_eq!(app.layout().unwrap().panes.iter().flatten().count(), 2);

    let mut zoomed = text_checkpoint_size("ZOOM", 1, 22, 79);
    zoomed[76..78].copy_from_slice(&2_u16.to_be_bytes());
    zoomed[78..80].copy_from_slice(&3_u16.to_be_bytes());
    let zoomed_snapshot = decode(&zoomed).unwrap();
    app.load_screen(target(1), 10, &zoomed_snapshot).unwrap();
    let zoomed_text = render_text(&app);
    assert!(zoomed_text.contains("ZOOM"), "{zoomed_text}");
    assert!(!zoomed_text.contains("resize pending"));
    let mut compositor = Compositor::try_new().unwrap();
    let mut output = Vec::new();
    app.render(&mut compositor, &mut output, Instant::now())
        .unwrap();
    let mut parser = vt100::Parser::new(24, 81, 0);
    parser.process(&output);
    assert_eq!(parser.screen().cursor_position(), (3, 4));
    assert!(!parser.screen().hide_cursor());
    let wraps = vec![false; usize::from(zoomed_snapshot.rows())];
    app.load_screen_with_wraps(target(1), 10, &zoomed_snapshot, Some(&wraps))
        .unwrap();
    feed(&mut app, &mut Fake::default(), b"\x1b[<0;2;2M");
    assert_eq!(
        app.selection.as_ref().map(|drag| drag.anchor),
        Some(Point { row: 0, column: 0 })
    );
    app.cancel_selection();

    let (catalog, current, _) = workspace_catalog();
    let mut headed = App::new(25, 81).unwrap();
    headed.install_workspace_tabs(catalog, current);
    install_operations_view(&mut headed, operations_view(two_panes(), Some(target(1))));
    headed.load_screen(target(1), 10, &zoomed_snapshot).unwrap();
    assert_eq!(headed.pane_at(80, 24), Some(target(1)));
    assert_eq!(headed.pane_at(80, 0), None);
    let mut headed_compositor = Compositor::try_new().unwrap();
    let mut headed_output = Vec::new();
    headed
        .render(&mut headed_compositor, &mut headed_output, Instant::now())
        .unwrap();
    let mut headed_parser = vt100::Parser::new(25, 81, 0);
    headed_parser.process(&headed_output);
    assert_eq!(headed_parser.screen().cursor_position(), (4, 4));

    let mut unzoomed = operations_view(two_panes(), None);
    unzoomed.layout.revision = 3;
    install_operations_view(&mut app, unzoomed);
    assert_eq!(
        app.display_layout().unwrap().panes.iter().flatten().count(),
        2
    );
    assert_eq!(app.focus(), Some(target(1)));
}

#[test]
fn stale_swap_chooser_cancels_without_retargeting_and_old_server_is_local() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    app.focus_target(target(1));
    feed(&mut app, &mut fake, b"\x02m");
    let mut newer = two_panes();
    newer.revision = 3;
    install_operations_view(&mut app, operations_view(newer, None));
    feed(&mut app, &mut fake, b"\r");
    assert!(fake.pane_operations.is_empty());

    let mut old_server = App::new(24, 81).unwrap();
    let mut old_fake = Fake::default();
    old_server.set_layout(one_pane(1));
    feed(&mut old_server, &mut old_fake, b"\x02z");
    assert!(old_fake.pane_operations.is_empty());
    assert_eq!(
        status(&old_server),
        Some("pane controls unsupported by this server")
    );
}

#[test]
fn zoom_request_and_completion_require_the_exact_admitted_action() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    app.focus_target(target(2));
    feed(&mut app, &mut fake, b"\x02z");
    let action = PaneAction::ZoomSet {
        target: target(2),
        expected_layout_revision: 2,
        zoomed: true,
    };
    assert_eq!(fake.pane_operations, vec![action]);
    let (id, pending) = app.pending_pane_operation.unwrap();
    app.complete_pane_operation(id.wrapping_add(1), pending, Err("unknown"), Instant::now());
    assert_eq!(app.pending_pane_operation, Some((id, action)));

    let mut zoomed = operations_view(two_panes(), Some(target(2)));
    zoomed.layout.revision = 3;
    app.complete_pane_operation(
        id,
        action,
        Ok(super::super::pane_operations::PaneOperationUpdate {
            view: zoomed,
            outcome: Outcome::Partial,
        }),
        Instant::now(),
    );
    assert_eq!(app.focus(), Some(target(2)));
    assert_eq!(
        app.display_layout().unwrap().panes.iter().flatten().count(),
        1
    );
    assert_eq!(status(&app), Some("pane operation applied; resize pending"));
}

#[test]
fn unknown_operation_result_and_escape_never_auto_submit_again() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    feed(&mut app, &mut fake, b"\x02m\x1b");
    assert!(fake.pane_operations.is_empty());
    assert_eq!(app.pane_mode, None);

    feed(&mut app, &mut fake, b"\x02z");
    let (id, action) = app.pending_pane_operation.unwrap();
    app.complete_pane_operation(id, action, Err("in progress"), Instant::now());
    assert_eq!(fake.pane_operations.len(), 1);
    assert_eq!(
        status(&app),
        Some("pane operation status unknown; layout unchanged")
    );
}

#[test]
fn queue_full_pane_operation_keeps_view_and_swallows_enter_repeat() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake {
        reject_pane_operations: true,
        ..Fake::default()
    };
    install_operations_view(&mut app, operations_view(two_panes(), None));
    let before = app.layout().copied();
    feed(&mut app, &mut fake, b"\x02m\r");
    assert_eq!(app.layout().copied(), before);
    assert!(app.pending_pane_operation.is_none());
    assert!(fake.pane_operations.is_empty());

    let repeat = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Enter,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    app.handle_event(
        &repeat,
        Instant::now(),
        Availability::Disabled(DisabledReason::NoPreviousLayoutSaved),
        &mut fake,
    );
    assert!(fake.pane_operations.is_empty());
}

#[test]
fn divider_preview_cancels_on_host_resize_and_ctrl_x_still_detaches() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    feed(&mut app, &mut fake, b"\x02e\x1b[C");
    assert!(app.pane_mode.is_some());
    app.set_host(30, 100);
    assert!(app.pane_mode.is_none());
    assert!(fake.pane_operations.is_empty());
    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(fake.pane_operations.is_empty());
}

#[test]
fn route_switch_clears_old_operation_view_and_accepts_lower_workspace_revision() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    feed(&mut app, &mut fake, b"\x02m");
    assert!(app.pane_mode.is_some());
    app.begin_route_switch(Instant::now());
    assert!(app.pane_mode.is_none());
    assert!(app.pane_operations.is_none());
    assert!(app.pending_pane_operation.is_none());

    let mut next = one_pane(1);
    next.panes[0].as_mut().unwrap().target.workspace = 2;
    install_operations_view(&mut app, operations_view(next, None));
    assert_eq!(app.layout().unwrap().revision, 1);
    assert_eq!(app.layout().unwrap().panes[0].unwrap().target.workspace, 2);
}

#[test]
fn unpadded_old_server_snapshot_has_an_actionable_upgrade_notice() {
    let mut app = App::new(24, 81).unwrap();
    app.set_layout(one_pane(1));
    let bytes = blank_checkpoint(26, 83, 1);
    app.load_screen(target(1), 1, &decode(&bytes).unwrap())
        .unwrap();
    assert!(render_text(&app).contains("restart server for pane padding"));
}

#[test]
fn frames_build_for_one_and_two_panes_with_placeholders_and_overlays() {
    let mut app = App::new(24, 81).unwrap();
    app.set_layout(one_pane(1));
    assert!(render_text(&app).contains("connecting..."));
    let record = blank_checkpoint(24, 81, 1);
    app.load_screen(target(1), 3, &decode(&record).unwrap())
        .unwrap();
    assert_eq!(app.screen_revision(target(1)), Some(3));
    assert!(!render_text(&app).contains("connecting..."));

    app.set_layout(two_panes());
    // Pane 1 has a stale-size screen; pane 2 has none yet.
    let text = render_text(&app);
    assert!(text.contains("connecting..."), "{text}");
    assert!(text.contains('|'), "divider drawn");
    let mut fake = Fake::default();
    feed(&mut app, &mut fake, b"\x1b[<2;50;5M");
    let text = render_text(&app);
    assert!(text.contains("Split Right"), "{text}");
    assert!(text.contains("Split Right"), "{text}");
}

#[test]
fn too_small_host_keeps_layout_and_shows_notice() {
    let mut app = App::new(24, 81).unwrap();
    app.set_layout(one_pane(1));
    app.set_host(10, 60);
    assert_eq!(app.desired_root(), Some((10, 60)));
    assert!(render_text(&app).contains("too small"));
    let mut fake = Fake::default();
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    assert!(!app.menu_open());
    app.set_host(3, 10);
    assert_eq!(app.desired_root(), None);
    assert!(app.layout().is_some());
}

fn screen_text(app: &App) -> String {
    let mut compositor = Compositor::try_new().unwrap();
    let mut output = Vec::new();
    app.render(&mut compositor, &mut output, Instant::now())
        .unwrap();
    let (rows, columns) = app.host;
    let mut parser = vt100::Parser::new(rows.max(1), columns.max(1), 0);
    parser.process(&output);
    parser.screen().contents()
}

#[test]
fn ctrl_b_enters_navigation_mode_with_a_visible_banner() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    assert!(!screen_text(&app).contains("NAV"));
    feed(&mut app, &mut fake, b"\x02");
    assert!(app.nav_active());
    assert!(screen_text(&app).contains(" NAV "), "banner visible");
    assert!(screen_text(&app).contains("? help"), "help is discoverable");
    // Esc leaves the mode; nothing reached the pane.
    feed(&mut app, &mut fake, b"\x1b");
    assert!(!app.nav_active());
    assert!(!screen_text(&app).contains(" NAV "));
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty());
}

#[test]
fn navigation_mode_times_out() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x02");
    app.expire_status(Instant::now() + Duration::from_secs(5));
    assert!(!app.nav_active());
}

#[test]
fn navigation_mode_direct_split_keys() {
    for (key, direction) in [
        (b'l', SplitDirection::Left),
        (b'r', SplitDirection::Right),
        (b'%', SplitDirection::Right),
        (b'u', SplitDirection::Up),
        (b'd', SplitDirection::Down),
        (b'"', SplitDirection::Down),
    ] {
        let mut app = App::new(24, 81).unwrap();
        let mut fake = Fake {
            split_reply: Some(Ok(target(2))),
            ..Fake::default()
        };
        app.set_layout(one_pane(3));
        feed(&mut app, &mut fake, &[0x02, key]);
        assert_eq!(fake.splits.len(), 1, "key {}", key as char);
        assert_eq!(fake.splits[0].direction, direction);
        assert_eq!(fake.splits[0].target, target(1));
        assert_eq!(fake.splits[0].expected_layout_revision, 3);
        assert!(!app.nav_active());
        assert!(fake.keys.is_empty(), "split keys never reach the pane");
    }
}

#[test]
fn navigation_mode_split_at_four_pane_capacity_reports_without_effect() {
    let mut app = App::new(32, 81).unwrap();
    let mut fake = Fake::default();
    let mut layout = two_panes();
    layout.rows = 32;
    for pane in layout.panes.iter_mut().flatten() {
        pane.rect.rows = 32;
    }
    if let Some(divider) = &mut layout.divider {
        divider.rows = 32;
    }
    for divider in layout.dividers.iter_mut().flatten() {
        divider.rows = 32;
    }
    let mut local = super::local_layout(&layout).unwrap();
    for (target, revision, direction, fresh) in [
        (
            target(1),
            2,
            panelink::pane_layout::SplitDirection::Down,
            target(3),
        ),
        (
            target(1),
            3,
            panelink::pane_layout::SplitDirection::Up,
            target(4),
        ),
    ] {
        let preview = local
            .preview_split(target, revision, direction, fresh)
            .unwrap();
        local.commit_split(preview).unwrap();
    }
    let snapshot = local.snapshot();
    app.set_layout(LayoutReply {
        revision: snapshot.revision + 2,
        rows: snapshot.rows,
        columns: snapshot.columns,
        panes: std::array::from_fn(|index| {
            snapshot.panes[index].map(|pane| UiPane {
                target: pane.target,
                rect: pane.rect,
                pending_confirmed: None,
            })
        }),
        divider: snapshot.divider,
        dividers: snapshot.dividers,
    });
    feed(&mut app, &mut fake, b"\x02r");
    assert!(fake.splits.is_empty());
    assert_eq!(
        app.status.map(|s| s.0),
        Some("split unavailable: pane limit reached (4)")
    );
}

#[test]
fn navigation_mode_moves_focus_between_panes_and_stays_active() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    assert_eq!(app.focus(), Some(target(1)));
    feed(&mut app, &mut fake, b"\x02\x1b[C");
    assert_eq!(
        app.focus(),
        Some(target(2)),
        "Right arrow focuses right pane"
    );
    assert!(app.nav_active(), "focus keys chain");
    feed(&mut app, &mut fake, b"\x1b[C");
    assert_eq!(app.focus(), Some(target(2)), "no pane further right");
    feed(&mut app, &mut fake, b"\x1b[D");
    assert_eq!(app.focus(), Some(target(1)));
    feed(&mut app, &mut fake, b"\t");
    assert_eq!(app.focus(), Some(target(2)), "Tab cycles");
    feed(&mut app, &mut fake, b"o");
    assert_eq!(app.focus(), Some(target(1)), "o cycles");
    feed(&mut app, &mut fake, b"\x1b");
    assert!(!app.nav_active());
    // Typing after leaving the mode goes to the newly focused pane.
    feed(&mut app, &mut fake, b"z");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('z')])]);
}

#[test]
fn navigation_mode_ctrl_x_still_detaches() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    assert_eq!(feed(&mut app, &mut fake, b"\x02\x18"), Flow::Quit);
    assert!(fake.closes.is_empty());
}

#[test]
fn workspace_header_picker_and_root_rows_stay_local_and_aligned() {
    let (index, current, next) = workspace_catalog();
    let mut app = App::new(25, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(two_panes());

    // Header click opens the chooser and never focuses a pane or sends keys.
    feed(&mut app, &mut fake, b"\x1b[<0;2;1M\x1b[<0;2;1m");
    assert_eq!(app.focus(), Some(target(1)));
    assert!(app.picker_selection.is_some());
    assert!(fake.keys.is_empty());
    assert!(fake.pastes.is_empty());

    // Picker rows are local and Enter requests the typed project/tab pair.
    feed(&mut app, &mut fake, b"\x1b[B\r");
    assert_eq!(app.take_route_request(), Some(next));
    assert!(app.route_switching);
    assert!(fake.keys.is_empty());
    assert!(fake.pastes.is_empty());

    // A controller preparation failure keeps the same App usable.
    app.finish_route_switch(true, Instant::now());
    assert!(!app.route_switching);
    assert!(!app.nav_active());
    feed(&mut app, &mut fake, b"z");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('z')])]);

    // The top screen row is not pane geometry; screen row 1 maps to root row 0.
    feed(&mut app, &mut fake, b"\x1b[<0;50;2M\x1b[<0;50;2m");
    assert_eq!(app.focus(), Some(target(2)));
    assert_eq!(app.desired_root(), Some((24, 81)));
    app.set_host(24, 81);
    assert_eq!(app.desired_root(), Some((23, 81)));
    app.set_host(1, 81);
    assert_eq!(app.desired_root(), None);
}

#[test]
fn route_selection_freezes_unsent_effects_copy_and_allows_local_detach_resize() {
    let (index, current, next) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(two_panes());
    feed(&mut app, &mut fake, b"unsent");
    app.pending_copy = Some("private selection".to_owned());

    let column = (0..81)
        .find(|column| {
            app.tabs_layout()
                .and_then(|layout| layout.hit_test(*column, 0))
                == Some(TabAction::Select(next))
        })
        .expect("second tab has a visible header hit region");
    let click = format!("\x1b[<0;{};1M\x1b[<0;{};1m", column + 1, column + 1);
    feed(&mut app, &mut fake, click.as_bytes());
    assert_eq!(app.take_route_request(), Some(next));
    assert!(app.route_switching);
    assert_eq!(app.pending_len, 0);
    assert!(app.selection.is_none());
    assert!(app.pending_copy.is_none());
    assert!(
        fake.keys.is_empty(),
        "unsent keys are abandoned before effects"
    );

    // Frozen key, paste, menu, and mouse input cause no child effects.
    feed(&mut app, &mut fake, b"x\x1b[200~paste\x1b[201~");
    feed(&mut app, &mut fake, b"\x1b[<2;10;5M\x1b[<2;10;5m");
    assert!(fake.keys.is_empty());
    assert!(fake.pastes.is_empty());
    assert!(fake.splits.is_empty());
    assert!(fake.closes.is_empty());
    assert!(fake.scrolls.is_empty());

    // Resize remains local and Ctrl-X detaches without flushing to the old route.
    app.set_host(30, 100);
    assert_eq!(app.desired_root(), Some((29, 100)));
    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(fake.keys.is_empty());
    assert!(fake.pastes.is_empty());
}

#[test]
fn choosing_current_route_closes_picker_without_freezing_or_requesting() {
    let (index, current, _) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    feed(&mut app, &mut fake, b"\x02w\r");
    assert!(app.picker_selection.is_none());
    assert_eq!(app.take_route_request(), None);
    assert!(!app.route_switching);
    assert!(!app.nav_active());
    assert!(fake.keys.is_empty());
}

#[test]
fn header_row_zero_has_no_pane_row_or_child_effects() {
    let (index, current, _) = workspace_catalog();
    let mut app = App::new(25, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(one_pane(1));
    assert_eq!(app.pane_row(0), None);

    // Right click, wheel, drag and release over the reserved header stay
    // local and exercise the row conversion boundary directly.
    feed(
        &mut app,
        &mut fake,
        b"\x1b[<2;2;1M\x1b[<64;2;1M\x1b[<32;2;1M\x1b[<0;2;1m",
    );
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());
    assert!(fake.splits.is_empty() && fake.closes.is_empty() && fake.scrolls.is_empty());
    assert!(app.selection.is_none());
}

#[test]
fn picker_scroll_tracks_short_viewport_and_hit_rows() {
    let (mut index, current, _) = workspace_catalog();
    let project = current.project;
    for id in 103..=108 {
        index
            .add_tab(
                project,
                WorkspaceTabId::new(id).unwrap(),
                &format!("Tab {id}"),
            )
            .unwrap();
    }
    let selected = WorkspaceRouteId {
        project,
        tab: WorkspaceTabId::new(105).unwrap(),
    };
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_host(6, 30); // short, narrow host: four route rows below the title
    feed(&mut app, &mut fake, b"\x02w\x1b[B\x1b[B\x1b[B\x1b[B");
    assert_eq!(app.picker_selection, Some(selected));
    let (top, rows) = app.picker_screen_geometry().unwrap();
    assert_eq!(rows, 5);
    assert_eq!(app.picker_route_at_row(rows - 2), Some(selected));
    let visible = screen_text(&app);
    assert!(visible.contains("> Editors / Tab 105"), "{visible}");
    assert!(top > 0);
}

#[test]
fn catalog_refresh_preserves_route_freeze_request_and_valid_chooser() {
    let (index, current, next) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    app.install_workspace_tabs(index.clone(), current);
    app.picker_selection = Some(next);
    app.route_request = Some(next);
    app.route_switching = true;

    assert!(app.update_workspace_catalog(index));
    assert_eq!(app.picker_selection, Some(next));
    assert_eq!(app.route_request, Some(next));
    assert!(app.route_switching());

    let mut refreshed = WorkspaceIndex::new();
    refreshed
        .add_project(current.project, "Editors", current.tab, "Main")
        .unwrap();
    assert!(app.update_workspace_catalog(refreshed));
    assert_eq!(app.current_route, Some(current));
    assert_eq!(app.picker_selection, Some(current));
    assert_eq!(app.route_request, Some(next));
    assert!(app.route_switching());

    // Invalid active-route snapshots do not replace the last valid catalog.
    assert!(!app.update_workspace_catalog(WorkspaceIndex::new()));
    assert!(app.route_exists(current));
}

#[test]
fn picker_open_coalesces_catalog_refresh_and_clears_navigation_prefix() {
    let (index, current, _) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x02w");
    assert!(!app.nav_active());
    assert!(app.take_catalog_refresh_request());
    assert!(!app.take_catalog_refresh_request());
    feed(&mut app, &mut fake, b"\x1b");
    feed(&mut app, &mut fake, b"z");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('z')])]);
}

#[test]
fn flush_during_route_freeze_discards_stale_pending_keys() {
    let (index, current, _) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(one_pane(1));
    app.begin_route_switch(Instant::now());
    // Guard against a stale host path populating the batch after freeze entry.
    app.pending[0] = Key::Char('x');
    app.pending_len = 1;
    app.pending_target = Some(target(1));
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty());
    assert_eq!(app.pending_len, 0);
    assert_eq!(app.pending_target, None);
}

#[test]
fn recovery_availability_skips_picker_header_help_and_preparing_events() {
    let (index, current, next) = workspace_catalog();
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(one_pane(1));

    feed(&mut app, &mut fake, b"\x02w\x1b[<2;2;1M");
    assert_eq!(fake.recovery_checks, 0);
    feed(&mut app, &mut fake, b"\x1b\x02?");
    feed(&mut app, &mut fake, b"\x1b[<2;2;1M");
    assert_eq!(fake.recovery_checks, 0);
    feed(&mut app, &mut fake, b"\x1b");
    app.picker_selection = Some(next);
    feed(&mut app, &mut fake, b"\r");
    assert!(app.route_switching());
    feed(&mut app, &mut fake, b"\x1b[<2;2;1M");
    assert_eq!(fake.recovery_checks, 0);
}

#[test]
fn header_composes_at_screen_row_zero_and_selection_uses_root_coordinates() {
    let (index, current, _) = workspace_catalog();
    let mut app = App::new(25, 81).unwrap();
    let mut fake = Fake::default();
    app.install_workspace_tabs(index, current);
    app.set_layout(one_pane(1));
    let checkpoint = blank_checkpoint(24, 81, 1);
    let decoded = decode(&checkpoint).unwrap();
    let wraps = vec![false; usize::from(decoded.rows())];
    app.load_screen_with_wraps(target(1), 1, &decoded, Some(&wraps))
        .unwrap();

    let mut compositor = Compositor::try_new().unwrap();
    let mut output = Vec::new();
    app.render(&mut compositor, &mut output, Instant::now())
        .unwrap();
    let mut parser = vt100::Parser::new(25, 81, 0);
    parser.process(&output);
    let rendered = parser.screen().contents();
    let header = rendered.lines().next().unwrap_or_default();
    assert!(header.contains("Editors"), "{header}");
    assert!(header.contains("Main"), "{header}");
    let selected_column = u16::try_from(header.find("Main").unwrap()).unwrap();
    assert!(parser.screen().cell(0, selected_column).unwrap().inverse());

    feed(&mut app, &mut fake, b"\x02?");
    feed(&mut app, &mut fake, b"\x1b[<0;2;1M\x1b[<0;2;1m");
    assert!(app.help_open());
    assert!(app.picker_selection.is_none(), "help owns header clicks");
    let rendered = screen_text(&app);
    let lines: Vec<_> = rendered.lines().collect();
    assert!(lines[0].contains("Editors"), "{}", lines[0]);
    assert!(lines
        .get(9)
        .is_some_and(|line| line.contains("Panelink help")));
    feed(&mut app, &mut fake, b"\x1b");

    // Screen row 0 is reserved; screen row 2 is content row 0 after the
    // one-row header and one-cell pane inset are both removed.
    feed(&mut app, &mut fake, b"\x1b[<0;2;1M\x1b[<0;2;1m");
    assert!(app.selection.is_none());
    feed(&mut app, &mut fake, b"\x1b");
    feed(&mut app, &mut fake, b"\x1b[<0;2;3M");
    let drag = app
        .selection
        .as_ref()
        .expect("content click starts selection");
    assert_eq!(drag.anchor, Point { row: 0, column: 0 });
}

#[test]
fn navigation_help_is_modal_persistent_and_has_no_hidden_pane_effects() {
    let (mut app, mut fake) = two_focused_on_second();
    // Pending input is flushed exactly once to the pane it was entered for
    // before Ctrl-B starts NAV; the ? itself is consumed locally.
    feed(&mut app, &mut fake, b"ab\x02?");
    assert!(app.help_open());
    assert!(!app.nav_active());
    assert_eq!(app.focus(), Some(target(2)));
    assert_eq!(
        fake.keys,
        vec![(target(2), vec![Key::Char('a'), Key::Char('b')])]
    );
    assert!(fake.splits.is_empty() && fake.closes.is_empty());

    // Every non-dismissal event is isolated while the modal is open.
    feed(
        &mut app,
        &mut fake,
        b"z\x1b[200~secret\x1b[201~\x1b[<0;5;5M\x1b[<2;50;5M",
    );
    app.expire_status(Instant::now() + Duration::from_secs(30));
    assert!(app.help_open(), "help does not inherit the NAV timeout");
    assert_eq!(app.focus(), Some(target(2)));
    assert!(!app.menu_open() && !app.confirm_open());
    assert!(fake.keys.len() == 1 && fake.pastes.is_empty());
    assert!(fake.splits.is_empty() && fake.closes.is_empty());

    let visible = screen_text(&app);
    assert!(visible.contains("Panelink help"), "{visible}");
    assert!(visible.contains("detach"), "{visible}");
    assert!(visible.contains("reconnect"), "{visible}");
    feed(&mut app, &mut fake, b"?");
    assert!(!app.help_open());
    feed(&mut app, &mut fake, b"q");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys[1], (target(2), vec![Key::Char('q')]));
}

#[test]
fn help_ctrl_x_detaches_and_esc_dismisses() {
    let (mut app, mut fake) = two_focused_on_second();
    feed(&mut app, &mut fake, b"\x02?");
    assert!(app.help_open());
    assert_eq!(feed(&mut app, &mut fake, b"\x18"), Flow::Quit);
    assert!(!app.help_open());
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());

    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    feed(&mut app, &mut fake, b"\x02?");
    feed(&mut app, &mut fake, b"\x1b");
    assert!(!app.help_open());
    assert!(!app.nav_active());
    assert!(fake.keys.is_empty());
}

#[test]
fn help_overlay_clips_on_resize_and_unchanged_frames_write_nothing() {
    let (mut app, mut fake) = two_focused_on_second();
    let mut compositor = Compositor::try_new().unwrap();
    let mut baseline = Vec::new();
    app.render(&mut compositor, &mut baseline, Instant::now())
        .unwrap();
    assert!(compositor.commit());
    feed(&mut app, &mut fake, b"\x02?");
    let mut first = Vec::new();
    app.render(&mut compositor, &mut first, Instant::now())
        .unwrap();
    assert!(compositor.commit());
    assert!(
        !first.windows(4).any(|window| window == b"\x1b[2J"),
        "help repaint cleared the screen: {:?}",
        String::from_utf8_lossy(&first)
    );
    let mut unchanged = Vec::new();
    app.render(&mut compositor, &mut unchanged, Instant::now())
        .unwrap();
    assert!(unchanged.is_empty(), "unchanged help frame is incremental");

    app.set_host(2, 5);
    let clipped = screen_text(&app);
    assert!(clipped.lines().count() <= 2);
    assert!(clipped.lines().all(|line| line.chars().count() <= 5));
    assert!(app.help_open());
    app.set_host(18, 60);
    app.set_status("lower priority notice", Instant::now());
    let resized = screen_text(&app);
    assert!(resized.contains("Panelink help"), "{resized}");
    assert!(!resized.contains("lower priority notice"), "{resized}");
    assert!(!resized.contains("NAV"), "help takes notice priority");
}

/// Two panes, pane 2 focused, with pending typed keys flushed first.
fn two_focused_on_second() -> (App, Fake) {
    let mut app = App::new(24, 81).unwrap();
    let fake = Fake::default();
    app.set_layout(two_panes());
    app.focus_target(target(2));
    (app, fake)
}

fn status(app: &App) -> Option<&'static str> {
    app.status.map(|s| s.0)
}

#[test]
fn ctrl_b_x_asks_for_confirmation_and_y_closes_the_focused_pane() {
    let (mut app, mut fake) = two_focused_on_second();
    assert_eq!(feed(&mut app, &mut fake, b"\x02x"), Flow::Continue);
    assert!(app.confirm_open());
    assert!(!app.nav_active());
    assert!(fake.closes.is_empty(), "nothing is sent before y");
    let text = screen_text(&app);
    assert!(text.contains("Close pane 2? (y/n)"), "{text}");

    feed(&mut app, &mut fake, b"y");
    assert!(!app.confirm_open());
    assert_eq!(
        fake.closes,
        vec![ClosePlan {
            target: target(2),
            expected_layout_revision: 2,
        }]
    );
    assert_eq!(status(&app), Some("pane closed"));
    // Focus leaves the closed pane at once; typing goes to the survivor.
    assert_eq!(app.focus(), Some(target(1)));
    feed(&mut app, &mut fake, b"k");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(1), vec![Key::Char('k')])]);
}

#[test]
fn confirmation_keys_and_cancel_keys_never_reach_the_child() {
    for (cancel, label) in [
        (&b"n"[..], "n"),
        (b"N", "N"),
        (b"\x1b", "Esc"),
        (b"q", "other key"),
        (b"\r", "Enter"),
        (b"\x03", "Ctrl-C"),
        (b"\x1b[200~y\x1b[201~", "paste"),
        (b"\x1b[<0;5;5M", "left click"),
        (b"\x1b[<2;5;5M", "right click"),
    ] {
        let (mut app, mut fake) = two_focused_on_second();
        feed(&mut app, &mut fake, b"ab\x02x");
        assert!(app.confirm_open(), "{label}");
        feed(&mut app, &mut fake, cancel);
        assert!(!app.confirm_open(), "{label}");
        assert!(!app.menu_open(), "{label}: the click did not open the menu");
        assert!(fake.closes.is_empty(), "{label}");
        assert_eq!(status(&app), Some("close cancelled"), "{label}");
        assert_eq!(app.focus(), Some(target(2)), "{label}");
        app.flush(Instant::now(), &mut fake);
        // Only the keys typed before Ctrl-B reached the pane.
        assert_eq!(
            fake.keys,
            vec![(target(2), vec![Key::Char('a'), Key::Char('b')])],
            "{label}"
        );
        assert!(fake.pastes.is_empty(), "{label}");
    }
}

#[test]
fn releases_do_not_cancel_the_prompt() {
    let (mut app, mut fake) = two_focused_on_second();
    feed(&mut app, &mut fake, b"\x02x\x1b[<2;5;5m\x1b[<0;5;5m");
    assert!(app.confirm_open());
    feed(&mut app, &mut fake, b"Y");
    assert_eq!(fake.closes.len(), 1);
}

#[test]
fn menu_close_item_uses_captured_target_and_server_revision_after_confirmation() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    // Right-click pane 2 while pane 1 has focus.
    feed(&mut app, &mut fake, b"\x1b[<2;50;5M\x1b[<2;50;5m");
    assert!(app.menu_open());
    let text = render_text(&app);
    assert!(text.contains("Close pane"), "{text}");
    // No letter shortcut reaches the close row: x is consumed.
    feed(&mut app, &mut fake, b"x");
    assert!(app.menu_open() && !app.confirm_open());
    // End selects the last (recovery) row; one Up reaches "Close pane".
    feed(&mut app, &mut fake, b"\x1b[F\x1b[A\r");
    assert!(!app.menu_open());
    assert!(app.confirm_open());
    assert!(fake.closes.is_empty());
    // A newer layout revision arrives; the captured one (2) is still used.
    let mut newer = two_panes();
    newer.revision = 3;
    app.set_layout(newer);
    assert!(app.confirm_open());
    feed(&mut app, &mut fake, b"y");
    assert_eq!(
        fake.closes,
        vec![ClosePlan {
            target: target(2),
            expected_layout_revision: 2,
        }]
    );
    assert!(fake.keys.is_empty() && fake.pastes.is_empty());
    assert_eq!(app.focus(), Some(target(1)));
}

#[test]
fn menu_close_on_single_pane_warns_that_the_workspace_ends() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(4));
    // End selects the recovery row; Up reaches "Close pane".
    feed(&mut app, &mut fake, b"\x02s\x1b[F\x1b[A\r");
    assert!(app.confirm_open());
    let text = screen_text(&app);
    assert!(text.contains("This is the last pane"), "{text}");
    feed(&mut app, &mut fake, b"y");
    assert_eq!(
        fake.closes,
        vec![ClosePlan {
            target: target(1),
            expected_layout_revision: 4,
        }]
    );
    assert_eq!(status(&app), Some("last pane closed; workspace closing"));
}

#[test]
fn close_results_map_to_fixed_status_messages_and_never_retry() {
    for (reply, message, focus_moves) in [
        (
            Ok(CloseOutcome::Stopped(StopOutcome::Exited)),
            "pane closed",
            true,
        ),
        (
            Ok(CloseOutcome::Stopped(StopOutcome::Escalated)),
            "pane closed (forced)",
            true,
        ),
        (
            Ok(CloseOutcome::Stopped(StopOutcome::Unknown)),
            "close status unknown; the pane may still be running",
            false,
        ),
        (
            Ok(CloseOutcome::Rejected(CloseRejection::StaleLayout)),
            "close rejected: layout changed",
            false,
        ),
        (
            Ok(CloseOutcome::Rejected(CloseRejection::StaleTarget)),
            "close rejected: pane changed",
            false,
        ),
        (
            Ok(CloseOutcome::Rejected(CloseRejection::AlreadyClosed)),
            "pane already closed",
            false,
        ),
        (Err("connection lost"), "connection lost", false),
    ] {
        let (mut app, mut fake) = two_focused_on_second();
        fake.close_reply = Some(reply);
        feed(&mut app, &mut fake, b"\x02xy");
        assert_eq!(fake.closes.len(), 1, "{message}: exactly one request");
        assert_eq!(status(&app), Some(message));
        let expected = if focus_moves { target(1) } else { target(2) };
        assert_eq!(app.focus(), Some(expected), "{message}");
        // Nothing is resent later.
        app.expire_status(Instant::now() + Duration::from_secs(10));
        app.flush(Instant::now(), &mut fake);
        assert_eq!(fake.closes.len(), 1, "{message}");
        assert!(fake.keys.is_empty(), "{message}");
    }
}

#[test]
fn resize_pending_flag_is_reported() {
    let result = CloseResult {
        key: OperationKey {
            generation: 7,
            lease: 1,
            sequence: 1,
        },
        outcome: CloseOutcome::Stopped(StopOutcome::Escalated),
        target: Some(target(2)),
        layout_revision: 3,
        survivor_resize_pending: true,
    };
    assert_eq!(
        close_message(&result, false),
        "pane closed (forced; resize pending)"
    );
    let exited = CloseResult {
        outcome: CloseOutcome::Stopped(StopOutcome::Exited),
        ..result
    };
    assert_eq!(
        close_message(&exited, false),
        "pane closed (resize pending)"
    );
    assert_eq!(
        close_message(&exited, true),
        "last pane closed; workspace closing"
    );
}

/// Pane 2 alone after pane 1 was removed (revision 3, full root).
fn survivor_only() -> LayoutReply {
    LayoutReply {
        revision: 3,
        rows: 24,
        columns: 81,
        panes: [
            Some(UiPane {
                target: target(2),
                rect: rect(0, 0, 24, 81),
                pending_confirmed: None,
            }),
            None,
            None,
            None,
        ],
        divider: None,
        dividers: [None; panelink::pane_layout::MAX_PANES - 1],
    }
}

#[test]
fn layout_shrink_repairs_focus_drops_stale_keys_and_frees_the_screen_cache() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    let left = blank_checkpoint(24, 40, 1);
    app.load_screen(target(1), 5, &decode(&left).unwrap())
        .unwrap();
    // Keys typed into pane 1 are still batched when its shell exits.
    feed(&mut app, &mut fake, b"zz");
    app.set_layout(survivor_only());
    assert_eq!(app.focus(), Some(target(2)));
    assert_eq!(app.screen_revision(target(1)), None);
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty(), "keys for a removed pane are dropped");
    // Single full-root pane, no divider; nothing from the old split remains.
    let text = render_text(&app);
    assert!(!text.contains('|'), "{text}");
    assert!(text.contains("connecting..."), "{text}");
    // Typing goes to the survivor.
    feed(&mut app, &mut fake, b"w");
    app.flush(Instant::now(), &mut fake);
    assert_eq!(fake.keys, vec![(target(2), vec![Key::Char('w')])]);
}

#[test]
fn auto_collapse_cancels_a_prompt_for_the_removed_pane() {
    let (mut app, mut fake) = two_focused_on_second();
    app.focus_target(target(1));
    feed(&mut app, &mut fake, b"\x02x");
    assert!(app.confirm_open());
    app.set_layout(survivor_only());
    assert!(!app.confirm_open());
    assert_eq!(status(&app), Some("pane already closed"));
    feed(&mut app, &mut fake, b"y");
    assert!(fake.closes.is_empty());
    app.flush(Instant::now(), &mut fake);
    assert_eq!(
        fake.keys,
        vec![(target(2), vec![Key::Char('y')])],
        "after cancellation, typing is normal again"
    );
}

#[test]
fn split_works_again_after_a_close_shrinks_the_layout() {
    let (mut app, mut fake) = two_focused_on_second();
    feed(&mut app, &mut fake, b"\x02r");
    assert!(fake.splits.is_empty(), "at capacity");
    // Close pane 1; the server then reports the collapsed layout.
    app.focus_target(target(1));
    feed(&mut app, &mut fake, b"\x02xy");
    assert_eq!(app.focus(), Some(target(2)));
    app.set_layout(survivor_only());
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    let text = render_text(&app);
    assert!(text.contains("Split Right"), "{text}");
    assert!(!text.contains("Pane limit"), "{text}");
    feed(&mut app, &mut fake, b"\x1b");
    fake.split_reply = Some(Ok(target(3)));
    feed(&mut app, &mut fake, b"\x02d");
    assert_eq!(
        fake.splits,
        vec![SplitPlan {
            target: target(2),
            expected_layout_revision: 3,
            direction: SplitDirection::Down,
            rows: 10,
            columns: 79,
        }]
    );
    assert_eq!(app.focus(), Some(target(2)));
    let mut split_layout = survivor_only();
    split_layout.revision = 4;
    split_layout.panes[1] = Some(UiPane {
        target: target(3),
        rect: rect(41, 0, 24, 40),
        pending_confirmed: None,
    });
    split_layout.panes[0].as_mut().unwrap().rect = rect(0, 0, 24, 40);
    split_layout.divider = Some(rect(40, 0, 24, 1));
    app.set_layout(split_layout);
    assert_eq!(app.focus(), Some(target(3)));
}

#[test]
fn pending_zoom_and_divider_preview_never_compose_wrong_sized_snapshots() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    install_operations_view(&mut app, operations_view(two_panes(), None));
    for pane in [1, 2] {
        let record = blank_checkpoint(24, 40, pane);
        app.load_screen(target(pane), 1, &decode(&record).unwrap())
            .unwrap();
    }
    feed(&mut app, &mut fake, b"\x02e\x1b[C");
    // The old screen fits the confirmed PTY, but not the projected preview.
    assert!(!render_text(&app).is_empty());
    assert!(fake.pane_operations.is_empty());

    let mut zoom = operations_view(two_panes(), None);
    zoom.zoomed_target = Some(target(1));
    zoom.layout.revision = 3;
    install_operations_view(&mut app, zoom);
    // Zoom committed while the PTY still has its old underlying size.
    let text = render_text(&app);
    assert!(text.contains("resize pending"), "{text}");
}
