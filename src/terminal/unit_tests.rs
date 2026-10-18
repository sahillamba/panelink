use super::*;

#[test]
fn render_snapshot_cache_copies_without_reallocation_across_viewport_changes() {
    let small_size = TerminalSize::new(5, 20).unwrap();
    let large_size = TerminalSize::new(80, 240).unwrap();
    let mut cache = ScreenSnapshot::try_new(small_size).unwrap();
    let storage = cache.cells.as_ptr();
    let mut parser = Parser::new_with_callbacks(80, 240, 0, CoreCallbacks::new());
    parser.process(b"CACHE-CANARY\x1b[80;240HZ\x1b[?25l");
    let mut large = ScreenSnapshot::new(large_size);
    large.refresh(parser.screen(), large_size);
    cache.copy_from(&large);
    assert_eq!(cache.cells.as_ptr(), storage);
    assert_eq!(cache.cells(), large.cells());
    assert_eq!(cache.cursor_row(), large.cursor_row());
    assert_eq!(cache.cursor_column(), large.cursor_column());
    assert!(cache.cursor_hidden());
    assert!(!format!("{cache:?}").contains("CACHE-CANARY"));

    let small = ScreenSnapshot::new(small_size);
    cache.copy_from(&small);
    assert_eq!(cache.cells.as_ptr(), storage);
    assert_eq!(cache.cells.len(), 100);
    assert_eq!(cache.cells(), small.cells());
    assert!(!cache.cursor_hidden());
    cache.copy_from(&large);
    assert_eq!(cache.cells.as_ptr(), storage);
    assert_eq!(cache.cells(), large.cells());
}

#[derive(Default)]
struct OscLengthOnly(Vec<usize>);

impl Callbacks for OscLengthOnly {
    fn unhandled_osc(&mut self, _: &mut Screen, params: &[&[u8]]) {
        self.0.push(params.iter().map(|param| param.len()).sum());
    }
}

fn query_replies(parser: &mut Parser<CoreCallbacks>, bytes: &[u8]) -> Vec<Vec<u8>> {
    parser.callbacks_mut().begin_chunk();
    parser.process(bytes);
    parser.callbacks().queued().map(<[u8]>::to_vec).collect()
}

#[test]
fn exact_queries_are_answered_and_malformed_queries_are_dropped() {
    let mut parser = Parser::new_with_callbacks(8, 20, 0, CoreCallbacks::new());
    parser.process(b"\x1b[3;7H");
    assert_eq!(
        query_replies(&mut parser, b"\x1b[c\x1b[5n\x1b[6n\x1b[18t"),
        [
            b"\x1b[?1c".to_vec(),
            b"\x1b[0n".to_vec(),
            b"\x1b[3;7R".to_vec(),
            b"\x1b[8;8;20t".to_vec()
        ]
    );
    assert!(query_replies(&mut parser, b"\x1b[?6n\x1b[6;1n\x1b[1$ c\x1b[4n").is_empty());
}

#[test]
fn query_reply_rate_and_pending_events_are_bounded() {
    let mut parser = Parser::new_with_callbacks(8, 20, 0, CoreCallbacks::new());
    let many = b"\x1b[5n".repeat(200);
    assert_eq!(query_replies(&mut parser, &many).len(), MAX_QUERY_REPLIES);
    assert!(parser
        .callbacks()
        .queued()
        .all(|reply| reply.len() <= MAX_QUERY_RESPONSE_BYTES));
}

#[test]
fn clipboard_reads_writes_and_clears_never_queue_child_input() {
    let mut parser = Parser::new_with_callbacks(8, 20, 0, CoreCallbacks::new());
    assert!(query_replies(&mut parser, b"\x1b]52;c;?\x07").is_empty());
    assert!(query_replies(
        &mut parser,
        b"\x1b]52;c;YWJj\x07\x1b]52;p;?\x07\x1b]52;c;\x07\x1b]52;c;?\x1b\\"
    )
    .is_empty());
}

