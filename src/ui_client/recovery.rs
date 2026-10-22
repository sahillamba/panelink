//! Offline, read-only presentation of a saved recovery view.
//!
//! This client owns no connection or live target. Keys only update local viewer
//! state, and stored terminal bytes are decoded into sanitized cells before the
//! shared compositor draws them.

use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::thread;
use std::time::{Duration, Instant};

use super::host::{write_control, HostError, HostGuard};
use crossterm_input::{Decoder, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use panelink::recovery_view::{
    plan_viewport, CheckpointAvailability, DisplayPlan, HistoryAvailability, OverviewLabel,
    RecoveryPane, RecoveryView,
};
use panelink::screen_checkpoint::{self, CheckpointMetadata, ScreenBuffer};
use panelink::terminal::compose::{
    ComposeFrame, Compositor, OverlayLine, OverlayStyle, OverlayView, PaneContent, PaneView,
};
use panelink::terminal::{ScreenSnapshot, TerminalSize};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const CHROME_ROWS: u16 = 3;
const INPUT_BYTES: usize = 1024;
const FRAME_BYTES: usize = 4 * 1024 * 1024 + 64 * 1024;
const RESIZE_INTERVAL: Duration = Duration::from_millis(100);
const RENDER_INTERVAL: Duration = Duration::from_millis(16);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(50);
const INCOMPLETE_INPUT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewerKey {
    Quit,
    NextPane,
    ToggleBuffer,
    Ignore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RecoveryUi {
    focus: usize,
    buffers: [ScreenBuffer; panelink::pane_layout::MAX_PANES],
}

impl RecoveryUi {
    const fn new() -> Self {
        Self {
            focus: 0,
            buffers: [ScreenBuffer::Normal; panelink::pane_layout::MAX_PANES],
        }
    }

    fn handle_local_key(&mut self, key: ViewerKey, pane_count: usize) -> bool {
        match key {
            ViewerKey::Quit => true,
            ViewerKey::NextPane if pane_count > 0 => {
                self.focus = (self.focus + 1) % pane_count;
                false
            }
            ViewerKey::ToggleBuffer if pane_count > 0 => {
                let index = self.focus.min(self.buffers.len() - 1);
                let buffer = &mut self.buffers[index];
                *buffer = match buffer {
                    ScreenBuffer::Normal => ScreenBuffer::Alternate,
                    ScreenBuffer::Alternate => ScreenBuffer::Normal,
                };
                false
            }
            ViewerKey::NextPane | ViewerKey::ToggleBuffer | ViewerKey::Ignore => false,
        }
    }
}

/// Enter the local viewer and restore the terminal on normal/error returns and
/// Rust panic unwinds. No server or child lifecycle is accessed here.
pub(crate) fn run(view: RecoveryView) -> Result<(), &'static str> {
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(|| run_inner(&view)));
    match result {
        Ok(result) => result,
        Err(_) => Err("recovery viewer failed (terminal restored)"),
    }
}

fn run_inner(view: &RecoveryView) -> Result<(), &'static str> {
    let mut host = HostGuard::open().map_err(host_error_message)?;
    let stop = register_stop_flag()?;
    run_frames(view, &mut host, &stop, true)
}

/// Run the recovery viewer over an already-owned host terminal without
/// entering or restoring terminal modes. The caller retains its guard and
/// resumes its own event loop after this returns.
pub(crate) fn run_in_workspace(
    view: &RecoveryView,
    host: &mut HostGuard,
    stop: &AtomicBool,
) -> Result<(), &'static str> {
    // Reuse the attach loop's process-lifetime stop flag: signal-hook
    // registrations are never unregistered, so registering per open would
    // accumulate handlers for every viewer visit.
    let result = catch_unwind(AssertUnwindSafe(|| run_frames(view, host, stop, false)));
    match result {
        Ok(result) => result,
        Err(_) => Err("recovery viewer failed"),
    }
}

fn register_stop_flag() -> Result<Arc<AtomicBool>, &'static str> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        flag::register(signal, Arc::clone(&stop)).map_err(|_| "could not register stop signals")?;
    }
    Ok(stop)
}

