# Phase 4 opposite-client and observer-health slices — 2026-10-24

Follow-up: [linked-child and project-isolation evidence](P4-V2-ISOLATION-RESULT.md).

Status: **both scoped development-fixture slices pass 3/3 fresh runs** on the
same pinned native OpenCode 2.0.32/hash as the [baseline](P4-V2-TUI-RESULT.md).
Two Luna High owners implemented disjoint identity and health/plugin changes;
the parent reviewed, integrated and independently verified them. Full Phase 4,
the live Panelink provider bridge and durable recovery remain open.

## Slice 1: opposite client and new-conversation outcome

The physical second TUI starts on B, switches B-A-B through the keyboard
picker, and leaves the first TUI continuously fresh on A. Role-aware result
fields distinguish this from the baseline first-client A-B-A case.

Ctrl-X then N is verified against the current official
[OpenCode TUI reference](https://dev.opencode.ai/v2/docs/cli/tui/). In all three
final runs it changes the active route to **Home with no selected session ID**.
The passive client stays fresh through this additional control. No prompt is
sent, so no claim is made about when a new ID would be allocated. The stronger
new-conversation-ID guarantee remains unproven.

If a new ID is observed, the driver requires exact hash equality through a
bounded owned-server lookup and canonical project/top-level validation. Known
fixture IDs cannot masquerade as new ones, and newest-session ordering is never
used. That lookup branch has behavioral unit coverage, not live new-ID evidence.
Any validated third session is included in empty-user-message verification.

## Slice 2: observation loss, expiry and explicit reconciliation

An optional private control file pauses only the first plugin's record writes.
The plugin timer, both TUIs and the owned server continue running; the second
observer stays fresh. The development monitor uses host receipt time and a 1s
expiry threshold. Re-reading the same row cannot refresh it. Valid batched
contiguous rows count as new delivery; missing/regressing sequence, scope,
nonce or source-instance data fails closed.

After receipt expiry the retained observation is stale and ineligible as a
current identity. Resumed transport alone does not revive it. A new observation
after an explicit reconciliation watermark restores only in-memory observation
eligibility. **Durability remains false**, and no current-at-crash barrier is
claimed. This 1s threshold is an experiment policy, not the production heartbeat
contract. Unknown identity inside a batch also requires reconciliation.

The live case proves controlled record-delivery loss while processes remain
alive. It does not prove arbitrary plugin crashes, provider-process failure,
durable acknowledgment, a shipped IPC bridge or production recovery.

## Frozen evidence

Receipt: `/private/tmp/panelink-p4-20261024/v2-extended-final-repetitions.json`.
Each scenario uses fresh private HOME/XDG/project roots, the exact binary
hash/version guard, loopback-only sandboxing and fixture-only authentication.
Every final run exits0, verifies empty user-message reads, and obtains both TUI
and server cleanup receipts. No user workspace, provider account or installed
configuration is changed. Reports contain no raw IDs, credentials, message
bodies, project paths or terminal contents.

| Run | Reverse/new wall ms | Reverse latency upper bounds ms | Health wall ms | Health case ms |
| --- | ---: | --- | ---: | ---: |
| 1 | 7438.302 | 115,123 | 9540.070 | 1362.905 |
| 2 | 7421.653 | 100,101 | 8745.743 | 1386.590 |
| 3 | 7546.369 | 116,102 | 9016.452 | 1379.045 |

All reverse runs report `home_without_selected_session_id`. All health runs
report expiry, passive freshness, both TUIs/server alive, no automatic promotion
after resume, explicit fresh reconciliation, and durability false. Both cases
have zero missed/extra/rejected route observations. Switch timings are the
existing conservative plugin-clock polling bounds, not exact keystroke timings.

## Review fixes and failed evidence

The first reverse diagnostic failed before keyboard controls because the
driver precreated empty pause-control files. The plugin correctly treated an
unknown directive as failure and stopped recording. Parent integration leaves
the paths absent until a health pause is requested and added a driver regression
check. Receipt: `v2-extended-reverse-diagnostic.json` in the same temporary
evidence directory; 18,909.527ms, zero controls, both TUIs/server cleaned up.

The corrected reverse diagnostic passed in 7580.170ms, and the first health
diagnostic passed in 9300.042ms (health portion 1364.507ms). Receipts:
`v2-extended-reverse-control-fixed.json` and `v2-extended-health-diagnostic.json`.
Final repetitions above supersede these single diagnostic passes.

Review also corrected batch handling, expiry before accepting late arrivals,
unknown identities hidden inside a batch, and invalid data being mistaken for
transport expiry. The plugin lifetime cap now applies while delivery is paused.
Reverse-new fails closed on unverified/failed controls, missing route changes,
existing fixture IDs, incomplete lookup proof and passive drift. The baseline
route gate retains its strict gap policy; only the separate health monitor
handles the deliberately induced delivery gap.

## Verification and remaining scope

- Parent full Python discovery: **143 passed**, no failures/skips, in 8.403s.
- Focused bootstrap/identity/health/plugin tests: **61 passed**. Tests include
  real Node fixture behavior and mocked driver/health lifecycles; these are
  distinct from the six executed live runs above.
- Changed Python compilation and tracked whitespace checks pass; ten affected/
  untracked files have zero whitespace findings. Docs: 287 Markdown files/0
  findings. The in-memory graph was refreshed without an artifact. No Rust/build input changes were made; the
  separate Phase 3 baseline remains 1111 passed/0 failed/15 ignored.

Next: fork/child/project isolation and the remaining new-ID case, then version
the real bridge and implement durable binding acknowledgments against the
existing pure core/codec. Preserve the explicit last-confirmed/manual fallback,
including crash-before/after-ack tests. No automatic provider resume or latest-
session heuristic is enabled. No commit or push.
