//! Pure, bounded rendering from a sanitized terminal snapshot.

use std::io::{self, Write};

use unicode_width::UnicodeWidthChar;

use super::{unsafe_for_host, Color, ScreenSnapshot, SnapshotCell};

/// Stream a visible snapshot as basic ANSI controls and cell content.
///
/// The caller owns host terminal IO and restoration. If a write fails after a
/// partial frame, this function returns that error without retaining or
/// retrying the frame; the caller must restore host modes during cleanup.
pub fn render<W: Write>(
    snapshot: &ScreenSnapshot,
    host_rows: u16,
    host_columns: u16,
    output: &mut W,
) -> io::Result<()> {
    validate_snapshot(snapshot, host_rows, host_columns)?;

    output.write_all(b"\x1b[?25l")?;
    output.write_all(b"\x1b[?7l")?;
    output.write_all(b"\x1b[0m")?;
    output.write_all(b"\x1b[2J")?;

    for row in 0..snapshot.rows() {
        write_row(output, snapshot, row)?;
    }

    output.write_all(b"\x1b[0m")?;
    write_cursor(output, snapshot)?;
    output.write_all(b"\x1b[?7h")?;
    if !snapshot.cursor_hidden() {
        output.write_all(b"\x1b[?25h")?;
    }
    Ok(())
}

/// Update a previously rendered snapshot without clearing the host screen.
///
/// The caller owns `previous` and should advance it only after the complete
/// output frame has been accepted by host writes. The renderer keeps no frame
/// state. A first frame or a dimension change uses [`render`] to establish a
/// complete screen; otherwise only changed cells, the final cursor, and the
/// cursor or wrapping modes needed for the update are emitted.
pub fn render_diff<W: Write>(
    snapshot: &ScreenSnapshot,
    previous: Option<&ScreenSnapshot>,
    host_rows: u16,
    host_columns: u16,
    output: &mut W,
) -> io::Result<()> {
    validate_snapshot(snapshot, host_rows, host_columns)?;

    let Some(previous) = previous else {
        return render(snapshot, host_rows, host_columns, output);
    };
    if previous.rows() != snapshot.rows() || previous.columns() != snapshot.columns() {
        return render(snapshot, host_rows, host_columns, output);
    }
    validate_snapshot(previous, host_rows, host_columns)?;

    if snapshots_equal(snapshot, previous) {
        return Ok(());
    }

    let cells_changed = snapshot.cells() != previous.cells();
    let cursor_changed = snapshot.cursor_row() != previous.cursor_row()
        || snapshot.cursor_column() != previous.cursor_column();
    let visibility_changed = snapshot.cursor_hidden() != previous.cursor_hidden();

    if !cells_changed {
        if cursor_changed {
            write_cursor(output, snapshot)?;
        }
        if visibility_changed {
            output.write_all(if snapshot.cursor_hidden() {
                b"\x1b[?25l"
            } else {
                b"\x1b[?25h"
            })?;
        }
        return Ok(());
    }

    if !previous.cursor_hidden() {
        output.write_all(b"\x1b[?25l")?;
    }
    output.write_all(b"\x1b[?7l")?;

    for row in 0..snapshot.rows() {
        if !row_changed(snapshot, previous, row) {
            continue;
        }
        if row_topology_requires_repaint(snapshot, previous, row) {
            write_row(output, snapshot, row)?;
        } else {
            write_changed_row(output, snapshot, previous, row)?;
        }
    }

    output.write_all(b"\x1b[0m")?;
    output.write_all(b"\x1b[?7h")?;
    write_cursor(output, snapshot)?;
    if !snapshot.cursor_hidden() {
        output.write_all(b"\x1b[?25h")?;
    }
    Ok(())
}

fn validate_snapshot(
    snapshot: &ScreenSnapshot,
    host_rows: u16,
    host_columns: u16,
) -> io::Result<()> {
    if host_rows < snapshot.rows() || host_columns < snapshot.columns() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "host viewport is smaller than the screen snapshot",
        ));
    }

    let expected_cells = usize::from(snapshot.rows())
        .checked_mul(usize::from(snapshot.columns()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid screen dimensions"))?;
    if snapshot.rows() == 0 || snapshot.columns() == 0 || snapshot.cells().len() != expected_cells {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "screen snapshot cell storage is inconsistent",
        ));
    }
    Ok(())
}

