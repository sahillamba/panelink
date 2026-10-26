# Phase 5: Project and workspace-tab model design

Status: engineering proposal, 2026-10-25. The hierarchy is confirmed by the
user: **Project → workspace tabs → split panes**. Workspace tabs are the
workspaces; there is no nested second tab layer. This note narrows an
implementation direction. It does not claim implementation or acceptance.

## Recommendation

For the first tab implementation, keep one existing Panelink server and its
four-pane runtime per workspace tab. Add a small project/workspace index above
those servers. Each tab continues to be one independently addressable runtime
and one independent layout/history domain. This fits the current owner, which
has one `layout: Option<PaneLayout>` and fixed four-pane size state, while
avoiding a rework of lease, event, launch, and snapshot ownership across
multiple layouts.

The UI attaches to only the selected tab. Switching tabs detaches that client
view from the old server and attaches it to the selected server. Detach and tab
switch keep server-owned PTYs and child processes running. Explicit **Stop
Workspace** stops that tab's panes; closing a tab removes it from the visible
tab order only after an explicit stop, or leaves it registered and running if
the user chooses **Close Tab (Keep Running)**. The intended default is keep
running, consistent with the existing background-server behavior. Pane close
continues to stop that pane; closing the final pane continues to end that
workspace. These actions must not be conflated.

Proposed engineering bound: at most 8 workspace tabs per project, at most 4
panes per tab. This is an initial resource/validation bound, not a product
promise. No project-count limit is selected here.

## Current boundaries that shape the design

