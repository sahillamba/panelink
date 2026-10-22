# Implementation session checkpoint — 2026-10-17

## Phase 6: robustness, ten-pane runs, audit — 2026-10-21 (read this first)

User approved all three follow-ups. Parent installed cargo-audit (Homebrew,
0.22.2): 0 vulnerabilities across 18 lockfile crates vs 1348 advisories.
L10 (Luna) hardened load-flaky tests with root causes (fork-to-exec flock
window, HUP fixture process group, short socket deadlines, two writer-test
races); test-only. L11 (Luna) fixed the ten-pane harness (idle-connection
timeout keepalive; proven live-bench input protocol) and recorded two
SYNTHETIC proxy runs. Parent added a p3_detach marker-wait fix and rebuilt
the repo binary. Committed; no push.

## P4 switch signal + Phase 6 start — 2026-10-21 (read this first)

Two GPT-6-Luna workers. Both were interrupted by a machine sleep + LLM auth
broker timeout, resumed, then stopped by the parent after exceeding their
timeboxes. L8: decisive bundle evidence that 1.19.0 keyboard selection never
publishes tui.session.select (polling only); probe regressions found/fixed
but 5-rep rerun incomplete. L9: build measurements and offline license
inventory done; ten-pane workload compiles (parent removed an input-resend
retry) but fails child-ack. Parent also killed two leaked 2-day-old test
servers (pl-p3b-close) that were burning ~80% CPU. Intermittent timing tests
reproduced on baseline. Committed; no push.

## Recovery relaunch + links — 2026-10-21 (read this first)

Two GPT-6-Luna (xhigh) workers in parallel against a parent-frozen link API
(P5-RELAUNCH-LINKS-API.md). L7 link store: clean, 14 tests. L6 relaunch:
5 real end-to-end tests (fresh workspace, saved orientation split with the
default shell, links, refusal while running, --previous). Parent accepted the
CLI-side link-writer limitation (lifecycle lock; single writer), added a
secrets assertion and fixed an outdated usage test. Verifier 8/8, 983/0/15.
All recovery decisions 1-7 are implemented as a prototype; manual Apple
Terminal acceptance not done. Committed; no push.

## Recovery chooser + in-workspace menu — 2026-10-21 (read this first)

Two GPT-6-Luna (xhigh) workers in parallel. L4 delivered the TTY-only
startup chooser (pure model + bounded raw-key read); L5 looped on failed
patches for ~50 min and left a non-compiling tree, so the parent stopped it
and finished integration (dedupe, Net::server_directory, shared stop flag,
lone-Esc guard, test path updates). Existing attach PTY suite now exercises
the real chooser. Verifier 8/8, 964 passed/0 failed/15 ignored. Manual Apple
Terminal check of the chooser/viewer not done. Next: explicit relaunch +
links. Committed; no push.

## Recovery viewer + P4 PTY experiment — 2026-10-21 (read this first)

User answered the seven recovery questions (recorded in P5-RECOVERY-VIEWER.md
and SPEC 0.14) and approved the P4 private-PTY experiment. Luna (xhigh)
workers: L1 P4 TUI identity (timeboxed by parent after ~50 min; plugin loads
from project-local `.opencode/tui.json` + `.opencode/plugins`, initial route
captured 3/3, one of two controlled switches missed each run -> NOT proven);
L2 recovery stages 1-2 (parent fix: absent layout dir = missing, not unsafe);
L3 stage 3 viewer/CLI (parent exposed the attach HostGuard/Output as
pub(crate) so the viewer reuses one terminal-restore path). Verifier 8/8,
949 passed/0 failed/15 ignored. Host /dev/tty viewer acceptance not run.
Next: chooser + menu item, then explicit relaunch + links. Committed; no push.

## Three parallel GPT-6-sol gates — 2026-10-21 (read this first)

User asked for three GPT-6-sol (medium) workers, one per gate, closely
monitored. S1 delivered the recovery-viewer/explicit-relaunch ADR and staged
tasks (design only; product choices left pending). S2 delivered bounded
end-intent reservations and byte-pressure row salvage; parent fixed slot
reuse and a pre-existing nlink-0 reader race. S3 ran an isolated live probe
on OpenCode 1.19.0: version pinned, two sessions created via local server,
SSE present, selected-session identity and switch NOT proven; next step is a
private-PTY TUI plugin experiment. User also asked that launchers fall back
to `tescode workflow custom` when opencode is not on PATH (AGENTS.md); S3 had
not hit missing_executable in this run, but its probe now resolves
opencode -> ~/.opencode/bin/opencode -> tescode. Committed; no push.

## Saved-layout implementation slice — 2026-10-21 (read this first)

User requested parent orchestration with Kimi K2.6 subagents, two parallel at
a time, next worker launched as soon as one finished. Six Kimi tasks ran
(codec, store, writer, CLI, lifecycle acceptance, independent review); one
store run was interrupted by shell exit and one lifecycle run aborted on a
provider content-filter error; both were relaunched detached. Parent owned
exports, the layout writer claim, driver integration and all review fixes.
Read [P5-LAYOUT-SLICE.md](P5-LAYOUT-SLICE.md). Verifier 8/8, lib 626/0/4,
bin 95/0, all 35 integration targets 162/0/11 ignored. Committed by the
parent under the phase-completion authorization; no push. All worker
ownership released. Phases 3/5/6 remain open; next gate needs user choice.

## Authorized checkpoint commit blocked — 2026-10-21

The user explicitly requested “commit it” after the saved-layout design slice.
Parent reviewed the accumulated71 changed/new paths as one checkpoint covering
the bounded background history writer/live adoption, fault/restart/cleanup and
workload evidence, diagnostics, final-tail/timeout metadata changes, corruption
policy/tests and saved-layout design/handoff. This is not phase completion;
phases3/5/6 remain open and the pure saved-layout codec is still next.

Staging failed before any commit: git add returned128 because creating
.git/index.lock was denied with Operation not permitted in this session's
filesystem permissions. No staged diff or new commit was created; nothing was
pushed and all work is preserved. Approval escalation is unavailable. The
explicit checkpoint authorization remains valid: when Git metadata is writable,
review current status/diffs and commit the checkpoint without asking again.
Suggested title: Checkpoint history persistence and saved-layout design.

