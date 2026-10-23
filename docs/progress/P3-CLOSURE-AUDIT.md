# Phase 3 closure gate audit — 2026-10-23

Read-only audit of the current integrated working tree and its recorded evidence.
This note does not close Phase 3 or promote human-host, reboot, or provider
acceptance to passing.

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
separate. Prior known test intermittency remains unresolved; investigate those
recorded failures before extending lifecycle claims.

Other open work should be specified from the existing phase contract before
implementation. In particular, do not turn provider identity, active-agent
load targets, physical reboot recovery, or unresolved product choices into new
Phase 3 requirements based only on aspirational roadmap prose.