- `default_open::resolve_root` resolves one global app state root from
  `PANELINK_HOME`, or `~/Library/Application Support/Panelink`. The current
  `StateRoot` derives `server/`, `terminfo/`, and `contexts/` from that root.
  `open_workspace` prepares that root, ensures one server, then calls the UI
  client with that server and context directory. `ensure_server` reuses a
  running endpoint or starts one under the lifecycle lock. See
  [`default_open.rs`](../../src/default_open.rs#L89) and
  [`default_open.rs`](../../src/default_open.rs#L113).
- `ServerStorage::open` takes the exclusive server lock and advances the
  generation counter before the server uses it. The same server directory is
  the namespace for the launch journal and the `layout/` and `history/`
  children. Layout/history stores verify their server-directory identity. See
  [`server_storage.rs`](../../src/server_storage.rs#L145),
  [`server_storage.rs`](../../src/server_storage.rs#L286),
  [`P5-SAVED-LAYOUT.md`](P5-SAVED-LAYOUT.md#L152), and
  [`P5-HISTORY-WRITER.md`](P5-HISTORY-WRITER.md#L31).
- `TargetRef` is the exact live target identity:
  `(generation, workspace, pane, session, occupant_revision)`. It is not a
  project or tab ID. Keep it unchanged and route it to the server/context that
  issued it. A pane number, visible tab index, or stable tab ID must never be
  substituted for any TargetRef field. See
  [`control_protocol.rs`](../../src/control_protocol.rs#L40).
- `ClientContext` persists server generation, lease, operation sequence,
  uncertain operation, and revision. Therefore each tab needs its own context
  file and connection/lease state. On tab switch, do not carry the previous
  tab's context, lease, sequence, or uncertainty into the selected tab. A
  stale generation/lease/TargetRef remains a refusal, not a cue to pick the
  current occupant. See [`client_context.rs`](../../src/client_context.rs#L31).
- The current `MultiSessionOwner` has one volatile layout and one set of
  four-pane state, not a map of independent workspace layouts. `TargetRef`
  already includes a workspace component, but that alone does not implement
  multiple layouts in one owner. See
  [`multi_session.rs`](../../src/server/multi_session.rs#L146).
- Saved layout rows are historical metadata, not live dispatch identities;
  history rows/checkpoints, current emulator state, and provider conversations
  remain distinct. The index must not absorb or replay any of those payloads.

## Identity and index proposal

Add stable, opaque IDs with types distinct from runtime IDs:

```text
ProjectId
WorkspaceTabId
Project { id, display_name, ordered_tabs, active_tab_id }
WorkspaceTab { id, display_name }
WorkspaceIndex { version, ordered_projects, active_project_id, projects }
```

The names and order belong to the user-facing hierarchy. A `WorkspaceTabId`
names one workspace/server namespace; the `workspace: u64` inside a live
TargetRef remains the existing server-issued runtime identity. IDs are
allocated by a later persistence/owner layer; the pure model receives IDs and
does not generate randomness or access the filesystem.

Persist only versioned index metadata: opaque project/tab IDs, display names,
ordering, and active project/tab IDs. Do not persist project roots or other
filesystem paths, launch cwd/executable/arguments, environment values,
credentials, pane input, scrollback/screen payloads, or provider conversation
data in this index. No automatic project discovery, path-based project
identity, symlink equivalence, sidebar, remote runtime, or worktree behavior is
implied. A project is initially a named grouping. Creating a tab can pass the
invoking client's chosen cwd/environment to its first launch as volatile,
explicit launch context; the index does not save that context. Future default
cwd behavior remains a separate product decision.

## Storage namespace and compatibility

Keep `PANELINK_HOME` as the override for the application-wide private state
root. Do not redefine it as a project root or a per-tab variable. Add a
versioned registry under that root and deterministic ID-based child namespaces:

```text
$PANELINK_HOME/
  workspace-index.v1       # names, IDs, order, active selection only
  server/                  # existing default server; preserve in place
  contexts/                # existing default-workspace client contexts; preserve
  projects/<project-id>/
    workspaces/<tab-id>/
      server/               # one server, journal, layout/, history/ for this tab
      contexts/             # client contexts scoped to this tab/server
```

The exact registry codec and atomic-write protocol are implementation work;
the registry must use the existing private-directory/strict-validation
approach, reject unknown/corrupt versions without destructive repair, and
bound names, record counts, and bytes. The proposed tree stores only Panelink
namespace paths derived from opaque IDs; it does not store source project
paths.

Migration is additive and lazy. On first index creation, expose the current
single workspace as one clearly named default project/tab entry that points to
the existing `$PANELINK_HOME/server/` and `$PANELINK_HOME/contexts/`. Do not
move or rewrite its generation counter, context files, operation journal,
layout snapshots, history, or live PTYs. If registry creation fails, retain
today's single-workspace entry path and report the failure; never initialize a
second server against the legacy storage to recover. New tabs get fresh opaque
IDs, a fresh per-tab directory and fresh server generation on first startup.
Existing CLI invocations without a tab selector continue to address the
legacy default workspace until an explicit CLI routing design is added.

The UI's outer route is `(ProjectId, WorkspaceTabId)` plus the exact existing
TargetRef where a pane target is needed. Route the request to that tab's
endpoint and client context; validate its generation and lease there. Do not
add project/tab IDs to TargetRef or change its wire bytes for this design. A
future single-server/multi-workspace wire protocol would need its own versioned
schema/capability negotiation and must not reinterpret existing messages.

## Runtime and safety invariants

1. Each tab owns one independent server process/storage lock, generation
   counter, launch journal/dedup domain, pane layout, layout writer, history
   writer, and per-tab client-context directory. Equal numeric TargetRefs or
   operation sequences from two servers are never interchangeable; route
   scope includes the selected stable tab ID and endpoint.
2. Tab selection changes only the client attachment and focused view. It does
   not restart the server, change generation, relaunch a child, move a pane, or
   publish a new TargetRef. All tabs' existing PTYs continue running while
   inactive, subject to their own server lifecycle.
3. On attach/switch, negotiate capabilities anew for that server and obtain a
   fresh server-authorized lease. Reconcile that tab's durable operation
   context/status before continuing. Reuse eligible operation keys only with
   the identical payload and target; preserve current server deduplication,
   owner fencing, and lease expiry. An uncertain input/paste/signal is never
   automatically replayed on reconnect or across tabs.
4. New pane launch uses an explicit environment from the invoking client at
   launch time, following the existing environment precedence contract. A
   server's original environment must not leak into a later tab launch. Never
   persist environment or credentials in the index; switching tabs does not
   mutate existing children.
5. Closing a tab is not process termination. **Stop Workspace** is a separate,
   explicit, target-scoped stop of every live pane in that workspace, using
   current stop/close fencing and confirmation. It must not stop another tab
   with equal numeric pane/TargetRef fields. Removing a tab from the registry
   is allowed only after its processes are stopped or an explicit keep-running
   lifecycle/retention record preserves a route to the server; never orphan
   background PTYs.
6. Recovery/reboot enumeration is over registered tabs. Recovered layouts and
   history are read from the matching tab namespace and remain historical;
   they do not mint targets, revive leases, or resume providers. Missing,
   corrupt, or stale tab metadata fails closed and offers explicit repair or
   stop paths without silently attaching to another tab.

## Architecture tradeoffs

| Option | Fit and advantages | Cost and risk | Assessment |
| --- | --- | --- | --- |
| One existing server per workspace tab | Preserves one-layout/four-pane owner, existing generation/lease/dedup model, and server-directory layout/history isolation. Switching is an outer client routing operation. Lowest disturbance to the current UI runtime. | Up to eight server processes per project under the proposed bound; each needs lifecycle/health tracking and a distinct socket/context route. Cross-tab CLI discovery must name a tab before target resolution. | Recommend for first implementation. |
| One server per project with many workspaces in one owner | Fewer server processes; one generation domain and potential atomic project-wide coordination. | Requires replacing the single `layout`/pane-size owner state with per-workspace owners, fencing leases/operations/events per workspace, and making layout/history writers and snapshots namespace-aware. TargetRef.workspace becomes central routing rather than a single current workspace. Broader protocol and recovery changes. | Plausible later consolidation; not the next slice. |
| One project manager plus a server/owner per tab | Allows a stable project-level control plane while retaining per-tab pane owners. | Adds manager availability, lifetime, and protocol authority before a demonstrated need; risks duplicating registry/server responsibilities. | No current need; defer. |

## Exact next implementation slice: pure model only

Implement a dependency-free, side-effect-free `WorkspaceIndexModel` (suggested
new module `src/workspace_index.rs`) before touching default-open, storage,
server, UI client/protocol, clipboard, terminal, or provider code. The model
should take caller-supplied typed IDs and names and support only:

- add/rename/remove project metadata;
- add/rename/remove/reorder workspace tabs within a project;
- select the active project and tab;
- query the selected IDs and ordered tab/project views.

Define explicit errors for duplicate IDs, unknown IDs, empty/invalid/overlong
names, removing a nonempty project, removing the last tab from an active
project, and the proposed eight-tab limit. Model `remove_tab` removes metadata
only and returns the `WorkspaceTabId` for a future lifecycle layer to require
stop/detach handling; it must not imply process shutdown. No paths, sockets,
leases, TargetRefs, launch data, `PANELINK_HOME`, environment, IO, or global
state enter the model. Keep stable ID types distinct from `TargetRef` numeric
fields even if their underlying representation matches.

Pure model acceptance tests for that slice:

1. Build two projects with multiple tabs; select/switch and verify the active
   IDs and each project's order remain independent.
2. Rename and reorder by stable IDs; ensure selected tab identity is preserved
   across reorder and display names do not affect identity.
3. Reject duplicate IDs, unknown project/tab IDs, invalid names, and a ninth
   tab without mutating the prior model.
4. Remove metadata and prove it returns the tab ID while leaving other project
   and tab records unchanged; assert removal has no stop/process side effect
   because the model has no runtime dependency.
5. Verify empty/last-tab policy and project removal errors are deterministic.
6. If a codec is included later, round-trip names/order/selection only and
   reject unknown versions, duplicate IDs, oversized input, and any schema
   fields for paths, env, credentials, launch payloads, history, or TargetRefs.

Do not combine the codec, storage migration, path resolution, or UI integration
with this pure slice. After parent review, a separate storage task can implement
the versioned index and legacy-default mapping, and a separate runtime task can
add tab-specific endpoint/context routing.

## Remaining choices and status

The hierarchy itself is settled. This recommendation does not need more user
input to begin the pure model. Before wiring storage/UI, retain these product
questions for review: whether a project is only a named grouping or has a
user-selected root; what happens when a user requests tab removal while its
server is running; and how the tab chooser is presented. This note proposes
named grouping, keep-running on ordinary tab close, and explicit **Stop
Workspace**; it does not treat those proposals as separately confirmed
requirements. Remote execution, auto-discovery, path/symlink identity,
worktrees, sidebar layout, and project-wide single-server ownership remain
out of scope.
