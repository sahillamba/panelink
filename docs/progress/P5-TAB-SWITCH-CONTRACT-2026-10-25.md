# P5 in-client workspace-tab switch contract — 2026-10-25

Status: implementation-ready design for the route-controller/host slice. This
document changes no runtime behavior and makes no build or acceptance claim.
Scope is limited to switching an existing attach client between registered
`(ProjectId, WorkspaceTabId)` routes. The plain `panelink` path continues to
open legacy Default/Main; the raw `ui_client::run(server_dir, contexts_dir)`
entry point remains unchanged.

## Ownership facts from the current source

- `default_open::open_routed_workspace` prepares terminfo and the exact route's
  server, rechecks registration inside `ensure_server_checked` under that
  route's lifecycle lock, then calls `ui_client::run(server, contexts)`.
  `WorkspaceIndexStore::load` holds the registry lock only for that store
  object's lifetime. Route resolution must drop it before server ensure,
  socket connect, terminal restoration, or worker cleanup.
- `Net::connect(server, contexts, &mut App, now)` performs synchronous
  context discovery, authenticated attach/handshake and initial layout
  negotiation. It resumes contexts only in the supplied route's context
  directory. `Net` is `Send + 'static` in practice because it is the
  `WorkerHandle` backend; confirm this bound at the actual worker spawn.
  `WorkerHandle::start(Net)` owns the Net thread. `detach_after_restore`
  submits a detach command and waits up to its deadline (currently 1.5s), then
  joins only an already-finished thread. `Drop` closes the command sender and
  never joins. A responsive Ctrl-X path restores the terminal first, then
  makes a bounded nonblocking attempt to enqueue `Command::Detach` and drops
  the handle without waiting. Do not call `detach_after_restore` inline.
  Accepted detach work and any blocked Net I/O stay on the worker; if the
  bounded command queue rejects it, socket closure still does not stop
  server-owned PTYs, but explicit context cleanup is not claimed.
- `Net::detach()` walks panes until its `DETACH_BUDGET` (currently 1.5s),
  calls each pane detach, and clears its pane list. The budget is checked
  between pane calls and cannot preempt one blocked call, so this is not a
  safe UI-thread cleanup primitive. Every uninstalled prepared Net stays
  worker-owned until cleanup runs there. Dropping a raw prepared Net on the
  host could run synchronous pane/socket cleanup and is forbidden.
- `App` owns all route-local focus, pane targets, pending keys, split/close
  requests, screen/history buffers, drag selection and pending copy. Its
  `cancel_for_detach` clears selection and pending copy; `abandon_pending_keys`
  clears keys not yet admitted to the network worker. There is no reusable
  route-neutral App state to transfer.
- `capture_launch_context` in `ui_client/net.rs` captures the client process's
  cwd and bounded environment (excluding `PANELINK_*`) for a managed split.
  Do not substitute the detached server's environment or cwd. If Net is
  changed to retain this context, capture it on the client attach side and
  pass it explicitly; do not capture it in the daemon or route resolver.
- `HostGuard::restore` sets `restored = true`; `enter` does not clear it.
  Reusing a restored guard would make its later Drop skip restoration. Create a
  fresh `HostGuard::open()` for each attached route, after restoring and
  dropping the previous guard. The new guard snapshots the restored host
  termios before entering raw mode.
- `Compositor`, `Output`, and `Decoder` are owned by `run_inner` today.
  Output can be discarded, compositor invalidated, and decoder replaced; all
  three must be reset at the attachment boundary. ClipboardWriter is
  client-owned and may persist across route switches, but route selection
  cancels a pending drag/copy before it can be submitted.

## Route and request types

Use the existing pure `workspace_tabs::WorkspaceRouteId { project, tab }` as
the only identity passed from App to the host. Do not add paths, `TargetRef`,
lease, generation, sequence, operation payload, or process identity to it.
The route worker resolves IDs from a fresh validated registry snapshot into a
private transient route:

```rust
struct ResolvedRoute {
    id: WorkspaceRouteId,
    server_dir: PathBuf,
    context_dir: PathBuf,
}

struct RouteRequest {
    request_id: u64,
    destination: WorkspaceRouteId,
}

struct PreparedAttachment {
    request_id: u64,
    route: ResolvedRoute,
    app: App,        // fresh App, initialized by Net::connect
    net: Net,        // connected to exactly route.server_dir / route.context_dir
    catalog: WorkspaceIndex,
}
```

The UI-state seam is a pending typed request, not a new `Flow` variant:

