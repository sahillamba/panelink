# P5 in-client workspace tab UI plan — 2026-10-25

Status: implementation plan based on the current shared checkout. It adds no
product decision beyond the confirmed **Project → workspace tabs → split
panes** hierarchy. CLI routing, the metadata-only index/store, per-tab server
and context namespaces, and legacy Default/Main preservation are already
implemented and under parent verification. This plan covers the remaining
in-client navigation and switching only.

## Recommendation

Keep one existing Panelink server per workspace tab and attach the client to
one selected tab at a time. Add a one-row project/workspace strip above the
existing pane compositor. Switching changes the client route
`(ProjectId, WorkspaceTabId)` and replaces its attachment; it never adds those
IDs to `TargetRef`, alters server wire identity, or represents one tab as a
pane. The target remains the exact server-issued
`(generation, workspace, pane, session, occupant_revision)` and is valid only
with that tab's endpoint and `contexts/` directory.

Use the existing `WorkspaceIndex`/store as the source of visible names, order,
and selection. The UI loads a bounded snapshot and passes typed route IDs; it
does not keep the store lock while rendering or handling input. Existing
`open_routed_workspace` currently resolves one route and invokes
`ui_client::run(server, contexts)` once, whose `run_inner` creates one `App`,
one `Net` worker, and one client clipboard writer. Keep that public entry
behavior and add a route-aware client entry/controller for interactive tab
navigation. The default no-selector CLI path remains the legacy Default/Main
route; only the existing explicit/saved-selection workspace commands route by
tab.

## UI and geometry contract

- Reserve screen row 0 for a persistent strip: `[ Project v ]  [Tab 1] [Tab
  2] ...`. Highlight the selected project and active tab; clip labels to whole
  terminal cells and keep stable-ID hit regions after clipping. Do not show
  filesystem paths or runtime targets.
- Project control opens a compact project chooser. Choosing a project shows
  that project's ordered tabs and selected tab; choosing a tab requests a
  route switch. Selecting the already active pair closes the chooser without
  reconnecting. When tabs exceed the available width, show left/right overflow
  controls and keep the active tab visible. No sidebar or nested pane tabs.
- Compute one canonical geometry snapshot for draw and hit testing:
  `screen = (0,0,min(host_rows,80),min(host_cols,240))`,
  `strip = first row of screen`, and `pane_root = screen below strip`. Pane
  rectangles, divider hit regions, status/NAV banner placement, and mouse
  content coordinates are all relative to `pane_root`; terminal output and
  header hit testing remain in screen coordinates. The one-row strip reduces
  the pane root by one row. If the reduced root fails current pane minimums,
  retain current too-small behavior and render the strip plus one status row;
  never overlap panes with the strip. On a zero-row or zero-column viewport,
  draw nothing and reject all hit tests.
- Left click on a tab requests selection; left click on the project control
  opens the chooser. Strip clicks, chooser keys, and chooser clicks are always
  consumed locally and never focus a pane, start drag selection, open a pane
  menu, or reach a child. A click elsewhere follows the current pane behavior.
- Integrate the new strip as a fixed top-level view in `App::render` and
  `ComposeFrame` (or an equivalent bounded header field), not as a status
  overlay: current overlay handling is modal/bottom-positioned and can replace
  help, menus, close prompts, and NAV/status. The strip stays visible while
  those existing overlays are open; modal overlays continue to own input.

### Keyboard and picker behavior

Reuse the established Control-B navigation prefix. Add `w` to open the
project/workspace chooser, `n` and `p` to request the next/previous tab in the
active project, and `[` / `]` for previous/next project. These bindings are
local commands, not child input. They do not leave NAV mode until the switch
result is known; Escape cancels an open chooser or exits NAV as it does now.
Update the NAV banner and help with these actions. Outside NAV mode, keep
existing input forwarding unchanged. The picker supports Up/Down over
projects and tabs, Left/Right to change project, Enter to request the
highlighted `(ProjectId, WorkspaceTabId)`, and Escape to close. This is a
reversible prototype binding choice; a configurable binding map can replace
it later.

