# BTN-02f strip feedback, history and empty body

Implementation sub-agent, shared checkout `t3-baecf477`, branch
`t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050`. Parent dirty work preserved.
Used codebase-memory index status/search before inspecting current source.
No subagents, commits, pushes, installs, live panes or runtime restarts.

## Delivered

Extended parent's StripPress with semantic surface action, full history TargetRef,
shared bounds, optional strip geometry and layout revision. Existing catalog,
route, host and focus stamps retained. Helper returns Option<Flow>, allowing
empty Detach to quit locally. Empty root delegates once to the direct empty-host
handler; that handler owns capture processing and needs no worker/live layout.
Down has no operation; same-target Up dispatches once. Drag-away, wrong button,
Escape, focus loss and host/catalog/layout changes cancel. History uses only
local return-to-live, with no input claim, typing-focus change or child bytes.

Strip pressed style is applied to copied HeaderCells, leaving TabsLayout semantic
comparison unchanged. History pressed state participates in committed slot metadata
and history damage; its full button bounds receive filled styles, restored after
painting. Empty controls show a local pressed marker in the existing overlay.
No child snapshot mutations. Existing header 6/3 geometry retained. Tab dialogs,
chooser pointer behavior and retained dialog keyboard behavior unchanged.

## Checks

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`.
Durable logs are relative to this directory:

- `cargo test --locked --bin panelink ui_client::state::tests`: **165/0**,
  [log](logs/btn02f-state.log). Three initial Down-expecting fixture failures
  retained in [initial log](logs/btn02f-state-initial.log); explicit releases
  added to those fixtures, without changing generic mouse helpers.
- `cargo test --locked --lib workspace_tabs`: **10/0**,
  [log](logs/btn02f-tabs.log).
- `cargo test --locked --lib terminal::`: **93/0**,
  [log](logs/btn02f-terminal.log).
- `cargo test --locked --test p5_workspace_tabs -- --test-threads=1`:
  **5/0**, 39.74 seconds, [log](logs/btn02f-p5.log). No fixture edits in this slice.
- `cargo clippy --locked --all-targets -- -D warnings`: passed,
  [log](logs/btn02f-clippy.log).
- `cargo fmt --check` and `git diff --check`: passed,
  [log](logs/btn02f-format-diff.log).

New raw regressions exercise direct and normal empty routes, both empty actions,
no effects before release, duplicate release, Escape and actual parser markers;
history/empty cancellation matrix includes wrong button, drag, focus, host,
catalog and layout replacement. Existing strip catalog/drag regression retained.
Actual parser strip/history colors, cancellation restoration, failed output retry
and unchanged committed frame silence are checked in
`surface_pressed_cells_restore_and_failed_output_retries` (**1/0** separately),
with decoded cells in [cell evidence](logs/btn02f-cells.log). Its initial missing
commit assertion was corrected to follow the compositor's explicit successful-write
commit contract; this was test setup, not a production cache change.

Canonical verifier not rerun; provider WouldBlock remains unclassified. These
bounded checks do not close host/provider acceptance or whole BTN-02. Parent owns
next tab-dialog/chooser capture slice and canonical docs/skill synchronization.
