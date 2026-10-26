# Phase 3 event, watch, launch, and layout contract audit

2026-10-23. Reconciles the broad Phase 3 checklist against the current source,
accepted contracts, and recorded evidence. This is a documentation audit only;
it changes no runtime behavior and does not complete Phase 3.

## Finding

The remaining roadmap item, “Complete event/watch/launch/layout contracts and
implementation; event framing alone does not enable subscriptions, full
snapshot queues or multi-session lifecycle,” is stale as a single acceptance
item. Its original warning was correct when only event framing and the
one-child runtime existed. The current implementation now has bounded pane
events and reconciliation, separate workspace layout invalidations and
reconciliation, finite server-side watches, a four-pane layout lifecycle, and
an explicit managed `pane run` path. Those newer contracts deliberately use
bounded invalidation pages plus authoritative reconciliation; they do not
require a persistent subscription queue or a full snapshot queue.

No remaining requirement from that historical checklist is supported as a
missing accepted runtime requirement. The two-session registry/direct-launch
design in `P3-MANAGED-LAUNCH-PLAN.md` is explicitly a proposal, and the current
`pane run` contract intentionally reuses split plus embedded launch instead of
enabling the standalone launch route. Treating that proposal as a blocker would
turn an unadopted architecture into a new requirement. The broad checkbox can
therefore be retired or replaced by references to the delivered scoped
contracts. This does not close Phase 3 or its independent acceptance gates.

## Current contract to implementation and evidence

| Accepted behavior | Current implementation | Evidence and limit |
| --- | --- | --- |
| Target-scoped pane metadata events use explicit negotiation, bounded event pages, finite waits, and atomic screen/metadata/cursor reconciliation. | `watch_protocol::{WatchRequest,Reconciliation}`; `MultiSessionOwner::{read_events,reconcile}` in `src/server/multi_session.rs`, backed by per-session handling in `src/server.rs`; bounded pending-watch delivery in `src/multi_runtime.rs`; CLI adapter in `src/p3_cli.rs` and `src/agent_cli.rs`. | [P3-WATCH-INTEGRATION.md](P3-WATCH-INTEGRATION.md) records authenticated socketpair/private-PTY proof, 302 passed / 13 ignored at that historical checkpoint, and strict Clippy/build. Indefinite subscriptions, provider waits, and expanded pathname CLI acceptance were not established there. Later current-tree verification is recorded in the newer slice results. |
| Workspace layout events are invalidations; the snapshot and cursor are captured atomically, and clients reconcile after a change or gap. Pages, retained history, pending waits, and bytes are bounded. | `workspace_watch_protocol::{Request,Reconciliation,LayoutEvent,WatchReply}`; `MultiSessionOwner::record_workspace_layout`, `reconcile_workspace`, and `read_workspace_events` in `src/server/multi_session.rs`; `dispatch_workspace_watch` and `resolve_pending_workspace_watch` in `src/multi_runtime.rs`; the agent CLI adapter in `src/agent_cli.rs`. | [P3-LAYOUT-EVENTS-RESULT.md](P3-LAYOUT-EVENTS-RESULT.md) records final 8/8 verifier, Cargo 1,036 passed / 0 failed / 15 ignored, decoder 39, Python 77, strict Clippy/fmt/build, and the real CLI gap-recovery scenario. It explicitly says derived observations may coalesce intermediate layouts and are not a complete lifecycle transcript. |
| Four-pane topology and lifecycle updates are committed through the serialized owner; failed notification bookkeeping does not roll back or stop the process. | `pane_layout::MAX_PANES`; `MultiSessionOwner` split/resize/close and workspace-event publication in `src/server/multi_session.rs`; ordered owner work and event settlement in `src/multi_runtime.rs`. | The layout-events result covers duplicate-split/multiple-exit publication and owner/runtime watch behavior. [P3-FOUR-PANES.md](P3-FOUR-PANES.md) records four-pane implementation and separately leaves Apple Terminal acceptance open. |
| A caller may explicitly create one new managed pane by splitting a pinned existing target, using literal argv and invocation cwd/environment, with durable operation identity and no automatic resend after uncertainty. | `agent_cli::pane_run::run` in `src/agent_cli/pane_run.rs`; `WorkspaceLink::run_pane` in `src/p3_cli.rs`; existing split admission/serialized launch worker through `src/multi_runtime.rs` and `src/launch_worker.rs`. `dispatch_launch` in `src/multi_runtime.rs` still rejects standalone client launch requests; that is outside the accepted `pane run` route. | [P3-PANE-RUN-RESULT.md](P3-PANE-RUN-RESULT.md) records final serial Cargo 1,084 passed / 0 failed / 15 ignored, seven separate non-Cargo checks, and CLI coverage for argv/cwd/environment, target/revision pinning, capacity/refusal, sibling survival, lost acknowledgement, and uncertain-context reopening. Applied means launch ownership/journal completion, not readiness or task success. |

The evidence counts above are snapshots from their own completed slices and
must not be added together or presented as one current-tree run. The latest
combined-tree serial result remains in `ROADMAP.md` and
`docs/progress/SESSION.md`; that result establishes automated software checks,
not the independent host/provider/reboot claims below.

## Proposal boundary and remaining independent gates

`P3-MANAGED-LAUNCH-PLAN.md` identifies itself as an implementation proposal;
it does not authorize source changes or select a hierarchy. Its proposed
two-session standalone launch registry, per-session cursor migration, memory
measurement, and fault matrix were not adopted by the later pane-run contract.
The active implementation uses the existing four-pane workspace and serialized
split worker. `src/multi_runtime.rs::dispatch_launch` retains the explicit
`launch_unsupported` response for a standalone schema-6 launch request. That
does not invalidate the accepted `pane run` contract, which embeds launch in
the existing split operation. No standalone launch route, separate session
registry, indefinite subscriber, or queued full-snapshot delivery is required
by this audit.

Keep these separate, still-open acceptance limits visible:

- Human Apple Terminal four-pane/host behavior remains distinct from automated
  private-PTY/socketpair evidence. The parent explicitly ran the existing ignored
  expanded pathname CLI fixture after this audit:1/1 pass in1.20s; see
  [reconciliation result](P3-RUNTIME-CONTRACT-RESULT.md). Its historical pending
  status is superseded for that bounded CLI scenario only.
- In-flight-request recovery and arbitrary descendant cleanup are not proven
  by attach/server SIGKILL acceptance; no crash-cleanup guarantee is implied.
- Provider identity and recovery remain blocked by their separate Phase 4
  evidence gate.
- Physical reboot, persisted-history recovery, and saved-layout behavior stay
  under their separate Phase 5 contracts.
- Prior test intermittency and ignored tests retain their recorded status; no
  individual result here converts them into passes.

These limits belong to the independent Phase 3/4/5 status and acceptance
records. They do not identify an unfinished event/watch/launch/layout
implementation requirement. Preserve the separate Phase 3 host-lifetime
checkbox rather than using this audit to close it.

## Roadmap disposition

Replace the broad checkbox with links to the three current scope records
([workspace notifications](P3-LAYOUT-EVENTS-RESULT.md), [managed pane run](P3-PANE-RUN-RESULT.md), and [combined Phase 3 status](SESSION.md)), or mark the historical umbrella item superseded. Do not claim Phase 3 complete: the separate host-lifetime/acceptance item and the explicit limits above remain open. No new runtime slice is recommended from this audit because no specific unmet accepted runtime contract was found.

No tests were run for this documentation-only reconciliation. The referenced
test outcomes are taken from their named evidence records, not rerun or
reclassified here.
