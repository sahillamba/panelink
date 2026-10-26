# Independent review: workspace layout event owner and runtime — 2026-10-23

## Scope and result

Reviewed the accepted contract in `P3-LAYOUT-EVENTS-TASK.md`, the capability-mask change in `server.rs`, workspace journal ownership and event hooks in `server/multi_session.rs`, and schema-11 request/reply handling plus pending-watch lifecycle in `multi_runtime.rs`. This was a read-only review; no source or test files were changed. I found no actionable correctness or authorization defect in the reviewed paths.

## Contract checks

- **Capability negotiation and resume.** Bit 8 is independently defined in `handshake_protocol.rs:34-39`; the generic single-session `GlobalAuthority::connect` keeps the legacy mask without bit 8 (`server.rs:743-754`). The multi-session owner supplies bit 8 only when workspace tracking is available (`server/multi_session.rs:1492-1515`). Fresh grants store the selected bits, while resumed grants negotiate against the prior grant intersected with the current availability and preserve the prior grant (`server.rs:774-818`). `require_live_connection` enforces expiry and that the connection selected no capability outside its grant (`server.rs:848-865`). No resume escalation path was found.
- **Workspace authorization and grant lifetime.** `require_workspace_authority` checks a live grant, Read plus bit 8, exact server generation, and that the grant's target belongs to the requested workspace; it does not require the originally named pane to remain in the registry (`server/multi_session.rs:532-567`). The lease expiry is checked through `require_live_connection`, and the workspace journal's workspace is checked separately. Pane Events alone do not pass this gate.
- **Mutation ordering and empty transition.** Initial layout establishment, both committed split paths, root resize and pane-exit collapse call `record_workspace_layout` after layout commit (`server/multi_session.rs:1068-1089,913-937,1227-1239,1298-1304,2785-2790`). Failed or duplicate mutations return before those successful commit hooks; a retained split duplicate is covered by a test asserting its cursor does not advance (`server/multi_session.rs:5844-5890`). `record_workspace_layout` deduplicates `(revision, empty)`, checks every target's generation/workspace, and disables only notifications on append failure (`server/multi_session.rs:492-530`). Poll records after exit observation/collapse (`server/multi_session.rs:1817-1854`); the last exited pane remains as a tombstone, allowing a same-revision `empty=true` event. Reconciliation returns `layout=None` with the last revision when terminal-empty, without synthesizing historical targets (`server/multi_session.rs:572-616`).
- **Snapshot/cursor atomicity.** Both are copied in the owner method after authority checks in one owner turn (`server/multi_session.rs:570-616`). Owner-serialized event reads either return a consecutive retained page, a `ResyncRequired` hint at the current cursor for expiry, or fail closed for a future cursor/wrong scope (`server/multi_session.rs:621-661`). Mutation and reconciliation cannot interleave inside the owner call.
- **Pending waits and stop behavior.** Workspace and pane pending watches share the same capacity count and admission check (`multi_runtime.rs:736-739,767-797`). The driver rechecks pending workspace watches on ticks and settles them during stop/drop (`multi_runtime.rs:698-713,1054-1077,433-455`). Positive waits are retained outside the owner turn; an empty zero-wait poll returns immediately (`multi_runtime.rs:1984-2058`). The request worker requires Read plus bit 8 before forwarding (`multi_runtime.rs:2329-2337`).
- **Response validation before write.** The worker validates the full workspace reconciliation scope/revision/layout targets and watch scope, limit, consecutive page after the requested cursor, terminal status, and cursor relation before writing one `Response` (`multi_runtime.rs:2172-2223,2362-2378`). Reconciliation and watch payloads are bounded by the codec before frame construction. No `Event` frames are introduced.
- **Journal failure behavior.** A failed append marks workspace notifications unavailable without rolling back committed layout or altering process state (`server/multi_session.rs:490-530`). Later reconciliation/watch calls fail with the fixed notification-unavailable error; ordinary layout observation remains independent, as tested in `server/multi_session.rs:3431-3437`.

## Remaining verification boundary

