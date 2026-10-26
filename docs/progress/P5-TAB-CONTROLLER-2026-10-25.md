# P5 workspace tab controller implementation — 2026-10-25

Status: implementation slice updated for parent review. No Cargo, build, or
test command was run by this owner. The only local checks here are Rust 2021
formatting and `git diff --check`.

## Implemented in this slice

- Routed workspace open now loads and validates an immutable
  `WorkspaceIndex` snapshot before entering the client, then calls the
  workspace-aware host entry. The existing raw `ui_client::run(server,
  contexts)` path remains unchanged.
- Route preparation resolves typed project/tab IDs from fresh registry
  snapshots, drops the store lock before ensure/connect, rechecks registration
  under the destination lifecycle lock, creates a fresh App before
  `Net::connect`, and performs a final registration check before publishing.
- One route worker accepts one request at a time and retains a ready
  attachment in a nonblocking, request-ID checked handoff slot. Cancellation
  and stale payload reclamation run on that worker. A canceled destination
  server is left running; no stop is attempted.
- The host freezes on a route request, keeps polling/rendering the old route,
  rejects stale results, and preserves the old attachment on preparation
  failure. A successful handoff restores the current guard, makes one
  nonblocking old-worker detach attempt, resets the decoder/output/compositor,
  opens a fresh guard, and installs the destination App/worker.
- Selection persistence uses a separate metadata worker; the host queues it
  after installation. The worker has a capacity-one wake channel, one
  route-only save slot guarded across queued and running work, and one
  coalesced catalog-refresh bit. A full wake channel means a wake is already
  queued, so refresh cannot leave a pending bit without a future wake. Save
  errors expose only fixed status text.
- Route admission now uses an explicit busy token held from accepted enqueue
  through failure/cancel reclamation or matching handoff take. Rejected
  admission does not clear an earlier cancellation marker; cancellation
  advances the marker monotonically. Selection saves have a separate token
  held across queued and running persistence. The metadata backend accepts
  only `WorkspaceRouteId`, so keys and terminal input have no forwarding path.
- Frozen route polling passes no desired root size to the old Net. Each route
  request captures the current host dimensions for its fresh App/Net attach.
  Catalog refresh calls `App::update_workspace_catalog`, which retains the
  current route freeze and active route.
- The host event-loop route state is grouped in `RouteLoopState`; the current
  handler has eight parameters. Transfer ordering is factored into the helper
  exercised by deterministic restore/reset/fresh-guard ordering tests. A
  restore failure short-circuits before retiring the old worker or opening a
  destination guard; prepared Net ownership has already moved to a worker.
- One `Ready` result represents both Prepared and Ready: the route worker does
  its final registry validation after connect and immediately before
  publishing the offer, so the host has no adopted-but-not-ready state.

## Integration interfaces

The parent-owned `src/ui_client.rs` needs `mod routes;` and this wrapper:

```rust
pub(crate) fn run_workspace(
    directory: PathBuf,
    context_dir: PathBuf,
    app_root: PathBuf,
    index: WorkspaceIndex,
    current: WorkspaceRouteId,
) -> i32 {
    host::run_workspace(directory, context_dir, app_root, index, current)
}
```

The host uses the App seams `install_workspace_tabs`, `take_route_request`,
`finish_route_switch`, `take_catalog_refresh_request`, and
`refresh_workspace_tabs`. The last two are additional parent/UI-state
coordination points for the contract's chooser refresh. The host also needs
`WorkerHandle::try_detach_nonblocking(&mut self) -> bool` in the parent-owned
worker module; the current API only exposes the bounded waiting detach method,
which cannot be called from a responsive switch or Ctrl-X path.

## Remaining verification and limits

- The parent must integrate the module export and worker method, then perform
  serialized compilation and tests. No build/test result is claimed here.
- Inline route-module tests cover channel bound, request-scoped cancellation,
  stale handoff rejection, nonblocking handoff contention, controller busy
  admission while a canceled request is still owned by its preparation worker,
  a canceled preinstall request with zero metadata writes, a stalled-save
  second-mutation rejection, coalesced refresh wake delivery while the backend
  is blocked, nonblocking controller drop while save is blocked, and fixed
  save-error text. These are injected-backend tests, added but not run here.
  Host unit tests cover the actual transition helper's restore/reset/open
  order and short-circuit on restore failure. The stale host-result path still
  needs parent serial verification. Full A→B→A live attachment, real fsync
  behavior/Ctrl-X timing, and real two-server PTY acceptance remain open; the
  injected backend gate is not evidence of an OS-level fsync stall.
- This slice does not change legacy Default/Main dispatch or the raw client
  API, and it adds no dependencies. Plain `panelink` remains outside the
  workspace-aware route selection path.
- The no-Cargo constraint prevented compile verification. Parent should review
  the cross-module App/worker API integration and run the focused controller
  tests plus shared serial cache checks. No test pass is claimed by this owner.

## Parent API coordination

- The state owner exposes `update_workspace_catalog(index) -> bool`; the host
  uses this method so a refresh cannot clear `route_switching`.
- `WorkerHandle::try_detach_nonblocking(&mut self)` remains parent-owned and
  is used only after successful old-guard restoration.
- `RouteController::start(app_root)` no longer takes an initial size.
  `RouteRequest` carries `host_size`, sampled at each selection event.