```rust
App::install_workspace_tabs(index: WorkspaceIndex, current: WorkspaceRouteId)
App::take_route_request(&mut self) -> Option<WorkspaceRouteId>
App::begin_route_switch(&mut self, now: Instant) // freezes child input;
                                                 // cancels selection/copy;
                                                 // abandons unsent keys
App::finish_route_switch(failed: bool, now: Instant) // unfreezes
```

`install_workspace_tabs(index, current)` replaces the immutable catalog in
App and marks `current` selected and active. On selection, App records one
request and immediately enters the frozen preparing state. After
`handle_event`, the host takes the request once, allocates a checked monotonic
request ID and submits it. No selection action calls store, filesystem,
socket, clipboard, or server APIs from App. Selecting the active route only
closes the chooser. `finish_route_switch(failed, now)` clears the pending
freeze after failed prepare or completed installation; save status is separate
so registry failure never misreports attachment failure.

The controller owns one route-preparation thread and bounded channels of
capacity one. Minimal host-facing shape:

```rust
RouteController::start(app_root: PathBuf, attach: ClientAttachContext)
RouteController::prepare(request: RouteRequest) -> Result<(), RouteBusy>
RouteController::try_result() -> Option<RoutePreparationResult>
RouteController::cancel(request_id: u64) // atomic mark; never join
RouteController::try_take_prepared(request_id: u64) -> Option<PreparedAttachment>
RouteController::queue_selection_save(request_id: u64, route: WorkspaceRouteId)
RouteController::try_status() -> Option<RouteStatus>

enum RoutePreparationResult {
    Prepared { request_id: u64 },
    Ready { request_id: u64 },
    Failed { request_id: u64, label: &'static str },
}
```

There is at most one in-flight prepare and no unbounded result backlog. A
second route request while busy is rejected locally with a fixed status. Every
result carries its request ID; the host accepts only the currently pending ID.
The controller checks cancellation between resolve, ensure, connect, and result
publication. A synchronous handshake may run to its existing deadline after
cancellation; cancel never blocks the host or waits for thread exit. The
prepare worker retains `PreparedAttachment` after publishing `Prepared` and
`Ready`. Use one bounded, request-ID-tagged handoff slot with nonblocking
host-side `try_take(id)`; the worker remains owner until the host takes the
matching payload in the same event-loop turn that commits to installation. If
Ctrl-X or cancellation wins first, the host marks cancellation and the worker
reclaims/detaches its Net on its own thread. No channel close, stale-result
discard, or host RAII drop may destroy a raw prepared Net. Reclaim canceled
results before accepting another prepare. The handoff has one payload and no
blocking host lock or wait.

There is no adoption point that can defer Ctrl-X. Before installation the old
route remains current and may be detached at any time. Ctrl-X restores the
active `HostGuard` immediately, then drops the old `WorkerHandle` (closing its
command sender without joining). It exits without waiting for metadata,
`Net::detach`, or thread termination. Any admitted detach command runs on the
worker; rejection is reported only as best-effort cleanup, not a client stall.
If Ctrl-X and Ready race, whichever the
event loop handles first wins: Ctrl-X cancels the uninstalled offer; install
makes the destination current, and a later Ctrl-X detaches that route.
Admitted commands remain scoped to their original server; new child input is
frozen through preparation and installation.

Selection persistence is a separate best-effort metadata job queued only
after successful installation. It runs on a distinct bounded worker
and may finish after Ctrl-X or client exit. The host never waits for it. Do
not promise rollback, stale-write suppression, or total ordering across
detached client lifetimes: the user made an explicit selection and that
selection may persist later. A request canceled or stale before installation
never queues/persists selection. A save failure is reported only while the
client remains alive.

`ClientAttachContext` contains the bounded host size and the explicitly
captured client `LaunchContext` if Net is updated to store one. Capture cwd and
environment from the client process before starting route preparation. Keep
it out of `WorkspaceRouteId`, registry metadata, server fallback logic and
diagnostics. If no Net API change is made in this slice, existing
`capture_launch_context()` remains the client-side source used by explicit
managed child creation; route preparation never asks the server daemon for
that environment.

## Prepare, install, and persistence ordering

The route worker performs this bounded prepare sequence without the host event
loop or registry lock held:

1. Load `WorkspaceIndexStore` into an immutable `WorkspaceIndex` snapshot;
   validate the exact typed project/tab pair and derive route paths only from
   the registered numeric IDs. Drop the store before continuing. Use the
   existing root safety, route namespace, socket-path, terminfo and
   `ensure_server_checked` behavior. The latter rechecks registration while
   holding the destination lifecycle lock immediately before ensure/start.
