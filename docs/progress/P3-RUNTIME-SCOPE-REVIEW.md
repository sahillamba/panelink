# Phase 3 runtime scope review — 2026-10-23

Read-only review of the current Phase 3 runtime contracts, implementation and
recorded evidence. It is a scope reconciliation, not phase closure. Full
Phase 3 remains open.

## Finding

The roadmap item “Complete event/watch/launch/layout contracts and
implementation” is too broad and now stale as an implementation gate. The
accepted bounded contracts it names have been delivered: workspace
reconciliation and finite watches, explicit managed-process launch through the
serialized split worker with existing exact-request deduplication, four-pane
layout, and workspace layout notifications. I found no separate, concrete
runtime behavior still required by those accepted contracts that this checkbox
currently captures.

The evidence supports that conclusion at different scopes:

| Contract | Implemented runtime behavior | Evidence and limit |
|---|---|---|
| Pane and workspace watch/reconcile | Target-scoped pane events use finite waits and atomic screen/metadata/cursor reconciliation. Workspace snapshot and cursor are taken in one owner turn; workspace watches share the eight-request bound with pane watches; cursors are scoped; gaps require explicit reconciliation. | Pane watch delivery is recorded in `docs/progress/P3-WATCH-INTEGRATION.md`; workspace behavior is in `MultiSessionOwner::reconcile_workspace` and `read_workspace_events` in `src/server/multi_session.rs`, with client scope checks in `WorkspaceLink::reconcile_workspace` in `src/p3_cli.rs`. `docs/progress/P3-LAYOUT-EVENTS-RESULT.md` records rollover, gap recovery, quiet-watch coexistence, and CLI behavior. Events are bounded invalidations, not a full lifecycle transcript. |
| Explicit launch and dedup | `pane run` captures an exact target/revision and explicit invoking-client argv/cwd/environment, then uses the existing serialized split/launch worker. The client reserves one key before one send; uncertain outcomes are reconciled without an automatic resend. | `src/p3_cli.rs` (`WorkspaceLink::run_pane`) and `src/launch_worker.rs`; acceptance and limits in `docs/progress/P3-PANE-RUN-RESULT.md`. This is managed-process launch, not provider identity or automatic conversation recovery. |
| Four-pane layout | The shared four-pane bound is `pane_layout::MAX_PANES`; split, close/reflow, resize, attach and rendering behavior use the bounded tree. | `src/pane_layout.rs`, `src/multi_runtime.rs`, `src/server/multi_session.rs`; automated proxy evidence is recorded in `docs/progress/P3-FOUR-PANES.md` and `docs/progress/P3-L17-ATTACH4-RESULT.md`. Human Apple Terminal four-pane acceptance remains open. |
| Workspace notifications | Schema 11/capability 8 provides a volatile bounded journal, atomic snapshot/cursor, finite watches, gap/resync and target pinning. Notification failure leaves ordinary layout reads and live processes available. | Contract in `docs/progress/P3-LAYOUT-EVENTS-TASK.md`; implementation and combined verification in `docs/progress/P3-LAYOUT-EVENTS-RESULT.md`. Notifications may coalesce intermediate layouts; they do not promise a complete lifecycle transcript. |

These are bounded implementations with corresponding automated acceptance,
not claims that all host or crash scenarios pass. The roadmap can mark this
implementation checkbox complete while preserving a separate acceptance gate
for evidence that remains pending. Do not use this finding to close Phase 3.

The proposed standalone launch registry in `docs/progress/P3-MANAGED-LAUNCH-PLAN.md`
is not part of the accepted `pane run` route. The current contract deliberately
embeds literal launch data in the existing split operation; the standalone
schema-6 launch route remains unsupported. This proposal boundary does not
leave an unmet implementation requirement in the accepted launch contract.

## Remaining evidence and scope boundaries

- **Human host acceptance:** four-pane Apple Terminal acceptance is explicitly
  pending. Private outer-PTY acceptance is useful automated evidence, not a
  human-host pass.
- **Killed client during an in-flight operation:** the current SIGKILL client
  regression kills the ready attach process and then reattaches; it does not
  kill a client during request execution. Existing lost-ack, socket-loss and
  dedup tests cover other boundaries. This is an acceptance coverage limit;
  this review found no source evidence of a missing deduplication or
  nonreplayability behavior in the accepted launch path.
- **Product decisions and future proposals:** broader workspace/tab hierarchy
  and autonomous scheduling remain unresolved product scope. Provider session
  identity belongs to the separate Phase 4 gate; reboot/history recovery is
  Phase 5. Active-agent load targets and a complete lifecycle transcript are
  not required by the accepted Phase 3 bounded watch contract. None becomes a
  Phase 3 implementation requirement solely because it appears in broader
  proposal text.
- **Server-death limits:** SIGKILL cannot run graceful cleanup or final flush;
  cooperative-shell exit and stale-endpoint/context fencing do not prove
  arbitrary descendant cleanup or in-flight request recovery. See
  `docs/progress/P3-SERVER-DEATH-RESULT.md`.

The closure audit already separates the delivered bounded runtime from
unproved client-death, host, provider, resource-budget, durability and product
scope gates (`docs/progress/P3-CLOSURE-AUDIT.md`). This review found no
additional implementation gap to name. Keep Phase 3 open until its remaining
required acceptance gates and any newly specified bounded runtime requirement
are explicitly resolved; do not invent a requirement from an unchecked broad
checkbox.

## Review method

Read `AGENTS.md`, the current Phase 3 roadmap and `docs/progress/SESSION.md`,
then checked the codebase-memory index before code exploration. The index was
ready and reported tracked working-tree changes; findings were cross-checked
against current source and the detailed task/result records above. No source,
tests, commits, or peer audit files were changed by this review.
