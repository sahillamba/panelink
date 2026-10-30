# BTN02g — workspace dialog and chooser release capture

Implementation worker in shared `t3-baecf477`, branch
`t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus parent dirty work.
Read the progress, approved plan, BTN02d/BTN02f and required repository docs.
Used the codebase-memory skill/project listing and graph search first; the
workspace graph search returned no symbols, so inspected exact current source.
The memory registry search supplied no relevant button implementation facts.
ms-rust remains unavailable; no compliance claim.

## Changes

Only this packet's changes are in `src/ui_client/state/workspace.rs`,
`src/ui_client/state.rs`, `src/ui_client/state/render.rs`,
`src/ui_client/state_tests.rs`, and this report/logs. Existing parent modifications
in those files and all other dirty files were preserved. No subagents, commits,
pushes, installation, live user panes or server restart.
No compositor, protocol, server, dependency, global skill or parent-doc edits.
The existing PTY helpers already send explicit releases and needed no edits.

WorkspaceState exposes semantic controls and exact screen-space bounds through
`workspace_controls` / `workspace_control_at`. The root adapter captures the
semantic action (including the exact route), bounds, full catalog equality,
catalog revision, current route, dialog state, filter/list selection, UI epoch,
host size, focus generation and optional runtime generation. It does not require
a live layout for an empty client. Opening/reopening dialogs and installing or
refreshing catalogs advance the UI epoch. Rename drafts are compared, never
replaced by the captured copy. Captured rows are never activated by resolving
an old row number against a new catalog.

Down records the capture without an operation. Left Up on the same valid control
activates once. Other buttons, drag, outside/different control, Escape, focus loss,
resize/host, route/catalog/dialog/filter/list and runtime changes cancel.
Rendering revalidates the capture immutably, so stale feedback disappears before
another input event. Existing strip processing keeps priority and consumes its
opener Up. Empty-host routing now gives open workspace dialogs/pickers priority
over empty New-tab shortcuts. The accepted header/pane-modal/strip/history/empty
capture implementations are preserved.

Old workspace Down dispatch arms were removed. Local keyboard navigation,
rename editor behavior, uppercase focus labels and Tab/Shift-Tab remain intact.
Save/Close retain pending guards; pending Cancel reports metadata busy without
changing the pending dialog. Existing saving/closing text remains visible.
Pressed rows append `[pressed]` independently of keyboard focus; the marker
never affects control bounds. This is provisional presentation for BTN04,
not filled-style acceptance.

## Verification

All Cargo commands used
`CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target` and `--locked --offline`.
Logs contain combined full stdout/stderr; final logs include commands and exit
codes. No zero-test result is counted.

| Command | Result | Log |
| --- | --- | --- |
| `cargo test --bin panelink ui_client::state::tests` | 167 passed, 0 failed | [state](logs/btn02g-state.log) |
| `cargo test --lib workspace_tabs::` | 10 passed, 0 failed | [tabs](logs/btn02g-tabs.log) |
| `cargo test --lib terminal::` | 93 passed, 0 failed | [terminal](logs/btn02g-terminal.log) |
| `cargo test --test p5_workspace_tabs -- --test-threads=1` | all 5 passed, 0 failed; 39.39s | [PTY](logs/btn02g-p5.log) |
| `cargo clippy --all-targets -- -D warnings` | passed | [Clippy](logs/btn02g-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn02g-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn02g-diff.log) |

The capture regression loops over all three Actions rows, Save/Cancel,
Close/Cancel, Clear and route rows, testing Down/no mutation, valid Up, duplicate
Up, outside/wrong-button/drag, focus/host, revision/catalog, runtime replacement,
route change, dialog reopening, Escape and filter/dialog invalidations. Actual
parser-decoded rendered text verifies pressed visibility and immediate stale
feedback removal, with no child keys/pastes/close effects. Pending-operation and
direct empty-host picker tests add duplicate guards and local routing coverage.
The existing full PTY suite retains cancellation and process identity assertions.

Decoded pressed/released frames for all nine controls are retained in
[cell evidence](logs/btn02g-cells.log) from the separately run regression with
`--nocapture` (1 passed, 0 failed). These are actual renderer/parser output,
not handwritten mockups. The full state suite also runs this regression.

Initial four fixture failures expecting Down activation are preserved in
[initial state log](logs/btn02g-state-initial.log). Only those fixtures were
migrated to explicit Down/Up; generic raw mouse helpers remain unchanged.
The pending test initially expected the transient busy status inside the rendered
frame even when the existing pending presentation owns that surface; corrected
it to assert the busy status and actual saving/closing text separately, retaining
pending identity and mutation assertions ([initial log](logs/btn02g-state-pending-initial.log)).
Strict Clippy initially found two redundant Rect struct updates; removed them
([initial Clippy log](logs/btn02g-clippy-initial.log)). An earlier mechanical parse
error was corrected before the retained red state run; its log was overwritten.

Canonical non-UI verification was not rerun. The canonical provider WouldBlock
failure remains unclassified. Host/provider/theme acceptance, broader BTN03/04
style/group migration, skill synchronization and whole-design acceptance remain
with the parent. This packet is ready for independent parent source review.