## Client routing and switch lifecycle

Introduce a client-only route value, distinct from `TargetRef`:

```rust
struct WorkspaceRoute {
    project_id: ProjectId,
    tab_id: WorkspaceTabId,
    server_dir: PathBuf,
    context_dir: PathBuf,
}
enum RouteSwitchState {
    Active(WorkspaceRoute),
    Switching { from: WorkspaceRoute, to: WorkspaceRoute, request_id: u64 },
}
```

Resolve paths from validated registry IDs using the existing route namespace
rules; do not accept paths from the UI model or persist them in the index.
Only the route controller/default-open boundary may construct a
`WorkspaceRoute`. Every `App`, `Net`/`WorkerHandle`, `TargetRef`, focus, pending
input, scroll request, selection and pane action belongs to exactly one active
route. Do not cache inactive `App` or worker objects in the first version.

Switch as a bounded detach-and-reattach operation:

1. Consume the selection action locally; cancel frozen drag and pending local
   copy state, close the picker, freeze further child input, and do not emit a
   new child operation.
2. Flush only input that the old `App` can submit through its existing
   nonreplayable path. Any still-unsent local keys are abandoned with the
   existing explicit notice. Never retarget them to the destination. Do not
   replay paste, clipboard data, split/close intent, or any uncertain input.
3. End and clear the old `App`/attachment. Invalidate pending host output and
   decoder bytes, restore the terminal before any potentially blocking worker
   cleanup, then re-enter host mode for the new view. Run existing detach
   cleanup for that worker; let its route-scoped `ClientContext` retain
   sequence and uncertainty so a later attach reconciles against the same server. The
   old server and its PTYs stay running. All panes, focus, viewport/history,
   `TargetRef`s, drag snapshots, request IDs, and status tied to old state are
   discarded from the new view.
4. Start a fresh connection to the destination route, negotiate capabilities,
   reconcile its own context, and build a fresh `App`. Show a local
   `connecting`/failure status without forwarding input while switching.
   Commit the selected project/tab metadata only through the route controller
   after validation; if registry persistence fails, report it and keep the
   route actually attached visible. Never silently fall back to Default/Main
   or to another tab.
5. On destination failure, leave a safe client with no active pane route and
   offer retry or selection of another registered route. Do not resurrect the
   old `App` or queue actions for either endpoint. Ctrl-X remains a detach
   from the currently attached server and keeps that server's children alive.

Host terminal mode restoration, output-buffer invalidation, decoder reset,
and attachment worker cleanup must be bounded and ordered. Reuse the
restore-before-blocking-cleanup guarantee in `run_inner`; do not hold the
registry/store lock while restoring the host, joining a worker, or opening a
socket. Route resolution, registry writes, network connect/handshake, and
worker teardown must not block the UI event loop. Put them behind a bounded
route worker/controller with request IDs and stale-result rejection. Clipboard
writer remains client-owned, but switch cancels pending selection/copy before
it can submit text; any already accepted clipboard write is not a server input
and cannot become a destination operation.

## First implementable slice

Add a pure client module for the view contract before wiring host routing:

- `WorkspaceTabsView`: borrowed project/tab labels and IDs plus selected
  `(ProjectId, WorkspaceTabId)`; no store, filesystem, socket, or `TargetRef`.
- `WorkspaceTabsGeometry::new(host_rows, host_columns, view)` (or a pure
  geometry function): strip/project/tab/overflow rectangles and reduced pane
  root, with checked/clamped arithmetic.
- `WorkspaceTabsHit::{ProjectChooser, SelectTab(ProjectId, WorkspaceTabId),
  PreviousTab, NextTab, PreviousProject, NextProject, None}` and a pure
  `hit_test(column,row)` using the same geometry as rendering.
- Pure clipped-cell label/layout helpers and deterministic overflow policy.
  The model uses the existing eight-tabs-per-project and 32-project bounds;
  the view can page without changing stored order.
- Unit tests for zero/one-cell boundaries, minimum pane height after adding
  the strip, clipping, overflow with selected-tab visibility, every hit region,
  and exclusion of pane/root rows from header hits.

