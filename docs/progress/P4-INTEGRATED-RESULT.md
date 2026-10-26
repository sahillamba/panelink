# Phase 4 integrated identity and explicit recovery

Status: **scoped Phase 4 complete — 2026-10-25**. All roadmap Phase4 gates
pass, including Luna and independent parent native managed acceptance.
The implementation checkpoint is `f1d7af9`; this follow-up closes acceptance
and documents the versioned adapter contract without changing application code.

## Scope and evidence

| Gate | Result |
| --- | --- |
| Two managed targets in one project; A-B-A while passive B remains unchanged | Luna and independent parent native runs pass |
| API-created new root and actual nonempty imported-history fork, selected through native UI | Luna and independent parent native runs pass |
| Home revokes eligibility, retains last confirmed identity, then fresh return saves A | Luna and independent parent native runs pass |
| Same-client foreign-project selection, refusal, preserved binding and return | Luna and independent parent native runs pass; exact observer, public manual refusal, A revision7→8 |
| Missing/deleted, foreign-project and linked-child explicit choices refused | Luna and independent parent native runs pass |
| Prepare preserves selection; separate resume spends once; fresh selection/save confirms; replay refused | Luna and independent parent native runs pass |
| Explicit recovery accepts a real nonempty-history fork root | Luna and independent parent native runs pass |
| Bridge failure revokes recovery while PTY survives; no implicit regrant | Luna and independent parent native runs pass |
| Joint record and Prepared/Spent process-kill/reopen boundaries | Three focused process-kill tests pass; restored identity stays stale |
| API fixture mutations preserve attach, pane, owner and A; subsequent new-root save receives exact runtime acknowledgment | Native liveness probe passes |

The pinned client is native OpenCode 2.0.32, SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
The isolated Panelink build is SHA-256
`1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`.
Earlier manual/failure native runs used
`1497ac732020573d633df85d38fdd594a92b31d7f1b8ee29d06e0087e52ec9ee`;
no Rust or shipped-plugin source changed during this acceptance follow-up.
Each native fixture uses disposable private state and an enforced loopback-only
macOS sandbox, checks absence of user messages, submits no model prompt, and
reaps its owned children. Reports retain fixed statuses, revisions and hashes;
provider IDs, credentials and screen text remain private to the fixture.

## Verification

The unchanged application and shipped-plugin tree retains the full serial
Rust result: **1,231 passed, 0 failed, 15 ignored**, across 63 targets. On this
continuation the exact IPC/manual, process-kill/reopen and historical CLI
targets pass **6/6**. Decoder **39/39**, component fingerprints, formatting,
strict all-target Clippy and isolated build pass. Final Python: **232 run,
231 passed, 1 intentionally opt-in native skip**, 0 failures. Markdown:
**332 files, 0 findings**; final documentation and whitespace checks also pass.
The native acceptance is explicitly run separately from the skipped unittest.
Luna final review found no material issues, and the parent independently
reviewed the guard, observer, public refusal and exact-save evidence.

Durable fixed-schema reports: [parent](p4-evidence/managed-native-parent-2026-10-25.json)
and [Luna](p4-evidence/managed-native-luna-2026-10-25.json). Parent foreign
sequence135 advances to138 after the public refusal, then returns to A on
the same TUI at152. The refusal is `manual_selection_required` /
`provider_project_mismatch`, without a ticket; the foreign route and A record
stay unchanged. Return saves A revision8, with runtime/disk revisions equal;
passive B remains revision1. Both reports classify proven, with cleanup true,
no user messages and zero prompts.

## Contract and remaining boundaries

See the [pinned adapter contract](P4-PINNED-ADAPTER.md),
[managed UI evidence](P4-MANAGED-BRIDGE.md),
[native explicit recovery](P4-NATIVE-MANUAL-RECOVERY.md),
[native bridge failure](P4-NATIVE-BRIDGE-FAILURE.md),
[joint-store kill boundaries](P4-MANAGED-CRASH.md), and
[API-mutation liveness](P4-API-MUTATION-LIVENESS.md).

The adapter is opt-in for the original exact managed occupant. Default launches
and later generic shell splits do not acquire provider recovery. Its public
promise is last-confirmed historical metadata and explicit validated one-shot
recovery. It does not claim current-at-crash identity, automatic resume, input
or conversation replay, provider executable attestation, other provider
versions, machine power-loss or reboot recovery. No dependency was added.

The separate Phase 3 human Apple Terminal acceptance and broader Phase 5
workspace/reboot gates remain open. Phase 4 completion alone does not close M1
or establish overall product readiness. The active workspace binary and user
configuration are unchanged; nothing is activated or pushed by these fixtures.