Pre-stage checks passed: diff whitespace,174 Markdown files/0findings,
component fingerprints and unchanged96 source/test/manifest +13 report hashes.
Prior full checks retain their recorded failures/ignored cases; no full suite
rerun or new pass is claimed for this commit-only task.

## Current saved-layout/restart design — 2026-10-21 (read this first)

Completed the queued design phase on the user's instruction with parent
architecture/source review and an independent Luna xhigh reviewer. Read the
accepted scoped [ADR](P5-SAVED-LAYOUT.md),
[independent review](P5-SAVED-LAYOUT-REVIEW.md) and exact queued
[codec task](P5-SAVED-LAYOUT-CODEC-TASK.md). All ownership released; preserve
prior dirty work. Canonical SPEC0.13 reflects the design, not enabled recovery.
No source/test/manifest/dependency/vendor changes, commit or push. ms-rust absent.

The design uses exact160-byte historical records in current/previous slots.
Prior current is durably archived before first higher-generation current save,
so normal fresh-shell startup does not erase the only preceding saved layout.
Old full targets remain read-only history links, never new live identities.
No launch/cwd/args/env/provider/title/input/focus data is persisted. Empty needs
positive owner-serialized Exited state for all layout panes; driver teardown
freezes topology before stopping children. Independent sequence orders final
empty without assuming the last-leaf layout revision changes. Same-generation
changed nonempty records require greater revision; final empty is terminal.

Future store/worker binds generation/workspace/exact directory to held storage,
has one writer per disjoint file stream, bounded3x160 encoded candidates and
one final nonblocking deferred try-submit with explicit unsubmitted status.
History/layout share one total250ms metadata-drain wait and retain singleton
exclusion through late IO. Read-only CLI must dispatch before live startup.
All these are design/acceptance requirements, not current application behavior.

This turn ran existing geometry filter17 and owner filter50:67passed/0failed/
0ignored. Component fingerprints match, docs174files/0findings and diff checks
pass;96 baseline Rust/test/manifest files and13 reports have unchanged hashes.
The first independent-note docs check found one trailing-whitespace line; it
was corrected before the final check. Full verifier/integration was not rerun
for docs-only work; previous7/8 and86passed/17failed/11ignored retain their
known socket/host gates and are not new saved-layout passes. Proposed tests in
the design have not run because no saved-layout implementation exists yet.

Next: implement ONLY the pure saved-layout codec/model under its exact task,
then parent export/verification. Store/rotation, guarded async writer, owner/
driver integration, read-only current/previous CLI and broader recovery follow
as separate slices. No new CLI command, automatic history recovery, restored
layout UI or process/provider relaunch is implemented. Pending hierarchy,
provider identity, representative workload, full reboot and phases3/5/6 stay open.

## Previous committed-record corruption policy — 2026-10-21

Delivered on the user's instruction with one Luna xhigh for public-store tests
and parent policy review, independent CLI acceptance and full verification.
Read [P5-COMMITTED-CORRUPTION.md](P5-COMMITTED-CORRUPTION.md) and its
[task](P5-COMMITTED-CORRUPTION-TASK.md). All ownership released; prior dirty
work preserved. No production source, format, manifest, dependency, vendor,
quota or timeout change. No commit/push. Graph refreshed fast/nonpersistent;
ms-rust absent. Canonical spec is revision0.12.

Committed rows/screens are accepted only as fully structurally valid files.
Malformed/partial files are preserved and refused, including complete-row
prefixes missing declared rows. No salvage/repair/temporary promotion. Combined
show/search fails the requested target with no partial output if an existing
component is invalid; healthy targets and independent buffer APIs stay usable.
No checksum exists: a format-valid text alteration can remain readable. These
limits are explicitly demonstrated, not hidden behind a corruption guarantee.

Parent reran6 store+2 fresh CLI tests:8passed/0failed/0ignored. Full verifier7/8,
library551passed/4ignored and binary94passed/1failed on pathname socket EPERM.
All31 integration targets ran:86passed/17failed/11ignored, none incomplete.
The same four host/startup targets fail; five directly report listener permission,
eleven cannot start the server before raw mode, and one is the known host stderr
assertion. Strict Clippy/fmt/build/rustdoc, Python45, decoder39, fingerprints,
doctest command(0cases), diff and docs171/0 pass. Fixture development corrections
are recorded in the parent/owner results. All65 baseline source/manifest files
and13 prior raw/provenance reports have unchanged hashes. No new workload run.

Next: design bounded saved-layout/restart metadata for the current two-pane
prototype; preserve pending hierarchy and provider decisions. Full machine
reboot/layout, representative workload, lost payload, unknown/unsubmitted end
intent and host/provider acceptance remain open. Corrupt-file salvage/export
and integrity-format migration are separate, unimplemented options. Parent
phases3/5/6 remain open; read-only history never replays terminal effects.

## Previous canceled end-metadata slice — 2026-10-21

Delivered on the user's explicit Luna request at xhigh. Parent approved the
policy, reviewed source, added independent store-helper and worker acceptance,
and ran two sequential release workloads. Read
[P5-DRAIN-METADATA.md](P5-DRAIN-METADATA.md) and its
[bounded task](P5-DRAIN-METADATA-TASK.md). All ownership released; prior dirty
work preserved. No commit/push, dependency, manifest, vendor or storage-format
change. Graph refreshed fast/nonpersistent after source freeze; ms-rust absent.

Explicit shutdown timeout alone enables best-effort gap/end metadata writes for
accepted canceled known-end batches on the retained guarded worker. No canceled
row/screen stage starts, canceled receipts/cursors do not become committed, and
the drain stays TimedOut. Plain Drop remains strict. The atomic helper preserves
compatible rows/screens and first timestamp, rejects unsafe/invalid records,
and stops future finalizers on terminal failure. At most16 accepted items with
three attempts each; caller wait remains250ms, filesystem wall time unbounded.

Both raw reports validate; all12 target observations have settled ends. Two
cases still time out and report canceled work/pending receipts plus actual late
metadata success. Both bursts still report final-payload byte loss. All96 probes
overlap producer output. End presence after lock settlement does not establish
complete payload, durability at Drop return, or a performance-budget pass.

