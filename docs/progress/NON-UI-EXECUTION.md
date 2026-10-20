# Continuous non-UI execution — 2026-10-18

The user authorized continued Luna work until the five non-UI milestones below
are finished, with automated tests for new behavior and no manual verification
handoffs. Parent specifies bounded tasks, assigns exclusive files to Luna
xhigh/medium as appropriate, reviews source and independently verifies results.
This is an execution queue, not completion evidence or a change to pending UI
decisions. [ROADMAP.md](../../ROADMAP.md) remains the status entrypoint.

Priority amendment2026-10-18: the user's newer instruction inserts
[Phase3A right-click splits](P3A-RIGHT-CLICK-SPLITS.md) before the previously
queued lifecycle/stop and history work. A split must open a fresh terminal
session on the selected side while preserving the existing session. This
explicit UI slice takes priority over the older non-UI-only queue; remaining
UI choices and all five milestone exit gates stay open.

1. **Launch-safety proof.** Add a bounded complete-descriptor child probe with
   known-inherited positive control and an actual accepted-socket/managed-exec
   race fixture. Preserve source-backed gate/atomic-creation behavior. A socket
   pair or synthetic accept does not certify pathname acceptance. Permission
   failures are blocked evidence, not successful tests.
2. **Managed launch.** Specify and implement a bounded two-session prototype:
   explicit cwd/environment, stable non-reused session IDs, global exact-byte
   command deduplication, lifecycle metadata Intent before effects and honest
   uncertainty after spawn/result failures. Automate duplicate/conflict/lost
   acknowledgment, capacity and storage-fault checks before enabling launch.
3. **Multiple-session controls.** Route read/input/ownership/watch/reconcile
   and explicit stop to complete immutable targets. Settle the proposed cursor
   namespace and stop authority before implementation. Automate cross-session
   isolation, stale identity/owner, cursor resync, slow-consumer and cleanup
   checks. Do not replay input or introduce autonomous dispatch.
4. **OpenCode identity and resume.** Verify the available supported version,
   selected conversation identity and session switches in one project. Bind
   acknowledged durable mapping revisions and test stale/missing identity
   fallback. Mocks alone cannot prove the actual provider integration; missing
   executable, account or required lifecycle evidence remains explicit.
5. **History, recovery and workload.** Implement bounded scrollback and screen
   checkpoints separately from provider conversation state. Test restart and
   retention/failure behavior without replaying terminal side effects. Measure
   the stated ten-agent workload and Panelink resource overhead; distinguish a
   ten-process synthetic load from ten actual OpenCode agents.

Keep known provider, product and minimum-OS choices unresolved until the
applicable task settles them within its scope. UI hierarchy and appearance
await the user's instructions; use the existing scoped prototype assumptions.
The [managed-launch plan](P3-MANAGED-LAUNCH-PLAN.md) remains a proposal until
the parent adopts each affected contract. No new dependency is approved here.

For each assignment record actual commands/results, failures corrected,
limitations and released ownership in a progress note. Run focused checks
during development and the required integrated suite after source integration.
If the environment blocks a host-only check, keep a runnable automated fixture
and report the exact blocker; continue work that does not depend on claiming
that check passed. Never substitute documentation or an ignored result for
executed acceptance. Phase completion and commits retain their existing rules.

The first two owners released the descriptor probe and accepted-socket race.
The full descriptor checks pass; the actual race stopped at bind with EPERM,
so that acceptance remains blocked. The launch codec and OpenCode preflight
were delivered; parent review corrected the codec's required result-target
contract and independently reran all nine Python preflight tests.
The registry, transaction coordinator, scoped watch, provider-binding state and
record codecs, and screen checkpoint codec have been delivered. Real private-PTY
checks cover two-child launch deduplication and post-write-fault child usability,
normal/alternate safe checkpoints, and a ten-shell synthetic workload. The parent
full locked/offline suite passed 400 tests with 14 ignored. These component
results do not complete all five milestones or enable launch/control endpoints.

Xhigh owners reached a usage limit after the checkpoint implementation. Fresh
Luna medium owners subsequently delivered the verifier, private TargetState and
borrowed-handler refactors, target-pinned grants, and explicit launch preparation.
Parent reviewed these changes. The latest completed integrated run passed
Rust411/14ignored, Python19, strict Clippy/fmt/build and121Markdown/0findings.
The subsequent seven-check integrated run includes fail-closed scope correction
and global preflight: root412passed/14ignored, separate decoder39passed,
Python20passed, strict Clippy/fmt/build and124Markdown/0findings. The contract
owner released P3-MULTI-OWNER-ASSEMBLY-TASK.md; parent adopted its amendments.
The assembly is released and parent-reviewed, including a real two-child PTY
test. Its seven-check integrated run passes root420/14ignored plus39decoder,
Python20, strict Clippy/fmt/build and128Markdown/0findings. The subsequent
session/scoped-read slices and real-PTY assertions are delivered and
parent-reviewed. Final eight-check verifier passes root431/14ignored plus
39decoder, Python31, matching component fingerprints, strict Clippy/fmt/build
and133Markdown/0findings; strict rustdoc also passes. All three Luna medium
owners released their files. These trusted-core slices add no runtime endpoint.
Next, superseding the previous lifecycle/history priority: Phase3A right-click
splits and fresh terminal sessions, then lifecycle/stop and history review;
do not infer runtime launch authority from Input or the trusted launch method.
Inspect live status before resuming ownership.
The source-backed
[P5 scrollback review](P5-SCROLLBACK-SOURCE-REVIEW.md) remains a proposal:
exact engine epoch/reset hooks and the complete post-patch memory/provenance
review precede any vendored changes.

Remaining gates: actual pathname accept/exec proof (bind EPERM), authenticated
multi-session runtime/CLI integration and explicit stop policy, live OpenCode
identity/switch proof (missing CLI and failed DNS acquisition), and durable
history/binding stores plus recovery/retention. No user manual verification is
requested. All blocked checks remain uncompleted, and no phase-completion commit
has been made. Preserve source/ownership evidence and continue independent work
when model capacity is available.

Update 2026-10-20: Phase 3A was completed and committed. The user selected
per-pane lifecycle/stop before history and then requested direct work without
agents. Phase 3B now integrates authenticated close, pane listing, CLI and
confirmed client actions, with real-PTY/runtime acceptance; see
[P3B-CLOSE-RUNTIME.md](P3B-CLOSE-RUNTIME.md). History work has started with a
private checkpoint-store library and restart-read tests, but no live capture,
scrollback export, retention or recovery UI; see
[P5-HISTORY-STORE.md](P5-HISTORY-STORE.md). The final integrated verifier was
8/8; a preceding 7/8 run failed a storage-lock test once, and that test passed
in isolation before the full rerun passed. These additions do not complete
the five non-UI milestones or the outstanding host/provider acceptance gates.

Follow-up 2026-10-20: live visible-screen checkpoint capture is now wired to
the two-pane server and tested together with close/exit and a corrupt-storage
failure that leaves its shell usable; see
[P5-LIVE-CAPTURE.md](P5-LIVE-CAPTURE.md). The verifier passed 8/8, real
close/capture runtime 4/4, history-store 4/4 and the earlier default-open
suite 6 passed/1 ignored. This does not complete bounded scrollback, retention,
clear, restart recovery UI or the other milestone gates.
