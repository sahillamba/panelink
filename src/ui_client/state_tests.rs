use super::*;
use crossterm_input::Decoder;
use panelink::screen_checkpoint::{decode, encode, CheckpointMetadata};
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
}

fn feed(app: &mut App, fake: &mut Fake, bytes: &[u8]) -> Flow {
    let mut decoder = Decoder::new();
    let now = Instant::now();
    let mut flow = Flow::Continue;
    for (index, byte) in bytes.iter().enumerate() {
        if let Some(event) = decoder.advance(*byte, index + 1 < bytes.len()).unwrap() {
            flow = app.handle_event(&event, now, fake);
        }
    }
    if let Some(event) = decoder.finish_escape().unwrap() {
        flow = app.handle_event(&event, now, fake);
    }
    flow
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
fn navigation_mode_x_detaches() {
    let mut app = App::new(24, 81).unwrap();
    let mut fake = Fake::default();
    app.set_layout(one_pane(1));
    assert_eq!(feed(&mut app, &mut fake, b"\x02x"), Flow::Quit);
}