This review did not run Cargo or exercise the host/socket test. The canonical task still requires the parent integration sweep, including real socketpair pending-watch stop behavior and broader pane watch/input nonreplay regression. Current source includes a private socketpair/PTY test for workspace scope, finite watch, capability denial and concurrent layout work (`tests/p3_workspace_events.rs:223-295`), plus owner tests for commit, empty transition, scope, expiry and future-cursor behavior (`server/multi_session.rs:3266-3438`). Those existing tests are evidence present in the tree, not results of this review.

## Final CLI review — 2026-10-23

Reviewed the current `agent_cli.rs` and `p3_cli.rs` changes against the
accepted CLI contract. I found no actionable correctness or security defect in
the reviewed paths.

- The workspace observer uses a separate context file and connection and only
  offers bit 8 from `open_workspace_events` (`p3_cli.rs:2151-2160,2162-2235`).
  Resumption repeats the optional offer but `validate_welcome` and the server's
  immutable lease grant determine selected capabilities; workspace authority
  is checked against generation/workspace before every operation
  (`p3_cli.rs:887-932,2295-2375`).
- Reconciliation validates the returned workspace scope, nonzero revision,
  layout revision and every occupied pane namespace. Watch decoding validates
  bounded reply structure; the client additionally checks response scope,
  requested page consecutiveness, limit, and status-specific cursor movement
  before returning a page (`p3_cli.rs:2295-2375`).
- The CLI starts from an atomic snapshot/cursor, retains the watch page cursor
  after `Changed` rather than adopting a potentially newer reconciliation
  cursor, and only resets to the atomic reconciliation cursor on
  `ResyncRequired` (`agent_cli.rs:1347-1415,1424-1452`). Last-page empty
  handling closes the previously observed panes if the runtime has already
  retired the route; an empty reconciliation also terminates cleanly
  (`agent_cli.rs:1424-1489`).
- `--pane N` is converted once to the initial full `TargetRef`; workspace
  snapshots and future polling compare that identity, so a replacement at
  index N is not silently adopted (`agent_cli.rs:1323-1335,1362-1365,
  1453-1463,1564-1591`). Spatial indices are refreshed for retained target
  cursors after topology changes (`agent_cli.rs:1491-1505`).
- Pane metadata watch batches are polled with zero wait alongside the
  workspace watch, with resync reconciling the pane cursor before continuing
  (`agent_cli.rs:1508-1525`). The compatibility fallback retains the prior
  target-scoped polling and applies the same pinned-target identity filter
  (`agent_cli.rs:1530-1656`).

Known bounded verification limits remain those identified by the parent:
workspace observation does not proactively renew its 30-minute lease, and the
CLI gap branch has no end-to-end simulation. I did not run Cargo or modify
source. Review discovery used the already-ready codebase-memory graph first
(`index_status`, then `search_graph` for workspace watch/reconciliation and
CLI cursor/pane paths); source evidence came from `git diff -- src/agent_cli.rs
src/p3_cli.rs`, `rg -n`, and `nl -ba` reads of those files plus the canonical
task and owner implementation.

## Discovery performed — owner/runtime review

Codebase-memory `index_status` reported project `Users-sahillamba-.codex-worktrees-6730-panelink` ready at 11,272 nodes / 52,968 edges. Graph-first discovery used `search_graph` for `require_workspace_authority`, `reconcile_workspace`, `read_workspace_events`, and capability/resume symbols, plus a semantic search for workspace watches, cursors and layout snapshots. Source inspection then used `rg -n`, `nl -ba`, and `git diff --check` over the listed files. The graph was a navigation aid; reviewed source lines are authoritative.


## Subsequent parent acceptance update

The previously recorded missing CLI gap simulation is now covered by
`real_cli_workspace_watch_gap_recovers_after_ring_overflow`: a real private
server and CLI, observed readiness, a paused test-only observer, 70 committed
resizes and exactly one `layout_resync` before the quiet deadline. The worker
reports 4/4 for the full CLI target; parent final combined verification is in
[P3-LAYOUT-EVENTS-RESULT.md](P3-LAYOUT-EVENTS-RESULT.md). The 30-minute lease
renewal limitation remains. This addendum is parent evidence, not an expansion
of the earlier read-only review's test claims.
