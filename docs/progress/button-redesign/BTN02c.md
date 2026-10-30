# BTN-02c pane-modal release activation

2026-10-30, shared checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`,
branch `t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus preserved parent work.
Bounded implementation and verification complete for independent parent review.
This does not complete BTN-02 or authorize tab-surface work.

## Implementation

App captures pane-modal controls before the existing admission router: action
rows and Back/More/Cancel, Close/Cancel confirmation, split-menu rows, and
pane-mode choices, decrement/increment and Apply/Cancel. Down records semantic
kind/index, full TargetRef, revision, exact bounds, modal instance, route, host
size and focus generation. It performs no button action. Up on the same valid
control dispatches the captured admission once. Duplicate releases do nothing.
Drag-away, wrong-button release, outside/different-control release and Escape
cancel the capture; focus/nonpointer events discard it. Layout/host changes
invalidate it; modal instance, route/focus and target validation prevent stale
or spatially reused controls from activating. Hidden modal/gesture ownership
excludes admission and pressed rendering. Pane-mode clicks beyond the actual
Apply/Cancel label cannot enter the older full-row pointer handler.

The existing admission handlers still enforce action availability, target and
revision checks and operation pending guards. Disabled action rows remain
selectable on release and expose their existing reason without execution.
Header capture and pressed rendering are preserved. The root action-menu branch
no longer records an opener latch for every Down; split-menu item handling also
stops doing that. Actual popup-opening gestures retain the existing release
latch. Header-opening Up remains consumed once by the header wrapper.

RenderView receives immutable validated modal capture separately from keyboard
selection. Confirmations show `[CLOSE]` / `[CANCEL]` while held; action, paging
and pane-mode controls mark the actual pressed control with `*` and bold status
style. Split-menu pressed rows use bold status rather than selected reverse
video. Feedback is removed when capture is canceled or invalid. Existing labels,
grouping, full-row action targets and layout stay otherwise unchanged; BTN-03
styling is deferred. No compositor change was necessary.

Action menus now cycle locally with Tab/Shift-Tab as well as arrows. Confirmation
Shift-Tab toggles locally alongside existing Tab/arrows, with Cancel initially
focused and the exact opener/repeat guard retained. Divider ancestor cycling and
arrows/Enter remain intact. No global shell Tab/Space handling changed.

Ten affected state fixtures now send explicit Down/Up events at their modal
click sites; no gesture/feed helper gained automatic release. Existing target,
Cancel, sibling/input, disabled and substantive geometry assertions remain.
Three new raw regressions cover wrong-button/opener/duplicate release,
drag-away, outside/different-control release, canceled rendered feedback,
Escape/focus/host/layout/modal-instance invalidation, stale target at the same
spatial index and revision, single activation, disabled selection and local
confirmation cycling. Existing pending and confirmation-repeat tests still pass.
Private PTY helpers already emit real SGR M/m pairs and were unchanged.

## Verification

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`.
Full stdout/stderr is retained below; no gate used a zero-test filter.

| Command | Result | Full log |
| --- | --- | --- |
| `cargo test --locked --offline --bin panelink ui_client::state::tests` | 159 passed, 0 failed, 0 ignored | [state](logs/btn02c-state.log) |
| `cargo test --locked --offline --lib split_menu::` | 31 passed, 0 failed, 0 ignored | [split menu](logs/btn02c-split-menu.log) |
| `cargo test --locked --offline --lib terminal::` | 93 passed, 0 failed, 0 ignored | [terminal](logs/btn02c-terminal.log) |
| `cargo test --locked --offline --test p5_workspace_tabs pointer_controls_preserve_live_ptys_and_confirm_close -- --test-threads=1` | 1 passed, 0 failed, 0 ignored; 14.10s | [pointer](logs/btn02c-private-pointer.log) |
| `cargo test --locked --offline --test p5_workspace_tabs workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe -- --test-threads=1` | 1 passed, 0 failed, 0 ignored; 5.24s | [tabs](logs/btn02c-private-tabs.log) |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | passed | [Clippy](logs/btn02c-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn02c-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn02c-diff.log) |

The final source adjustment positions pressed markers on More/+ themselves;
state and Clippy were rerun after it. Private fixture runs preceded that
render-only adjustment. Intermediate logs are retained: initial state146/10,
fixture migration153/3, compile errors while plumbing immutable RenderView,
compatibility156/0, new regression157/1 (used a correctly disabled narrow split;
changed its positive-activation fixture to a sufficiently wide pane), and
keyboard fixture158/1 (explicitly opened confirmation to isolate local cycling
from navigation-prefix state). No production availability rule or meaningful
assertion was relaxed to make a fixture pass.

Graph discovery used the exact checkout's existing project index, then current
source. The initial short project name was rejected; the exact registered name
worked. No refreshed-index claim is made. ms-rust is absent and unused.

No subagents, commits, push, installation, live user panes or server restart.
No server/protocol/dependency/global skill/planning changes. Canonical combined
verification and host/provider/clipboard/touch acceptance were not run. Original
canonical provider WouldBlock failures remain unclassified; these focused green
checks do not close that gate. Parent retains independent review and all wider
acceptance decisions.
