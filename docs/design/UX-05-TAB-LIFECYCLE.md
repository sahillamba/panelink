# UX-05 — tab lifecycle and persistence design

Status: **approved by the parent orchestrator on 2026-10-27 with the amendments
below**; implementation in progress. This specifies UX-05 boundaries, not shipped
behavior or UX-06 controls.

## Parent review amendments (binding)

1. Persistence choice approved: v1 stays the sole catalog; `workspace-tabs.v1`
   sidecar only hides reserved `(1,1)`; closing a non-reserved tab removes its v1
   entry only after verified stop. History/layout files are never deleted.
2. `ExpectedRuntimeIdentity` is simplified to what the client actually knows:
   `{ route: WorkspaceRouteId, generation: Option<u64> }` (the server generation
   from the attached `TargetRef`). PID/start-time checks stay internal to the
   adapter (existing PID-record verification under the route lock). A generation
   mismatch with a live server returns `Partial { reason: "tab was replaced" }`
   and stops nothing.
3. Closing the last *visible* tab never spawns anything and never removes `(1,1)`.
4. `ProjectLaunchContext.cwd` is the absolute cwd of the attached client process
   at startup (captured once, transient, never persisted).
5. Each worker task keeps the canonical verifier green for its own files
   (fmt, strict clippy, its tests); parent runs the full serial verifier.

## Authority and source baseline

- **Decided by the user / UI-UX plan:** visible tabs; `+` creates and selects one
  managed shell; rename is metadata-only; close stops the exact route; close keeps
  history; no replacement shell after the final close; ordinary `panelink` enters
  tabs; raw attach stays headerless; eight tabs/project remains the limit.
- **Proposed here:** retain v1 as the sole catalog, use a 32-byte sidecar only to
  hide reserved `(1,1)`, and remove other closed tab entries after verified stop.
  API names, migration and task split below need parent approval.
- **Source checked:** `workspace_index.rs` enforces 32 projects/eight tabs, stable
  IDs and a selected registered route; v1 caps at 23,352 bytes and disallows an
  empty active project. Store save is revision-CAS and temp+sync+rename durable.
  CLI protects `(1,1)`; stop/removal use route locks. Plain startup calls
  `default_open::open`; raw attach calls `ui_client::run`; the current strip
  requires a concrete route. Checked files: `src/workspace_index.rs`,
  `src/workspace_index/{codec,store}.rs`, `src/workspace_cli.rs`,
  `src/default_open.rs`, `src/main.rs`, `src/ui_client/{routes,host,state}.rs`,
  `src/workspace_tabs.rs`.

## Persistence decision

**Proposed choice: v1 catalog plus a single reserved-route hidden sidecar.**
In v1, every registered non-reserved tab is open/presented. Closing one removes
its tab metadata only after stop verification. `(1,1)` is never removed; the
sidecar's `hidden` bit controls only whether that route appears in the tab UI.
The effective `TabPresentationSnapshot` carries
`active_project: Option<ProjectId>` and `active_route: Option<WorkspaceRouteId>`;
both are `None` when no visible route remains, even though the compatibility
catalog still has its required active `(1,1)` selection.

Alternatives considered:

- **Option A: v2 plus v1 compatibility projection — reject.** Dual catalogs,
  cross-file ordering/reconciliation and downgrade projection add risk; old
  binaries can still see stale projection.
- **v1 plus closed-ID sidecar for every tab — not chosen.** Retains names/IDs,
  but duplicates up to 256 states, closed slots consume the eight-tab cap, and
  every open path needs filtering.
- **v1 removes closed tabs; sidecar hides only `(1,1)` — choose.** Reuses v1
  CAS/atomic writer, fixed-size state, retained legacy registration, and empty
  UI from zero visible routes. Closed metadata is not reopenable; history/layout
  files remain untouched.

### Sidecar wire and store contract

Filename: `workspace-tabs.v1`, in the private app root under the workspace-index
lock. Exact canonical big-endian 32-byte record:

| Offset | Width | Value |
| ---: | ---: | --- |
| 0 | 8 | magic `PLWKHT01` |
| 8 | 2 | version `1` |
| 10 | 2 | header length `32` |
| 12 | 4 | total length `32` |
| 16 | 8 | nonzero sidecar revision |
| 24 | 1 | `hidden`: exactly `0` or `1` |
| 25 | 7 | reserved zero bytes |

No paths, names, environment, credentials, input or process claims are stored.
The sole state is whether registered route `(1,1)` is omitted from tab
presentation. Maximum size is 32 bytes. Reject wrong magic/version/length,
zero revision, noncanonical flag or nonzero reserved bytes. Writes use a private
same-directory temp, file sync, identity/content recheck, atomic rename and
directory sync, following `WorkspaceIndexStore` safety rules. `save_hidden` is
CAS on `expected_sidecar_revision`; catalog edits use the existing expected v1
revision and `revision + 1` CAS. No new dependencies.