This slice is independently useful and reviewable without changing
`open_routed_workspace`, `App`, host IO, or any server. The next UI-state slice
adds header rendering and event routing against a fake selection callback. The
following route-controller slice establishes safe route replacement using
fake endpoints before the host loop is connected.

## Exclusive implementation assignments

Keep each task bounded and hand off before editing another owner's files:

1. **Pure view/geometry:** new `src/ui_client/workspace_tabs.rs` and its inline
   tests only; add `mod workspace_tabs;` in `src/ui_client.rs` only after
   parent/module-owner coordination. Define typed hit results and no effects.
2. **App presentation/controller intent:** `src/ui_client/state.rs`,
   `src/ui_client/state_tests.rs`, and `src/ui_client/keys.rs` only. Consume
   strip/picker input, reserve the root row, compose header, add NAV commands,
   and emit a typed `RouteRequest` through an effect seam. No endpoint open,
   store call, worker join, or filesystem access.
3. **Route resolution and transition owner:** `src/default_open.rs`,
   `src/ui_client/host.rs`, and a new `src/ui_client/routes.rs` only. Load a
   validated index snapshot, resolve typed route IDs into endpoint/context
   paths, and implement the bounded switch controller. Host restoration,
   old-worker cleanup, decoder/output/compositor reset, and fresh attach are
   integrated here. Use fake route/worker seams first; do not edit `state.rs`
   or the workspace store.
4. **Connection-worker adaptation only if needed:** `src/ui_client/conn.rs`,
   `src/ui_client/net.rs`, and `src/ui_client/worker.rs` only. Add an explicit
   route-scoped shutdown/reconnect API without changing operation replay,
   context identity, or server protocol. Parent reviews the interface before
   this task starts; avoid parallel edits with assignment 3.
5. **Host/PTY integration acceptance:** new `tests/p5_workspace_tabs.rs`
   only. Own two private workspace routes/servers, attach clients, drive the
   top-strip and keyboard selection through the host decoder, and verify both
   servers stay live. Do not edit production code or existing fixtures.

Parent integrates and serializes build/tests after each owner releases files.
No assignment edits server identity/protocol, `TargetRef`, clipboard
implementation, saved history/layout formats, or default no-selector
dispatch.

## Tests and acceptance gates

1. Pure view tests cover overflow, clipping, stable-ID hit mapping, selected
   tab visibility, top-row exclusion from pane click/drag/menu routes, reduced
   pane geometry, and exact too-small thresholds. A fake renderer asserts
   header and pane cells do not overlap and resize recomputes both draw and hit
   rectangles from the same geometry.
2. App-state tests cover project chooser open/close, keyboard and mouse
   selection, already-active no-op, modal precedence, NAV timeout, resize,
   and proof that tab-row input never reaches `Effects::send_keys` or
   `send_paste`. Existing pane navigation and pane hit tests stay unchanged
   apart from the root row offset.
3. Route-controller tests use two fake endpoints with deliberately equal
   numeric `TargetRef` components and operation sequence values. Verify route
   identity still separates them; switch A→B→A starts fresh attachments,
   negotiates/reconciles each route's own context, resets focus and local
   state, and does not reuse a target, lease, sequence, pending key, paste,
   clipboard payload, or uncertain operation across routes.
4. Two-server host acceptance creates two registered tabs with unique marker
   output and a distinct generation in each. Type into A, switch B and type
   into B, switch back A: each marker and process remains in its original
   server; A's original focus/context is reacquired only from A's server
   snapshot; B stays live while inactive. Ctrl-X detaches only the current
   client and both server-owned process sets remain alive until explicitly
   stopped by the test fixture.
5. Clipboard isolation tests start/freeze a drag in A, request a switch before
   release, then prove no copy is sent for that canceled gesture and no paste
   or child clipboard request is sent to B. Existing negotiated row-wrap
   requirements and host clipboard denial remain in force.
