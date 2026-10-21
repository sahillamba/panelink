use super::*;
use crossterm_input::Decoder;
use panelink::protocol::OperationKey;
use panelink::screen_checkpoint::{decode, encode, CheckpointMetadata};
use panelink::split_menu::DisabledReason;
use panelink::ui_protocol::UiPane;

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
        ],
        divider: None,
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
        ],
        divider: Some(rect(40, 0, 24, 1)),
    }
}

#[derive(Default)]
struct Fake {
    keys: Vec<(TargetRef, Vec<Key>)>,
    pastes: Vec<TargetRef>,
    splits: Vec<SplitPlan>,
    split_reply: Option<Result<TargetRef, &'static str>>,
    closes: Vec<ClosePlan>,
    close_reply: Option<Result<CloseOutcome, &'static str>>,
    recovery_available: bool,
    recovery_checks: usize,
}

impl Effects for Fake {
    fn send_keys(&mut self, target: TargetRef, keys: &[Key]) -> Result<(), &'static str> {
        self.keys.push((target, keys.to_vec()));
        Ok(())
    }
    fn send_paste(&mut self, target: TargetRef, _text: &str) -> Result<(), &'static str> {
        self.pastes.push(target);
        Ok(())
    }
    fn split(&mut self, plan: SplitPlan) -> Result<TargetRef, &'static str> {
        self.splits.push(plan);
        self.split_reply.unwrap_or(Ok(target(2)))
    }
    fn close(&mut self, plan: ClosePlan) -> Result<CloseResult, &'static str> {
        self.closes.push(plan);
        let outcome = self
            .close_reply
            .unwrap_or(Ok(CloseOutcome::Stopped(StopOutcome::Exited)))?;
        Ok(CloseResult {
            key: OperationKey {
                generation: 7,
                lease: 1,
                sequence: self.closes.len() as u64,
            },
            outcome,
            target: Some(plan.target),
            layout_revision: plan.expected_layout_revision + 1,
            survivor_resize_pending: false,
        })
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
fn right_click_opens_menu_for_pane_under_pointer_and_menu_inputs_never_reach_panes() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    app.focus_target(target(2));
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    assert!(app.menu_open());
    assert_eq!(app.menu.captured_target(), Some(target(1)));
    // Every kind of input while open is consumed: keys, Enter on a disabled
    // item (capacity), paste, Ctrl-X, mouse, arrows.
    let flow = feed(
        &mut app,
        &mut fake,
        b"xyz\r\x1b[B\x1b[200~paste\x1b[201~\x18\x03\x1b[<2;5;5m",
    );
    assert_eq!(flow, Flow::Continue);
    assert!(app.menu_open(), "disabled items keep the menu open");
    app.flush(Instant::now(), &mut fake);
    assert!(fake.keys.is_empty() && fake.pastes.is_empty() && fake.splits.is_empty());
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
            rows: 24,
            columns: 40,
        }]
    );
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
    assert_eq!(fake.splits[0].rows, 11);
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

/// A blank checkpoint record (cell-level conversion is covered by
/// `terminal::checkpoint_load` tests).
fn blank_checkpoint(rows: u16, columns: u16, pane: u64) -> Vec<u8> {
    let snapshot = ScreenSnapshot::try_new(TerminalSize::new(rows, columns).unwrap()).unwrap();
    encode(
        &snapshot,
        &CheckpointMetadata::new(target(pane), 0, 1, 1).unwrap(),
    )
    .unwrap()
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
    assert!(text.contains("Pane limit"), "{text}");
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
    let mut parser = vt100::Parser::new(24, 81, 0);
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
fn navigation_mode_split_at_capacity_reports_without_effect() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(two_panes());
    feed(&mut app, &mut fake, b"\x02r");
    assert!(fake.splits.is_empty());
    assert_eq!(
        app.status.map(|s| s.0),
        Some("split unavailable: pane limit reached (2)")
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
    // Up from the first (disabled) item wraps to the recovery row; one more
    // Up reaches "Close pane".
    feed(&mut app, &mut fake, b"\x1b[A\x1b[A\r");
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
        ],
        divider: None,
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
    fake.split_reply = Some(Ok(target(3)));
    feed(&mut app, &mut fake, b"\x02d");
    assert_eq!(
        fake.splits,
        vec![SplitPlan {
            target: target(2),
            expected_layout_revision: 3,
            direction: SplitDirection::Down,
            rows: 12,
            columns: 81,
        }]
    );
    assert_eq!(app.focus(), Some(target(3)));
    // The menu also offers splits again on the survivor.
    feed(&mut app, &mut fake, b"\x1b[<2;5;5M");
    let text = render_text(&app);
    assert!(text.contains("Split Right"), "{text}");
    assert!(!text.contains("Pane limit"), "{text}");
}