Verifier7/8; library551passed/4ignored, binary94passed/1failed on socket EPERM.
Strict all-target Clippy/fmt/build, Python45, decoder39 and component sources
pass. Additional integration/rustdoc/final docs results are in INTEGRATION.md
and the result note. Store fault/refusal3/3 and independent real-store2/2 pass.
No new production failure required correction during parent full verification.

Next: review committed-record corruption/partial-tail policy with bounded
fault/restart acceptance. Unknown or unsubmitted end intent, payload loss,
unbounded filesystem calls, full reboot/layout and representative workload
remain open alongside socket/host/provider acceptance. Parent phases3/5/6 stay
open. Preserve current quotas/budget and never infer implicit corrupt repair.

## Previous bounded final-tail slice — 2026-10-21

Delivered the explicit Luna assignment at xhigh with parent design/source
review, independent real-store acceptance, two shipping-policy release runs,
a matched suffix-disabled control and integrated verification. Read
[P5-FINAL-TAIL.md](P5-FINAL-TAIL.md). All owners released files; prior dirty
work is preserved. No dependency/format/commit/push change. Graph connector
recovered; parent rebuilt/refreshed a fast/nonpersistent index after edits.
ms-rust remains absent.

A genuine final byte rejection gets one nonblocking attempt at the newest
complete-row suffix that fits with ALL candidate screens and the first end
time. Invalid originals are rejected before trimming; snapshot contention is
not an admission diagnostic. Dropped prefixes remain gap/failure. No extra
retained row queue, quota increase or capacity wait;16batches/32MiB and the
single250ms drain stay unchanged. A successful receipt alone advances durable
state. Parent actual-store acceptance verifies exactly685 newest rows through
ordinal900 in epoch2, both80x240 screens, first end777 versus repeated999, and
submitted/durable separation. It passes after the final plan refactor.

LIVE RELIABILITY REMAINS OPEN: shipping run1 times out in burst/shutdown;
run2 in all3 cases. Each timeout records canceled work/pending receipts and
one end marker missing after lock settlement. A same-runtime isolated copy
with only suffix retry disabled also times out in both wide cases and misses
one end each. Thus suffix retry is not necessary to reproduce the wide-case
failure; one control cannot exclude extra write cost or quantify regression.
All144 probes overlap producer output. Exact raw reports and control provenance
are retained. Do not claim a general reliability/performance or ten-agent pass.

Final verifier7/8; library540passed/4ignored, binary94passed/1failed (socket
EPERM). All29 integrations ran:78passed/17failed/11ignored, no incomplete
case. Strict all-target Clippy/fmt/build, strict rustdoc, decoder39, Python44,
component fingerprints and docs166/0 pass. The first default root run failed an
existing FIFO fixture on valid accounting contention; a bounded test-setup
retry with the same owned batch/token fixes it, without production retries.
Original failure and corrected run are recorded in INTEGRATION.md. Known
socket/host failures and historical lock-probe uncertainty remain explicit.

Next: review bounded drain/final-metadata scheduling and cancellation policy
using this reproduced timeout evidence. Do not enlarge quotas or the250ms
budget without reviewing the contract. Committed-record corruption/partial-tail
policy, full reboot/layout, representative workload, provider identity and
host acceptance remain separate. Parent phases3/5/6 stay open.

## Previous history diagnostics slice — 2026-10-21

Delivered the user's explicit Luna assignment. Read
[P5-HISTORY-DIAGNOSTICS.md](P5-HISTORY-DIAGNOSTICS.md). Luna xhigh froze the
production changes, then hit its usage limit; parent finished attribution
tests, public API/CLI/example integration, review, two sequential release
measurements and verification. All ownership is released. Preserve all prior
uncommitted work. Graph Transport closed required source fallback; no fresh
index or ms-rust installation is claimed.

Nineteen fixed reason bits and actual writer drain outcome survive driver
teardown through a pre-enable observer. No content, IDs, paths or error text
enter diagnostics. Ordinary Full remains potentially harmless; final loss has
its own paired cause. No quota, scheduling, retry, loss-policy, format or250ms
budget change was made.

Two schema2 actual-store release reports are preserved alongside the unchanged
three historical schema1 reports. Run2 burst reproduced
admission_bytes_full + final_payload_dropped_bytes, with writer drain=drained.
Run1 burst was clean. Both shutdown cases were clean; all six cases had settled
end markers. Thus the reproduced failure is byte-pressure final-payload loss;
the older missing end markers remain unexplained. All96 probes overlapped
producer output. These remain bounded synthetic observations, without a
representative workload/release or host UI performance budget.

Current root checks: library535 passed/4ignored; binary94passed/1failed
(pathname socket EPERM), verifier7/8. Strict all-target Clippy/fmt/build,
Python44 including13 report tests, decoder39, component fingerprints and strict
library rustdoc pass. See [INTEGRATION.md](INTEGRATION.md) for the independent
integration sweep and final documentation count. The earlier parallel lock
probe failure did not recur in this default run; its historical cause is still
unconfirmed. No failed/ignored case is acceptance.

Next: review final-payload admission under byte pressure and test a bounded
way to preserve the final tail before changing policy or quotas. Keep the
current history budget and memory contract. Parent phases3/5/6, socket/host,
provider identity and full reboot/layout gates remain open. No commit or push.

## Previous sustained-output measurement — 2026-10-21

Delivered the requested bounded live history benchmark with one Luna xhigh
harness/report owner and parent-owned validator, independent measurement and
review. Read [P5-LIVE-HISTORY-BENCH.md](P5-LIVE-HISTORY-BENCH.md) for actual
measurements, raw report links and limits; the original contract is
[P5-LIVE-HISTORY-BENCH-TASK.md](P5-LIVE-HISTORY-BENCH-TASK.md).

Two final release runs cover paced 80-column output, burst 240-column output
and active-output shutdown with two 24-row real PTYs and the actual store.
Authenticated split admission and ScreenQueries verify both live pane sizes;
post-settlement saved rows/checkpoints validate identity, isolation and bounds.
All 96 probes began and were child-acknowledged before observed producer
completion. Across the final cases, tick p95 ranges 164–331 us and child-ack p95
4,138–15,470 us (16 probe samples/case, so p95=max). These are synthetic
socketpair observations, with no chosen release/UI/ten-agent quality budget.