fn snapshots_equal(left: &ScreenSnapshot, right: &ScreenSnapshot) -> bool {
    left.rows() == right.rows()
        && left.columns() == right.columns()
        && left.cursor_row() == right.cursor_row()
        && left.cursor_column() == right.cursor_column()
        && left.alternate_screen() == right.alternate_screen()
        && left.cursor_hidden() == right.cursor_hidden()
        && left.application_cursor() == right.application_cursor()
        && left.bracketed_paste() == right.bracketed_paste()
        && left.cells() == right.cells()
}

fn write_cursor<W: Write>(output: &mut W, snapshot: &ScreenSnapshot) -> io::Result<()> {
    let cursor_row = snapshot.cursor_row().min(snapshot.rows() - 1);
    let cursor_column = snapshot.cursor_column().min(snapshot.columns() - 1);
    write!(output, "\x1b[{};{}H", cursor_row + 1, cursor_column + 1)
}

fn write_row<W: Write>(output: &mut W, snapshot: &ScreenSnapshot, row: u16) -> io::Result<()> {
    let mut column = 0;
    while column < snapshot.columns() {
        let cell = snapshot.cell(row, column).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "screen snapshot cell is missing",
            )
        })?;

        if cell.is_wide_continuation() {
            write_positioned_cell(output, row, column, cell, CellContent::Blank, 1)?;
            column += 1;
            continue;
        }

        if cell.is_wide() {
            let has_continuation = column + 1 < snapshot.columns()
                && snapshot
                    .cell(row, column + 1)
                    .is_some_and(SnapshotCell::is_wide_continuation);
            if has_continuation {
                write_positioned_cell(
                    output,
                    row,
                    column,
                    cell,
                    CellContent::Text(cell.text(), 2),
                    2,
                )?;
                column += 2;
            } else {
                // A wide cell without its continuation cannot be trusted to
                // occupy two host cells. Keep its fallback inside this row.
                let occupied_width = usize::from(snapshot.columns() - column).min(2);
                write_positioned_cell(
                    output,
                    row,
                    column,
                    cell,
                    CellContent::Fallback,
                    occupied_width,
                )?;
                column += 1;
            }
            continue;
        }

        let content = if cell.text().is_empty() {
            CellContent::Blank
        } else {
            CellContent::Text(cell.text(), 1)
        };
        write_positioned_cell(output, row, column, cell, content, 1)?;
        column += 1;
    }
    Ok(())
}

fn row_changed(snapshot: &ScreenSnapshot, previous: &ScreenSnapshot, row: u16) -> bool {
    let start = usize::from(row) * usize::from(snapshot.columns());
    let end = start + usize::from(snapshot.columns());
    snapshot.cells()[start..end] != previous.cells()[start..end]
}

fn row_topology_requires_repaint(
    snapshot: &ScreenSnapshot,
    previous: &ScreenSnapshot,
    row: u16,
) -> bool {
    let mut topology_changed = false;
    for column in 0..snapshot.columns() {
        let current = snapshot.cell(row, column).expect("validated snapshot cell");
        let old = previous.cell(row, column).expect("validated snapshot cell");
        if current.is_wide() != old.is_wide()
            || current.is_wide_continuation() != old.is_wide_continuation()
        {
            topology_changed = true;
            break;
        }
    }

    topology_changed
        || !row_has_valid_wide_topology(snapshot, row)
        || !row_has_valid_wide_topology(previous, row)
}

fn row_has_valid_wide_topology(snapshot: &ScreenSnapshot, row: u16) -> bool {
    for column in 0..snapshot.columns() {
        let cell = snapshot.cell(row, column).expect("validated snapshot cell");
        if cell.is_wide() && cell.is_wide_continuation() {
            return false;
        }
        if cell.is_wide() {
            let Some(next_column) = column.checked_add(1) else {
                return false;
            };
            let Some(next) = snapshot.cell(row, next_column) else {
                return false;
            };
            if !next.is_wide_continuation() || next.is_wide() {
                return false;
            }
        }
        if cell.is_wide_continuation() {
            if column == 0 {
                return false;
            }
            let Some(previous_column) = column.checked_sub(1) else {
                return false;
            };
            let Some(previous) = snapshot.cell(row, previous_column) else {
                return false;
            };
            if !previous.is_wide() || previous.is_wide_continuation() {
                return false;
            }
        }
    }
    true
}

