# Phase 3 closure gate audit — 2026-10-23

Latest amendment, 2026-10-24: the user's three-pane screenshot reports blocked
agent metadata storage, nonworking scrolling and a request for pane padding.
These usability repairs are delivered and parent-verified: final serial
1111 passed/0 failed/15 ignored in164.34s, plus all seven supporting checks.
See [repair result](P3-USABILITY-FIXES-RESULT.md). After explicit user approval, the launcher was switched and a fresh workspace
started on the verified worktree build; read-only pane discovery/read pass. The 1091-test
evidence below is the prior baseline. Human four-pane acceptance remains open;
partial host feedback and automated outer-PTY tests do not close it.


Read-only audit of the current integrated working tree and its recorded evidence.
This note does not close Phase 3 or promote human-host, reboot, or provider
acceptance to passing.

Current status (2026-10-24): the accepted bounded implementation and accumulated
automated regressions are reviewed and integrated verification passed
1091/0/15 ignored with all seven supporting checks. Three Luna High workers
found no further required production task. Human four-pane Apple Terminal
evidence is the sole explicit remaining Phase3 gate; no manual task is assigned.
See [orchestration result](P3-ORCHESTRATION-RESULT.md),
[remaining-gates audit](P3-REMAINING-GATES-2026-10-24.md) and
[PTY audit](P3-PTY-INTERRUPTION-AUDIT.md). Optional admitted-input timing windows
remain observation limits and must not become new closure requirements merely
because they are untested.

| Gate | Implemented | Tested evidence | Open limit |
|---|---|---|---|
| Client and host lifetime | PTYs and launch ownership live in the server; `Detach` is separate from explicit stop. UI attach restores terminal state on orderly Ctrl-X. | `tests/p3_socket_detach.rs` exercises actual abrupt socket loss and reconnect. `tests/p3_detach.rs` separately proves lost input acknowledgment is reconciled after explicit detach/reconnect without client resend; the child emits exactly one `GOT` marker. `tests/p3_attach_four.rs` runs a real attach client in a private outer PTY, detaches with Ctrl-X, checks terminal restoration, then describes all surviving children as running. | The new post-ready SIGKILL attach-and-reattach regression proves server/child/target/output survival with exact PID checks; see P3-OWNER-LIFETIME-RESULT.md for parent execution results. The post-completion/pre-client-ack killed input CLI is now covered (P3-INFLIGHT-CLIENT-RESULT.md); death before admission or while owner-pending remains outside that evidence. The server-death slice adds cooperative-shell exit, endpoint replacement and generation/context fencing; arbitrary descendant cleanup remains unproved (P3-SERVER-DEATH-RESULT.md). |
| Input replay and launch deduplication | Persistent client context reserves operation keys; exact requests are deduplicated; uncertain input/launch outcomes are reconciled by status and never automatically resent. | Lost-ack/no-replay in `p3_detach`; exact retry dedup in `p3_multi_owner`; pane-run uncertainty/reopen and no-new-launch behavior in `src/p3_cli.rs` tests. The earlier pane-run frozen-tree serial evidence was 1084 passed, 0 failed, 15 ignored; current combined evidence is recorded in P3-OWNER-LIFETIME-RESULT.md. | Existing cases cover controlled acknowledgments, actual socket loss, and explicit detach. The killed control-CLI scenario now covers a terminal Applied response withheld before caller receipt, then same-context recovery; it does not cover an interactive attach killed before admission or while owner-pending. |
| Watch, launch, layout runtime | Async serialized split/launch, target-pinned pane observation, workspace layout journal with snapshot/cursor gap recovery, finite watch slots, observer renewal, and bounded managed `pane run` are implemented. | Current evidence notes record real CLI layout-gap recovery and pane-run cases; combined tree was parent-verified at 1084/0/15 ignored. Pane run uses existing launch deduplication and reports unknown outcomes without retry. | This does not establish an active-agent resource budget, provider identity, crash durability, or full lifecycle transcript. Product choices for broader workspace hierarchy and autonomous scheduling remain unresolved and are not closure prerequisites inferred by this audit. |
| Owner identity | Current `SessionInfo` owner lease, revision and expiry are exposed as neutral `none`/`client` observations; the current caller is identified only by its target-scoped lease. No agent/provider identity or authority was added. | `P3-OWNER-IDENTITY-TASK.md` defines the accepted contract; `P3-OWNER-IDENTITY-REVIEW.md` found no actionable issue in source review. `tests/p3_owner_identity.rs` covers cross-caller observation and CAS refusal, with parent results recorded in P3-OWNER-LIFETIME-RESULT.md. | Owner identity is not authenticated role identity. Per-pane owner reads are point-in-time observations, not an atomic workspace owner snapshot. |
| Pathname socket and descriptor acceptance | Private pathname listener, same-user credential checks, bounded accept/connect, spawn serialization, and accepted-socket CLOEXEC protections are implemented. | `P3-PATHNAME.md` records operator-confirmed corrected pathname CLI acceptance (startup, context reuse, input, detach/reconnect, singleton refusal, endpoint cleanup); raw rerun output was not supplied. `P3-ACCEPT-EXEC-RACE.md` records six successful serial host runs plus pathname listener-authentication and peer-UID fixtures. | The earlier pathname CLI record was operator-confirmed; parent now explicitly ran the expanded ignored CLI fixture1/1 passing in1.20s (P3-RUNTIME-CONTRACT-RESULT.md). Descriptor results remain host-specific. They do not prove all supported macOS releases or hostile same-user isolation. |
| Four-pane host/proxy acceptance | Four-pane split, routing, isolation, rendering/dividers, pane-limit refusal, close/reflow, and orderly detach are implemented. | `P3-L17-ATTACH4-RESULT.md` records three consecutive passes of the private outer-PTY attach scenario on macOS arm64. Current `tests/p3_attach_four.rs` has the scenario and asserts all four panes plus surviving children after detach. | This is automated proxy acceptance, not a human Apple Terminal four-pane session. Phase 2's scoped gates are marked done, and its OSC 52 clipboard-denial check has a separate operator pass; those do not close the distinct four-pane human-host gate. The separate SIGKILL regression covers automated post-ready client death/reconnect; it does not promote this proxy to human-host acceptance. |

