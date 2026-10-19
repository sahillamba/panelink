# P3A wave 1 task A — split menu model

Status: implemented as a pure library module, `src/split_menu.rs`, with unit
tests. This is groundwork only. It adds no host mouse decoding, ANSI
rendering, runtime routing, split transaction, or PTY launch, and it does not
complete Phase 3A. Integration note: the worker added `pub mod split_menu;`
(unconditional) to `src/lib.rs` so that the module compiles and its tests run.
`src/lib.rs` belongs to the parent, who should review or adopt that line.

## API summary

- `MenuState`: either closed or open with one `OpenMenu`. It is `Copy`, and
  its `Debug` output redacts the target and revision.
  - `open_at_pointer(&PaneLayout, x, y, viewport_rows, viewport_columns) -> Result<(), MenuOpenError>`
    hit-tests one `snapshot()`. A divider cell or an exterior cell returns
    `NoPaneAtPointer`, and the state does not change.
  - `open_for_focused(&PaneLayout, focused: TargetRef, rows, columns)` requires
    an exact `TargetRef` match. Otherwise it returns `TargetNotInLayout` and the
    state does not change. The overlay is anchored at the pane's top-left cell.
  - `handle(MenuInput) -> MenuResponse`, `close`, `set_viewport`,
    `is_open`, `captured_target`, `captured_revision`, `items`, `selected`,
    `mode`, `overlay_rect`, `overlay_lines`.
- `MenuInput`: `Up`, `Down`, `Home`, `End`, `Activate`, `Cancel`, `Letter(char)`,
  `MousePress { x, y, right }`, `MouseRelease { x, y, right }`, `Other`.
- `MenuResponse`: `NotOpen`, `Consumed`, `Closed`, or `Activated(SplitIntent)`.
- `SplitIntent { target, expected_layout_revision, direction }`.
- `MenuItem { direction, availability }`, where `Availability` is `Enabled` or
  `Disabled(DisabledReason)`.
- `DisabledReason`: `TooNarrow` and `TooShort` (each with direction, needed
  size and actual size), `TooLarge`, `PaneLimit { limit }`, `RevisionExhausted`,
  `TargetStale`, `Unavailable`. It is `Copy` and its `Display` formats text on
  demand, so it holds no heap storage.
- `OverlayMode` is `Normal`, `Compact`, or `TooSmall`.
- `OverlayLines` and `OverlayLine` hold a fixed array of 7 rows, each up to
  240 ASCII bytes. Each row has an `OverlayStyle`: `Normal`, `Selected`,
  `Disabled`, `Border`, or `Status`.
- Constants: `SPLIT_MIN_COLUMNS = 41`, `SPLIT_MIN_ROWS = 11`,
  `PANE_LIMIT = 2`, `MIN_MENU_ROWS = 5`, `MIN_MENU_COLUMNS = 20`, and the
  overlay size constants.

## Rules

- Both open paths take the target and the revision from the same
  `LayoutSnapshot`. Activation returns exactly those captured values; it never
  reads the current focus.
- Every item's availability comes from `PaneLayout::can_split`, which is the
  authority. A `LayoutError` maps to a reason as follows:
  - `LayoutTooSmall` becomes a column reason for Left/Right or a row reason
    for Up/Down. The needed size is `2*MIN+1` and the actual size is the
    pane rectangle's size.
  - `PaneLimitExceeded` becomes `Pane limit reached (2)`.
  - `RevisionExhausted` keeps its own reason.
  - The stale variants become `TargetStale`.
  - Identity and internal variants become `Unavailable`.
- Items always appear in the order Left, Right, Up, Down. When the menu opens,
  the first enabled item is selected. If no item is enabled, Left is selected.
- While the menu is open, `handle` never returns `NotOpen`, so input is never
  passed through. Activating a disabled item returns `Consumed`; the menu stays
  open and the status row shows the item's reason.
- `Letter` keys `l`, `r`, `u` and `d` are case-insensitive. Each one selects
  its item and then activates it. Any other letter is consumed.
