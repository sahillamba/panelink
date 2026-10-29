# Codex wheel responsiveness

Status: client/server0.1.8 activated after explicit user-approved restart;
scoped capability1024 verified (mask2047). Two fresh shell panes restored.
Real Codex/native-host acceptance remains open. Previous0.1.7 event-retention
repair and scoped follow-up evidence are recorded below.


Astra consultation: [diagnosis](CODEX-WHEEL-ASTRA-DIAGNOSIS-2026-10-29.md)
identifies six sequential RPCs and six durable full flushes per wheel report,
serial poll contention, and a no-mouse alternate-screen routing regression.
Wall-clock contribution remains to be measured. Do not enlarge FIFO/TTL again.

Scoped-transaction follow-up: [exact implementation contract](SCOPED-WHEEL-IMPLEMENTATION-2026-10-29.md).
Durable context-only measurements independently reproduced about52 reports/sec
for three reserve/ack pairs, with roughly3ms per reserve or ack. This excludes
transport and rendering. Implementation proceeds W1 codec -> W2 owner/server ->
W3 client -> W4 private measurements -> W5 docs/canonical verification.
New protocol requires a supporting server. Do not restart the live server or
terminate existing panes; activation remains separate from source verification.
First Luna attempt produced a pipe-only fixture, which parent rejected as
unrepresentative. Its timings are excluded. Correction must exercise the actual
outer PTY -> attached client -> server -> private mouse-reporting child path.
The existing deterministic alternate-screen drop regression passed (1/0).
Checkout: `t3/improve-codex-mouse-scroll-speed`, starting at
`daf28a63f062331234ffd63daba577101d3315fa` (initially clean).

User authorized measurement, fixes and cost-effective Luna delegation.
Parent owns design, integration, review, docs and final verification. Workers
have exclusive files and make no commits. Do not install, restart the user's
server, or send input to existing panes. Build only with a separate
`CARGO_TARGET_DIR` under `/private/tmp`.

## Packets

- [x] A: reproduce alternate-screen wheel loss and measure private-PTY baseline.
- [x] B: parent specify compatibility-safe lower-overhead delivery and bounded
  gesture admission from A's evidence; Luna implement with focused regressions.
- [x] C: parent review; verify mode transitions, target replacement, ownership,
  direction changes, typing, uncertain outcomes and normal-history behavior.
- [x] D: repeat measurements; update README, canonical SPEC and usage skill,
  validate documentation links and record remaining host/provider gates.

## Source observations to verify

`App::scroll_pane` drops alternate-screen wheel events while history_pending;
normal history accumulates a delta. `Net::scroll_inner` reads history before
claim/send/release for each accepted alternate-screen wheel. These observations
are not measurements of the installed client or proof of Codex's active mode.

## Acceptance

Baseline corrected private-PTY run: paced 20/20, burst 1/20, first visible
response approximately 97 ms (20 ms polling resolution). See baseline report.

## Packet B implementation specification

Prefer existing protocol/server validation: no new wire command, capability or
dependency. `PtyEffects::validate_mouse_wheel` and `mouse_wheel` already check
live/closing/stopped/alternate-screen/mouse mode, coordinates and input capacity
on the serialized server owner before delivery. Preserve those checks unchanged.

Client may retain up to 32 *fresh unsent* alternate-screen wheel intents per
pane, FIFO with their own coordinates, direction, target/view epoch and captured
Instant. Maximum age one second, measured from capture, never refreshed by output or
more wheel input. Overflow discards new events (bounded). Do not net opposite
directions or translate wheel into keys. At most one event in flight; consume an
intent once on admission, never reinsert it after any outcome. Normal-history
delta accumulation remains separate and unchanged.

Introduce a dedicated alternate-wheel Effects/worker/Net path. It must perform
claim -> existing typed wheel operation -> release only after known Applied or
explicit pre-admission Unsupported, without a preliminary history snapshot. It
returns MouseDelivered only for Applied plus confirmed release. Unsupported is a
failure that cancels the rest of the queue; do not fall back to shell history or
send a replacement event. InputBusy, errors and uncertain outcomes also cancel
remaining gestures and preserve existing retained-claim/reconciliation policy.
Unknown/normal mode keeps the current history-query path; read-only seek never
claims. Existing servers already validate this unchanged typed wheel request.