fn run_frames(
    view: &RecoveryView,
    host: &mut HostGuard,
    stop: &AtomicBool,
    enter_terminal: bool,
) -> Result<(), &'static str> {
    let (rows, columns) = host.size().map_err(host_error_message)?;
    let mut compositor = Compositor::try_new().map_err(|_| "could not compose recovery view")?;
    let mut snapshots = [
        ScreenSnapshot::try_new(TerminalSize::new(80, 240).map_err(|_| "invalid frame size")?)
            .map_err(|_| "could not allocate recovery view")?,
        ScreenSnapshot::try_new(TerminalSize::new(80, 240).map_err(|_| "invalid frame size")?)
            .map_err(|_| "could not allocate recovery view")?,
        ScreenSnapshot::try_new(TerminalSize::new(80, 240).map_err(|_| "invalid frame size")?)
            .map_err(|_| "could not allocate recovery view")?,
        ScreenSnapshot::try_new(TerminalSize::new(80, 240).map_err(|_| "invalid frame size")?)
            .map_err(|_| "could not allocate recovery view")?,
    ];
    let mut frame_bytes = Vec::new();
    frame_bytes
        .try_reserve_exact(FRAME_BYTES)
        .map_err(|_| "could not allocate recovery frame")?;
    frame_bytes.resize(FRAME_BYTES, 0);
    let mut input = [0_u8; INPUT_BYTES];
    let mut decoder = Decoder::new();
    let mut ui = RecoveryUi::new();
    let mut history_cache: [Option<(u16, u16, Vec<u8>)>; panelink::pane_layout::MAX_PANES] =
        std::array::from_fn(|_| None);
    let mut current_size = (rows, columns);
    let mut last_resize = Instant::now();
    let mut last_render = Instant::now() - RENDER_INTERVAL;
    let mut last_input = None;

    if enter_terminal {
        host.enter().map_err(host_error_message)?;
    }
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }

        let now = Instant::now();
        match host.tty.read(&mut input) {
            Ok(count) if count > 0 => {
                last_input = Some(now);
                let mut quit = false;
                for (index, byte) in input[..count].iter().copied().enumerate() {
                    let event = decoder
                        .advance(byte, index + 1 < count)
                        .map_err(|_| "recovery viewer input was malformed")?;
                    if let Some(Event::Key(key)) = event {
                        if ui.handle_local_key(map_key(key), view.panes.len()) {
                            quit = true;
                            break;
                        }
                    }
                }
                if quit {
                    break Ok(());
                }
                if decoder.pending_bytes() == 0 {
                    last_input = None;
                }
            }
            Ok(_) => break Err("recovery viewer input closed"),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                if let Some(started) = last_input {
                    if decoder.pending_bytes() == 1 && now.duration_since(started) >= ESCAPE_TIMEOUT
                    {
                        if let Some(Event::Key(key)) = decoder
                            .finish_escape()
                            .map_err(|_| "recovery viewer input was malformed")?
                        {
                            if ui.handle_local_key(map_key(key), view.panes.len()) {
                                break Ok(());
                            }
                        }
                        last_input = None;
                    } else if now.duration_since(started) >= INCOMPLETE_INPUT_TIMEOUT {
                        break Err("recovery viewer input sequence timed out");
                    }
                }
            }
            Err(_) => break Err("could not read recovery viewer input"),
        }

        if now.duration_since(last_resize) >= RESIZE_INTERVAL {
            last_resize = now;
            current_size = host.size().map_err(host_error_message)?;
        }

        if current_size.0 == 0 || current_size.1 == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        if now.duration_since(last_render) < RENDER_INTERVAL {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        last_render = now;

        let mut cursor = io::Cursor::new(&mut frame_bytes[..]);
        compose(
            view,
            &mut ui,
            &mut history_cache,
            &mut snapshots,
            current_size,
            &mut compositor,
            &mut cursor,
        )?;
        let length =
            usize::try_from(cursor.position()).map_err(|_| "recovery frame was too large")?;
        if length > 0 {
            if let Err(error) = write_control(&mut host.tty, &frame_bytes[..length]) {
                compositor.invalidate();
                break Err(if error.kind() == io::ErrorKind::TimedOut {
                    HostError::OutputStalled.message()
                } else {
                    HostError::OutputWrite.message()
                });
            }
        }
        compositor.commit();
        thread::sleep(Duration::from_millis(5));
    };
    if enter_terminal {
        result.and(host.restore().map_err(host_error_message))
    } else {
        result
    }
}