- A mouse press on an item row activates that item. A press on the border,
  title or status row is consumed. A press outside the overlay closes the
  menu. Mouse releases are consumed.
- Overlay modes:
  - **Normal** is used when the viewport is at least 7 rows by 24 columns. It
    is a bordered box of 7 rows by `min(52, viewport columns)` columns, placed
    at the anchor and moved left and up so that it fits.
  - **Compact** is used when the viewport is at least 5 rows by 20 columns but
    cannot fit Normal. It has exactly 5 rows (4 item rows and 1 status row),
    no border, and spans the full width, up to 240 columns.
  - **TooSmall** is used whenever the viewport is below the terminal-core
    minimum of 5 rows by 20 columns. That minimum is
    `MIN_MENU_ROWS`/`MIN_MENU_COLUMNS`, which equal `MIN_PANE_ROWS` and
    `MIN_PANE_COLUMNS`. The overlay shows one clipped status row. Only
    `Cancel` closes the menu; every other input is consumed, so activation is
    impossible. If `set_viewport` enlarges the viewport back to at least the
    minimum, the interactive mode returns with the same captured target,
    revision and selection. (Review fix: the earlier version made Compact
    interactive from 14 columns.)
- Text is written as printable ASCII. Any other character becomes `?`. Text
  that is too long is clipped and ends with `...`; it never wraps.

## Tests

`src/split_menu.rs` contains 23 tests. They cover:

- all four directions enabled at the 80x240 size;
- the threshold of 41 vs 40 columns, including the reason text for 38 columns;
- the threshold of 11 vs 10 rows, including the reason text for 9 rows;
- agreement with `can_split` across a sweep of 9x9 sizes;
- the pane-limit reason after a split is committed;
- the `LayoutError`-to-reason mapping, including revision exhaustion;
- that the pointer chooses the pane under it, whichever pane has focus;
- that a divider or exterior click opens nothing;
- rejection of a stale keyboard target;
- that activation still returns pane A and revision 1 after the layout
  changes;
- that no input variant passes through while the menu is open, in every mode;
- that a closed menu returns `NotOpen`;
- that activating a disabled item keeps the menu open;
- wrapping navigation and the letter shortcuts;
- mouse presses on an item, on the chrome, and outside the overlay;
- clamping at all four corners;
- the compact layout at 5x20 and in a short, wide viewport;
- the TooSmall mode, including a 0x0 viewport;
- that 5x19 and 4x40 viewports are TooSmall: the letter shortcuts and
  `Activate` do not activate, and only `Cancel` closes;
- that a 5x20 viewport is Compact and activates;
- opening at 24x81, then `set_viewport(5,19)`: no activation; then
  `set_viewport(24,81)`: activation returns the original target, revision 1
  and the same selection, even though the layout changed in between;
- that `set_viewport` keeps the captured values;
- clipping widths from 0 to 59 and replacement of control and non-ASCII
  characters;
- `Debug` redaction.

## Verification (2026-10-18)

- `cargo test --locked --offline --lib split_menu`: 23 passed, 0 failed
  (rerun after the review fix).
- `rustfmt --edition 2021 --check src/split_menu.rs`: clean.
- `cargo fmt --check`: failed, but only on two diffs in
  `src/terminal/render.rs`. Task C owns that file and was editing it at the
  same time, so this worker did not touch it.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed
  (rerun after the review fix).
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps`: passed.

## Limits

- This is a pure model. Nothing here has been tested with a host mouse,
  Apple Terminal right-click delivery, an ANSI renderer, or the runtime.
- The availability shown in the menu is advisory. The server must still
  revalidate the target, the revision, the geometry and the capacity.
- Revision exhaustion is tested only through the reason mapping, because
  `PaneLayout`'s revision cannot be changed from outside its module.
- A right-click outside the overlay only closes the menu; it does not reopen
  it at the new location. The caller decides whether to reopen.
- The key bindings that open the menu, and the labels of the shortcuts, are
  still an open R-08 choice.
- `MenuInput` debug output redacts the character in `Letter`.