2. Create a fresh `App` using the captured host size, then call the route
   attach seam (`Net::connect(destination.server_dir,
   destination.context_dir, &mut app, ...)`) to negotiate/reconcile only the
   destination's own contexts and load its initial layout. No old App,
   TargetRef, lease, operation key/sequence, pending key/paste, clipboard
   text, status, decoder byte, screen buffer or uncertainty crosses this
   boundary. Do not replay stored terminal side effects.
3. Publish `Prepared { request_id }` while retaining the destination App and
   Net on the route worker. Until the host accepts this matching request ID,
   the current App and old `WorkerHandle` remain the visible, attached route.
   The destination may have caused a new server to start; cancel does not stop
   it. There is no automatic stop, cleanup or input replay for a started
   destination server.
4. On matching `Prepared`, let the ready handshake finish while keeping the
   old route visible and frozen. On matching `Ready`, in one event-loop turn
   take the matching payload from the bounded handoff, restore/drop the old
   guard, drop the old WorkerHandle without join, reset terminal-facing state,
   install the fresh App and start the new WorkerHandle, then open/enter a new
   HostGuard and render. No registry read/write/fsync, lock acquisition,
   thread join or metadata wait occurs between Ready and destination
   installation. Ctrl-X cannot interleave inside this synchronous event turn.
   Queue selection persistence only after installation. A save error leaves
   the attached destination as the actual view and may show fixed
   `selection not saved` status while still attached.

On matching `Ready`, the host performs the transfer in this order:

1. Finish the already requested freeze: call `cancel_for_detach`,
   `abandon_pending_keys`, and discard any host-side deferred selection/copy
   action. Prevent further child-bound effects from the old App. Already
   admitted old-worker commands stay scoped to the old Net/server.
2. Restore and drop the old `HostGuard`. Do not call
   `detach_after_restore()` inline: it waits up to its deadline. Make a
   bounded nonblocking attempt to enqueue `Command::Detach`, then drop the old
   `WorkerHandle`; Drop closes the command sender without joining. If queued,
   its worker performs Net cleanup independently. If rejected, no explicit
   context cleanup is claimed; server-owned children remain running either
   way.
3. Discard old `Output`, replace `Decoder::new()`, invalidate the compositor,
   discard pending terminal input, and drop the old App and route snapshot.
   Recreate the compositor/output if their reset APIs cannot prove a clean
   baseline. ClipboardWriter stays client-owned; canceled pending copy is not
   sent to it. Any copy write already accepted before the switch remains a
   host clipboard operation and cannot become destination input.
4. Install the prepared App and start a new `WorkerHandle` for its Net. Open a
   new `HostGuard`, then enter host mode and render the already connected
   destination. The first destination frame is never rendered under old
   decoder/compositor/App state.

## Failure, stale results, and catalog policy

- **Preparation failure:** the old attachment was never retired. Keep its
  actual route, App, worker and rendered pane visible; show a fixed
  `workspace switch failed` status and unfreeze the App so the user can retry
  or choose another registered route. The failed destination must not be
  displayed as connected, and no fallback route or queued input is allowed.
  This intentionally supersedes the original tab UI plan's proposal to leave
  the client with no active pane route after destination failure: preserving
  a still-live attachment avoids destroying usable state before the
  replacement is ready.
- **Stale/canceled completion:** compare request IDs at `Prepared` and
  `Ready`. A stale request is canceled before installation; its worker
  reclaims/detaches Net off-host and never queues selection or publishes App.
  There is no adopted-but-not-installed state. A superseding request waits
  until prior preparation is canceled/drained or installation completes.
- **Destination removed after snapshot:** `ensure_server_checked` revalidates
  registration under its lifecycle lock. If removal wins before ensure, fail
  and preserve the old route. If removed after connection but before install,
  fail readiness, reclaim Net on the route worker, and preserve old route.
  Once installed, the connected route is the actual view; the later metadata
  save does not revalidate or roll it back.
- **Catalog snapshot:** source display names/order/selection from one
  `MetadataRecord` snapshot (`record.index().clone()`); never retain a store or
  store lock in App, render, hit testing or event handling. Load an initial
  catalog before host entry and refresh it asynchronously when the chooser is
  opened and after each completed route preparation. Refreshes share the
  single bounded controller and coalesce to the newest requested refresh.
  A tab created/removed concurrently may be absent from the cached picker;
  exact route validation at preparation is authoritative and returns a
  bounded failure. Preserve the old snapshot on refresh error and show a
  fixed status. Catalog refresh alone never changes active selection. The
  picker keeps the already specified fixed header geometry while rows and
  selection update from the immutable snapshot.