Both final paced cases had no gap/failure; both final burst cases raised the
sticky flag. All final end markers were present. A preserved preliminary
parent report, before the added geometry/content checks, had missing end
markers after settlement in both wide cases. The variation and unsuccessful
attempts remain recorded. Saved gap flags do not distinguish retention,
overflow or pressure, and the public signal cannot identify failure/drain
reasons. Drop timing includes child cleanup, and reads happen after the
writer lock settles; they do not acknowledge writes by Drop's return.

Checks: full verifier 7/8; library 531 passed/4 ignored; default binary 92 passed/
2 failed. The known pathname socket EPERM remains. An additional parallel
lock-probe assertion failed; it passed in isolation and in a serialized binary
run 93 passed/1 failed. Cause is not established and no failure is hidden.
Actual-store live integration 3/3 passes; Python 40 (including 9 new report tests),
decoder 39, all-target strict Clippy/fmt/build, component fingerprints and
docs 162/0 pass. See [INTEGRATION.md](INTEGRATION.md). The previous full 28-target
integration sweep is historical and was not repeated for this harness-only
change. All 63 application/manifest fingerprints are unchanged.

All source ownership is released. No runtime, dependency/vendor, commit or
push change; preserve accumulated uncommitted work. Parent phases 3/5/6 and
full host/provider/reboot/layout gates remain open. Graph transport remains
closed; no current index refresh is claimed.

Next: specify bounded sanitized failure/drain reasons, then diagnose burst
capture flags and variable final tail/end records with this harness. Review
scheduling/loss changes before altering queue quotas or the 250 ms history
budget. Keep the parallel lock-probe failure and existing socket/host gates
explicit; committed-record corruption and full restoration remain separate.

## Previous live-writer integration — 2026-10-21

Delivered the explicitly requested Luna xhigh live capture slice with parent
integration/review and a read-only Luna lifecycle review. Read
[P5-LIVE-WRITER-RESULT.md](P5-LIVE-WRITER-RESULT.md) for actual evidence and
[P5-LIVE-WRITER-TASK.md](P5-LIVE-WRITER-TASK.md) for its bounded contract.
Both initial and split-created PTYs now submit to one writer; live history
filesystem work runs off the owner thread after startup preparation. Encoding
and terminal sampling remain on the owner thread. Submitted/durable cursors
and screen identities are separate, with at most eight target slots and two
deferred screens per target. Final metadata survives dropped payloads and
pane retirement; pressure marks a visible gap/failure, and a full capture
registry cannot reject a valid split. Children stop before a single250ms
history drain; receipt waits/writer shutdown do not hold the coordinator lock.
The CLI observes shutdown failure after driver Drop.

Final checks: root library **531 passed/4 ignored**, binary **93 passed/1
failed**, full verifier **7/8**. All **28 integration targets** independently
ran, serial within each target: **76 passed/17 failed/11 ignored**. All nine
new live cases pass: coordinator3/runtime3/actual-store integration 3. Strict
all-target Clippy/fmt/build, strict library rustdoc, zero-doctest command,
decoder 39, Python31, fingerprints and docs160/0 pass. The failure set is
unchanged: pathname bind EPERM and four runtime targets with sixteen startup
failures plus the previously reproduced host diagnostic assertion. No ignored
or blocked case is counted as a pass. See [INTEGRATION.md](INTEGRATION.md).

The parent independent public-driver tests save rows and both buffers before
natural exit, then prove final/end capture and lock release on driver Drop.
Blocked/failing fake backends prove control/input/split progress, quota fallback,
first end-time preservation and delayed durable receipts across epoch reset;
they are separate from actual-store durability evidence. Initial sync retry
regressions2/2 were fixed by retaining sync-local failure semantics. Review
also fixed lock-held waits and a shutdown sticky-error race. No remaining
lifecycle blocker was found; a failure race can waste one bounded encoding
before the next receipt poll. No performance budget or machine-reboot claim.

All source ownership is released. No new dependency/vendor change, commit or
push; preserve accumulated uncommitted work. Phase3/5 and full host/provider/
reboot/layout gates remain OPEN. The graph connection remains closed; source
and tests are current, without a refreshed-index claim.

Next bounded task: sustained-output measurement through live scheduling,
recording owner progress, committed history gaps and shutdown outcomes. Keep
committed-record corruption policy and full V-HISTORY acceptance separate;
do not infer a release throughput or UI latency pass from mock scheduling.

## Previous writer component slice — 2026-10-21


Delivered the bounded [background history writer](P5-HISTORY-WRITER.md), using
one Luna xhigh implementation owner and a read-only Luna lifecycle reviewer,
with parent-owned design, lock support, independent tests and source review.
The component caps accepted work at 16 batches/32 MiB encoded payload including
in-flight work, owns sanitized data, and exposes token-identified durable
receipts. It retries only failed stages and fails closed on exhaustion. Bounded
shutdown may detach a blocked worker; the worker retains the singleton lock
and an exclusive writer claim through storage teardown. A second writer and
a history store from another server domain are rejected.

At this previous component checkpoint, the writer was not wired into
`PtyEffects` or `MultiServerDriver`; live capture remained synchronous. The
subsequent slice above delivers that adoption with submitted/durable cursor separation, bounded reserved gap/end
state surviving pressure and pane retirement, and blocked-writer PTY/control
progress tests. No UI latency improvement is claimed by an unwired component.

Parent review also found/fixed existing store retries after byte eviction and
across sparse retained ordinals, plus durable gap recording on duplicate-only
rows. Two regressions reproduced `InvalidData` before reaching a second
injected directory-sync failure; the corrected ordered merge passes. All
incoming records remain validated; retained conflicts, interior backfill and
entirely stale evicted batches are rejected. Discarded cells are not recovered
or compared. Earlier throughput timings describe the preceding source.