fn write_changed_row<W: Write>(
    output: &mut W,
    snapshot: &ScreenSnapshot,
    previous: &ScreenSnapshot,
    row: u16,
) -> io::Result<()> {
    let mut column = 0;
    while column < snapshot.columns() {
        let cell = snapshot.cell(row, column).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "screen snapshot cell is missing",
            )
        })?;
        let old = previous.cell(row, column).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "previous screen snapshot cell is missing",
            )
        })?;

        if cell.is_wide() {
            let next_column = column + 1;
            let next = snapshot.cell(row, next_column).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "wide screen snapshot cell is missing its continuation",
                )
            })?;
            let old_next = previous.cell(row, next_column).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "previous wide screen snapshot cell is missing its continuation",
                )
            })?;
            if cell != old || next != old_next {
                write_positioned_cell(
                    output,
                    row,
                    column,
                    cell,
                    CellContent::Text(cell.text(), 2),
                    2,
                )?;
            }
            column += 2;
            continue;
        }

        if cell.is_wide_continuation() {
            // A valid row skips paired continuations after rendering the lead.
            // Keep a defensive path if the topology contract changes later.
            if cell != old {
                write_positioned_cell(output, row, column, cell, CellContent::Blank, 1)?;
            }
            column += 1;
            continue;
        }

        if cell != old {
            let content = if cell.text().is_empty() {
                CellContent::Blank
            } else {
                CellContent::Text(cell.text(), 1)
            };
            write_positioned_cell(output, row, column, cell, content, 1)?;
        }
        column += 1;
    }
    Ok(())
}

enum CellContent<'a> {
    Text(&'a str, usize),
    Blank,
    Fallback,
}

fn write_positioned_cell<W: Write>(
    output: &mut W,
    row: u16,
    column: u16,
    cell: &SnapshotCell,
    content: CellContent<'_>,
    occupied_width: usize,
) -> io::Result<()> {
    write!(output, "\x1b[{};{}H", row + 1, column + 1)?;
    write_sgr(output, cell)?;

    match content {
        CellContent::Blank => output.write_all(b" "),
        CellContent::Fallback => write_fallback(output, occupied_width),
        CellContent::Text(text, expected_width) => {
            if text_fits_cell(text, expected_width) {
                write_sanitized_text(output, text)
            } else {
                write_fallback(output, occupied_width)
            }
        }
    }
}

fn write_fallback<W: Write>(output: &mut W, occupied_width: usize) -> io::Result<()> {
    output.write_all(b"?")?;
    for _ in 1..occupied_width {
        output.write_all(b" ")?;
    }
    Ok(())
}

fn text_fits_cell(text: &str, expected_width: usize) -> bool {
    let mut scalars = text.chars();
    let Some(first) = scalars.next() else {
        return false;
    };
    let first = sanitize_scalar(first);
    if UnicodeWidthChar::width(first) != Some(expected_width) {
        return false;
    }
    scalars.all(|scalar| UnicodeWidthChar::width(sanitize_scalar(scalar)) == Some(0))
}

fn write_sanitized_text<W: Write>(output: &mut W, text: &str) -> io::Result<()> {
    for scalar in text.chars() {
        let sanitized = sanitize_scalar(scalar);
        let mut encoded = [0_u8; 4];
        output.write_all(sanitized.encode_utf8(&mut encoded).as_bytes())?;
    }
    Ok(())
}

fn sanitize_scalar(scalar: char) -> char {
    if unsafe_for_host(scalar) {
        '\u{fffd}'
    } else {
        scalar
    }
}

fn write_sgr<W: Write>(output: &mut W, cell: &SnapshotCell) -> io::Result<()> {
    output.write_all(b"\x1b[0")?;
    if cell.bold() {
        output.write_all(b";1")?;
    }
    if cell.dim() {
        output.write_all(b";2")?;
    }
    if cell.italic() {
        output.write_all(b";3")?;
    }
    if cell.underline() {
        output.write_all(b";4")?;
    }
    if cell.inverse() {
        output.write_all(b";7")?;
    }
    write_color(output, cell.foreground(), true)?;
    write_color(output, cell.background(), false)?;
    output.write_all(b"m")
}