Absent sidecar means legacy migration: all v1 entries are visible and
`(1,1)` is not hidden. Initialize revision 1 atomically; never rewrite v1 just
to migrate. Corrupt/unsupported sidecar fails closed with a fixed error; do not
silently treat a possibly hidden legacy route as open. Older binaries ignore the
sidecar and still parse v1. On downgrade they may show/start `(1,1)` despite its
hidden bit; retain the sidecar and document that compatibility limit. Never
remove history/layout directories as part of tab metadata removal.

### Close ordering and crash model

One adapter acquires the route lifecycle lock, checks exact route/runtime
identity, stops and verifies server-lock release plus endpoint absence. **Only
then** CAS-remove a non-reserved v1 entry or CAS-set `hidden=1` for `(1,1)`,
holding the lifecycle lock through commit. Lock order is route lock then registry
lock, matching startup registration checks. Never clear history/layout.

Crash between verified stop and commit leaves the tab visible/registered with
history intact; it is not reported closed. No durable close intent precedes stop,
so next ordinary open may follow existing startup behavior for that entry. This
is an explicit failure window, not exactly-once close. Commit error returns
`Partial`; reconcile that route/revision and never signal a replacement. Timeout
or inconsistent endpoint returns `Partial`/`Unknown`, leaves metadata unchanged,
and keeps the tab visible.

Non-reserved last-tab removal must first select another valid registered route
in the same v1 mutation; use deterministic adjacent visible route, else hidden
`(1,1)` as the compatibility selection. Empty projects stay inactive. For
reserved close, only the sidecar changes. Selection of `(1,1)` from explicit
legacy CLI remains possible regardless of its presentation hidden bit.

## Lifecycle adapter API (proposed Rust surface)

These blocking functions run on the client metadata/lifecycle worker, not the UI
event loop. IDs are stable project/tab IDs, not names.

```rust
pub(crate) struct CreateIntentId(u64); // monotonic within one RouteController
pub(crate) struct ProjectLaunchContext { pub cwd: PathBuf, pub context: LaunchContext }
pub(crate) enum CreateStatus { Ready, Unknown }
pub(crate) struct CreateShellTab { pub route: WorkspaceRouteId, pub name: String,
                                  pub status: CreateStatus }
pub(crate) enum RenameOutcome { Saved { revision: u64 },
    Conflict { current_name: Option<String>, revision: u64 } }
pub(crate) enum CloseOutcome { Stopped, Partial { reason: &'static str },
                               Unknown { reason: &'static str } }
pub(crate) struct ExpectedRuntimeIdentity { pub route: WorkspaceRouteId,
    pub pid: Option<i32>, pub process_start_time: Option<u64>,
    pub endpoint_identity: Option<(u64, u64)> }

fn create_shell_tab(project: ProjectId, intent: CreateIntentId,
                    launch: &ProjectLaunchContext)
    -> Result<CreateShellTab, LifecycleError>;
fn rename_tab(route: WorkspaceRouteId, name: &str, expected_revision: u64)
    -> Result<RenameOutcome, LifecycleError>;
fn close_tab(route: WorkspaceRouteId, expected: &ExpectedRuntimeIdentity)
    -> CloseOutcome;
```

One intent is admitted at a time. Same-ID retry reuses candidate route/name and
exact v1 bytes; reject another intent while uncertain. Re-read v1 and reconcile
only that route under lifecycle locking; reuse existing readiness, never allocate
again or replay input. Across process restart, resolve the committed route as a
normal catalog entry; never replay the create request. Return `Ready` only after
route readiness, else `Unknown` with the same route/name. The ready payload is
the requested `(route, name)` pair.

Use existing `default_shell`/`StartupLaunch` with explicit `LaunchContext`, never
terminal text. `ProjectLaunchContext.cwd` is the absolute invocation cwd captured
at CLI startup; no per-project cwd setting exists, so do not invent persisted
configuration. Pass filtered environment transiently, never persist it. An
explicit project profile may override cwd only if separately introduced. Preserve
old-server refusal; never auto-stop/restart or create a second tab on failure.

Name is `Shell N`, smallest positive unused integer in the project (ASCII
case-insensitive across registered names). Rename uses current 1–64 UTF-8-byte
validation; the next create rechecks names. CAS conflict preserves the draft and
returns current revision/name without overwriting.

`close_tab` applies exact PID/start-time/UID checks under the route lock and
verifies lock/endpoint absence. `Stopped` means shutdown is verified and metadata
commit durable. Identity mismatch never signals a replacement; absent runtime is
`Stopped` only after PID/endpoint uncertainty checks pass.

## Startup and client integration contracts

- Change only no-argument dispatch to `default_open::open_tabbed()`. Read/init
  v1+sidecar before route preparation. First-ever startup initializes visible
  `Default/Main` and opens `(1,1)` routed. Existing metadata selects its visible
  route with deterministic fallback. Zero visible routes enters client-only
  empty state; do not call `open_workspace_ids`, start `serve`, or spawn. A new
  explicit `+` intent may create/start one route; preserve recovery decisions.