Actual checks: all **25 new cases pass** (14 writer units, 6 writer integration,
3 store retry, 2 storage-lock units). Final root library **525 passed/4 ignored**;
binary **93 passed/1 failed**, with the same socket bind EPERM. The integrated
verifier remains **7/8**. All 26 integration targets ran independently, serial
within each target: **73 passed/17 failed/11 ignored**. The same four runtime
targets fail: 16 server-start failures and the host diagnostic assertion
previously reproduced at unchanged HEAD. Strict all-target Clippy/fmt/build,
strict library rustdoc, zero-doctest command, decoder 39, Python31, component
fingerprints and docs158/0 pass. See [INTEGRATION.md](INTEGRATION.md).

The parent baseline of 23 existing history integration tests passed. Initial
component compile/lint issues and a parent 2-row snapshot fixture (below the
supported terminal minimum) were corrected before the passing runs. Parent
review fixed quota-release order, panic/submit receipt cleanup and duplicate
writer exclusion before final verification. No test was ignored to hide a
failure; host/provider/reboot gates remain open. No dependency/vendor change,
commit or push was made, and all previous uncommitted work is preserved.
Both owners released their files. The parent graph connection remains closed;
the reviewer's final fast/nonpersistent refresh also returned `Transport closed`.
Source/tests are authoritative and no current graph refresh is claimed.

## Previous cleanup/throughput continuation — 2026-10-21

The user requested the next phase after the retry/read task. Previous
uncommitted code and evidence remain intact. This task adds bounded discard
of recognized uncommitted history temporaries, startup cleanup before capture,
an explicitly stopped-workspace `history cleanup` command, and measured
synthetic write throughput. It does not promote temporary contents, repair
corrupt committed records, settle layout hierarchy or enable provider resume.

Both Luna xhigh owners finished and released their files. The parent reviewed
the cleanup and benchmark source, added the CLI/runtime wiring and tests, and
independently verified 28 focused cases: store units 4, cleanup integration 6,
CLI 3, startup helper 1, existing store 10, fresh recovery CLI 1 and screen
checkpoints 3. All pass. A first parent CLI run caught a reversed parser tuple
(1 pass/2 failures); it was fixed before the passing run. Unknown/unsafe
temporary names do not authorize repair or removal of committed records.
The invalid-UTF8 filename branch could not get an actual APFS fixture because
creation returned EILSEQ; that is not counted as exercised.

The final integrated verifier remains **7/8**: root library 506 passed/4 ignored,
binary 93 passed/1 failed at the same socket bind EPERM. All 25 integration
targets were run independently: 67 passed/17 failed/11 ignored, with the same
four failing runtime targets (16 server-start failures and the earlier host
diagnostic assertion reproduced at unchanged HEAD during the previous task).
Strict Clippy/fmt/build/rustdoc, decoder 39, Python31, component fingerprints,
documentation checks and zero-doctest command pass. Nothing ignored, failed
or environment-blocked was promoted to acceptance.

Two actual release benchmark runs with 24 operations per case are recorded
in [P5-HISTORY-THROUGHPUT.md](P5-HISTORY-THROUGHPUT.md) and its raw JSON links.
Median append storage-call times span about 22–49 ms across those runs/cases;
the widest case retains 722 rows below 8 MiB. These are synthetic storage
observations, not UI latency, ten-agent throughput or a release-budget pass.
Cleanup policy and actual commands are in
[P5-TEMP-CLEANUP.md](P5-TEMP-CLEANUP.md).

No dependency or vendored component changed, and all earlier uncommitted work
was preserved. No commit/push was made because parent phases and full required
checks remain open. No live agent ownership remains. The graph connection
closed during lookup and final refresh; source and test evidence are current,
but the graph index is not claimed refreshed for this task.

Next: specify/review bounded history writes off the owner thread while retaining
cursor acknowledgment, gaps, end-time retry and bounded shutdown guarantees.
Full reboot/layout, committed-record repair policy, representative workload,
host and provider identity gates remain open.

## Previous retry/read task — 2026-10-21

The user explicitly resumed implementation and requested parent orchestration
with GPT-6-Luna for suitable bounded work. The starting tree was clean at
`0fec6a4` on `main`, tracking `origin/main` with no ahead/behind difference.
The parent selected P5's post-rename/failed-end-sync evidence and read-only
restart acceptance as the next task. Full reboot/layout recovery, interrupted
temporary-file repair, throughput and provider/host acceptance remain open.

Initial parent checks: the existing history-store tests passed 10/10, row
codec 1/1 and cursor 6/6. The serial close/runtime suite failed 0 passed/5
failed before reaching its history assertions: attach could not start the
server. An independent AF_UNIX pathname bind returned errno 1 (`EPERM`);
direct `serve` returned exit 1 with `listener_permission_denied`. These are
environment-blocked runtime checks, not passes or new history regressions.

Both bounded GPT-6-Luna xhigh owners finished and released their files. The
parent reviewed the changes and requested the same-observation checkpoint
metadata fix, then independently reran 19 focused checks: store faults 3,
real-PTY retries 2, fresh CLI recovery 1, history store 10 and checkpoints 3.
All passed. Retry acknowledgments now require directory sync, first end times
remain unchanged and screen retries preserve their original capture metadata.
See [P5-RECOVERY-FAULTS.md](P5-RECOVERY-FAULTS.md) and
[P5-RESTART-ACCEPTANCE.md](P5-RESTART-ACCEPTANCE.md).

Final integrated verifier: **7/8**, with library 505 passed/4 ignored and
binary 92 passed/1 failed. Cargo stops at the existing pathname bind test's
`EPERM`; the other seven checks pass. The parent therefore ran the integration
targets independently: 58 passed, 17 failed and 11 ignored. Sixteen failures
cannot start the socket server; one host stderr diagnostic assertion also
fails at the unchanged starting commit `0fec6a4`, verified from a disposable
`git archive` checkout. Its cause remains unresolved. Strict rustdoc passes.
The first verifier run was 6/8 because the new evidence link was added before
its target file existed; the completed note and final docs check resolve that
finding. Full outputs and scoped limits are summarized in
[INTEGRATION.md](INTEGRATION.md). No failing or ignored case is counted as a
pass; the previous 8/8 run belongs to the earlier environment/checkpoint.