#[test]
fn input_queue_is_bounded_atomic_and_preserves_partial_suffix() {
    let mut queue = InputQueue::new();
    let too_large = "x".repeat(MAX_INPUT_BYTES + 1);
    assert_eq!(
        enqueue_paste(&mut queue, &too_large, false),
        Err(TerminalError::InputQueueFull)
    );
    assert_eq!(queue.len(), 0, "rejected paste must not add a prefix");

    let exact = "x".repeat(MAX_INPUT_BYTES - 12);
    enqueue_paste(&mut queue, &exact, true).unwrap();
    assert_eq!(queue.len(), MAX_INPUT_BYTES);
    assert!(queue.front().starts_with(b"\x1b[200~"));
    assert!(queue.front().ends_with(b"\x1b[201~"));

    let mut queue = InputQueue::new();
    queue.append_parts(&[b"abcdef"]).unwrap();
    let mut enabled = true;
    assert_eq!(
        write_attempt(&mut queue, &mut enabled, |_| Ok(2)).unwrap(),
        2
    );
    assert_eq!(queue.front(), b"cdef");
    assert_eq!(
        write_attempt(&mut queue, &mut enabled, |_| Err(
            io::ErrorKind::WouldBlock.into()
        ))
        .unwrap(),
        0
    );
    assert_eq!(queue.front(), b"cdef");
    assert_eq!(
        write_attempt(&mut queue, &mut enabled, |_| Ok(1)).unwrap(),
        1
    );
    assert_eq!(queue.front(), b"def");
    let error = write_attempt(&mut queue, &mut enabled, |_| {
        Err(io::ErrorKind::BrokenPipe.into())
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(!enabled);
    assert!(queue.is_empty(), "hard failure discards the suffix");
    let mut called = false;
    assert_eq!(
        write_attempt(&mut queue, &mut enabled, |_| {
            called = true;
            Ok(1)
        })
        .unwrap(),
        0
    );
    assert!(!called, "disabled input cannot be replayed");

    let mut queue = InputQueue::new();
    queue.append_parts(&[b"unsent"]).unwrap();
    let mut enabled = true;
    let error = write_attempt(&mut queue, &mut enabled, |_| Ok(0)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    assert!(!enabled);
    assert!(queue.is_empty(), "zero-byte write discards the suffix");
}

#[test]
fn typed_keys_use_engine_cursor_mode_and_reject_control_scalars() {
    assert_eq!(encode_key(Key::Up, false).unwrap(), b"\x1b[A");
    assert_eq!(encode_key(Key::Up, true).unwrap(), b"\x1bOA");
    assert_eq!(encode_key(Key::Home, false).unwrap(), b"\x1b[H");
    assert_eq!(encode_key(Key::End, true).unwrap(), b"\x1bOF");
    assert_eq!(encode_key(Key::Char('界'), false).unwrap(), "界".as_bytes());
    assert_eq!(
        encode_key(Key::Char('\u{1b}'), false),
        Err(TerminalError::InvalidKey)
    );
}

#[test]
fn resize_is_committed_only_after_the_system_operation_succeeds() {
    let initial = TerminalSize::new(24, 80).unwrap();
    let requested = TerminalSize::new(30, 100).unwrap();
    let mut parser = Parser::new(initial.rows, initial.columns, 0);
    let mut accepted = initial;
    let result = commit_resize(parser.screen_mut(), &mut accepted, requested, |_| {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    });
    assert!(matches!(
        result,
        Err(TerminalError::Io {
            operation: "resize PTY",
            kind: io::ErrorKind::PermissionDenied
        })
    ));
    assert_eq!(accepted, initial);
    assert_eq!(parser.screen().size(), (initial.rows, initial.columns));

    commit_resize(parser.screen_mut(), &mut accepted, requested, |_| Ok(())).unwrap();
    assert_eq!(accepted, requested);
    assert_eq!(parser.screen().size(), (requested.rows, requested.columns));
}

#[test]
fn output_bounds_cover_resize_insert_and_grid_count_commands() {
    let mut parser = Parser::new(24, 239, HISTORY_ROWS);
    parser.screen_mut().set_size(24, 240);
    parser.process(b"\x1b[1;1H\x1b[240@\x1b[65535L\x1b[65535T");
    assert_eq!(parser.screen().size(), (24, 240));
    assert!(parser.screen().cell(0, 239).is_some());
    assert!(std::mem::size_of::<vt100::Cell>() <= 32);

    let lines = b"row\r\n".repeat(1_300);
    parser.process(&lines);
    parser.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(parser.screen().scrollback(), HISTORY_ROWS);
    parser.screen_mut().set_scrollback(0);
}

#[test]
fn fixed_osc_storage_survives_chunked_oversize_and_combining_is_capped() {
    let mut parser = Parser::new_with_callbacks(8, 80, 0, OscLengthOnly::default());
    let mut bytes = b"\x1b]9;".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 20_000));
    bytes.extend_from_slice(b"\x07OSC-MARK");
    for chunk in bytes.chunks(4_096) {
        parser.process(chunk);
    }
    assert_eq!(parser.callbacks().0.len(), 1);
    assert!(
        (1_000..=1_024).contains(&parser.callbacks().0[0]),
        "the callback may see a truncated prefix, but it must stay within VTE's fixed OSC buffer"
    );
    let rendered: String = (0..8)
        .filter_map(|column| parser.screen().cell(0, column))
        .map(|cell| cell.contents())
        .collect();
    assert!(rendered.starts_with("OSC-MARK"));

    let mut combining = Parser::new(8, 80, 0);
    let flood = format!("X{}", "\u{301}".repeat(40));
    combining.process(flood.as_bytes());
    assert_eq!(
        combining
            .screen()
            .cell(0, 0)
            .unwrap()
            .contents()
            .chars()
            .count(),
        9,
        "a real base plus eight combining scalars is the engine cap"
    );
}

#[test]
fn explicit_shutdown_releases_controller_and_preserves_only_readable_snapshot() {
    let launch = ResolvedLaunch {
        requested_path: "/bin/sh".into(),
        executable_path: "/bin/sh".into(),
        working_directory: "/private/tmp".into(),
    };
    let environment = [(OsString::from("TERM"), OsString::from("panelink-probe"))]
        .into_iter()
        .collect();
    let mut session = TerminalSession::spawn(
        &launch,
        &[OsString::from("-c"), OsString::from("exit 0")],
        &environment,
        TerminalSize::new(8, 20).unwrap(),
    )
    .unwrap();
    session.paste("unsent").unwrap();
    session.shutdown().unwrap();
    assert!(
        session.master.is_none(),
        "shutdown must release the owned controller before Drop"
    );
    assert!(session.child.is_none());
    assert_eq!(session.pending_input_bytes(), 0);
    assert_eq!(session.pump(), Err(TerminalError::SessionClosed));
    assert_eq!(
        session.request_resize(TerminalSize::new(10, 30).unwrap()),
        Err(TerminalError::SessionClosed)
    );
    assert_eq!(session.snapshot().cells().len(), 8 * 20);
    session.shutdown().unwrap();
}

#[test]
fn snapshot_text_sanitization_and_debug_redaction_hold() {
    for forbidden in [
        '\u{1b}', '\u{85}', '\u{7f}', '\u{2028}', '\u{2029}', '\u{202e}', '\u{2066}',
    ] {
        assert!(unsafe_for_host(forbidden));
    }
    assert!(!unsafe_for_host('é'));
    assert_eq!(format!("{:?}", Key::Char('X')), "Char(<redacted>)");

    let mut parser = Parser::new_with_callbacks(8, 20, 0, CoreCallbacks::new());
    parser.process("café界e\u{301}".as_bytes());
    let mut snapshot = ScreenSnapshot::new(TerminalSize::new(8, 20).unwrap());
    snapshot.refresh(parser.screen(), TerminalSize::new(8, 20).unwrap());
    assert_eq!(snapshot.cell(0, 0).unwrap().text(), "c");
    assert!(snapshot.cell(0, 4).unwrap().is_wide());
    assert!(snapshot.cell(0, 5).unwrap().is_wide_continuation());
    assert_eq!(snapshot.cell(0, 6).unwrap().text(), "e\u{301}");
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains("café"));
    assert!(std::mem::size_of::<SnapshotCell>() * 80 * 240 <= 2_000_000);

    let mut controls = Parser::new_with_callbacks(8, 20, 0, CoreCallbacks::new());
    controls.process("A\u{202e}\u{2028}\u{2029}B".as_bytes());
    snapshot.refresh(controls.screen(), TerminalSize::new(8, 20).unwrap());
    let visible: String = snapshot.cells().iter().map(SnapshotCell::text).collect();
    assert!(visible.contains("A���B"));
    assert!(
        !visible.contains('\u{202e}')
            && !visible.contains('\u{2028}')
            && !visible.contains('\u{2029}')
    );
}

use std::io;
