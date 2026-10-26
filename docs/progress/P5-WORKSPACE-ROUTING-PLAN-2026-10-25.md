# P5 workspace routing integration plan — 2026-10-25

Status: read-only implementation plan. No runtime code, build, test, or host
interaction was performed for this plan. The next independent slice should
route a single CLI invocation to one registered workspace before entering the
existing attach client. Project → workspace tabs → split panes is the accepted
hierarchy. Each tab keeps its own existing server, pane layout, history,
operation journal, and client contexts.

## Current implementation state

The pure metadata model, versioned codec, and locked registry store are already
present in this shared checkout (`src/workspace_index.rs`,
`src/workspace_index/codec.rs`, `src/workspace_index/store.rs`). The store's
`initialize_legacy()` creates Project 1 `Default` / tab 1 `Main` and writes
only `workspace-index.v1`; it does not resolve a route, create a tab namespace,
or touch the legacy server. The current default-open path still resolves one
`StateRoot`, starts/reuses its `server/`, and invokes `ui_client::run(server,
contexts)`. The UI `Net` connects to one server and resumes context files only
from the supplied context directory. No in-client tab switch exists.

The indexed graph was refreshed once but still contains only 39 nodes and no
`src/` symbols. Source pointers below were checked directly. The worktree has
other dirty runtime and design changes; keep this slice exclusive to the new
plan until separately assigned.

## Proposed route and CLI contract

Add a transient `WorkspaceRoute` resolved from stable `(ProjectId,
WorkspaceTabId)` metadata. It contains the exact server directory, UI context
directory, per-workspace CLI context file, and the IDs for UI ownership. It
does not contain or alter `TargetRef`; once connected, a pane request still
uses the exact TargetRef issued by that routed server.

Keep the no-selector behavior backward compatible: `panelink` and existing
commands such as `panelink stop`, `panes`, `close`, `layout`, and `history`
continue using the legacy default paths. The registry is lazily initialized
additively when a workspace-aware command first needs it; Project 1 / tab 1
routes directly to the existing `$PANELINK_HOME/server/` and
`$PANELINK_HOME/contexts/`. A missing registry may be initialized; a corrupt,
unknown-version, locked, or unsafe registry must fail closed with a fixed
diagnostic and must never cause a second server to start at the legacy path.

Proposed opt-in syntax, consistent with the current nested command parser:

```text
panelink workspace list
panelink workspace project create <project-name> <initial-tab-name>
panelink workspace tab create <project-id> <tab-name>
panelink workspace select <project-id> <tab-id>
panelink workspace open [<project-id> <tab-id>]
panelink workspace stop <project-id> <tab-id>
panelink workspace remove <project-id> <tab-id>
```

`workspace open` without IDs opens the persisted active project/tab; with both
IDs it opens exactly that route and does not silently substitute the active
selection on lookup or server errors. `workspace select` only changes active
metadata; it neither starts nor stops a server. `project create` creates its
initial tab. Newly allocated IDs come from `MetadataRecord::next_id()` and are
committed with the next revision; do not derive IDs from names or paths.
`workspace list` is read-only and prints IDs/names/order/active markers only.

The legacy `stop` remains the default-workspace stop. `workspace stop` applies
the existing verified PID + process-start-time check under that route's
lifecycle lock. `workspace remove` is refused while the tab server lock is
held or a live endpoint/PID is verified; removal is metadata-only after the
workspace is stopped. Closing/detaching an attach client is never a remove or
stop operation. Retain the model's active-project/last-tab rules.

## Minimal next implementation slice and owned files

Implement route selection and CLI management first; do not implement the
interactive tab bar or hot switching in this slice.