fn write_color<W: Write>(output: &mut W, color: Color, foreground: bool) -> io::Result<()> {
    let base = if foreground { 38 } else { 48 };
    match color {
        Color::Default => {
            if foreground {
                output.write_all(b";39")
            } else {
                output.write_all(b";49")
            }
        }
        Color::Idx(index) => write!(output, ";{base};5;{index}"),
        Color::Rgb(red, green, blue) => {
            write!(output, ";{base};2;{red};{green};{blue}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{CoreCallbacks, TerminalSize};
    use vt100::Parser;

    fn empty_snapshot(rows: u16, columns: u16) -> ScreenSnapshot {
        ScreenSnapshot::new(TerminalSize::new(rows, columns).unwrap())
    }

    fn snapshot_from(input: &str, rows: u16, columns: u16) -> ScreenSnapshot {
        let size = TerminalSize::new(rows, columns).unwrap();
        let mut parser = Parser::new_with_callbacks(rows, columns, 0, CoreCallbacks::new());
        parser.process(input.as_bytes());
        let mut snapshot = ScreenSnapshot::new(size);
        snapshot.refresh(parser.screen(), size);
        snapshot
    }

    fn set_text(cell: &mut SnapshotCell, text: &str) {
        assert!(text.len() <= cell.text.len());
        cell.text.fill(0);
        cell.text[..text.len()].copy_from_slice(text.as_bytes());
        cell.text_len = u8::try_from(text.len()).unwrap();
    }

    fn rendered(snapshot: &ScreenSnapshot) -> Vec<u8> {
        let mut bytes = Vec::new();
        render(snapshot, snapshot.rows(), snapshot.columns(), &mut bytes).unwrap();
        bytes
    }

    fn rendered_diff(snapshot: &ScreenSnapshot, previous: Option<&ScreenSnapshot>) -> Vec<u8> {
        let mut bytes = Vec::new();
        render_diff(
            snapshot,
            previous,
            snapshot.rows(),
            snapshot.columns(),
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn parser_from(bytes: &[u8], rows: u16, columns: u16) -> Parser<CoreCallbacks> {
        let mut parser = Parser::new_with_callbacks(rows, columns, 0, CoreCallbacks::new());
        parser.process(bytes);
        parser
    }

    fn assert_same_rendered_screen(
        actual: &Parser<CoreCallbacks>,
        expected: &Parser<CoreCallbacks>,
        rows: u16,
        columns: u16,
    ) {
        let actual_screen = actual.screen();
        let expected_screen = expected.screen();
        assert_eq!(actual_screen.contents(), expected_screen.contents());
        assert_eq!(
            actual_screen.cursor_position(),
            expected_screen.cursor_position()
        );
        assert_eq!(actual_screen.hide_cursor(), expected_screen.hide_cursor());

        for row in 0..rows {
            for column in 0..columns {
                let actual_cell = actual_screen.cell(row, column).unwrap();
                let expected_cell = expected_screen.cell(row, column).unwrap();
                assert_eq!(actual_cell.contents(), expected_cell.contents());
                assert_eq!(actual_cell.is_wide(), expected_cell.is_wide());
                assert_eq!(
                    actual_cell.is_wide_continuation(),
                    expected_cell.is_wide_continuation()
                );
                assert_eq!(actual_cell.fgcolor(), expected_cell.fgcolor());
                assert_eq!(actual_cell.bgcolor(), expected_cell.bgcolor());
                assert_eq!(actual_cell.bold(), expected_cell.bold());
                assert_eq!(actual_cell.dim(), expected_cell.dim());
                assert_eq!(actual_cell.italic(), expected_cell.italic());
                assert_eq!(actual_cell.underline(), expected_cell.underline());
                assert_eq!(actual_cell.inverse(), expected_cell.inverse());
            }
        }
    }

    fn assert_diff_matches_full_target(previous: &ScreenSnapshot, snapshot: &ScreenSnapshot) {
        let previous_output = rendered(previous);
        let difference = rendered_diff(snapshot, Some(previous));
        let mut replayed = parser_from(&previous_output, previous.rows(), previous.columns());
        replayed.process(&difference);
        let full_target = parser_from(&rendered(snapshot), snapshot.rows(), snapshot.columns());
        assert_same_rendered_screen(&replayed, &full_target, snapshot.rows(), snapshot.columns());
    }

    #[test]
    fn engine_snapshot_renders_unicode_combining_wide_cells_and_sanitized_controls() {
        let snapshot = snapshot_from("café界e\u{301}", 5, 20);
        let output = rendered(&snapshot);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(output.contains("é"));
        assert!(output.contains("界"));
        assert!(output.contains("e\u{301}"));
        assert!(output.contains("\x1b[1;5H"));
        assert!(
            !output.contains("\x1b[1;6H"),
            "matched continuation is skipped"
        );

        let malicious = snapshot_from(
            "\x1b]0;TITLE-CANARY\x07\x1b]52;c;CLIP-CANARY\x07A\u{202e}\u{2028}\u{2029}B",
            5,
            20,
        );
        let output = rendered(&malicious);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(!output.contains("TITLE-CANARY"));
        assert!(!output.contains("CLIP-CANARY"));
        assert!(!output.contains("\x1b]0;"));
        assert!(!output.contains("\x1b]52;"));
        assert!(!output.contains('\u{202e}'));
        assert!(!output.contains('\u{2028}'));
        assert!(!output.contains('\u{2029}'));

        let mut synthetic = empty_snapshot(5, 20);
        set_text(&mut synthetic.cells[0], "\x1b]52;SYNTHETIC-CANARY");
        let output = rendered(&synthetic);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(!output.contains("SYNTHETIC-CANARY"));
        assert!(!output.contains("\x1b]52;"));
        set_text(&mut synthetic.cells[1], "\u{0085}");
        let output = rendered(&synthetic);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(!output.contains('\u{0085}'));
    }

    #[test]
    fn revalidation_falls_back_for_expanding_replacements_and_positions_next_cell() {
        let mut snapshot = empty_snapshot(5, 20);
        set_text(&mut snapshot.cells[0], "\u{202e}\u{2029}");
        set_text(&mut snapshot.cells[1], "Z");

        let output = rendered(&snapshot);
        let output = std::str::from_utf8(&output).unwrap();
        let first_cell = output.find("\x1b[1;1H").unwrap();
        let next_cell = output.find("\x1b[1;2H").unwrap();
        assert!(output[first_cell..next_cell].ends_with("?"));
        assert!(output[next_cell..].contains("Z"));
        assert!(!output.contains('\u{202e}'));
        assert!(!output.contains('\u{2029}'));
    }

    #[test]
    fn wide_and_orphan_continuation_fallbacks_stay_inside_their_rows() {
        let mut snapshot = empty_snapshot(5, 20);
        snapshot.cells[19].wide = true;
        set_text(&mut snapshot.cells[19], "界");
        snapshot.cells[20].wide_continuation = true;
        set_text(&mut snapshot.cells[20], "unexpected");

        let output = rendered(&snapshot);
        let output = std::str::from_utf8(&output).unwrap();
        let last_cell = output.find("\x1b[1;20H").unwrap();
        let next_row = output.find("\x1b[2;1H").unwrap();
        assert!(output[last_cell..next_row].ends_with("?"));
        let orphan = output.find("\x1b[2;1H").unwrap();
        let orphan_end = output.find("\x1b[2;2H").unwrap();
        assert!(output[orphan..orphan_end].ends_with(' '));
        assert!(!output.contains('\n'));

        let paired = snapshot_from("界X", 5, 20);
        let output = rendered(&paired);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(output.contains("界"));
        assert!(!output.contains("\x1b[1;2H"));
        assert!(output.contains("\x1b[1;3H"));
    }

    #[test]
    fn renderer_emits_only_typed_style_colors_and_clamped_cursor_controls() {
        let mut snapshot = empty_snapshot(5, 20);
        set_text(&mut snapshot.cells[0], "A");
        snapshot.cells[0].bold = true;
        snapshot.cells[0].dim = true;
        snapshot.cells[0].italic = true;
        snapshot.cells[0].underline = true;
        snapshot.cells[0].inverse = true;
        snapshot.cells[0].foreground = Color::Idx(196);
        snapshot.cells[0].background = Color::Rgb(1, 2, 3);
        snapshot.cursor_row = u16::MAX;
        snapshot.cursor_column = u16::MAX;

        let output = rendered(&snapshot);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(output.starts_with("\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J"));
        assert!(output.contains("\x1b[0;1;2;3;4;7;38;5;196;48;2;1;2;3mA"));
        assert!(output.contains("\x1b[0;39;49m "));
        assert!(output.ends_with("\x1b[0m\x1b[5;20H\x1b[?7h\x1b[?25h"));

        snapshot.cursor_hidden = true;
        let output = rendered(&snapshot);
        let output = std::str::from_utf8(&output).unwrap();
        assert!(!output.ends_with("\x1b[?25h"));
    }

    #[test]
    fn undersized_viewport_rejects_before_the_first_write() {
        let snapshot = empty_snapshot(5, 20);
        for (rows, columns) in [(4, 20), (5, 19)] {
            let mut output = Vec::new();
            let error = render(&snapshot, rows, columns, &mut output).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(output.is_empty());
        }
    }

    struct FailAfter {
        limit: usize,
        written: usize,
    }

    impl Write for FailAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let remaining = self.limit.saturating_sub(self.written);
            if remaining == 0 {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let count = remaining.min(bytes.len());
            self.written += count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_failure_propagates_after_partial_output_without_restarting_frame() {
        let snapshot = snapshot_from("NO-LEAK-CANARY", 5, 20);
        let mut output = FailAfter {
            limit: 8,
            written: 0,
        };
        let error = render(&snapshot, 5, 20, &mut output).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(output.written, 8);
        assert!(!format!("{error:?}").contains("NO-LEAK-CANARY"));
    }

    #[test]
    fn diff_repaints_only_the_changed_narrow_cell_and_matches_full_target() {
        let previous = snapshot_from("abc", 5, 20);
        let snapshot = snapshot_from("aXc", 5, 20);
        let output = rendered_diff(&snapshot, Some(&previous));
        let output_text = std::str::from_utf8(&output).unwrap();

        assert!(output_text.contains("\x1b[1;2H"));
        assert!(!output_text.contains("\x1b[1;1H"));
        assert!(!output_text.contains("\x1b[1;3H"));
        assert!(!output_text.contains("\x1b[2J"));
        assert_diff_matches_full_target(&previous, &snapshot);
    }

    #[test]
    fn diff_preserves_unicode_combining_and_matched_wide_cells() {
        let previous = snapshot_from("café界e\u{301}", 5, 20);
        let snapshot = snapshot_from("café語o\u{301}", 5, 20);
        let output = rendered_diff(&snapshot, Some(&previous));
        let output_text = std::str::from_utf8(&output).unwrap();

        assert!(output_text.contains("語"));
        assert!(output_text.contains("o\u{301}"));
        assert!(!output_text.contains("\x1b[2J"));
        assert_diff_matches_full_target(&previous, &snapshot);
    }

    #[test]
    fn diff_clears_cells_and_preserves_typed_styles() {
        let previous = snapshot_from("\x1b[1;31mBEFORE", 5, 20);
        let snapshot = snapshot_from("\x1b[2J\x1b[H\x1b[1;38;5;196;48;2;1;2;3mN", 5, 20);
        let output = rendered_diff(&snapshot, Some(&previous));
        assert!(!output.windows(4).any(|window| window == b"\x1b[2J"));
        assert_diff_matches_full_target(&previous, &snapshot);
    }

    #[test]
    fn diff_repaints_rows_for_wide_cell_topology_transitions() {
        for (before, after) in [("界X", "AB"), ("ABX", "界Y")] {
            let previous = snapshot_from(before, 5, 20);
            let snapshot = snapshot_from(after, 5, 20);
            let output = rendered_diff(&snapshot, Some(&previous));
            assert!(!output.windows(4).any(|window| window == b"\x1b[2J"));
            assert_diff_matches_full_target(&previous, &snapshot);
        }
    }

    #[test]
    fn diff_resize_uses_full_render_and_matches_target() {
        let previous = snapshot_from("old dimensions", 5, 20);
        let snapshot = snapshot_from("new dimensions 界", 6, 21);
        let output = rendered_diff(&snapshot, Some(&previous));
        assert!(output.starts_with(b"\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J"));

        let replayed = parser_from(&output, snapshot.rows(), snapshot.columns());
        let full_target = parser_from(&rendered(&snapshot), snapshot.rows(), snapshot.columns());
        assert_same_rendered_screen(&replayed, &full_target, snapshot.rows(), snapshot.columns());
    }

    #[test]
    fn diff_noop_writes_nothing_and_cursor_only_update_does_not_repaint() {
        let previous = snapshot_from("unchanged", 5, 20);
        assert!(rendered_diff(&previous, Some(&previous)).is_empty());

        let snapshot = snapshot_from("unchanged\x1b[3;4H\x1b[?25l", 5, 20);
        assert_eq!(snapshot.cells(), previous.cells());
        let output = rendered_diff(&snapshot, Some(&previous));
        let output_text = std::str::from_utf8(&output).unwrap();
        assert!(output_text.contains("\x1b[3;4H"));
        assert!(output_text.contains("\x1b[?25l"));
        assert!(!output_text.contains("\x1b[2J"));
        assert!(!output_text.contains("\x1b[?7"));
        assert!(output.len() < 32);
        assert_diff_matches_full_target(&previous, &snapshot);

        let visible = snapshot_from("unchanged\x1b[3;4H", 5, 20);
        let show_cursor = rendered_diff(&visible, Some(&snapshot));
        assert!(show_cursor.windows(6).any(|window| window == b"\x1b[?25h"));
        assert!(!show_cursor.windows(4).any(|window| window == b"\x1b[2J"));
        assert_diff_matches_full_target(&snapshot, &visible);
    }

    #[test]
    fn diff_first_frame_uses_full_render_and_rejects_small_viewport_before_write() {
        let snapshot = snapshot_from("first frame 界", 5, 20);
        assert_eq!(rendered_diff(&snapshot, None), rendered(&snapshot));

        let mut output = Vec::new();
        let error = render_diff(&snapshot, None, 4, 20, &mut output).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(output.is_empty());
    }

    #[test]
    fn diff_revalidates_synthetic_child_controls() {
        let previous = empty_snapshot(5, 20);
        let mut snapshot = empty_snapshot(5, 20);
        set_text(&mut snapshot.cells[0], "\x1b]52;c;DIFF-CANARY");

        let output = rendered_diff(&snapshot, Some(&previous));
        let output_text = std::str::from_utf8(&output).unwrap();
        assert!(!output_text.contains("DIFF-CANARY"));
        assert!(!output_text.contains("\x1b]52;"));
        assert_diff_matches_full_target(&previous, &snapshot);
    }

    #[test]
    fn diff_writer_failure_propagates_and_keeps_errors_redacted() {
        let previous = snapshot_from("before", 5, 20);
        let snapshot = snapshot_from("NO-DIFF-LEAK-CANARY", 5, 20);
        let mut output = FailAfter {
            limit: 9,
            written: 0,
        };
        let error = render_diff(&snapshot, Some(&previous), 5, 20, &mut output).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(output.written, 9);
        assert!(!format!("{error:?}").contains("NO-DIFF-LEAK-CANARY"));
    }

    #[test]
    fn maximum_diff_streams_under_four_mib_without_a_frame_buffer() {
        let previous = empty_snapshot(80, 240);
        let mut snapshot = empty_snapshot(80, 240);
        let maximum_cell_text = format!("A{}", "\u{20d0}".repeat(7));
        for cell in &mut snapshot.cells {
            set_text(cell, &maximum_cell_text);
            cell.bold = true;
            cell.dim = true;
            cell.italic = true;
            cell.underline = true;
            cell.inverse = true;
            cell.foreground = Color::Rgb(255, 255, 255);
            cell.background = Color::Rgb(255, 255, 255);
        }

        let mut output = CountingSink::default();
        render_diff(&snapshot, Some(&previous), 80, 240, &mut output).unwrap();
        assert!(output.0 > 0);
        assert!(output.0 <= 4 * 1024 * 1024);
    }

    #[derive(Default)]
    struct CountingSink(usize);

    impl Write for CountingSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn maximum_snapshot_streams_under_four_mib_without_a_frame_buffer() {
        let mut snapshot = empty_snapshot(80, 240);
        let maximum_cell_text = format!("A{}", "\u{20d0}".repeat(7));
        assert_eq!(maximum_cell_text.len(), 22);
        for cell in &mut snapshot.cells {
            set_text(cell, &maximum_cell_text);
            cell.bold = true;
            cell.dim = true;
            cell.italic = true;
            cell.underline = true;
            cell.inverse = true;
            cell.foreground = Color::Rgb(255, 255, 255);
            cell.background = Color::Rgb(255, 255, 255);
        }

        let mut output = CountingSink::default();
        render(&snapshot, 80, 240, &mut output).unwrap();
        assert!(output.0 > 0);
        assert!(output.0 <= 4 * 1024 * 1024);
    }
}