## Delivered automated client-death task

The new regression in `tests/p3_attach_four.rs` kills only the exact live attach
process, without Ctrl-X, then starts a fresh outer-PTY client against the same
server. It verifies the same full target and managed child PID, retained ready
output, one new input marker, orderly second-client detach, and explicit server
stop/child cleanup. Parent results and intermediate fixture corrections are in
[P3-OWNER-LIFETIME-RESULT.md](P3-OWNER-LIFETIME-RESULT.md). This is post-ready
client-death evidence; no request is deliberately killed in flight.

## Delivered bounded server-death task

The existing `p3a_default_open` SIGKILL test now observes a cooperative shell's
exit, stale endpoint replacement, new full target/generation, old-context
refusal without mutation, and an unchanged once-only external side effect.
Both intentional and failure-cleanup server signals validate PID, owner and
kernel start time. See [the result](P3-SERVER-DEATH-RESULT.md),
[audit](P3-SERVER-DEATH-AUDIT.md) and
[review](P3-SERVER-DEATH-REVIEW.md) for exact execution status and limits.
SIGKILL runs no graceful cleanup or final flush; arbitrary descendants may
survive. This does not establish orphan-free crash or in-flight input recovery.

## Runtime scope reconciliation — 2026-10-23

The [contract audit](P3-RUNTIME-CONTRACT-AUDIT.md) and
[independent review](P3-RUNTIME-SCOPE-REVIEW.md) found no missing implementation
in the accepted bounded event/watch/launch/layout scope. The stale umbrella
checkbox is replaced by the delivered finite-watch/reconciliation, managed
pane-run and four-pane lifecycle contracts. The parent also ran the previously
ignored expanded pathname CLI fixture explicitly:1/1 pass in1.20s, separate
from the normal suite. See [current result](P3-RUNTIME-CONTRACT-RESULT.md).
Indefinite subscriptions, large Snapshot queues and standalone direct launch
remain separate future scope; this does not close fullPhase3.