No dependency or component source changed. The graph was refreshed from this
working tree (fast, nonpersistent). No agent ownership remains; no commit or
push was made because the parent phases and required full checks remain open.
Next: safe interrupted-temporary-file cleanup/recovery and bounded-history
throughput evidence, followed by the remaining reboot/layout gates. Preserve
these uncommitted changes on the next resume.

## Earlier pause handoff — 2026-10-21 (superseded by the resume above)

**Paused at local `main` commit `f82d968`** after the user requested a stop.
At the start of this handoff, the working tree was clean; `main` was three
commits ahead of `origin/main` (`e1126c7`, `c3e207e`, `f82d968`). These commits
were authorized locally; no push was requested. Check status again before
editing or any source-control action. This handoff/documentation refresh is
post-`f82d968` work and should not be mistaken for another source phase.
The user subsequently requested a push on 2026-10-21; inspect remote
tracking state after publication rather than treating the earlier no-push
observation as the current instruction.

**Done within their explicitly bounded scope:** Phase 3A two-pane splits
(`e1126c7`); Phase 3B individual pane stop/close, auto-collapse, re-split and
CLI/menu actions (`c3e207e`); P5 sanitized visible screen checkpoints, bounded
normal scrollback and read-only `panelink history list|show|search`, plus
stopped-workspace `clear` (`c3e207e`, `f82d968`). The later user-requested
independent reviewer found close-output, row-retry, clear-lock, end-time and
cursor issues. The parent addressed them before `f82d968`; see
[P5-SAVED-HISTORY.md](P5-SAVED-HISTORY.md). The final eight-check verifier
passed 8/8 (root library 500 passed/4 ignored, binary 93 passed, 15 Rust
ignored overall; cursor 6/6, history store 10/10, screen-checkpoint 3/3,
close/history runtime 5/5, decoder 39, Python 31). Strict Clippy, fmt,
rustdoc and component fingerprints passed. Earlier test failures and their
reruns remain recorded in the evidence; ignored tests are not acceptance.

**Open gates, not phase-completion claims:** parent Phase 3 still needs wider
event/watch/launch/layout contracts and host acceptance; parent Phase 5 still
needs full V-HISTORY reboot/layout and partial-tail acceptance. A deterministic
post-rename fsync-fault test, expiry after actual end-time sync failure,
interrupted temporary-file repair and representative disk-throughput review
remain unproved. Phase 2 Apple Terminal/clipboard sentinel acceptance and
Phase 4 live OpenCode identity/session-switch proof are also open. Normal
scrollback/checkpoints are distinct from provider conversations; no stored
terminal effects are replayed. The firm review, workspace hierarchy and
broader release options remain unresolved.

**On explicit resume:** read [ROADMAP.md](../../ROADMAP.md), AGENTS.md,
[SETUP.md](../SETUP.md), [P3B-CLOSE-RUNTIME.md](P3B-CLOSE-RUNTIME.md),
[P5-LIVE-CAPTURE.md](P5-LIVE-CAPTURE.md) and
[P5-SAVED-HISTORY.md](P5-SAVED-HISTORY.md). Inspect the current Git tree and
run targeted tests for the chosen gap. Use `python3 scripts/verify_non_ui.py`
for the integrated suite, counting ignored/blocked cases separately. Follow
the later user instruction to implement directly unless they specifically
request an agent. Await that instruction before beginning another phase.

## Historical checkpoints (superseded where inconsistent with the above)

Authority: user requested implementation, parent orchestration, gpt-6-luna xhigh where possible, and durable status for interruptions.

P1A, P1B and P1C source tasks are finished. Parent reviewed integration and independently verified 21 passing tests, formatting and Clippy with warnings denied. See INTEGRATION.md for actual commands, limits, fixes and next step. All task ownership is released after reports; no active agent is required for resumption.

Initial Rust absence was resolved during the session; Rust/Cargo 1.99.1 now tested and recorded. Cargo generated a dependency-free lockfile. ms-rust still absent. Cargo registry DNS remains unavailable, preventing fetching the next phase's terminal-engine candidates. No implementation commits or pushes attempted; preserve existing planning edits and current working tree.

ROADMAP.md is authoritative. Resume follow-up: preliminary exact dependency candidates and the phase 2 experiment are now recorded in P2-DEPENDENCIES.md and P2-EXPERIMENT.md. Parent reran the 21 foundation tests, formatting and Clippy successfully. Cargo registry DNS is still unavailable and OpenCode is still absent from active PATH. Next: obtain sources, resolve/review the full dependency graph and portable-pty spawn/cleanup behavior, adopt pins, then implement P2B. Alacritty's convenience PTY launcher cannot meet the explicit environment/cwd contract unchanged. No source/manifest changes, commits or pushes in this follow-up. The working binary currently only supplies help/version/doctor, not a multiplexer. No corporate approval or OpenCode recovery claim.

Latest resume supersedes the access blocker above: Cargo registry access restored; use a writable CARGO_HOME under current restrictions. Downloaded/resolved candidate sources reveal failed engine allocation and backend launch/cleanup gates. See P2-SOURCE-GATE.md and p2-review/ for exact evidence and durable resolution artifacts. Independent Luna xhigh review confirmed the engine gate failure; no agent owns files now. Foundation tests (21), fmt and Clippy pass again. P2B remains unstarted, with application sources/manifest/lockfile unchanged. Next action is viable replacement/upstream-correction review, followed by full dependency review before adoption. No commits/pushes or host/PTY/provider compatibility evidence.