1. **Resolve route safely** — own `src/default_open.rs`. Extend the state
   description so the application root (registry and shared terminfo) is
   distinct from each workspace runtime root. Preserve the legacy `StateRoot`
   mapping byte-for-byte for project 1/tab 1. Derive new namespaces only from
   validated numeric opaque IDs:

   ```text
   $PANELINK_HOME/projects/<project-id>/workspaces/<tab-id>/server/
   $PANELINK_HOME/projects/<project-id>/workspaces/<tab-id>/contexts/
   ```

   Create each missing directory in order, validate ownership/mode/no-symlink
   at every new component, and check the resulting control socket path against
   the existing macOS `sun_path` limit before creating or starting anything.
   Use the shared application `terminfo/`; keep server lock, generation,
   journal, layout/history, lifecycle lock, PID record, and logs in the
   selected workspace namespace. Never store source-project roots, cwd, argv,
   environment, credentials, or pane content in the index.

2. **Route-aware open and stop/remove** — continue in `src/default_open.rs`.
   Route default metadata to existing root paths, and nondefault IDs to their
   fresh namespace. Open/prep the selected route, compile/use shared terminfo,
   ensure that route's server, and pass its route object into the attach
   client. Stop only that route. Remove only after checking stopped state under
   the per-workspace lifecycle lock; a failed/ambiguous liveness check refuses
   removal. Never stop a matching pane number or generation from another
   server.

3. **CLI selection/management** — own `src/main.rs` plus parser tests there.
   Dispatch the `workspace` command before the existing no-argument open and
   legacy command branches. Parse bounded UTF-8 names and nonzero decimal IDs;
   reject partial ID pairs, extra args, unknown commands, and lossy numeric
   conversions. `list`, `select`, `project create`, `tab create`, `open`,
   `stop`, and `remove` use the store's load/revision/save API. Keep errors
   redacted and list output free of paths/environment values. Existing CLI
   forms stay on the default route until a later explicit selector is designed
   for each existing command.

4. **Attach boundary seam** — own `src/ui_client.rs` and
   `src/ui_client/host.rs` only to replace the two raw `PathBuf` arguments
   with an immutable `AttachedWorkspaceRoute` carrying project/tab IDs and the
   route's server/context paths. `Net::connect` still connects exactly once to
   that route for this slice. Make the host/event-loop ownership boundary
   explicit so a future tab command can detach the old worker and construct a
   new `Net` for another route. Do not add tab UI, keyboard shortcuts, or
   preserve view/cache state across a switch now.

5. **CLI context fencing** — own the route-building code in
   `src/default_open.rs`; use a per-tab CLI context path for new workspaces.
   Keep the existing root `cli.ctx` as the legacy mapping. UI pane contexts
   remain in the selected `contexts/` directory. Thus a tab's lease, operation
   sequence, uncertain operation, and revision cannot be resumed through a
   different tab. `Net::connect`/`PaneConn` must retain their current exact
   context reconciliation: status-check an uncertain operation and refuse
   while unresolved; never replay input, paste, signal, or launch on attach.
   Do not fall back to another tab if a context is stale or uncertain.

Do not edit `src/workspace_index.rs`, its codec/store, `src/control_protocol.rs`,
`TargetRef`, server wire schemas, `src/multi_runtime.rs`, terminal selection,
clipboard code, history/layout codecs, or provider recovery in this slice.

## UI switching seam and background lifetime

This invocation-level router allows separate `panelink workspace open` calls;
it is not an in-place switch. The route object is the future switch seam:
switching must dispose of the old client attachment, clear pane focus,
selection, screen/history caches, pending resize and menu state, then connect
with the new route's context directory and negotiate a fresh lease/capabilities.
No old route's lease, sequence, uncertainty, TargetRef, or cached pane can
cross that boundary. If detach outcome is uncertain, it cannot authorize any
operation on the new server.

Servers are detached with `setsid` and outlive the client today. Preserve that
per-tab lifetime: opening or closing the client does not stop inactive tabs.
Each namespace's lifecycle lock serializes its own ensure/stop/remove checks;
each server lock remains held for server lifetime. A tab with a live server
must remain indexed and routable. Registry errors or an ambiguous stop leave
the metadata in place and refuse removal. Cross-tab input, automatic cleanup,
worktree creation, and changed TargetRef identity are not introduced.

## Acceptance for the implementation task

Add focused coverage in a new `tests/p5_workspace_routing.rs` and parser tests
under `src/main.rs` for:

1. Lazy default registration selects Project 1/tab 1 and reuses the exact
   legacy server and context directories without moving, rewriting, or
   replacing legacy generation, context, history, layout, socket, PID, or
   running process state.
2. Two nondefault tabs resolve to different server/context/CLI-context
   namespaces, shared terminfo, and independent lifecycle/PID/log paths.
   Equal numeric TargetRefs in the test fixtures never make routes equal.
3. `workspace select` persists stable IDs/order/selection; explicit `open`
   chooses the exact IDs; missing IDs, corrupt/unknown registry, lock conflict,
   unsafe directories, socket path overflow, or failed startup never falls
   back to the default workspace.
4. A no-selector invocation still takes its current legacy path. Existing CLI
   commands retain their present grammar and default target.
5. A running workspace cannot be removed, including a matching endpoint with
   an ambiguous PID/start identity. A stopped workspace can be removed only
   after its per-route lifecycle lock and server lock checks; removal does not
   signal, close, or clean another route.
6. Context uncertainty is isolated: unresolved uncertainty in tab A blocks
   that tab's operation, while opening tab B neither reads nor clears A's
   context; no input or launch is retransmitted during route selection.
7. Attach receives the selected route's server/context paths; detach leaves
   its server running. The route type carries IDs separately from TargetRef.

Suggested parent-run commands after implementation (serially, using the
existing `/private/tmp/panelink-p4-bridge-store-target` target directory):

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --test p5_workspace_routing -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --bin panelink workspace -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo fmt --check
```

Run only after the active shared-tree compile errors are resolved by their
owners. Then use the repository verifier and record each command/result
separately; no result from the earlier codec/store component slices proves
this routing integration. No Cargo command was run for this plan.

## Routine engineering versus approval

Routine implementation choices: opaque numeric IDs already defined by the
model; initialize legacy metadata additively; keep shared terminfo; use the
existing private-directory, lifecycle-lock, server-lock, PID/start-time,
strict context, and redacted-error contracts; reject removal unless stopped;
keep no-selector invocations on the old default route. The named-project
grouping is a prototype assumption only: this routing stores no project root
and performs no filesystem project discovery.

Product approval remains needed before changing the no-selector default to
follow active selection, adding an in-client tab strip/shortcut or choosing
its presentation, persisting a project root/cwd, allowing removal of a running
tab, or introducing automatic workspace/server cleanup. This first slice does
none of those. Removing a workspace is stopped-only; ordinary client detach
keeps its PTYs running.

## Source pointers checked

- [`default_open.rs`](../../src/default_open.rs#L89) defines the current
  `StateRoot`; `resolve_root`, `prepare_root`, `spawn_server`, `ensure_server`,
  `open`, `open_workspace`, `stop_inner`, and `running_workspace` are the
  root-resolution, private preparation, detached lifetime, and CLI routing
  seams.
- [`main.rs`](../../src/main.rs#L101) dispatches no-argument open and current
  `stop`/`panes`/`close`/`history`/`layout` grammar.
- [`ui_client.rs`](../../src/ui_client.rs#L21),
  [`host.rs`](../../src/ui_client/host.rs#L252), and
  [`net.rs`](../../src/ui_client/net.rs#L107) pass and consume one server and
  one context directory per process.
- [`conn.rs`](../../src/ui_client/conn.rs#L299) and
  [`client_context.rs`](../../src/client_context.rs#L297) resume/reconcile
  uncertain operations and durably reserve operation keys before effects.
- [`workspace_index.rs`](../../src/workspace_index.rs#L164) provides stable
  IDs, selection, metadata mutation, and last-tab rules;
  [`store.rs`](../../src/workspace_index/store.rs#L289) provides load,
  additive legacy initialization, and revisioned save.
- [`P5-WORKSPACE-TABS-DESIGN-2026-10-25.md`](P5-WORKSPACE-TABS-DESIGN-2026-10-25.md)
  records the selected hierarchy, one-server-per-tab direction, and namespace
  invariants. The codec/store artifacts describe their current scope and do
  not claim this routing is already implemented.
