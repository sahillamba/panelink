# P5 pane operations client UI — 2026-10-26

Status: client UI and host completion integration are implemented in the
assigned files. Rust formatting and whitespace checks pass. No Cargo, build,
or test command was run; parent serial verification remains required.

## Client behavior

`App` retains a `PaneOperationsView` only after strict view validation through
`encode_view`. Older revisions are ignored within the same generation and
workspace. The underlying
`LayoutReply` remains the complete partition and target registry, so hidden
targets and their screen slots continue to be accepted and retained.

Rendering, pointer hit testing, focus movement, scrolling, and selection use a
separate display layout. Zoom projects the exact target to the full root and
removes internal dividers from the frame without changing the authoritative
layout. Pane resize state is compared with each target's confirmed actual size
and the effective display size. A screen matching its confirmed actual size
remains visible during an intentional zoom mismatch. Zoom observation moves
local focus to the visible target; unzoom leaves that focus in place.

Ctrl-B `z` submits an absolute zoom/unzoom action at the observed revision.
Ctrl-B `m` opens a local exact-target swap chooser. Ctrl-B `e` opens a local
ancestor-divider chooser and validates each one-cell preview with a fresh pure
`PaneLayout` candidate. Only Enter submits an operation. Escape, paste, mouse,
and other modal input stay local; Enter and Escape repeats/releases are
isolated, while Ctrl-X remains responsive. Stale layout, focus, host resize,
and route changes cancel local modes. Split and close receive actionable
unzoom notices while zoom is active.

One pending operation retains its exact ID and `PaneAction`. Host completion
must match both before it can update UI state. Valid Applied and Partial views
are authoritative, but an older completion cannot replace a newer polled
revision. Queue rejection leaves the prior view in place and does not retry.
Unknown completion clears the local busy state with a fixed status and does not
replay the request. Route switching clears the old route's UI intent; it does
not roll back work already admitted by the server.

The worker's `Effects::pane_operation` adapter delegates to the bounded
`WorkerHandle::pane_operation(id, action)` method. Legacy effect adapters have
a default unsupported result. A connection without a validated operations
view reports that pane controls are unsupported and keeps the existing
layout/input paths.

## Focused client cases added

The state tests now cover exact swap capture and local input consumption,
divider preview without underlying-layout mutation, full target retention and
hidden screen updates during zoom, root projection and cursor coordinates,
workspace-header hit testing, unzoom restoration, route-switch cancellation
and lower revision acceptance in another workspace, stale chooser cancellation,
old-server local notices, exact and partial completion matching, unknown
completion, and queue-full no-retry behavior. Key mapping tests cover the
three prototype bindings. These tests were added but not run in this task.

## Scoped checks and remaining verification

Passed:

- `rustfmt --edition 2021 --check src/ui_client/state.rs src/ui_client/state_tests.rs src/ui_client/keys.rs src/ui_client/host.rs src/ui_client/worker.rs`
- `git diff --check`
- trailing-whitespace scan of this new, untracked note

Not run, per task boundary: Cargo tests, build, dependency changes, PTY/host
acceptance, clipboard, activation, commit, and push. Parent serial verification
must exercise the new tests against the integrated worker/module and server
tree. This note records only client source integration and scoped formatting;
it does not mark Phase 5 or native acceptance complete.

## Parent focused review and execution

Initial client compile failed on a borrowed KeyEvent and a borrowed fixture
layout held across mutation; parent corrected both and removed an unreachable
render arm. The r2 client target passed119/0. Parent source review then found
that a pending PTY screen was passed to a different-sized display rectangle,
which compositor validation rejects. The UI now uses a placeholder during
this effective resize mismatch; confirmed-underlying metadata alone does not
hide a successfully zoomed screen. Added a regression with existing snapshots
for divider preview and pending zoom.

The corrected complete ui_client filter passes **120 tests, 0 failed**:
`/private/tmp/panelink-p5-pane-client-ui-r3.log`. Initial compiler evidence is
at `panelink-p5-pane-client-ui-r1.log`, intermediate result at r2. Independent
read-only review and outer-PTY controls acceptance remain active; no whole-tree
or manual host acceptance is claimed here.