## Delivered killed input-client acceptance

The [task](P3-INFLIGHT-CLIENT-TASK.md),
[barrier audit](P3-INFLIGHT-BARRIER-AUDIT.md),
[review](P3-INFLIGHT-CLIENT-REVIEW.md) and
[result](P3-INFLIGHT-CLIENT-RESULT.md) record actual control-process SIGKILL
once a test-owned proxy holds the matching terminal Applied reply. The same
context retains its uncertain key; status/resync observes and clears it without
resubmission, and the same child reports one handled token. This closes that
post-completion/pre-client-ack coverage gap, not death before admission or
while the owner is still processing input. Human four-pane acceptance remains
separate. The history-writer startup contention and ACK42 attach-host test
intermittencies received bounded stabilizations and parent verification; see
[the result](P3-INTERMITTENT-ACCEPTANCE-RESULT.md). Their earlier failed runs
remain historical evidence, and repeated passing runs do not establish a timing
guarantee.

Other open work should be specified from the existing phase contract before
implementation. A proxy barrier can establish pre-delivery client death; a
separate in-process harness would be needed to pause after server dispatch but
before ledger admission. Do
not turn provider identity, active-agent load targets, physical reboot recovery,
or unresolved product choices into new Phase 3 requirements based only on
aspirational roadmap prose.

## Pre-delivery client-death slice specified — 2026-10-23

The bounded test contract is now recorded in
[P3-PRE-ADMISSION-CLIENT-DEATH-TASK.md](P3-PRE-ADMISSION-CLIENT-DEATH-TASK.md).
The selected automated acceptance uses the authenticated proxy to pause after
decoding the real Submit but before forwarding it. This proves durable client
reservation while the server reports `NotAdmitted`; same-context resync clears
uncertainty without sending input, and an independent shell CHECK remains at
zero. An attempted in-test-file design for a server-side validation latch is
not available in the current fixture: it launches a separate production
server, and injecting `EffectSink::validate` would require a production hook or
an in-process harness. The narrower pre-delivery test is therefore the current
slice. It does not prove post-dispatch/pre-admission or admitted owner-pending/
PTY interruption.

## Pre-delivery client-death acceptance delivered — 2026-10-23

The acceptance is implemented in `tests/p3_inflight_client.rs`; see
[P3-PRE-ADMISSION-CLIENT-DEATH-RESULT.md](P3-PRE-ADMISSION-CLIENT-DEATH-RESULT.md).
The proxy captures the exact Submit and never forwards it. The real CLI Child
is SIGKILLed after context reservation; the server reports `NotAdmitted`,
same-context resync clears uncertainty without resend, and the controlled
shell CHECK remains zero. Parent's focused target passed 2/2, with docs/fmt/
whitespace checks passing. This is explicitly pre-delivery evidence and does
not test server dispatch before admission or owner-pending/PTy interruption.
Full Phase 3 and the separate human four-pane/provider/reboot limits remain
open.

## Post-dispatch barrier feasibility audited — 2026-10-23

The actual CLI cannot be paused deterministically inside server validation by
the current separate-process integration fixture. Its proxy stops before
delivery, or after the server completes processing. The in-crate `StateOwner`
handler can be tested at `EffectSink::validate` with a bounded fake sink, but
this omits socket ingress and real CLI death. The audit and limits are recorded
in [P3-POST-DISPATCH-FEASIBILITY.md](P3-POST-DISPATCH-FEASIBILITY.md). Next is
a handler-boundary unit test only; do not claim real client-death coverage or
add a production hook. Owner-pending/PTy, human four-pane, provider and reboot
gates remain separate.

The in-crate `StateOwner` test is now delivered in `src/server.rs`; the focused
test passed 1/1. It blocks at test-only validation, observes zero effects, then
verifies one terminal effect and exact-duplicate deduplication. This verifies
handler ordering only; it is not socket ingress or real CLI-death evidence.
The admitted owner-pending/PTy audit is now recorded in
[P3-PTY-INTERRUPTION-AUDIT.md](P3-PTY-INTERRUPTION-AUDIT.md). It preserves the
evidence limits without adding a production hook or a new closure requirement.
