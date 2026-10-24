# Phase 3 remaining gates — 2026-10-24

Read-only scope audit of the current roadmap, handoff and closure evidence. This
note identifies the remaining Phase 3 requirement; it does not update shared
status, mark a gate complete, or claim new verification.

## Current continuation amendment

After the original audit below, the user reported blocked agent context storage,
nonworking scrolling and cramped panes. Those repairs are delivered and verified
in [the usability result](P3-USABILITY-FIXES-RESULT.md): complete serial Cargo
1111 passed, 0 failed, 15 ignored, plus all seven supporting checks. The older
1089-test figure and source line references below describe the earlier audit
snapshot and are not current verification counts.

The continuation preflight rechecked Git history/index, the built binary hash,
the PATH launcher and live pane discovery. HEAD remains `8ad9bd6`, accumulated
changes remain uncommitted, and there are no unmerged index entries. The binary
still matches the verified SHA-256 in the usability result. PATH resolves to
the separate main checkout; one live pane remains. No input was sent and no
server or child was stopped.

The user subsequently selected **Activate and restart workspace**. The launcher
was switched to the verified worktree build, supported stop completed, and a
fresh workspace was independently observed running that executable. Activation
is complete; the preflight's old-PATH/live-pane observation above is historical. Human four-pane Apple Terminal acceptance remains separate;
no new code task or extra synthetic acceptance window is inferred from it.

## Required work still open

The roadmap has one unchecked Phase 3 item: independent host/lifetime
acceptance, explicitly naming human four-pane Apple Terminal evidence. It also
requires preserving the known limits on in-flight/client/server crash
observations and forbids inferring provider, reboot or arbitrary descendant
cleanup guarantees. See `ROADMAP.md` Phase 3, especially the complete checked
implementation list and final unchecked item (lines 149–193).

The only unambiguously outstanding acceptance in that item is a human
four-pane Apple Terminal session. The private outer-PTY fixture has passed
three consecutive runs and covers split, routing, isolation, rendering,
dividers, pane-limit refusal, close/reflow and orderly detach; it is explicitly
proxy evidence, not human-host evidence (`P3-CLOSURE-AUDIT.md`, lines 14–15;
`ROADMAP.md`, line 185). The automated owner-death, server-death and
killed-input cases are already delivered and documented with their observation
limits (`P3-CLOSURE-AUDIT.md`, lines 16–36 and 50–65; `ROADMAP.md`, lines
189–191). Current session evidence records the latest full serial suite as
1089 passed, 0 failed, 15 ignored and the pre-delivery client-death focused
target as 2 passed, 0 failed (`SESSION.md`, lines 67–74 and 45–50).

This leaves no additional production implementation requirement established by
the current Phase 3 contract. The roadmap says its bounded runtime is delivered
and reconciles older proposals; indefinite subscriptions, large Snapshot
queues, and standalone direct launch are future scope (`ROADMAP.md`, lines
151–162, 192). The audit explicitly says not to turn provider identity,
active-agent load targets, physical reboot, or unresolved product choices into
Phase 3 requirements based on aspirational prose (`P3-CLOSURE-AUDIT.md`, lines
67–73). The spec's provider identity/recovery and reboot evidence belong to
their separate gates, not this Phase 3 acceptance.

## Suggested next action and ownership

There is no source-file task established by the required gate. Human four-pane
Apple Terminal evidence remains unavailable in this run. The standing
automated-work/no-manual-handoff instruction remains in force: this audit does
not assign a manual task to the user. The parent records the open gate and
completes the remaining automated review and integrated verification. Automated
proxy results cannot be relabeled as human acceptance.

Once that evidence exists, the parent can independently assess whether it
satisfies the named host gate, then update shared status and decide whether
Phase 3 is complete. This audit itself needs no test rerun. The parent is
separately verifying the accumulated uncommitted pre-delivery and handler
regressions because those were added after the latest complete suite.

## Optional coverage, not a closure prerequisite

The handler-boundary test for `EffectSink::validate` exists and proves
validation precedes admission/effects; it is explicitly not socket ingress,
real CLI death, or admitted owner-pending/PTy interruption
(`SESSION.md`, lines 3–17; `P3-POST-DISPATCH-FEASIBILITY.md`). A further
in-process owner-pending interruption test could be explored only if the
parent identifies a concrete contract question it would answer. The current
roadmap and closure audit do not require this extra test, and adding layers of
synthetic barriers would not substitute for the outstanding human-host gate.
No production hook, cancellation claim, or change to nonreplay behavior is
justified by this audit.

## Requirements explicitly outside this gate

- Provider identity/session-switch or provider recovery: Phase 4 / upstream
  capability limitation; do not block Phase 3 closure on it.
- Physical reboot, saved layout/history restoration and representative
  workload acceptance: Phase 5 scope.
- Arbitrary descendant cleanup after server SIGKILL: not guaranteed by the
  bounded server-death acceptance; the recorded cooperative-shell result must
  retain that limit.
- Repeated timing windows or new optional repetitions: previous stabilization
  evidence already includes fixed serial repeats and a complete suite. Re-run
  only when new code, a failure, or a specific unresolved acceptance justifies
  it; no additional arbitrary observation window is a Phase 3 deliverable.

No source was inspected or modified for this note. No tests were run.