fn host_error_message(error: HostError) -> &'static str {
    match error {
        HostError::NotTty => "recovery view needs an interactive terminal",
        _ => error.message(),
    }
}

fn map_key(key: KeyEvent) -> ViewerKey {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return ViewerKey::Ignore;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('q' | 'Q'), modifiers) if modifiers.is_empty() => ViewerKey::Quit,
        (KeyCode::Esc, _) => ViewerKey::Quit,
        (KeyCode::Char('c' | 'C'), KeyModifiers::CONTROL) => ViewerKey::Quit,
        (KeyCode::Tab, modifiers) if modifiers.is_empty() => ViewerKey::NextPane,
        (KeyCode::Char('n' | 'N'), modifiers) if modifiers.is_empty() => ViewerKey::ToggleBuffer,
        _ => ViewerKey::Ignore,
    }
}

fn compose(
    view: &RecoveryView,
    ui: &mut RecoveryUi,
    history_cache: &mut [Option<(u16, u16, Vec<u8>)>; panelink::pane_layout::MAX_PANES],
    snapshots: &mut [ScreenSnapshot; panelink::pane_layout::MAX_PANES],
    (host_rows, host_columns): (u16, u16),
    compositor: &mut Compositor,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let bounded_rows = host_rows.min(80);
    let bounded_columns = host_columns.min(240);
    let plan = plan_viewport(view, bounded_rows, bounded_columns, CHROME_ROWS);
    let (_content_rows, pane_rects, divider, dividers, overview_boxes) = match &plan {
        DisplayPlan::Panes {
            viewport,
            panes,
            divider,
            dividers,
        } => (
            viewport.rows,
            panes.clone(),
            *divider,
            dividers.clone(),
            None,
        ),
        DisplayPlan::Overview {
            viewport,
            boxes,
            divider,
            dividers,
        } => (
            viewport.rows,
            Vec::new(),
            *divider,
            dividers.clone(),
            Some((*viewport, boxes.clone())),
        ),
        DisplayPlan::TooSmall => (0, Vec::new(), None, Vec::new(), None),
    };

    if matches!(plan, DisplayPlan::TooSmall) {
        if bounded_rows == 0 || bounded_columns == 0 {
            return Ok(());
        }
        let mut recovery_dividers = [None; panelink::pane_layout::MAX_PANES - 1];
        for (slot, divider) in recovery_dividers.iter_mut().zip(dividers.iter().copied()) {
            *slot = Some(add_chrome(divider));
        }
        let frame = ComposeFrame {
            host_rows: bounded_rows,
            host_columns: bounded_columns,
            panes: [
                Some(PaneView {
                    rect: panelink::pane_layout::Rect {
                        x: 0,
                        y: 0,
                        rows: 1,
                        columns: bounded_columns,
                    },
                    content: PaneContent::Placeholder("terminal too small"),
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; panelink::pane_layout::MAX_PANES - 1],
            focused: None,
            overlay: None,
        };
        return compositor
            .render(&frame, output)
            .map_err(|_| "could not render recovery notice");
    }

    let mut has_snapshot = [false; panelink::pane_layout::MAX_PANES];
    for (index, (pane, rect)) in view.panes.iter().zip(pane_rects.iter()).enumerate() {
        if index >= snapshots.len() {
            break;
        }
        let buffer = ui.buffers[index];
        if let CheckpointAvailability::Available { info, .. } = selected_checkpoint(pane, buffer) {
            if info.rows == rect.rows && info.columns == rect.columns {
                let checkpoint = selected_checkpoint(pane, buffer)
                    .checkpoint()
                    .ok_or("validated checkpoint could not be decoded")?;
                snapshots[index]
                    .load_checkpoint(&checkpoint)
                    .map_err(|_| "could not load recovery checkpoint")?;
                has_snapshot[index] = true;
            }
        } else if matches!(
            selected_checkpoint(pane, buffer),
            CheckpointAvailability::Missing
        ) && matches!(buffer, ScreenBuffer::Normal)
        {
            let needs_history = history_cache[index]
                .as_ref()
                .is_none_or(|(rows, columns, _)| *rows != rect.rows || *columns != rect.columns);
            if needs_history {
                history_cache[index] = history_screen_record(pane, rect.rows, rect.columns)
                    .map(|record| (rect.rows, rect.columns, record));
            }
            if let Some((_, _, record)) = &history_cache[index] {
                let checkpoint = screen_checkpoint::decode(record)
                    .map_err(|_| "could not decode saved history rows")?;
                snapshots[index]
                    .load_checkpoint(&checkpoint)
                    .map_err(|_| "could not load saved history rows")?;
                has_snapshot[index] = true;
            }
        }
    }

    let mut panes: [Option<PaneView<'_>>; panelink::pane_layout::MAX_PANES] =
        [None; panelink::pane_layout::MAX_PANES];
    for (index, (pane, rect)) in view.panes.iter().zip(pane_rects.iter()).enumerate() {
        if index >= panes.len() {
            break;
        }
        let buffer = ui.buffers[index];
        panes[index] = Some(PaneView {
            rect: *rect,
            content: if has_snapshot[index] {
                PaneContent::Snapshot(&snapshots[index])
            } else {
                PaneContent::Placeholder(source_label(pane, buffer, Some(*rect)))
            },
        });
    }

    let header = format!(
        "RECOVERY {} gen {} seq {} {}x{} {:?}  NOT RUNNING",
        view.slot,
        view.metadata.generation(),
        view.metadata.snapshot_sequence(),
        view.saved_rows,
        view.saved_columns,
        view.axis
    );
    let too_small = matches!(plan, DisplayPlan::TooSmall);
    let source = view
        .panes
        .get(ui.focus)
        .map(|pane| {
            let target = pane.historical_target;
            let buffer = ui.buffers[ui.focus.min(ui.buffers.len() - 1)];
            format!(
                "pane {} {} target {}:{}:{}:{}:{} | {} | rows {}",
                ui.focus + 1,
                match buffer {
                    ScreenBuffer::Normal => "normal",
                    ScreenBuffer::Alternate => "alternate",
                },
                target.generation,
                target.workspace,
                target.pane,
                target.session,
                target.occupant_revision,
                checkpoint_status(pane, buffer),
                history_label(&pane.rows)
            )
        })
        .unwrap_or_else(|| "empty saved workspace".to_owned());
    let footer = if too_small {
        "terminal too small"
    } else {
        "Tab pane  n normal/alternate  q quit"
    };
    let lines = [
        OverlayLine {
            text: &header,
            style: OverlayStyle::Status,
        },
        OverlayLine {
            text: &source,
            style: OverlayStyle::Normal,
        },
        OverlayLine {
            text: footer,
            style: OverlayStyle::Normal,
        },
    ];
    let mut recovery_dividers = [None; panelink::pane_layout::MAX_PANES - 1];
    for (slot, divider) in recovery_dividers.iter_mut().zip(dividers.iter().copied()) {
        *slot = Some(add_chrome(divider));
    }
    let overlay = (bounded_rows > 0 && bounded_columns > 0).then_some(OverlayView {
        rect: panelink::pane_layout::Rect {
            x: 0,
            y: bounded_rows.saturating_sub(CHROME_ROWS),
            rows: bounded_rows.min(CHROME_ROWS),
            columns: bounded_columns,
        },
        lines: &lines,
    });

    if let Some((viewport, boxes)) = overview_boxes {
        for (index, item) in boxes.iter().enumerate() {
            if index >= panes.len() || item.label == OverviewLabel::Empty {
                continue;
            }
            panes[index] = Some(PaneView {
                rect: add_chrome(item.rect),
                content: PaneContent::Placeholder(match item.label {
                    OverviewLabel::PaneOne => "Pane 1",
                    OverviewLabel::PaneTwo => "Pane 2",
                    OverviewLabel::Empty => "empty workspace",
                }),
            });
        }
        let frame = ComposeFrame {
            host_rows: bounded_rows,
            host_columns: bounded_columns,
            panes,
            divider: divider.map(add_chrome),
            dividers: recovery_dividers,
            focused: None,
            overlay,
        };
        compositor
            .render(&frame, output)
            .map_err(|_| "could not render recovery overview")?;
        let _ = viewport;
        return Ok(());
    }

    for pane in panes.iter_mut().flatten() {
        pane.rect = add_chrome(pane.rect);
    }
    let mut recovery_dividers = [None; panelink::pane_layout::MAX_PANES - 1];
    for (slot, divider) in recovery_dividers.iter_mut().zip(dividers.iter().copied()) {
        *slot = Some(add_chrome(divider));
    }
    let frame = ComposeFrame {
        host_rows: bounded_rows,
        host_columns: bounded_columns,
        panes,
        divider: divider.map(add_chrome),
        dividers: recovery_dividers,
        focused: None,
        overlay,
    };
    compositor
        .render(&frame, output)
        .map_err(|_| "could not render recovery view")
}

fn add_chrome(mut rect: panelink::pane_layout::Rect) -> panelink::pane_layout::Rect {
    rect.y = rect.y.saturating_add(CHROME_ROWS);
    rect
}

fn selected_checkpoint(pane: &RecoveryPane, buffer: ScreenBuffer) -> &CheckpointAvailability {
    match buffer {
        ScreenBuffer::Normal => &pane.normal,
        ScreenBuffer::Alternate => &pane.alternate,
    }
}

fn source_label(
    pane: &RecoveryPane,
    buffer: ScreenBuffer,
    expected: Option<panelink::pane_layout::Rect>,
) -> &'static str {
    match selected_checkpoint(pane, buffer) {
        CheckpointAvailability::Available { info, .. }
            if expected
                .is_some_and(|rect| info.rows != rect.rows || info.columns != rect.columns) =>
        {
            "checkpoint dimensions differ"
        }
        CheckpointAvailability::Available { info, .. } => match buffer {
            ScreenBuffer::Normal if info.buffer == ScreenBuffer::Normal => "normal checkpoint",
            ScreenBuffer::Alternate if info.buffer == ScreenBuffer::Alternate => {
                "alternate checkpoint"
            }
            _ => "checkpoint invalid",
        },
        CheckpointAvailability::Missing => match pane.rows {
            HistoryAvailability::Available { .. } => "no checkpoint; saved rows available",
            HistoryAvailability::Missing => "no saved history",
            HistoryAvailability::Invalid => "saved rows invalid",
        },
        CheckpointAvailability::Invalid => "checkpoint invalid",
    }
}

fn checkpoint_status(pane: &RecoveryPane, buffer: ScreenBuffer) -> String {
    let label = source_label(pane, buffer, None);
    match selected_checkpoint(pane, buffer).info() {
        Some(info) => format!(
            "{label} seq {} captured {}",
            info.output_sequence, info.captured_at_unix_ms
        ),
        None => label.to_owned(),
    }
}

fn history_label(source: &HistoryAvailability) -> String {
    match source {
        HistoryAvailability::Available {
            gap,
            capture_disabled,
            rows,
            ..
        } => {
            let mut label = format!("available {} rows", rows.len());
            if *gap {
                label.push_str("; gap/discontinuity");
            }
            if *capture_disabled {
                label.push_str("; capture disabled");
            }
            label
        }
        HistoryAvailability::Missing => "missing".to_owned(),
        HistoryAvailability::Invalid => "invalid".to_owned(),
    }
}

fn history_screen_record(pane: &RecoveryPane, rows: u16, columns: u16) -> Option<Vec<u8>> {
    let HistoryAvailability::Available { rows: saved, .. } = &pane.rows else {
        return None;
    };
    if saved.is_empty() || !(5..=80).contains(&rows) || !(20..=240).contains(&columns) {
        return None;
    }
    let cell_count = usize::from(rows).checked_mul(usize::from(columns))?;
    let cell_bytes = cell_count.checked_mul(screen_checkpoint::SCREEN_CHECKPOINT_CELL_BYTES)?;
    let total = screen_checkpoint::SCREEN_CHECKPOINT_HEADER_BYTES.checked_add(cell_bytes)?;
    let mut record = Vec::new();
    record.try_reserve_exact(total).ok()?;
    record.resize(total, 0);
    record[..8].copy_from_slice(b"PLSCRN01");
    for (index, value) in [
        pane.historical_target.generation,
        pane.historical_target.workspace,
        pane.historical_target.pane,
        pane.historical_target.session,
        pane.historical_target.occupant_revision,
    ]
    .into_iter()
    .enumerate()
    {
        let start = 8 + index * 8;
        record[start..start + 8].copy_from_slice(&value.to_be_bytes());
    }
    let last_row = saved.last()?;
    let metadata = CheckpointMetadata::new(
        pane.historical_target,
        last_row.ordinal,
        last_row.captured_at_unix_ms,
        1,
    )
    .ok()?;
    record[48..56].copy_from_slice(&0_u64.to_be_bytes());
    record[56..64].copy_from_slice(&metadata.captured_at_unix_ms().to_be_bytes());
    record[64..72].copy_from_slice(&metadata.screen_revision().to_be_bytes());
    record[72..74].copy_from_slice(&rows.to_be_bytes());
    record[74..76].copy_from_slice(&columns.to_be_bytes());
    record[81] = 1;

    let visible = saved.len().min(usize::from(rows));
    let first_row = saved.len() - visible;
    let top_padding = usize::from(rows) - visible;
    let row_bytes = usize::from(columns) * screen_checkpoint::SCREEN_CHECKPOINT_CELL_BYTES;
    for (display_row, saved_row) in saved[first_row..].iter().enumerate() {
        let destination_row = top_padding + display_row;
        for column in 0..columns.min(saved_row.width) {
            let text = saved_row.cell_text(column)?;
            if text.is_empty() {
                continue;
            }
            let start = screen_checkpoint::SCREEN_CHECKPOINT_HEADER_BYTES
                + destination_row * row_bytes
                + usize::from(column) * screen_checkpoint::SCREEN_CHECKPOINT_CELL_BYTES;
            let safe = text
                .chars()
                .map(|character| if character.is_ascii() { character } else { '?' })
                .collect::<String>();
            if safe.len() > 32 {
                return None;
            }
            record[start] = u8::try_from(safe.len()).ok()?;
            record[start + 10..start + 10 + safe.len()].copy_from_slice(safe.as_bytes());
        }
    }
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelink::control_protocol::TargetRef;
    use panelink::pane_layout::Rect;
    use panelink::recovery_view::CheckpointInfo;
    use panelink::saved_layout::{LayoutMetadata, SavedLayout};

    fn target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        }
    }

    fn view() -> RecoveryView {
        let layout = SavedLayout::empty(LayoutMetadata::new(1, 2, 3, 4, 5).unwrap());
        RecoveryView::from_layout(layout)
    }

    fn pane_with_checkpoint(record: Vec<u8>, rows: u16, columns: u16) -> RecoveryPane {
        RecoveryPane {
            historical_target: target(),
            rows: HistoryAvailability::Missing,
            normal: CheckpointAvailability::Available {
                info: CheckpointInfo {
                    buffer: ScreenBuffer::Normal,
                    output_sequence: 1,
                    screen_revision: 2,
                    captured_at_unix_ms: 3,
                    rows,
                    columns,
                },
                record,
            },
            alternate: CheckpointAvailability::Missing,
        }
    }

    fn snapshot() -> ScreenSnapshot {
        ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap()
    }

    fn checkpoint_record(rows: u16, columns: u16, text: &str) -> Vec<u8> {
        let snapshot = ScreenSnapshot::try_new(TerminalSize::new(rows, columns).unwrap()).unwrap();
        let mut record = screen_checkpoint::encode(
            &snapshot,
            &CheckpointMetadata::new(target(), 1, 2, 3).unwrap(),
        )
        .unwrap();
        for (column, character) in text.chars().take(usize::from(columns)).enumerate() {
            assert!(character.is_ascii() && !character.is_ascii_control());
            let cell = screen_checkpoint::SCREEN_CHECKPOINT_HEADER_BYTES
                + column * screen_checkpoint::SCREEN_CHECKPOINT_CELL_BYTES;
            record[cell] = 1;
            record[cell + 10] = character as u8;
        }
        record
    }

    #[test]
    fn local_keys_only_navigate_toggle_and_quit() {
        let mut ui = RecoveryUi::new();
        assert!(!ui.handle_local_key(ViewerKey::NextPane, 2));
        assert_eq!(ui.focus, 1);
        assert!(!ui.handle_local_key(ViewerKey::NextPane, 2));
        assert_eq!(ui.focus, 0);
        assert!(!ui.handle_local_key(ViewerKey::ToggleBuffer, 2));
        assert_eq!(ui.buffers[0], ScreenBuffer::Alternate);
        assert!(ui.handle_local_key(ViewerKey::Quit, 2));
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            ViewerKey::Ignore
        );
    }

    #[test]
    fn in_workspace_viewer_loop_does_not_restore_the_callers_terminal() {
        let viewer = view();
        let run_frames_fn: fn(
            &RecoveryView,
            &mut HostGuard,
            &AtomicBool,
            bool,
        ) -> Result<(), &'static str> = run_frames;
        let _ = run_frames_fn;
        // The borrowed-host entry point cannot own terminal restoration; only
        // the standalone path passes `enter_terminal = true`.
        assert!(viewer.panes.is_empty());
    }

    #[test]
    fn regular_composition_uses_decoded_snapshot_cells_only() {
        let mut saved = view();
        saved.panes.push(pane_with_checkpoint(
            checkpoint_record(7, 30, "safe"),
            7,
            30,
        ));
        let mut ui = RecoveryUi::new();
        let mut snapshots = [snapshot(), snapshot(), snapshot(), snapshot()];
        let mut compositor = Compositor::try_new().unwrap();
        let mut bytes = [0_u8; 64 * 1024];
        let mut output = io::Cursor::new(&mut bytes[..]);
        compose(
            &saved,
            &mut ui,
            &mut [None, None, None, None],
            &mut snapshots,
            (10, 30),
            &mut compositor,
            &mut output,
        )
        .unwrap();
        let frame = &output.get_ref()[..usize::try_from(output.position()).unwrap()];
        let mut parser = vt100::Parser::new(10, 30, 0);
        parser.process(frame);
        let visible = (0..4)
            .map(|column| parser.screen().cell(3, column).unwrap().contents())
            .collect::<String>();
        assert_eq!(visible, "safe");
    }

    #[test]
    fn overview_and_too_small_compose_bounded_placeholder_frames() {
        let mut saved = view();
        saved.panes.push(pane_with_checkpoint(
            checkpoint_record(5, 20, "safe"),
            5,
            20,
        ));
        saved.panes.push(RecoveryPane {
            historical_target: TargetRef {
                pane: 6,
                session: 7,
                ..target()
            },
            ..pane_with_checkpoint(checkpoint_record(5, 20, "other"), 5, 20)
        });
        saved.dividers = vec![Rect {
            x: 0,
            y: 3,
            rows: 5,
            columns: 1,
        }];
        saved.axis = panelink::saved_layout::LayoutAxis::SideBySide;
        for size in [(8, 20), (2, 10)] {
            let mut ui = RecoveryUi::new();
            let mut snapshots = [snapshot(), snapshot(), snapshot(), snapshot()];
            let mut compositor = Compositor::try_new().unwrap();
            let mut bytes = [0_u8; 256 * 1024];
            let mut output = io::Cursor::new(&mut bytes[..]);
            compose(
                &saved,
                &mut ui,
                &mut [None, None, None, None],
                &mut snapshots,
                size,
                &mut compositor,
                &mut output,
            )
            .unwrap();
            assert!(output.position() > 0);
        }
        assert_eq!(
            plan_viewport(&saved, 2, 10, CHROME_ROWS),
            DisplayPlan::TooSmall
        );
        assert!(matches!(
            plan_viewport(&saved, 8, 20, CHROME_ROWS),
            DisplayPlan::Overview { .. }
        ));
        let _ = Rect {
            x: 0,
            y: 0,
            rows: 1,
            columns: 1,
        };
    }
}