Active replacement resume: P2-REPLACEMENT.md now approves/adopts the scoped core graph and local VT100 correction, with notices/provenance preserved. Independent disposable probes passed nine engine and four PTY checks, rerun by the parent. Core implementation is assigned to one Luna xhigh owner (src/terminal/**, tests/p2_pty.rs, P2-CORE-RESULT.md) under P2-CORE.md; parent owns shared files. Host input/rendering/manual Apple Terminal checks remain pending. Root lib export now references the in-progress terminal module: if interrupted before the owner finishes, inspect that module and rerun integration before assuming the working tree builds. No commits/pushes.

Latest completed checkpoint: the bounded replacement library core is implemented and parent-verified; see P2-CORE-RESULT.md. All retained lifecycle allocations now precede spawn; shutdown closes the controller and rejects later IO; dead input/replies are discarded and WriteZero is a hard failure. Exact paste bytes, fixed OSC/combining bounds, final output draining and handshaked HUP→KILL cleanup have regression coverage. Final locked/offline suite: 30 library + 1 binary + 11 PTY = 42 passed; fmt/Clippy exit 0; diagnostic CLI exits checked. Cargo registry search works with the writable temporary CARGO_HOME. OpenCode remains absent. No commits/pushes.

The follow-up agents hit a usage limit after partial core edits/source findings; parent finished/reviewed those edits and recorded results. No live agent ownership remains and no agent must survive to resume. Follow ROADMAP.md next-action: specify/review the bounded host input correction and its complete graph before adoption; P2-HOST-INPUT.md records source findings and acceptance criteria. Then implement snapshot rendering/restoration and the host probe. Apple Terminal compatibility, persistent server/reconnect and OpenCode identity remain unproved. The executable still has help/version/doctor only. Preserve the working tree; do not infer Phase 2 completion from the passing core library suite.

Latest host-component checkpoint: crossterm-input0.29.0-panelink.1 parser/types-only extraction is adopted under vendor/crossterm-input, with originals/patch/provenance/fingerprints/MIT notice. No upstream host framework is included. Parent independently reran39 decoder tests and verified13 fingerprints; root final suite49 passed (37lib+1binary+11PTY), fmt/Clippy/expected CLI exits passed. Pure streamed renderer has7 focused tests; maximum styled grid measured1,506,040 bytes. See P2-INPUT-REVIEW.md and latest INTEGRATION.md. A query/exit race in a drain test fixture was corrected; the test now measures normal EOF drain only. All agent ownership released; no temporary candidate source is required to resume, no commits/pushes.

Resume ROADMAP.md next-action: specify bounded owned-TTY host IO and restoration guard, then CLI pty-probe with outer-PTY EOF/error/panic/non-TTY tests before Apple Terminal validation. No host IO/raw-mode/restoration/interactive probe exists yet. The diagnostic binary still cannot run a managed terminal session. Phase2/M1 remain incomplete. Do not use unbounded Crossterm global readers or repeat the rejected candidate review; preserved parser-only component and snapshot renderer are the accepted building blocks.

Latest host probe checkpoint supersedes the unimplemented-host notes above: binary-private `src/p2_probe.rs` and the strict `pty-probe --terminfo <absolute-directory> -- <executable> [arguments...]` CLI are implemented and source-reviewed. Parent independently verified 66 application tests passing, nine device-dependent outer-PTY scenarios explicitly ignored, unchanged component's39 decoder tests passing earlier this resume, fmt/Clippy/CLI exits/fingerprints. See latest INTEGRATION.md and P2-PROBE-RESULT.md. Signal-hook0.4.5 defaults=false adds process-lifetime flag handlers only; no unregister in bounded cleanup. Exact graph/notices are preserved.

All ownership is released and no live agent is needed to resume. Cargo registry access was rechecked successfully again. The harness establishes a controlling session, but the probe's separate `/dev/tty` open returns EPERM in this sandbox. Do not duplicate stdio or weaken nonblocking ownership to bypass it. Run the nine ignored host scenarios in an environment with device access, add the ready-child restore-before-wait observation, then complete the Apple Terminal checklist in P2-EXPERIMENT.md. The existing unwind test only establishes eventual restoration. Phase2/M1 remain incomplete; OpenCode absent; no source-control publication occurred. Preserve all working files.

Latest operator-debug checkpoint: user ran the nine ignored host cases in Terminal, with4passed/5failed (four post-exit slave ENOTTY assertions, one no-drain process timeout). See P2-OPERATOR-RUN.md. The harness now keeps a controlling-session supervisor alive across probe exit/assertions, then drains/reaps only during teardown; preserves bounded metadata/process cleanup; finite overflow/tail producer; HUP-readiness marker. Two new host fixture regressions and an application-loop output-stall regression pass. Safe-Rustix experiments reproduce session-leader exit waiting for unread output, and retained-slave ioctl revocation; nonleader exits promptly. This supports the correction but does not establish that the complete corrected operator suite passes.

Parent final locked/offline application suite69 passed /9ignored, fmt/strict Clippy pass; graph refreshed. See latest INTEGRATION.md, P2-HOST-DEBUG.md and P2-STALL-DEBUG.md. No production semantics/dependency changes, commits/pushes; all agent ownership released. Resume with serial external command `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`, then remaining timing/manual Apple Terminal gates. Local sandbox still denies the full probe's independent /dev/tty open.

Latest operator confirmation: user replied "all corrected" after the requested host-suite rerun. Recorded as confirmation of resolution of the reported failures, with no new raw log/counts/timings. See P2-OPERATOR-RUN.md; prior failure history is retained. Next gate is manual Apple Terminal validation and the outstanding restoration-before-child-wait observation. Phase2/M1 remain incomplete. No source changes, tests, commits or pushes in this confirmation turn.

Latest UX resume checkpoint (2026-10-17): user reports most manual checks were fine, without identifying each completed item. Ctrl-Q intentionally exited the entire disposable probe, interrupting the checklist; remaining child-cleanup/restoration-timing observation is deferred at their request. Full-screen redraw on each change was traced to unconditional erase/full paint and corrected: pure render_diff plus two maximum-capacity, pre-spawn snapshot caches; baseline advances only after the full pending target is accepted by host writes. Resize/discard invalidates it. See P2-DELTA-RESULT.md and latest INTEGRATION.md for actual 81-pass/9-ignored verification. User should retest the changed renderer when ready; remaining manual gates are incomplete. No active agent ownership, dependency changes, commits or pushes. Resume ROADMAP.md; do not discard the working tree or infer persistent server readiness.

Latest restore-order checkpoint (2026-10-17): explicit cleanup and unwind Drop now have runnable private-PTY tests observing exact visual reset and saved termios while a ready HUP-ignoring child is still in its shutdown grace. The negative reversed-order mutation failed as intended; production order remains restore-first. Parent review hardened complete-marker PID parsing and exact ESRCH checking. Final83passed /9ignored, fmt/strict Clippy pass; see P2-RESTORE-ORDER.md and INTEGRATION.md. This scoped timing observation no longer depends on a manual child fixture. Remaining Apple Terminal redraw retest and interrupted manual acceptance remain pending; no phase3 runtime, dependency changes, commits/pushes or active ownership. OpenCode still absent from active PATH at this resume.

## Ctrl-X and P3-A1 checkpoint — 2026-10-17

Operator reports the short redraw/Unicode/vi/resize/paste/Ctrl-C checklist works; Ctrl-Q produces a host window-close confirmation. Source has no host-window-close action; exact host mapping unknown. Added/documented Ctrl-X as quit with Ctrl-Q alias; actual decoder/dispatch test confirms local quit queues no input while plain x still reaches the queue. Live Ctrl-X/outer-shell restoration remains pending, and no separate clipboard/full-checklist completion is inferred.

Parent approved the bounded pure wire-envelope task and supervised Luna xhigh implementation. Original std-only codec enforces the exact24byte v1.0 header, six kinds, kind-specific payload caps, strict complete-frame lengths, borrowed payloads, redacted diagnostics and validated prefix-preserving writes. Parent reviewed full source and independently verified full suite **94passed /9ignored**, fmt, strict Clippy and diff checks. Broader plan/typed messages/socket authentication/ledger bounds/journal/server remain pending; no persistent runtime claim. All ownership released, no dependency/lockfile changes, commits or pushes. Resume ROADMAP.md and P3-WIRE-TASK.md/P3-WIRE.md; next independent task is parent specification of ledger/global limits and typed handshake, while operator tests Ctrl-X.

## Global ledger and agent-control planning — 2026-10-17

Parent-specified and supervised Luna xhigh P3-B1 delivered global admission caps, explicit expiry payload/terminal-metadata reclamation preserving pending completion, and monotonic volatile lease issuance. Parent reviewed full source/accounting tests and independently verified103passed /9ignored, fmt/strict Clippy/diff checks. Full-global-count duplicate/conflict assertions were strengthened and their focused test passed. Ledger ownership released; no dependency/manifest/lock changes, commits or pushes. See P3-LEDGER.md and P3-LEDGER-TASK.md.

During this task the user explicitly requested a Herdr-like easy API/command system for agents to discover/read/control other managed panes/processes. Recorded requirementR-15; a separate Luna xhigh owner researches official Herdr behavior and edits only docs/AGENT-CONTROL.md. Parent review requested stable target references, non-interleaving input ownership, reusable caller/lease context for short-lived CLI commands, and scoped collaboration access without per-key prompts. This design must inform typed schemas/server routes before implementation; it does not imply autonomous scheduling or a working control API. Inspect the plan/ROADMAP.md for latest owner/status; Phase2 Ctrl-X/clipboard evidence remains pending.

Policy/design steering: user requested four-direction pane splits in right-click menus; a dedicated design owner edits docs/PANE-LAYOUT.md, with parent review/canonical integration. Agent-control plan has completed its targeting/ownership/context refinements and awaits final parent integration. User also authorized parent commits after each fully completed/verified phase, including affected code/docs/evidence. No push authorized and workers remain no-commit. Pending phases are not marked complete merely to create a commit; do not stage mixed incomplete work. See AGENTS.md and ROADMAP.md.

## Agent-control/four-way split design handoff — 2026-10-17

Both design owners released files. Parent reviewed docs/AGENT-CONTROL.md (official Herdr behavioral references, independently written R-15 proposal) and docs/PANE-LAYOUT.md (R-16 four-direction menus/shared API). Refinements include exact target refs before input, reusable caller lease/sequence context, explicit input-owner CAS/handoff, layout-owned resize, dedup before stale-layout preconditions, tentative split IDs before committed watermarks, viewport-bounded menus and truthful resize-pending geometry. These are design contracts/proposed defaults, not implemented API/UI or completed host tests. Canonical spec/discovery and roadmap link both plans.

Document validation checked6documents and38local links with no missing links, trailing whitespace or conflict markers; git diff --check passed. Earlier code verification remains103passed /9ignored; no new production code was changed during final design review. Index refresh failed with Transport closed after the earlier owner refresh, so no final refreshed-graph claim is made. All ownership released. Next independent work is typed schemas/bound choices/socket API source review; Phase2 Ctrl-X/clipboard evidence remains pending.

User authorized parent commits after every fully completed and verified phase, including its affected code/docs/evidence. AGENTS.md records this override of older no-commit guidance; workers still do not commit, no push authorized. Current phases remain unfinished and no completion commit was made merely to checkpoint partial work.

## Typed control and event framing — 2026-10-17

Parent specified two disjoint Luna xhigh tasks: control_protocol.rs/P3-CONTROL.md and protocol_wire.rs/P3-EVENT.md. They delivered schema1 borrowed Request payloads for bounded Read/Text/Paste/Submit/Interrupt, full numeric target tuples and expected owner revisions; Event kind7 uses requestID0 with explicit payload/frame caps. Parent reviewed source, added a cross-component ledger-identity test, and independently verified **122 passed / 9 ignored** (86 library, 21 binary, 4 host fixture/preflight, 11 core PTY), fmt/strict Clippy/diff checks. No dependencies or manifest/lock changes. Transient worker compile/format issues were corrected and recorded before passing verification. All ownership released.

Pure codecs validate bytes and preserve nonreplayable input classification; they do not authenticate or resolve live targets, claim input ownership, run commands, send events, or provide the agent-control CLI. Initial server API launch requests will fit the complete 65,536-byte encoded budget or reject before effects; probe 1 MiB capture remains separate, staging deferred. Future queue accounting now includes 24-byte headers and control-reply reserve; no queue implementation exists.

Parent checked installed Rust 1.99.1 and Rustix 1.1.5 peer-credential entry points. std peer_cred compile check fails E0658 (unstable), Rustix socket_peercred is Linux-only. P3-SOCKET-GATE.md records source evidence and required stable safe macOS wrapper review before operational sockets; no manifest adoption or weaker identity fallback. Code-index tools remain unavailable with Transport closed, so source fallback and durable evidence are authoritative for this update. Next is wrapper/API review and typed handshake/owner/context/response contracts. Phase2 Ctrl-X/clipboard acceptance and Phase3 runtime remain incomplete; saved completion-commit policy remains, no new completion commit/push.