Cancel unsent intents on typing/paste, non-wheel pointer interactions, navigation
or overlays, route/target/layout changes, resize, normal-screen transition,
disconnect, stale result and any failed wheel result. Ordinary alternate-screen
output revisions must not cancel valid gestures. An admitted event retains the
existing server authority and cannot be recalled. Check age, exact target and
view identity again immediately before worker admission. Server mode remains
authoritative at each delivery; cached screen mode is only a routing hint.

Worker accounting includes the dedicated command. Normal-history scroll_pending
gating remains; alternate-screen delivery allows bounded screen polls so queued
gestures do not hide visible movement until the entire burst drains. No unbounded command queue and no retained idle claim.
Focused tests must prove FIFO/coordinates, independent expiry, overflow, every
cancellation category, failure/uncertainty without retry, and shell history
unchanged. Repeat the same private fixture and require burst 20/20 on the healthy
fixture before declaring the event-loss fix verified. Timing remains observational.

Record received/delivered events, first visible response and burst drain time
using private fixtures. Avoid timing-sensitive CI thresholds; use deterministic
event-count and no-replay assertions. Compare identical baseline/final builds.
Real Codex/host observations remain separate from synthetic evidence. Preserve
exact target, retained ownership and operation deduplication; no retry of
uncertain input and no stale movement after transitions. No sensitivity change
until loss and latency are understood. No new dependencies planned.

## Resume

Read ROADMAP.md, SESSION.md and this file. Read completed worker reports;
review their diffs before the next packet. Do not infer acceptance from a
source change or successful PTY delivery.

## Parent review and verification

Luna delivered transport and state routing in separate bounded packets. Parent
review corrected successful-result queue clearing and moved cancellation from the
copy-only handler to main event dispatch; added route/resize/buffer/disconnect
cancellation, deterministic expiry/overflow/cancellation/admission-failure tests,
and allowed screen polling during alternate-wheel operations.

The original 500 ms limit produced 19/20 healthy burst events after the queue-clear
repair. Parent changed the limit explicitly to one second and retained the cap32,
per-event authority, FIFO and no-retry rules. With polls enabled, repeated healthy
private fixture runs deliver 20/20. The final assertion now requires 20/20 for
paced and burst input, validates exact SGR coordinates/direction in the receiver,
and requires an observed visible count. See baseline report for original results.

Parent binary tests: 317 passed / 0 failed. Strict all-target Clippy passed.
Scoped documentation checker: 4 files / 0 findings. Full canonical verification
results will be recorded below when complete.

Final exact-report fixture observation (coarse20ms polling, during broader checks):
paced20/20 first117.3ms, burst20/20 first120.1ms. Earlier final-source observation
before exact-report matcher: paced20/20 first96.5ms, burst20/20 first99.7ms. This
fix recovers burst movement; these observations do not establish reduced first
response latency. Observation duration includes a one-second stability window,
so it is not burst delivery time.

Installed binary inspected read-only: client0.1.6 revision daf28a63f062 fingerprint
1a02d323d6bb30a5 clean; live server0.1.4 revision10d81c24cdc2 fingerprint
1edf2c210c2de12d dirty. Neither replaced/restarted. This source change is not an
installed fix or actual Codex/Apple Terminal acceptance. No dependencies, wire
changes, commits or push.

## Final canonical verification

`CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target RUST_TEST_THREADS=1
python3 scripts/verify_non_ui.py` exited0: all8 checks passed. Root Rust suite
1580passed/0failed/18ignored (the new opt-in measurement is one of the ignores and
was explicitly run separately); decoder39/0; source fingerprints match; fmt,
strict all-target Clippy, build pass; Python233run/1skip; docs407files/0findings.
Full log: `/private/tmp/panelink-wheel-verification.log`. Final doc-only handoff
updates were checked separately. No material findings from independent Luna
read-only source review.

No phase-completion commit: this repair does not close the roadmap's outstanding
host/provider/phase acceptance gates, and no checkpoint commit was requested.

## User-authorized installation

2026-10-29: built and installed0.1.7; see current SESSION.md install evidence.
Root Cargo.lock change is only the package version. Post-bump lib921/0/4ignored,
bin317/0 and explicit wheel fixture20/20 both cases pass. Installed fingerprint
`a801d78163935d32` (dirty source). Server and existing panes remain unchanged.
Ctrl-X then `panelink` loads the new client without a server restart.
