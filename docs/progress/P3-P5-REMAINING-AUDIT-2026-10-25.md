# Phase 3 / Phase 5 remaining-work audit — 2026-10-25

Read-only review of the current `codex/p3-scrollback-integration` checkout.
Only this audit note was added. No Cargo/build/test/verifier command, host or
clipboard interaction, activation, configuration change, reboot, or Git
publication was performed. Phase status and canonical gates remain owned by
the parent.

## Ranked next work

1. **Finish the parent's current source-frozen verification.** The current
   keyboard-copy focused checks are 108 client and 25 compositor tests; the
   clipboard-free private-PTY test is 2/0 with strict child clipboard policy.
   These are the current copy-slice evidence. The older workspace-tab full
   run (1,344 passed, 15 ignored) predates copy-mode edits and must not be
   presented as the current full-tree result. The full current serial run and
   supporting checks are pending in the handoff. Continue that run once, record
   any failure without waiving it, and apply only source-frozen fixes that it
   demonstrates are needed. The real OS clipboard was not used. See
   [SESSION.md](SESSION.md), [tab integration](P5-TAB-INTEGRATION-2026-10-25.md),
   and [keyboard-copy result](P5-KEYBOARD-COPY-RESULT-2026-10-25.md).

2. **Reconcile the remaining Phase 5 checkbox with the accepted M2 outcome
   before opening a new UI task.** The new workspace CLI and in-client project
   and tab switcher now cover workspace selection. Existing pane split, focus,
   close/reflow, viewport resize, and the new keyboard copy/selection slices
   are implemented and have automated evidence at their recorded scopes.
   [ROADMAP.md](../../ROADMAP.md)'s broad “remaining keyboard
   selection/layout operations” text no longer accurately describes the
   delivered tab/copy functionality. The implementation is in the
   [workspace CLI](../../src/workspace_cli.rs),
   [tab/client routing](../../src/ui_client/state.rs),
   [copy key map](../../src/ui_client/keys.rs), and
   [selection client](../../src/ui_client/selection.rs); acceptance lives in
   [workspace tabs](../../tests/p5_workspace_tabs.rs) and
   [selection extraction](../../tests/p5_terminal_selection.rs).
   [SPEC.md](../SPEC.md) M2 does state the outcome that multiple processes keep running
   while switching, moving, resizing, and zooming panes. The implementation
   details for pane movement, interactive resizing, zoom, menu bindings, and
   ratios remain in the [PANE-LAYOUT.md](../PANE-LAYOUT.md) proposal; do not
   infer a new binding or mouse/drag contract from it. Parent should decide
   whether those M2 outcomes are still in Phase 5's required completion scope,
   then either define a bounded task/acceptance for the missing outcome(s) or
   narrow the canonical gate with a documented rationale. No specific missing
   runtime defect was established by this read-only review.

3. **Recheck the stale pathname-startup blocker with a real fixture if it is
   still needed for Phase 5 closure.** `P5-TEMP-CLEANUP.md` records the
   startup helper ordering test and cleanup/store CLI cases, but its note that
   full pathname runtime acceptance is blocked by listener permission reflects
   the old environment. Current environment permissions differ. The current
   workspace-routing CLI fixture proves a different path; it does not prove
   history cleanup through actual server startup before capture is enabled.
   The smallest useful next task is a disposable private-root pathname startup
   acceptance that seeds an orphan history temp, starts the actual runtime,
   and proves cleanup completed before history capture starts, including a
   refusal case for unsafe metadata/temp state. Reuse existing source and
   fixtures before adding a test; do not change cleanup semantics unless the
   fresh run exposes a defect. The candidate command, if a new dedicated test
   is needed, is
   `cargo test --locked --offline --test p5_history_cleanup_startup -- --test-threads=1`.
   See [temporary cleanup](P5-TEMP-CLEANUP.md),
   [workspace routing](P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md), and
   [restart acceptance](P5-RESTART-ACCEPTANCE.md).

4. **Keep the remaining acceptance gates separate.** The new
   [composite recovery test](P5-COMPOSITE-RECOVERY-2026-10-25.md) now passes
   orderly process/store restart, read-only inspection, and explicit fresh
   topology relaunch with historical links and no input replay. It does not
   establish physical reboot or power-loss durability. If useful, an abrupt
   server-death version is a bounded automated follow-up, but it is not a
   substitute for the physical-reboot gate. Representative history workload
   acceptance still needs user-approved measurable loss/lag/error/latency
   budgets before more benchmark runs can be called a pass. The
   [live-history benchmark](P5-LIVE-HISTORY-BENCH.md) preserves burst failures
   and gaps; no new budget or performance claim is made here.

## Explicit host gates and existing automated commands

- **Phase 3:** No new engineering gap was found. The current explicit Phase 3
  gate remains human four-pane acceptance in Apple Terminal. The repeatable
  proxy is `cargo test --locked --offline --test p3_attach_four -- --test-threads=1`;
  the prior audit records 2/2 current tests passing on this tree. This proxy
  does not close the human host gate. Follow the steps in
  [P3 resume audit](P3-RESUME-AUDIT-2026-10-25.md).
- **Workspace routing:** The already documented focused commands are
  `cargo test --locked --offline --test p5_workspace_routing -- --test-threads=1`
  and `cargo test --locked --offline --bin panelink workspace_cli::tests -- --test-threads=1`.
  Current final routing evidence and its limits are in
  [workspace routing result](P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md).
- **Copy/selection:** Current focused and private-PTY results are recorded in
  the handoff and copy result. Real Apple Terminal key/mouse delivery,
  mouse-reporting pass-through, actual clipboard contents/error handling,
  and restoration behavior still need host acceptance. Private fake sinks and
  clipboard-free PTYs are not real clipboard proof.
- **Recovery/workload:** Physical reboot and representative workload remain
  distinct open gates; process restart evidence does not replace either.

## Scope distinctions

The confirmed hierarchy is **Project → workspace tabs → split panes**. The
opt-in routing CLI and in-client tab switcher are implemented. The bounded
keyboard copy shortcut is documented as a prototype assumption. These are
delivered slices, not proof that every M2/M3/M4 milestone is complete.

Other proposed sidebar presentation, shortcut configuration, arbitrary
history scrolling/search from copy mode, pane move/zoom interaction details,
divider dragging, and mouse-menu behavior should not be silently promoted to
requirements. Conversely, the M2 milestone outcome about continued processes
while switching/moving/resizing/zooming should not be dismissed as mere
proposal text. The parent must reconcile that outcome against the current
Phase 5 gate before claiming completion.

No Phase 3 or Phase 5 completion, commit, push, activation, clipboard access,
or physical reboot is claimed.
