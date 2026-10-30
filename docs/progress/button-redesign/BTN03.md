# BTN03 — filled pane controls and grouped workflows

Implementation worker handoff for independent parent review, 2026-10-31.
Checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`, branch
`t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus preserved parent dirty work.
No commit, push, subagent, install, live user pane, runtime restart, server,
protocol, manifest, workspace owner, skill or parent planning edits.

## Source changes

- `src/ui_client/state/interaction.rs:256`: enumerates semantic modal controls
  once from the existing geometry owner; capture hit testing finds the matching
  control in that same enumeration. Exact target/revision/instance and original
  capture validation remain intact. The implementation uses a small transient
  Vec of visible controls, not a framework or label parser.
- `interaction.rs:1136`: action pages respect Layout (four directional splits
  and Resize), Copy/History, Close, and Secondary boundaries. Each page has a
  noninteractive group heading, plain availability/reason row and distinct
  Back/More/Cancel controls. Secondary retains Zoom, Swap, Saved layout, Help
  and Detach. Short hosts paginate within groups; no extra permanent row or
  change to PTY/layout minimums or pane capacity.
- `src/ui_client/state/render.rs:606`: converts the owner's exact actionable
  bounds into filled per-control role/state spans. Rows/status/gaps are plain.
  Close has destructive rest styling; Cancel retains initial keyboard focus.
  Pressed feedback uses validated capture independently of typing focus.
  Removed pane pressed `*` label replacement and uppercase pressed Close/Cancel.
  Existing directional availability/reasons, resize decrement/increment and
  Apply/Cancel effects, captured close target and all lifecycle guards remain.
  Existing tab/chooser presentation and capture are left for BTN04.
- `src/terminal/compose.rs:457`: `OverlayControl` describes absolute bounds,
  role and state alongside `OverlayView`. A fixed 32-entry optional span array
  is staged in FrameMeta (current pane surfaces remain below that bound).
  `overlay_unchanged` includes the span array; style-only changes repaint.
  `paint_overlay` clips each fill to the overlay and resets attributes after
  each span and overlay. Multiple buttons on one row have separate styles;
  status text cannot inherit their fills. Existing successful-write commit and
  baseline invalidation rules also own span metadata.
- Compiler-required `controls: &[]` initializers were added to compositor test
  literals and the single `OverlayView` literal in `src/ui_client/recovery.rs:471`.
  Recovery otherwise unchanged. No `workspace.rs`/`workspace_tabs.rs` edits.
- Relevant state fixtures follow grouped pages and assert actual parser-decoded
  background colors instead of provisional pressed glyphs. All original target,
  effect, gesture and reachability assertions remain. The minimum-size traversal
  now allows up to PANE_ACTION_COUNT pages rather than four so it still asserts
  every action, including all secondary actions, is reachable.
- `tests/p5_workspace_tabs.rs:1524` intentionally checks the group pages in
  sequence, retaining Split, Zoom, Close, Detach and all subsequent PTY assertions.

Existing parent changes in shared files are preserved. No changes to state.rs,
controls.rs or split_menu.rs were necessary in this packet.

## Verification

All Cargo commands used
`CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`, `--locked --offline`.
Full combined stdout/stderr is in the linked logs; no zero-test gate counted.

| Command | Result | Log |
| --- | --- | --- |
| `cargo test --bin panelink ui_client::state::tests` | 168 passed, 0 failed, 0 ignored | [state](logs/btn03-state.log) |
| `cargo test --lib terminal::` | 94 passed, 0 failed, 0 ignored | [terminal](logs/btn03-terminal.log) |
| `cargo test --lib split_menu::` | 31 passed, 0 failed, 0 ignored | [split](logs/btn03-split.log) |
| `cargo test --test p5_workspace_tabs -- --test-threads=1` | 5 passed, 0 failed; 42.95s | [PTY](logs/btn03-p5.log) |
| `cargo clippy --all-targets -- -D warnings` | passed | [Clippy](logs/btn03-clippy.log) |
| `cargo fmt --check` | passed (no output) | [fmt](logs/btn03-fmt.log) |
| `git diff --check` | passed (no output) | [diff](logs/btn03-diff.log) |

[Generated cell evidence](logs/btn03-cells.log), from
`cargo test --lib overlay_controls_fill_exact -- --nocapture` (1/0), contains
actual renderer/parser rows and cell background values for rest, focus, pressed,
disabled and pending. It asserts exact span edges, a four-cell unfilled gap,
plain trailing status, independently filled adjacent action, style-only repaint,
unchanged-frame zero bytes, failed-write refusal to commit, retry, and full
content/background restoration after removal. Existing compositor movement,
damage and partial-failure tests also pass in terminal94/0.

[Actual pane evidence](logs/btn03-pane-cells.log), from
`cargo test --bin panelink btn03_pane -- --nocapture` (1/0), checks every visible
menu control's fill across its full hit width, plain heading/reason rows,
destructive Close rest, initially focused Cancel, unfilled gap, pressed Close
and cancellation without a close effect. Existing state tests retain minimum
7x22 pagination, all menu actions, exact targets, modal invalidation, sibling
identity, focus and input safety coverage. Existing private fixtures preserve
real PTY process/cancel assertions.

Intermediate development included seven expected fixture failures after grouped
pagination/glyph migration, then two remaining page/appearance expectations;
those intermediate outputs were overwritten during iteration and are not claimed
as retained red evidence. The initial PTY run was 4/1 because its old first-page
assertion expected Zoom alongside Split; the final group-aware full run is 5/0.
An initial compositor test borrow error was corrected by creating a per-iteration
frame borrowing that iteration's controls. No assertion was ignored or removed
to hide a behavioral failure.

Graph discovery first used the short project name (rejected), then the exact
registered checkout name and found OverlayView/paint_overlay. Later exact snippet
lookup failed with `Transport closed`; current source was inspected directly.
The memory registry supplied no relevant implementation facts. ms-rust absent.

## Boundaries and parent follow-up

Pane presentation/workflows are implemented for source review. Pending palette is
proved by the pure compositor fixture; this packet does not invent a pending
modal after the existing admission path closes it. Existing pending/unknown
operation behavior stays authoritative. No light-theme/host/provider/clipboard/
touch acceptance claim. Canonical provider WouldBlock remains unclassified;
combined gate, canonical docs/skills, and BTN04 tab layout/style are parent work.
No installation or live-runtime claim follows from these source checks.

Final appearance adjustment: keyboard-focused menu Cancel now renders `[Cancel]`
at its actual eight-cell shared bounds; removed its old two-cell `> ` prefix
which left the label outside those bounds. The keyboard-cancel fixture now
asserts cyan fill at both label edges and retains its cancellation/no-effect
assertions. Final state and strict checks were rerun after this adjustment.
The serial PTY run preceded this keyboard-selected Cancel label-only adjustment.