- Registry selection is metadata only. Persistence is asynchronous and
  best-effort after installation; it can complete after detach, and cross-
  lifetime ordering or rollback is not guaranteed. Failure cannot roll back
  an established connection or cause the view to claim another route.
- Bound work: one route prepare, one prepared handoff, one catalog refresh
  request, and one pending selection-save job. Coalesce catalog refreshes to
  the newest request. If a metadata job is stalled (including in fsync),
  reject new registry mutations and metadata jobs locally as busy. Do not
  queue them unboundedly or block the host. Metadata worker shutdown is
  nonjoining. No contract assumes fsync is cancellable or thread termination
  bounded.

## Bounded owner assignments

Keep the original exclusive file boundaries from
`P5-WORKSPACE-TAB-UI-PLAN-2026-10-25.md`:

1. **UI state owner only:** `src/ui_client/state.rs`,
   `src/ui_client/state_tests.rs`, `src/ui_client/keys.rs`. Own route request
   intent, local freeze/unfreeze, cancellation of selection/copy/unsent keys,
   and tests proving frozen events cannot call `Effects::send_keys` or
   `send_paste`. Consume the route request with `App::take_route_request`;
   do not edit controller or host files.
2. **Controller/host owner only:** `src/default_open.rs`,
   `src/ui_client/host.rs`, new `src/ui_client/routes.rs`. Own immutable
   catalog loading/refresh, typed ID resolution, lifecycle-checked server
   ensure, bounded route worker/request IDs, handoff, async selection save, host restore,
   old-worker cleanup, fresh attachment install, and reset ordering. Consume
   the state seam; do not edit App state or the workspace store.
3. **Optional Net adaptation is separately reviewed/assigned:**
   `src/ui_client/net.rs`, `src/ui_client/conn.rs`,
   `src/ui_client/worker.rs`. Only if the controller cannot meet ownership
   and cleanup requirements through the current APIs, add a route-scoped
   prepare/detach operation with an explicit attach context. Do not change
   context reconciliation, command replay, server protocol or TargetRef.
   Review this API before the controller owner edits these files.
4. **Acceptance owner only:** new `tests/p5_workspace_tabs.rs`. Two private
   registered routes/servers, real client attach/switch, host decoder input,
   route survival and isolation. No production source changes.

Parent serializes compilation and tests after each owner releases their files.
No owner edits the other assignment's files, server identity/protocol,
clipboard implementation, history/layout formats, or default no-selector
dispatch.

## Required acceptance evidence

Controller tests with fake route endpoints must cover A→B→A using deliberately
equal numeric TargetRef fields; each attach reads only its route contexts,
gets fresh App/Net state, and cannot carry pending keys, paste, copy, target,
lease, sequence or uncertainty across routes. Assert the old route remains
active until matching preparation succeeds; preparation failure preserves
that actual route and permits retry. Assert corrupt/missing registry, removed
route, ensure/handshake timeout, selection save failure, canceled preparation,
stale `Prepared`, cancel/install interleavings, and simultaneous
prepare-completion/cancel leave the correct visible route/status and cause no
operation replay. Assert selection is never queued before installation, and
canceled/stale pre-install requests never persist. Exercise worker cleanup
without joining it on the host. Stall metadata commit/fsync, press Ctrl-X,
and assert raw mode is restored and local detach returns before releasing the
commit. Release it and assert it never forwards input to either the old route
or a later route. The selected route's write may complete after Ctrl-X; there
is no rollback or cross-lifetime ordering promise.

Host tests must assert restore occurs before old worker cleanup, no
`detach_after_restore` wait occurs on the Ctrl-X/UI path, each switch creates
a new `HostGuard` after restoration, and output/decoder/
compositor/App are reset before destination render. Inject route-selection
input followed by child keys, paste, copy-release, Ctrl-X and resize while
preparing: the local Ctrl-X/resize/render loop remains responsive, but no new
child input or copy is submitted; pending unsent input is reported/discarded.
Also verify a failed prepare unfreezes the old route, canceled destination
startup may remain running without automatic stop, stale prepared Net cleanup
is off-host, and both server-owned process sets survive A→B→A and client
detach. Use a fake slow worker to prove request capacity one and nonblocking
cancellation/drop.

Suggested parent commands after source freeze and serialized ownership release
(not run for this design task):

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --bin panelink ui_client:: -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --test p5_workspace_tabs -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo fmt --check
```

These checks establish only their local automated coverage. Apple Terminal
interaction, real clipboard behavior, physical reboot recovery, provider
recovery and representative workload budgets remain separate acceptance
gates. No source-only result establishes nonreplay behavior; tests must
observe the server-side operations/contexts at both routes.
