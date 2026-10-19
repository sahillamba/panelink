# P3A wave 1 task C — pane composition renderer

Status: implemented and unit-tested as pure library code (2026-10-18). It is not
yet integrated into a host client, so it does not complete Phase 3A. No host IO,
menu logic, dependency or manifest change.

Files: `src/terminal/compose.rs` (new), `src/terminal/render.rs` (private
internals only), one `pub mod compose;` line in `src/terminal/mod.rs`.

## API (`panelink::terminal::compose`)

- `ComposeFrame<'a> { host_rows, host_columns, panes: [Option<PaneView>; 2], divider: Option<Rect>, focused: Option<usize>, overlay: Option<OverlayView> }`
- `PaneView { rect: Rect, content: PaneContent }`. `PaneContent::Snapshot(&ScreenSnapshot)` or `PaneContent::Placeholder(&str)`.
- `OverlayView { rect, lines: &[OverlayLine] }`, `OverlayLine { text, style }`.
- `OverlayStyle`: `Normal` and `Border` use default attributes, `Selected` uses reverse video (SGR 7), `Disabled` uses dim (SGR 2), and `Status` uses bold (SGR 1). The style applies across the full overlay row.
- `Compositor::try_new()`, `render(&mut self, &ComposeFrame, &mut impl Write) -> io::Result<()>`, `commit() -> bool`, `invalidate()`.
- `MAX_RECT_ROWS = 80`, `MAX_RECT_COLUMNS = 240`.

`Rect` is `crate::pane_layout::Rect`. The compositor does not depend on `split_menu`.

## Validation (before any byte is written)

The compositor returns `InvalidInput` (or `InvalidData` for inconsistent
snapshot storage) for these cases:

- an empty host;
- a pane rectangle that is empty, outside the host or larger than 80x240;
- a snapshot whose dimensions differ from its rectangle;
- overlapping panes;
- a divider outside the host, not one cell thick or overlapping a pane;
- an overlay whose origin is outside the host or that is empty.

An overlay that extends past the right or bottom edge is clipped to the host.
An out-of-range or empty `focused` slot hides the cursor; it is not an error.
Overlay and placeholder text shows printable ASCII only. Other scalars become
`?`. Text is clipped to the available width and never wraps.

## Repaint rules

- **Full repaint** (`?25l ?7l 0m 2J`, then panes, divider, overlay): first frame, after `invalidate()`, after a writer error inside `render`, or on any change to host size, pane rectangle or divider.
- **Snapshot pane:** each row uses `render_diff`'s rules through shared internals: unchanged rows emit nothing, wide-topology changes repaint the row, and other changes emit only the changed cells, translated by the rectangle origin.
- **Placeholder pane:** repainted in full when its text changes or the pane changes between snapshot and placeholder content. Otherwise it emits nothing. The text is on row `(rows-1)/2` and is horizontally centered.
- **Overlay:** pane cells never paint inside the current overlay rectangle. A wide glyph that straddles the overlay edge shows `?` or a blank in its visible half. The overlay paints last, but only when its rectangle, text or styles change.
- **Overlay moved or removed:** for each snapshot pane, the compositor repaints every row that intersects the old or new overlay rows in full and diffs the other rows. Placeholder panes repaint fully. Divider cells under the old overlay are redrawn. Exposed cells outside every pane are blanked. This trades some bytes for simple correctness. Ordinary pane output beneath an unchanged overlay stays incremental.
- **Divider:** ASCII `|` when the divider is one column wide, otherwise `-`.
- **Cursor:** placed at the focused snapshot's clamped cursor and translated by its origin. It is shown only if that snapshot's cursor is not hidden. It is always hidden while an overlay is present or when the focused pane is a placeholder or missing. A no-change frame emits nothing. A cursor-only change emits only `CUP` and/or `?25l`/`?25h`.
- `2J` is emitted only on a full repaint. SGR is reset before the frame ends. Autowrap is disabled during painting.

## Commit semantics

`render` stages the frame into preallocated target buffers and writes against
the last committed baseline. Only `commit()` swaps target and baseline. This
mirrors p2_probe's commit-after-complete-write. Rendering again before commit
re-emits the same difference.

If a frame is abandoned after a partial host write, the caller must call
`invalidate()`. A writer error inside `render` invalidates automatically, and
the next `commit()` returns `false`.

## Memory bound

All memory is allocated in `try_new` and never grows:

| Item | Bytes |
| --- | ---: |
| 4 `ScreenSnapshot`s (displayed + target per slot), each reserving 80×240 cells × 48 B (`size_of::<SnapshotCell>()` measured on aarch64) | 3,686,400 |
| 2 overlay cell buffers, 80×240 B each | 38,400 |
| `Compositor` inline struct (two frame metadata records with 240-byte placeholder copies) | 1,432 |
| **Total** | **≈ 3.73 MB** |

Under the 64-byte cell ceiling used in P2-DELTA-RESULT, the snapshots would
total 4,915,200 B. This is source accounting, not an RSS measurement. Output
streams straight to the writer, and a maximum-size two-pane frame stays at or
below 4 MiB through a counting sink.

## `render.rs` refactor

Private `Placement { row, column, exclude }` plus `pub(super)` `paint_row`,
`paint_row_diff` and `validate_snapshot`. `render` and `render_diff` use the
host origin with no exclusion, so their signatures and output bytes are
unchanged. The `render.rs` test module was not modified, and all 17 tests pass.

## Tests

`compose.rs` contains 15 tests. They cover:

- left/right and top/bottom translation and dividers;
- a single-cell diff in pane 2, with exact positions and no pane-1 bytes or `2J`;
- an identical frame with no output and a cursor-only move;
- full repaint after rectangle, host or invalidation changes;
- overlay styles, clipping and incremental output under an overlay;
- overlay removal or move compared with fresh full renders;
- cursor translation, plus hiding for overlays and placeholders;
- placeholder centering, sanitization and clipping;
- eight invalid geometries rejected before any byte reaches a counting sink;
- parser-derived and synthetic ESC, OSC 52 and bidi canaries in full and diff output;
- commit and re-emit behavior, plus writer-error invalidation;
- wide glyphs at the pane edge and overlay edge;
- a mixed incremental sequence equal to a full render;
- the maximum-frame byte bound.

Equivalence tests replay the outputs through a `vt100` parser. They compare
the text, width flags, colors, attributes and cursor of every host cell.

## Verification

| Command | Result |
| --- | --- |
| `cargo test --locked --offline --lib terminal::` | 48 passed, 0 failed: 15 compose, 17 render and 16 other terminal tests |
| `rustfmt --edition 2021 --check src/terminal/compose.rs src/terminal/render.rs` | exit 0 |
| `cargo fmt --check` | exit 0 |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | exit 0, including the concurrent workspace state |
| `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps` | exit 0 |

## Limits

- This is pure output generation. Host-mode setup and restoration, resize
  timing and Apple Terminal rendering remain unobserved.
- The compositor does not interpret menu state. The future menu module must map
  its rows to `OverlayLine`.
- Placeholder and overlay text is ASCII-only by design.
- Cells exposed outside all panes are blanked only when an overlay moves. Other
  non-pane host areas are cleared only by a full repaint.
