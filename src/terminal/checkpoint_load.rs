//! Bounded conversion from a validated screen checkpoint into a preallocated
//! [`ScreenSnapshot`] so a remote pane can be drawn by the existing renderer.
//!
//! The checkpoint is already validated and sanitized by
//! [`crate::screen_checkpoint::decode`]; this copy re-applies the host-safety
//! scalar filter defensively and never allocates beyond the snapshot's
//! reserved 80x240 cell capacity. No terminal emulator state is rebuilt and
//! nothing is replayed.

use super::{ScreenSnapshot, SnapshotCell, TerminalError, TerminalSize};
use crate::screen_checkpoint::{CheckpointColor, ScreenBuffer, ScreenCheckpoint};
use vt100::Color;

impl ScreenSnapshot {
    /// Replace this snapshot with the visible cells of one decoded checkpoint.
    ///
    /// Dimensions must be within the terminal-core bounds. Cell storage is
    /// resized within the capacity reserved by [`ScreenSnapshot::try_new`].
    /// Application-cursor and bracketed-paste modes are not part of a
    /// checkpoint and are reported as disabled. On error the snapshot is left
    /// unchanged.
    pub fn load_checkpoint(
        &mut self,
        checkpoint: &ScreenCheckpoint<'_>,
    ) -> Result<(), TerminalError> {
        let size = TerminalSize::new(checkpoint.rows(), checkpoint.columns())?;
        let count = usize::from(size.rows()) * usize::from(size.columns());
        if checkpoint.cells().len() != count
            || checkpoint.cursor_row() >= size.rows()
            || checkpoint.cursor_column() >= size.columns()
        {
            return Err(TerminalError::InvalidSize);
        }
        if self.cells.capacity() < count {
            return Err(TerminalError::InvalidSize);
        }
        self.cells.resize_with(count, SnapshotCell::default);
        for (target, source) in self.cells.iter_mut().zip(checkpoint.cells()) {
            target.text.fill(0);
            target.text_len = 0;
            for scalar in source.text().chars() {
                let safe = if super::unsafe_for_host(scalar) {
                    '\u{fffd}'
                } else {
                    scalar
                };
                let mut encoded = [0_u8; 4];
                let bytes = safe.encode_utf8(&mut encoded).as_bytes();
                let start = usize::from(target.text_len);
                let end = start + bytes.len();
                if end <= target.text.len() {
                    target.text[start..end].copy_from_slice(bytes);
                    target.text_len = u8::try_from(end).unwrap_or(0);
                }
            }
            target.wide = source.is_wide();
            target.wide_continuation = source.is_wide_continuation();
            target.foreground = color(source.foreground());
            target.background = color(source.background());
            target.bold = source.bold();
            target.dim = source.dim();
            target.italic = source.italic();
            target.underline = source.underline();
            target.inverse = source.inverse();
        }
        self.rows = size.rows();
        self.columns = size.columns();
        self.cursor_row = checkpoint.cursor_row();
        self.cursor_column = checkpoint.cursor_column();
        self.alternate_screen = checkpoint.buffer() == ScreenBuffer::Alternate;
        self.cursor_hidden = checkpoint.cursor_hidden();
        self.application_cursor = false;
        self.bracketed_paste = false;
        Ok(())
    }
}

fn color(value: Option<CheckpointColor>) -> Color {
    match value {
        Some(CheckpointColor::Indexed(index)) => Color::Idx(index),
        Some(CheckpointColor::Rgb(red, green, blue)) => Color::Rgb(red, green, blue),
        Some(CheckpointColor::Default) | None => Color::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::TargetRef;
    use crate::screen_checkpoint::{decode, encode, CheckpointMetadata};
    use crate::terminal::CoreCallbacks;
    use vt100::Parser;

    fn snapshot_from(input: &str, rows: u16, columns: u16) -> ScreenSnapshot {
        let size = TerminalSize::new(rows, columns).unwrap();
        let mut parser = Parser::new_with_callbacks(rows, columns, 0, CoreCallbacks::new());
        parser.process(input.as_bytes());
        let mut snapshot = ScreenSnapshot::new(size);
        snapshot.refresh(parser.screen(), size);
        snapshot
    }

    fn metadata() -> CheckpointMetadata {
        let target = TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        };
        CheckpointMetadata::new(target, 0, 1, 9).unwrap()
    }

    #[test]
    fn checkpoint_round_trip_reproduces_cells_cursor_and_styles() {
        let source = snapshot_from(
            "plain \x1b[1;31mbold-red\x1b[0m \x1b[7minv\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m 界x\r\nnext\x1b[?25l",
            6,
            30,
        );
        let record = encode(&source, &metadata()).unwrap();
        let checkpoint = decode(&record).unwrap();
        let mut loaded = ScreenSnapshot::try_new(TerminalSize::new(80, 240).unwrap()).unwrap();
        loaded.load_checkpoint(&checkpoint).unwrap();
        assert_eq!(loaded.rows(), 6);
        assert_eq!(loaded.columns(), 30);
        assert_eq!(loaded.cells(), source.cells());
        assert_eq!(loaded.cursor_row(), source.cursor_row());
        assert_eq!(loaded.cursor_column(), source.cursor_column());
        assert!(loaded.cursor_hidden());
        assert!(!loaded.alternate_screen());
        assert!(loaded.cells().len() <= 80 * 240);
    }

    #[test]
    fn loading_reuses_reserved_storage_and_shrinks_and_grows() {
        let mut loaded = ScreenSnapshot::try_new(TerminalSize::new(5, 20).unwrap()).unwrap();
        let capacity = loaded.cells.capacity();
        for (rows, columns) in [(80, 240), (5, 20), (24, 100)] {
            let source = snapshot_from("abc", rows, columns);
            let record = encode(&source, &metadata()).unwrap();
            loaded.load_checkpoint(&decode(&record).unwrap()).unwrap();
            assert_eq!(loaded.cells(), source.cells());
            assert_eq!(loaded.cells.capacity(), capacity);
        }
    }
}