- Keep `p3_cli::Attach -> ui_client::run` headerless. Preserve `workspace open
  P T`, pane/history commands and legacy `(1,1)` targeting. Reserved stays
  registered; explicit CLI targeting bypasses presentation filtering. Keep
  old-server refusal behavior; no automatic compatibility restart.
- `routes.rs` metadata-worker signatures (proposal):

  ```rust
  fn queue_create(&self, project: ProjectId, intent: CreateIntentId,
                  launch: ProjectLaunchContext) -> Result<(), QueueError>;
  fn queue_rename(&self, route: WorkspaceRouteId, name: String,
                  expected_revision: u64) -> Result<(), QueueError>;
  fn queue_close(&self, route: WorkspaceRouteId,
                 expected: ExpectedRuntimeIdentity) -> Result<(), QueueError>;
  enum MetadataResult { Created(CreateShellTab), Renamed(RenameOutcome),
    Closed(CloseOutcome), Catalog(TabPresentationSnapshot), /* errors */ }
  ```

  Keep one admitted create/close mutation per worker; results carry intent ID,
  exact route and observed revisions. Queue bounds and cancellation remain
  finite; never coalesce destructive operations as if they were refreshes.
- `host.rs`: `PreparedAttachment { route, app, net }` remains exact-route
  handoff; `install_prepared_route(prepared)` accepts matching request ID only.
  Add `PreparedWorkspace::{Attached(PreparedAttachment), Empty(App)}`; empty has
  no `Net` or route-start effect. Drop stale results.
- `state.rs` signatures: `install_workspace_tabs(snapshot:
  TabPresentationSnapshot, current: Option<WorkspaceRouteId>)` accepts empty;
  `App::new_workspace_empty(rows, columns, snapshot)` initializes header-only
  state. Attached constructor still requires a route. Empty has no `Net` worker;
  Detach exits locally.

## Acceptance tests (proposed; no tests run for this design task)

Unit tests use private roots and injected effects:

- `hidden_record_exact_bytes_round_trip_and_bounds`: reject malformed
  magic/version, reserved bytes, hidden flag, length and revision.
- `legacy_v1_without_sidecar_migrates_all_tabs_visible`: atomic retry, v1 bytes
  unchanged. `create_shell_tab_same_intent_reconciles_one_registered_route`:
  uncertain save/readiness retries candidate, not a new ID/spawn.
- `shell_names_skip_casefolded_collisions`: `Shell 1`, `shell 2` -> `Shell 3`.
  `rename_tab_revision_conflict_preserves_current_name`: invalid/unknown route
  also do not write.
- `close_tab_verifies_identity_before_commit`: mismatch/timeout leaves metadata;
  verified stop then CAS commit returns `Stopped`.
  `close_reserved_route_sets_hidden_but_keeps_1_1_registered`: legacy lookup works.
- `eight_tabs_capacity_and_close_releases_slot`: ninth rejected, close frees
  slot and new ID is monotonic. `close_selected_last_project_tab_uses_valid_route`:
  v1 never receives an active empty project.

Private integration fixtures (exact fixture names):

- `workspace_tab_crash_after_stop_before_metadata_commit`: route remains
  registered/visible, process lock+endpoint absent, metadata uncommitted and
  history/layout bytes retained.
- `workspace_tab_last_close_plain_start_is_empty_no_spawn`: all visible tabs
  closed; ordinary startup renders empty and spawn count stays zero.
- `workspace_tab_reserved_hidden_legacy_target_still_opens`: hidden in tabs,
  still registered to explicit legacy CLI.
- `workspace_tab_old_server_no_auto_restart`: report unsupported, same process
  identity and no second server.
- `workspace_tab_create_rename_close_route_identity_and_cleanup`: one shell,
  stable IDs, exact stop, retained history and verified cleanup.

All fixtures use private state roots/processes, bounded deadlines, exact spawn
counts and verified teardown. Never inspect or mutate live user panes.

## Worker split (exclusive ownership; dependency order)

1. **Persistence — first:** own `src/workspace_index.rs`,
   `src/workspace_index/store.rs`, new `src/workspace_index/tab_visibility.rs`,
   `tests/p5_workspace_tab_state.rs`. Implement sidecar codec/store/CAS,
   migration/projection and unit tests; no CLI/client edits.
2. **Lifecycle/startup — after 1:** own `src/default_open.rs`,
   `src/workspace_cli.rs`, `src/main.rs`, new `tests/p5_workspace_lifecycle.rs`.
   Implement adapter, exact-lock close, create/rename CAS, ordinary/raw dispatch
   and private process fixtures; no client files.
3. **Client boundary — after 2:** own `src/ui_client/routes.rs`, `host.rs`,
   `state.rs`, `src/workspace_tabs.rs`, `src/ui_client/state_tests.rs`,
   `tests/p5_workspace_tabs.rs`. Wire bounded requests/results, exact route
   install and empty state; no lifecycle/persistence files. UX-06 owns controls.

Parent owns interface approval, integration/review and serial verification.
Keep UX-05 completion separate from UX-06 controls and host/provider acceptance.
