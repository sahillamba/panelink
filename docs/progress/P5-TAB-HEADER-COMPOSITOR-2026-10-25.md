# P5 persistent workspace-tab header compositor slice — 2026-10-25

Status: implemented as a bounded compositor API slice. This does not wire the
header into the client view, alter `ComposeFrame`, or establish interactive
tab navigation or host-terminal acceptance.

## API and behavior

`terminal::compose::HeaderCell` stores at most 32 UTF-8 bytes inline. Its
constructors represent sanitized width-one/two glyphs, blanks and wide-glyph
continuations; its debug output reveals only length, width and flags. Glyph
construction rejects empty, overlong, control-containing, leading zero-width
and zero/over-width content while allowing combining marks after a base glyph.
`Compositor::render_with_header` accepts an optional borrowed cell
row whose length must equal the host width. Wide cells must have a matching
continuation with the same selection state.

The row is painted at host row zero, independently of the existing modal
overlay. Pane, divider and modal rectangles that touch row zero are rejected
before any bytes are written whenever a header is present. The legacy
`render` and `render_with_selection` APIs keep their signatures and delegate
through the no-header path. Existing minimum `TerminalSize` rules are
unchanged.

The compositor keeps fixed 240-cell displayed and pending header baselines.
The pending row becomes displayed only when `commit` succeeds. First render,
invalidated state and host or pane geometry changes trigger a full repaint;
the header is forcibly repainted after the screen clear even when its contents
match the committed baseline. Header-only edits and identical frames remain
incremental, with identical frames emitting no bytes. Header appearance or
removal triggers a full repaint so a modal that moves onto row zero after
header removal is painted without a later blank-header write covering it. A
writer error invalidates all baselines, so the next successful render repaints
fully. Header cells are emitted as validated UTF-8, with attributes reset
around each changed cell; continuation cells emit a space only when
independently painted, and wide leaders occupy their paired terminal column.

## Local checks and limits

- `rustfmt --edition 2021 src/terminal/compose.rs` — passed.
- `git diff --check -- src/terminal/compose.rs
  docs/progress/P5-TAB-HEADER-COMPOSITOR-2026-10-25.md` — passed.
- No Cargo/build/test command was run, as required by the parent task's shared
target serialization constraint.
- Focused unit coverage includes incremental changes, unchanged frames, full
  repaint after geometry changes, header removal with a row-zero modal,
  parser-verified modal/header cells, invalid wide topology, leading zero-width
  glyph rejection, row-overlap refusal, uncommitted replay and writer-failure
  invalidation. These tests remain unexecuted pending the parent's serialized
  compositor test run.

Suggested parent command after the shared target is available:
`cargo test --lib terminal::compose::`

Parent verification: locked/offline serial focused run passed **24/0** at
`/private/tmp/panelink-p5-header-parent-r3.log`. Parent corrected two test
rectangles to respect the local `(row, column, rows, columns)` helper order;
the earlier r2 run failed those assertions and remains recorded separately.

Known integration requirement: callers using a header must lay out pane,
divider and overlay geometry strictly below row zero; this API enforces that
rather than shifting geometry itself. Header presentation remains caller-owned.