6. Decoder/geometry host tests inject SGR clicks on the project control, each
   tab, overflow controls, and pane coordinates on rows immediately above,
   at, and below the strip/root boundary. Repeat after resize and at short,
   wide, one-column, zero-row, and pane-minimum terminals. Verify no top-row
   click focuses a pane and no row-offset error shifts child mouse coordinates.
7. Failure tests cover corrupt/missing route registry, removed tab between
   snapshot and switch, connection/handshake timeout, failed old-worker
   cleanup, stale route completion, and selection-write failure. Each leaves
   a bounded status, valid host terminal restoration, no implicit fallback,
   no orphan client worker, and live server processes untouched unless an
   explicit stop command is issued.

Local model/state tests and two-server process tests do not establish Apple
Terminal interaction, real host clipboard behavior, physical reboot recovery,
provider recovery, or workload budgets. Keep those evidence classes separate.

## Assumptions and deferred choices

- The project is initially a named grouping; do not infer a project root from
  cwd, Git state, symlinks, or worktrees. Default project/tab selection comes
  from the index for `workspace open` only. Plain `panelink` and existing
  legacy commands without selectors keep Default/Main behavior.
- Prototype presentation is one top row, a project chooser, and visible tabs.
  A project sidebar, alternate chooser presentation, richer status indicators,
  and mouse-drag reordering remain deferred.
- Tab creation/removal and stop-workspace lifecycle are not bundled here.
  Existing CLI may create/stop/remove according to its own safety contract;
  the first UI only selects among registered routes. Future tab close/removal
  must preserve a route to live PTYs or require verified stop; hiding a live
  tab cannot orphan its processes.
- One server per tab is the first runtime shape. Project-manager processes,
  one server per project, remote execution, automatic project discovery,
  automatic worktrees, provider-driven subagent tabs, and restoration of
  provider conversations remain deferred.
- Keyboard choices above are provisional prototype bindings using the
  existing Ctrl-B prefix; no configurable keymap or product approval is
  required to validate this reversible presentation slice.

## Current source seams inspected

- `src/default_open.rs`: `open_routed_workspace` resolves route startup then
  calls `ui_client::run(state.server, state.contexts)` once; `open_workspace_ids`
  constructs the route-specific `StateRoot` and namespace.
- `src/ui_client/host.rs`: `run_inner` owns host guard, `App`, compositor,
  output, decoder, clipboard writer and one `WorkerHandle`; cleanup restores
  the terminal before blocking clipboard/network cleanup. `event_loop` owns
  local key decode, resize, worker polling, frame output, and server-ended
  exit.
- `src/ui_client/state.rs`: `App::render` maps pane-root rectangles directly
  into a `ComposeFrame`; `App::handle_event` gives help/confirm/menu modal
  precedence, then routes all mouse clicks through `pane_at`. `pane_at` tests
  pane rectangles in root coordinates. `App::viewport` currently caps the
  root at 80×240; the new strip must adjust render, input, resize and minimum
  fit consistently.
- `src/terminal/compose.rs`: `ComposeFrame` carries pane rectangles, dividers,
  focus and one modal overlay. The strip needs a fixed header presentation
  path so current overlays cannot replace it.
- `src/ui_client/keys.rs` and `vendor/crossterm-input`: the host decoder
  delivers typed key and mouse events, including SGR mouse coordinates; add
  local route navigation after decoding, without sending header events into
  pane input.
- `src/workspace_index.rs` / `workspace_index/store.rs` and
  `src/workspace_cli.rs`: typed distinct `ProjectId` / `WorkspaceTabId`,
  bounded index (32 projects, 8 tabs each), metadata-only selection, and
  per-route path/lifecycle handling are already present. Keep that identity
  separate from control `TargetRef` and avoid filesystem/store work in draw or
  pointer handling.

The graph-backed source discovery used the current worktree index. Relevant
entry symbols were `default_open::open_routed_workspace`,
`ui_client::host::run_inner` / `event_loop`, `ui_client::state::App::render` /
`handle_event` / `pane_at` / `viewport`, and `Compositor::render`. This plan is
based on those current implementations and the 2026-10-25 resumed integration
and routing result notes, not earlier pre-storage tab sketches.
