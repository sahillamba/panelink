# Implementation session checkpoint — 2026-10-17

## Phase 4 complete — 2026-10-25 (read first)

The user resumed this checkout from `f1d7af9` with GPT-6-Luna implementation
and audit agents, parent orchestration and independent verification. All
scoped Phase4 roadmap gates now pass. The completion commit is recorded in
Git history; no push or live activation is authorized or performed.

Managed native acceptance passes both Luna and an independent parent run:
A-B-A while passive B stays at revision1, API-created new root, imported
nonempty-history fork and return, Home stale/return, actual foreign project
selection, visible structured manual refusal with A preserved, and guarded
same-TUI return to A with exact runtime/disk revision8 (previously7).
Parent observer sequences are foreign135, post-refusal foreign138, returnA152.
The CLI refusal is `manual_selection_required` / `provider_project_mismatch`,
without a ticket or navigation. Both runs submit zero prompts, verify absence
of user messages, enforce loopback-only sandboxing, and complete cleanup.

Durable sanitized reports are in [p4-evidence](p4-evidence/managed-native-parent-2026-10-25.json)
and [the Luna report](p4-evidence/managed-native-luna-2026-10-25.json).
The integrated result is [P4-INTEGRATED-RESULT.md](P4-INTEGRATED-RESULT.md).
No agents remain active; prior assignments below are historical.

Parent verification: locked/offline serial Rust **1231pass/0fail/15ignored**,
63 result groups; all seven supporting checks pass, decoder39, component
fingerprints, fmt, strict all-target Clippy, isolated build, Python **232run /
231pass /1 opt-in native skip**, Markdown **332/0**. Logs are under
`/private/tmp/panelink-p4-20261025`; final documentation changes also pass
Markdown and whitespace checks. The explicit native run closes the gate
separately from the intentionally skipped opt-in unittest. Luna final review
found no material issues; parent reviewed the query/observer/refusal gates.

The accepted isolated binary SHA is
`1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`;
the active workspace binary remains
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
The exact pinned native OpenCode2.0.32 SHA remains
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`;
launcher used `~/.opencode/bin/opencode` after PATH lookup. No Rust, shipped
plugin or dependency changes were needed after the implementation checkpoint.

Earlier failed attempts and corrections remain in the dated P4 startup,
plugin-merge, picker, observer, exact-query, modal-rows, foreign-refusal and
exact-return notes. They are not relabeled as passes. The harness now keeps
server initialization directories empty, composes both plugins, continuously
drains attach output, uses stable fresh modal reads, bounds actual result
rows, and uses the supported exact-ID native picker query for return.

Next: follow Phase5's remaining hierarchy/interaction and full V-HISTORY
acceptance work in ROADMAP.md. Preserve unresolved product decisions. The
separate Phase3 human four-pane Apple Terminal acceptance remains open.
Phase4 completion does not close M1, imply overall readiness, prove a machine
reboot/power-loss boundary, guarantee current-at-crash identity, or enable
automatic resume/replay. No push or user configuration changes.

## Phase 4 resumed — 2026-10-24 (read first)

The user resumed continuous Luna slices until Phase4 completion after checkpoint
`f1d7af9`. This revokes the pause below. Current owners: managed picker probe
(private fixture input/keybind only), managed transition fixture helper/review,
and parent integration/review/final verification. Historical acceptance remains
1231Rustpass/15ignored, Python200(one opt-in live skip), docs319/0; no automatic
resume/current-at-crash guarantee. Finish real managed normal UI transitions;
explicit manual recovery and native bridge-failure gates already pass Luna and
independent parent. Keep building in the isolated P4 target. No push or live
activation. See P4-CONTINUOUS-TASK.md and the preserved paused handoff for detail.


## PAUSED by user — 2026-10-24 (read first)

User requested a pause and durable handoff. All active agents were stopped.
Read [P4-PAUSED-HANDOFF-2026-10-24.md](P4-PAUSED-HANDOFF-2026-10-24.md)
for the exact stopping point and next bounded slice. Do not resume until asked.
Latest full serial Rust **1231pass/0fail/15ignored** and strict all-target Clippy
pass. Native explicit recovery and bridge-failure acceptance both pass Luna plus
independent parent runs. Managed normal UI transitions remain open: the native
Ctrl-X leader collides with attach's detach key; the test's Ctrl-O/P session-list
assumption was wrong. The user subsequently requested a checkpoint commit of
the accumulated work; this is not phase completion or permission to resume.
No push or live activation. The historical in-progress
section below records the preceding work and is superseded by this pause note.



## Phase 4 continuous completion in progress — 2026-10-24 (read first)

User requested continued Luna slices until the phase is complete. See
[P4-CONTINUOUS-TASK.md](P4-CONTINUOUS-TASK.md). Do not stop after two slices.
Parent integrates and reviews; workers own bounded exclusive modules. No new
commit or push. Existing dirty Phase3/Phase4 work and active binary are retained.

Implemented since the previous full1157 baseline: the fresh challenge-bound
bridge, pinned plugin/private launch configuration, asynchronous joint
managed/provider store, scoped historical CLI and explicit prepare/resume/status
flow. The owner validates before prepare, saves a one-shot choice, requires a
separate resume, durably spends before navigation, and confirms only after a
fresh selected snapshot and joint mapping save. No automatic recovery/replay or
current-at-crash claim.

Current verification snapshot: full locked/offline serial Rust1230passed,
0failed,15ignored; all seven supporting checks pass, decoder39, Python196
with one intentionally opt-in live test skipped, docs316/0. Endpoint drop/gate
and runtime receipt-clock review corrections have additional focused checks;
final source freeze verification remains required. Active binary SHA remains
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.

Native managed manual acceptance now passes both Luna and an independent
parent run: missing/deleted classification, prepare preserves A, explicit
resume selects/saves B, replay refusal, historical joint record, imported
nonempty fork-root acceptance, foreign-project and linked-child refusal. The
binary used was `1497ac732020573d633df85d38fdd594a92b31d7f1b8ee29d06e0087e52ec9ee`.
The native bridge-failure fixture also passes Luna: an exactly verified owned
TUI PID is paused, owner readiness revokes, PTY/serve remain alive, source A
stays at revision1, preparation is refused before and after SIGCONT, and no
implicit source recovery occurs. Parent independent failure run is underway.

Managed native keyboard acceptance remains open. Earlier incidental heading
predicates were too weak; do not count their A-B-A observations as safe keyboard
acceptance. The current stricter guard withholds title/Enter until a real native
modal is established. Luna is diagnosing key/query/terminal behavior. New/fork/
project transitions through normal provider UI remain distinct from the passing
explicit manual-recovery route. Standalone native cases passed independently;
these do not close managed UI acceptance.

See [native recovery](P4-NATIVE-MANUAL-RECOVERY.md),
[native failure](P4-NATIVE-BRIDGE-FAILURE.md),
[managed bridge](P4-MANAGED-BRIDGE.md) and
[operator flow](P4-MANUAL-RECOVERY.md). Preserve separate Phase3 human host gate.


## Phase 4 bridge wire/channel verified — 2026-10-24 (read first)

Two GPT-6 Luna High owners delivered the independent wire codec and grant-scoped
Unix endpoints. Parent review, Frame accessors, combined tests and full serial
verification are complete. **1157 Rust tests passed,0 failed,15 ignored** in
188.292s; library762, main binary148, integration247. Focused bridge31/0 and
new combined IPC target2/0 pass. All seven supporting checks pass: strict
all-target Clippy, formatting, build, decoder39, source fingerprints, Python160
(no skips,8.182s) and docs299/0. See
[result](P4-BRIDGE-TRANSPORT-RESULT.md),
[task](P4-BRIDGE-TRANSPORT-TASK.md), [wire](P4-BRIDGE-WIRE.md) and
[channel](P4-BRIDGE-CHANNEL.md).

The `PLBRDG01` envelope is separate from the existing control protocol, with a
32-byte header and4388-byte total bound. Exact lengths/tags/metadata are checked
before bounded body allocation. Both Unix endpoints check kernel peer UID
before bytes, then match a one-use issuer grant for full target/project/exact
provider version/source instance. OS-random grants now have an explicit typed
Hello provisioning method. Streams stay private and nonblocking; one absolute
deadline spans each whole message, and errors permanently poison/shut down.
This is possession plus same-user authentication, not provider executable
attestation, hostile same-UID isolation or a global replay/revocation service.

The parent private0700/0600 pathname fixture passes handshake → A/B admission
→ synchronized binding saves → Home invalidation, followed by stale reopen.
A second fixture deliberately fragments valid metadata then sends an oversized
header; rejection precedes body read, the channel remains poisoned and explicit
owner-side failure handling preserves the last saved binding while marking the
receiver unavailable. Actual different-UID process acceptance was not added;
the existing peer-credential unit mismatch gate is separate evidence.

Initial module/compile/test/lint/format failures and parent corrections are
preserved in the notes/result. All final Rust source was present before the
passing serial run. No dependencies or lockfile changes. The full Cargo command
ran separately with a360s deadline and supporting checks separately; not an
unmodified combined-verifier8/8 claim. Cache/build isolation remains
`/private/tmp/panelink-p4-cargo-home` and
`/private/tmp/panelink-p4-bridge-store-target`. The activated binary hash stays
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
No live provider/workspace/launcher/configuration changed.

Next: opt-in owner/worker wiring, trusted grant delivery/revocation and explicit
fresh-snapshot reconciliation, bounded queues and ordered durable completions.
The channel proves receipt and grant scope, not when a provider observed a route;
buffered metadata is not a fresh-reconciliation or selection barrier. Keep
slow IO outside the owner. Production heartbeat policy, managed-session atomic
linkage, cross-generation/manual recovery and remaining identity cases stay
open, along with the separate human Phase3 host gate. No automatic resume or
current-at-crash guarantee. Preserve accumulated dirty work. No commit or push.


## Phase 4 bridge/store components verified — 2026-10-24 (read first)

Two GPT-6 Luna High owners delivered `provider_bridge` and
`provider_binding_store` with exclusive ownership; the parent reviewed,
integrated and verified them. Full serial locked/offline Rust:
**1136 passed,0 failed,15 ignored**,191.629s including isolated compilation.
Final focused provider library37/0 and combined integration4/0 pass. All seven
supporting checks pass: Clippy, formatting, build, decoder39, source fingerprints,
Python160 (no skips,8.287s) and docs295/0. See
[result](P4-BRIDGE-STORE-RESULT.md), [task](P4-BRIDGE-STORE-TASK.md),
[bridge contract](P4-BRIDGE-CONTRACT.md) and [store](P4-BINDING-STORE.md).

The typed protocol-v1 receiver owns the binding core and gates exact pinned
provider identity on target/project/instance, contiguous source sequence,
monotonic receipt expiry and explicit fresh reconciliation. Duplicates do not
renew. Child/Home/unknown/rejected identity cannot confirm. The host supplies
a separate verification Unix timestamp and must tick idle receivers and check
freshness after blocking IO. Source authentication, transport and production
heartbeat policy remain future integration work.

The store uses existing bounded codec bytes, one private caller-selected
per-target directory, lifetime exclusive lock and inode checks. File sync →
rename → directory sync precedes success. Ambiguous sync failure latches the
store until reopen; unsafe/corrupt files are preserved, and interrupted temps
are never promoted. The parent proved that a save failure blocks confirmation
and expired pending state cannot invoke the writer.

Actual subprocess SIGKILL tests cover before storage, after storage/before
acknowledgment and after acknowledgment, with ready handshakes and reaping.
Before-write reopen sees old revision; both later cases see the new record.
All restore stale. This exposes lost-ack uncertainty and proves neither the
provider's current-at-crash identity nor full machine/power-loss recovery.
No joint managed-session transaction is implemented.

The old P2 Cargo cache was empty. The parent copied only the installed registry
into `/private/tmp/panelink-p4-cargo-home`; builds use
`/private/tmp/panelink-p4-bridge-store-target`. Initial compile/lint/format
failures and final corrections are recorded in the result. Full Cargo ran
separately under a360s deadline; seven supporting checks ran separately, not an
unmodified combined verifier8/8. The activated `target/debug/panelink` hash
remains `3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
No live provider/workspace/launcher/configuration changed; prior private live
identity fixtures remain separate evidence.

Next: bounded bridge wire encoding/handshake and source/target authentication,
then opt-in owner/worker wiring with ordered storage completion and visible
expiry/reconciliation/failure states. Cross-generation linkage, last-confirmed
manual recovery and remaining provider identity cases stay open, along with
the separate Phase3 human host gate. No automatic resume/current-at-crash
claim. Preserve all accumulated dirty work. No commit or push.


## Phase 4 linked-child and project isolation verified — 2026-10-24 (read first)

The next two slices were implemented by two GPT-6 Luna High owners, integrated
and independently verified by the parent. **Both pass3/3 fresh live runs** on
pinned native OpenCode2.0.32 with loopback-only sandboxing. Full Python suite:
**160 passed**, no failures/skips, in8.269s. Eight changed Python modules
compile and whitespace passes. See [result](P4-V2-ISOLATION-RESULT.md) and
[task](P4-V2-ISOLATION-TASK.md). No Rust/build changes; the separate1111/0/15
ignored baseline was not rerun for this tooling work.

A third TUI selects a linked child, reports it as non-top-level, then returns
to its exact parent through the keyboard picker. Original observer streams
advance and preserve their identities. An empty-parent copied-history fork
returns400; linked creation does not prove a nonempty history fork. The other
slice attaches a third TUI to a second private directory, verifies exact
identity there, and rejects those rows in the original directory's scope.
This is separate-client directory isolation, not same-client project switching
or distinct Git repository identity. Every final run verifies empty user
messages and all three TUI/PTY plus server cleanup. No prompts or user state.

Parent review corrected draining/freshness, warm-up and invalid records,
ownership checks and late failure downgrades. Two failed child diagnostics are
preserved: canonicalizing the provider query path returned an empty list.
Queries now retain the created fixture's path spelling; identity comparisons
remain canonical. A symlink-alias lifecycle regression passes. Current result
contains the timings, receipts and exact scope. Documentation verification:291 Markdown files checked, zero findings.

Next: version and integrate the opt-in bridge for supported top-level
observations with ordering/expiry/reconciliation, then durable last-confirmed
store/acknowledgment and crash tests against the existing core/codec. New
conversation ID, nonempty fork, same-client project switch and current-at-crash
remain open. Preserve unsupported-state/manual fallback and the separate
Phase3 human host gate. Activated worktree/binary and all accumulated dirty
work are retained. No commit or push.


## Phase 4 next two Luna slices verified — 2026-10-24 (read first)

Opposite-client/new-route outcome and observer-health slices pass **3/3 fresh
runs each** on pinned native OpenCode2.0.32. Two Luna High owners implemented
disjoint files; parent review/integration and permitted live runs verified the
results. Parent full Python suite: **143 passed**, no skips, in8.403s; focused
suite61/0; changed Python compiles. No Rust/build input changes. See
[P4-V2-EXTENDED-RESULT.md](P4-V2-EXTENDED-RESULT.md) and
[task](P4-V2-EXTENDED-TASK.md). Docs287/0; tracked whitespace and ten
affected/untracked files pass. The in-memory graph was refreshed without an
artifact.

Physical TUI2 switches B-A-B with TUI1 fresh on A. Ctrl-X N then produces
Home with no selected session ID; this does not prove allocation timing or
new-conversation identity. Both sessions have empty user-message reads and all
process groups/PTYs/server receive cleanup receipts. The health case pauses
only plugin record delivery: both TUIs/server stay alive, passive B stays fresh,
active receipt expires, and transport resume alone cannot revive eligibility.
A further row after explicit reconciliation restores only in-memory observed
identity; durability stays false. The1s threshold is diagnostic policy only.

Parent corrected precreated empty control files, runtime caps while paused,
batched/late receipt handling, unknown identity inside batches and fail-closed
new-control checks. Failed diagnostic evidence is retained. No prompt, user
account, implicit background service, live Panelink pane or launcher was changed.
Preserve the activated worktree/binary and all accumulated uncommitted work.

Next: remaining new-ID/fork/child/project cases, then the versioned real bridge
and durable binding/store acknowledgment work, retaining last-confirmed/manual
fallback and pre/post-ack crash tests. Full Phase4 and the separate Phase3 human
host gate remain open. No commit or push.


## Phase 4 selected-route/two-client gate passes — 2026-10-24 (read first)

Two Luna High owners built the V2 fixture/harness. Parent review and live
integration corrected a direct-file loader assumption, a mistaken legacy-config
hypothesis, warm-up parsing and the picker background-header guard. The final
pinned2.0.32 gate passes **3/3 fresh runs**: two simultaneous TUIs in the same
private project, initial A/B identities, keyboard A-B-A in the first while the
second stays fresh on B, zero missed/extra/rejected observations, verified empty
user messages and both cleanup receipts. No provider account/model work or live
Panelink pane input. See [results](P4-V2-TUI-RESULT.md) and [task](P4-V2-TUI-TASK.md).

Parent verification: **121 Python tests pass**, no skips, in6.962s; focused39/0.
Compilation and tracked/untracked whitespace pass; docs285/0. The in-memory
code graph was refreshed without writing an artifact.
No Rust/build changes; prior Phase3 source baseline remains1111/0/15 ignored.
The earlier approval-service usage failure resolved through normal review after
its stated retry time; no live test remains blocked by it.

Actual current CLI config uses plural `plugins` pointing to a directory with a
`tui` entry. Direct files are skipped. Singular `plugin` is legacy migration,
not the current schema. Do not repeat either failed configuration. Valid startup
rows can have unknown project/cache fields; they do not count as identity proof.
Require strict observations after initial identity is ready. Raw output, IDs,
paths, bodies and secrets are absent from reports. Polling remains no crash
barrier; this is development-fixture proof, not a production provider bridge.

Next: opposite-client/new/fork/child isolation and bridge-loss/staleness gates,
then live bridge integration and durable binding acknowledgments using the
existing pure core/codec. Preserve last-confirmed/manual fallback. Full Phase4
and the separate Phase3 human host gate remain open. No commit/push.


## Phase 4 V2 identity gate — 2026-10-24 (read first)

The user requested Phase 4. Two GPT-6 Luna High owners delivered an old-probe
correlation correction and a separate exact 2.0.32 bootstrap preflight. Parent
review and independent verification: **95 Python tests pass**; changed Python
modules compile. No Rust/build changes were made in this slice. Current result:
[P4-V2-IDENTITY-RESULT.md](P4-V2-IDENTITY-RESULT.md).

The current installed binary hash/version are pinned. Initial anonymous access
returned HTTP401. A bounded source audit found the supported fixture-local
Basic-auth mechanism; the final parent run **passed authenticated bootstrap**
in 587.354ms: exact owned child PID/version, two empty top-level sessions
created/listed and cleanup completed under enforced loopback-only sandboxing.
Zero prompts; no user/provider credentials. The probe refuses redirects and
sends its fresh credential only after listener ownership verification. Exit2
remains intentional because selected TUI identity is not proven. No TUI adapter
or identity-switch proof is claimed.

Next gate: implement and validate the selected-route adapter on this verified
fixture-local transport, including ordered keyboard A-B-A and two concurrent
agents in the same project. Existing binding core/codec are pure
components, not runtime persistence/resume. Preserve current-at-crash/manual
fallback boundaries, the activated workspace and all accumulated dirty work.
Full Phase 4 and the separate Phase3 human host gate remain open. No commit/push.


## Verified build activated — 2026-10-24 (read first)

The user explicitly selected **Activate and restart workspace**. The parent
verified the accepted binary hash, atomically switched `~/.local/bin/panelink`
to this worktree's launcher, and ran supported `panelink stop` (exit0,
workspace stopped). A fresh default workspace is now running on the worktree
binary (observed PID95331). Read-only pane list/read both pass with the explicit
private `.panelink-agent-contexts` root and permitted socket access. No pane
input was sent. The noninteractive open's UI exit1 is not an interactive attach
pass. See [activation evidence](P3-USABILITY-FIXES-RESULT.md).

Keep worktree7060 and its built binary in place: the PATH launcher now resolves
here. Activation is complete; earlier pending-choice and old-PATH observations
below are historical. Existing history remains; ended processes/provider
conversations were not resumed. Final source verification remains1111/0/15
ignored plus seven supporting checks. Full Phase3 human four-pane host
acceptance remains open. No commit or push.


## Phase3 continuation preflight — 2026-10-24 (read first)

The user requested Phase3 continuation after the usability result. The parent
rechecked current Git history/status and index (no unmerged entries), verified
that the built binary hash still matches the accepted 1111/0/15-ignored tree,
and confirmed PATH still targets the separate main checkout. Read-only pane
discovery currently sees one live pane. No new source edit, commit, push,
input or process stop was performed.

The accepted Phase3 code scope is delivered. Activation is prepared; a concise
restart choice is pending because stopping the live workspace ends its pane
processes. Preserve it while that answer is absent. Human four-pane Apple
Terminal acceptance remains open and cannot be replaced with the passing
private outer-PTY tests. The remaining-gates audit now marks its older test
counts/line references as historical and points to the usability result.

## User-reported pane usability repairs — 2026-10-24 (read first)

The user supplied a real three-pane screenshot and reported agent access failing
with `context_directory_unavailable`, nonworking scrolling, and cramped pane
edges. This reopens usability implementation; the older 1091-test baseline
below predates these changes. Three bounded GPT-6 Luna High workers implemented
and cross-reviewed private agent-context selection, typed scroll/history paths,
and one-cell pane padding. Parent integration verification passed:
**1111 passed/0 failed/15 ignored in164.34s**, plus all seven supporting
checks (decoder39/Python77/docs281/0). Actual private outer-PTY tests cover
normal wheel history and alternate-screen app mouse input with inset coordinates.
The parent fixed startup output loss by launching directly at the content size,
and an observed rotation fixture race by waiting for drained writer shutdown.
See [result](P3-USABILITY-FIXES-RESULT.md) and [task](P3-USABILITY-FIXES-TASK.md).
All ownership is released. Full Phase3 remains open; no commit or push.

A read-only live check using this worktree's binary and an explicit private
context root successfully listed three panes and read pane 2 with permitted
local-socket access. The same calls in the restricted sandbox could not connect;
the context override does not bypass socket permissions. No pane input was sent. A final refresh saw one current pane, correctly refused
now-absent pane2, and successfully read pane1; terminal contents were not printed.

The PATH launcher still points to `/Users/sahillamba/panelink/bin/panelink`,
not this checkout. The running workspace uses older server code. No user server
or child has been stopped or restarted; new scrolling/padding need a server
running the new build. Native Terminal computer-use controls are unavailable
in this session. The user's screenshot is partial host feedback, not a complete
four-pane acceptance record. Preserve all dirty work; no commit or push.


## Phase3 Luna orchestration and integrated verification — 2026-10-24 (read first)

Three native GPT-6 Luna High workers audited the remaining required gates,
reviewed PTY interruption semantics, and independently reviewed accumulated
source/test changes. No actionable source findings or additional required
production task were identified. A proposed fake waiter-disconnect test was
removed because its test-created channel did not exercise the production
waiter. The existing internal validation barrier and real-CLI pre-delivery
regressions are preserved with their limited claims. All worker ownership is
released; native collaboration launcher, no cost telemetry.

Parent frozen-source verification: complete locked/offline serial Cargo
**1091 passed/0 failed/15 ignored in182.72s** (library713/binary142/
integration236 across53 targets). All seven supporting checks pass:
decoder39/Python77/fingerprints/package fmt/strict all-target Clippy/build/docs279/0.
No timeout or failed check in this run; ignored cases remain separate from
acceptance. See [result](P3-ORCHESTRATION-RESULT.md),
[remaining gates](P3-REMAINING-GATES-2026-10-24.md) and
[PTY audit](P3-PTY-INTERRUPTION-AUDIT.md).

The sole explicit remaining Phase3 gate is human four-pane Apple Terminal
evidence, unavailable in this run. No manual chore is assigned and no automated
proxy is promoted to human acceptance. The accepted bounded implementation is
delivered; do not schedule more optional synthetic input windows as required
Phase3 work. Review actual host evidence if it becomes available before full
phase closure. Provider/reboot/Phase4/Phase5 and crash observation limits remain
separate. Preserve the dirty tree based on checkpoint8ad9bd6; no commit/push.

## Phase3 handler-boundary validation test delivered — 2026-10-23 (read first)

Added `server::tests::submit_validation_barrier_precedes_admission_and_duplicate_does_not_replay`
using a test-only blocking `EffectSink::validate`. It observes the Submit
revision/text at the deterministic barrier, confirms input count zero before
release, then verifies one `Completed(Applied)` effect and no replay for an
exact duplicate. Parent focused rerun: 1 passed/0 failed, 716 filtered out.
This tests internal handler ordering only: no socket ingress, real CLI death,
or admitted owner-pending/PTy interruption. See
[feasibility and result](P3-POST-DISPATCH-FEASIBILITY.md).

The subsequent admitted owner-pending/PTy audit is complete and recorded above.
It identified an observation limit, not an additional required Phase3 task.
Do not infer client cancellation or add production hooks. Preserve separate
real-CLI pre-delivery, human four-pane, provider and reboot evidence.

## Phase3 post-dispatch barrier feasibility audited — 2026-10-23 (read first)

The real-CLI integration fixture cannot deterministically stop inside server
validation before ledger admission: it launches the production server as a
separate process, while the proxy boundary is either before upstream delivery
or after the server reply. Forwarding Submit and killing by timing would not
establish dispatch. An in-crate fake `EffectSink::validate` can block the
`StateOwner` handler at that internal point, but does not prove socket ingress
or real CLI death. No production hook is justified by this coverage gap. See
[P3-POST-DISPATCH-FEASIBILITY.md](P3-POST-DISPATCH-FEASIBILITY.md).

The narrow deterministic handler-boundary test is delivered above and is
labeled as internal dispatch/admission evidence only. Keep the real-CLI
pre-delivery test as the separate process-death evidence. All existing dirty
work remains intact and uncommitted.

## Phase3 pre-delivery client-death acceptance delivered — 2026-10-23 (read first)

The new real-CLI regression pauses the exact decoded Submit at the private
authenticated proxy before forwarding it to the production server. It SIGKILLs
and reaps the exact submitting Child, then verifies durable uncertain-key
state, server `NotAdmitted`, same-context resync without sending the request,
zero controlled-shell effect, unchanged target/PID and explicit server cleanup.
See [task](P3-PRE-ADMISSION-CLIENT-DEATH-TASK.md) and
[result](P3-PRE-ADMISSION-CLIENT-DEATH-RESULT.md).

Parent reran the full focused target serially: 2 passed/0 failed/0 ignored in
1.21s, including the existing post-completion/pre-ack case. Rustfmt check,
diff whitespace and docs check pass (274 Markdown files/0 findings). Luna's
initial fixture readiness-marker mismatch was corrected before its passing
focused rerun. No full Cargo suite was run for this test-only change. No
production code or input policy changed; full Phase3 remains open.

The pre-delivery client-death acceptance remains distinct. Post-dispatch test
feasibility and the next narrow handler-boundary test are recorded above. Keep
admitted owner-pending/PTy interruption, human four-pane, provider and reboot
gates separate.

## Phase3 intermittent acceptance stabilization delivered — 2026-10-23 (read first)

The recorded history-writer admission and attach-host ACK42 flakes are
stabilized with bounded, test-only admission retry on the exact `Contended`
reason and an attach-client focus acknowledgment before ACK42 is typed. The
client now changes its status from “waiting for layout” to “new pane focused”
when a later layout completes a pending split focus. No production admission,
input routing, replay, wire or storage policy changed. See
[result](P3-INTERMITTENT-ACCEPTANCE-RESULT.md).

The history panic case passed20/20 serial repetitions; the delayed-layout focus
unit test passed1/1; the ACK42 attach-host case passed1/1, then10/10 serial
repetitions. Complete locked/offline serial Cargo passed 1089/0/15 ignored in
179.27s across 53 integration targets. All seven supporting checks passed in
the permitted environment (decoder39/Python77/docs273/0). An initial restricted
support run lacked offline crate-index entries and loopback binding; it is not
counted as passing. Earlier failures and raw logs are preserved in the result
note. Repeated success is not a timing guarantee.

Two GPT-6-Luna High read-only audits identified the acceptance assumptions;
parent reviewed and implemented the bounded repairs. No cost telemetry is
available. FullPhase3 remains open. The pre-delivery client-death acceptance
is recorded above. Next, determine whether a post-dispatch/pre-admission barrier
can be tested in-process without a production hook. Do not claim current
owner-pending or PTY interruption coverage or change production input/retry
policy. Human four-pane/provider/reboot gates remain separate.
No commit or push; the existing `8ad9bd6` checkpoint remains the base.

## User-authorized Phase3 checkpoint — 2026-10-23 (read first)

The user explicitly requested committing the accumulated verified work. This
checkpoint includes live scrollback, managed pane run, neutral owner discovery,
client/server-death acceptance, runtime contract reconciliation, and killed
input-client recovery acceptance plus their documentation/evidence. The latest
full serial result is1089passed/0failed/15ignored with all seven supporting
checks passing; details and prior failures remain below. This is a checkpoint,
not fullPhase3 completion. No push is authorized. Git history records its ID.
This checkpoint predates the intermittent-acceptance stabilization recorded
above. Preserve the separate original source checkout and unresolved
host/provider/reboot gates.

## Phase3 killed input-client acceptance delivered — 2026-10-23 (read first)

A real `control submit` Child is SIGKILLed once a test-owned authenticated proxy
holds its matching terminal Completed(Applied) response without forwarding it.
The exact key remains uncertain on disk after death; read-only status preserves
it, resync clears it, and repeated resync leaves generation/lease/next sequence
unchanged. The same controlled shell/full target reports one handled token,
confirmed by a private append record and ordered CHECK. Explicit SIGTERM reaps
the owned serve Child and removes its socket. See [task](P3-INFLIGHT-CLIENT-TASK.md),
[audit](P3-INFLIGHT-BARRIER-AUDIT.md), [review](P3-INFLIGHT-CLIENT-REVIEW.md)
and [result](P3-INFLIGHT-CLIENT-RESULT.md).

Complete locked/offline serial Cargo: **1089 passed/0 failed/15 ignored** in
157.19s (library712/binary142/integration235 across53 targets). Seven supporting
checks pass:decoder39/Python77/fmt/strict all-target Clippy/build/fingerprints/
final docs272/0. Parent final targeted1/1 in0.84s. Worker startup denied pathname
bind before scenario; parent fixture runs exposed newline-sensitive output
assertion and wrong default-workspace stop usage for directly launched serve.
These were corrected; failures and logs remain in the result note, not counted
as passing runs. No Cargo deadline expired. At that checkpoint, ACK42 and
history-writer intermittency remained unresolved; the later investigation and
stabilization are recorded above.

Only new tests/p3_inflight_client.rs changes executable test behavior this
slice. All156 pre-existing source/test/script hashes match the turn-start
snapshot; production policy, wire/storage, dependencies and prior dirty work
are unchanged. This covers post-server-completion/pre-client-ack death, not
before admission or while owner-pending, and implies no provider/reboot or
arbitrary descendant cleanup guarantee. FullPhase3 remains open.

User renewed Luna cost routing for suitable tasks. Native Luna High implemented
the exclusive test and independently audited/reviewed it; Luna Medium drafted
evidence. Parent specified, corrected/reviewed and verified. No cost telemetry;
all worker ownership released; native collaboration launcher; ms-rust absent.
Next investigate the recorded history-writer startup admission contention and
attach-host ACK42 intermittency with bounded tasks, without changing permitted
nonblocking semantics or asserting an unproved production fix. Keep human
four-pane acceptance distinct; no manual chores handed to the user.
Preserve accumulated uncommitted work on codex/p3-scrollback-integration based
on9f3d8a0. No commit/push; original source checkout untouched. Source graph is
ephemeral; refresh before code exploration as needed.

## Phase3 runtime reconciliation and pathname acceptance delivered — 2026-10-23 (read first)

Parent and two independent native Luna High source/document audits found the
broad event/watch/launch/layout implementation checkbox stale. Accepted finite
pane/workspace watches, reconciliation/gap recovery, four-pane lifecycle and
managed pane run are delivered. ROADMAP now records that exact scoped closure;
fullPhase3 remains open. Direct schema6 launch, indefinite subscriptions and
large Snapshot queues are outside these accepted bounded contracts, not missing
implementation inferred from an older proposal. Spec revision0.24 and
agent-control notes distinguish current behavior from historical one-child
text. See [audit](P3-RUNTIME-CONTRACT-AUDIT.md),
[review](P3-RUNTIME-SCOPE-REVIEW.md) and [result](P3-RUNTIME-CONTRACT-RESULT.md).

The audit identified an old pending CLI acceptance we could run directly:
`cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse
-- --exact --ignored --test-threads=1` passed1/1 in1.20s with sockets enabled.
This covers expanded reconcile/watch/context-preservation/same-child reconnect
and socket lifecycle assertions. The standard ignore attribute remains; this
explicit result is separate from the prior1088pass/15ignored full run below,
not an additional combined-suite count. No source/test/script edits or full
suite rerun this slice. Source hashes match the turn-start snapshot. Final
document/link check:268 files/0 findings; diff whitespace check passes.

Next specify deterministic client death during an in-flight input request under
the existing admission/status/nonreplay contract. Identify an observable barrier
and exact fixture-owned process before implementation; no timing-inferred
outcome or input replay. Human four-pane acceptance remains a distinct gate;
provider/reboot and arbitrary descendant cleanup remain separate. No manual
chores are assigned to the user. Prior test intermittency remains unresolved.
All worker ownership released; native collaboration launcher; ms-rust absent.
Preserve accumulated uncommitted work on codex/p3-scrollback-integration based
on9f3d8a0. No commit/push; original source checkout untouched.

## Phase3 bounded server-death acceptance delivered — 2026-10-23 (read first)

Strengthened the existing default-open SIGKILL test: a cooperative HUP-trapped
zsh exits, the socket remains, explicit reopen replaces it with a new durable
generation/full target, and the old control context is refused without changing
its bytes. An external once-only append record stays unchanged through fresh
input/detach; the replacement live pane has no old marker and produces its new
marker once. Explicit replacement stop cleans up. Intentional and fixture
failure-cleanup signals validate PID, owner and kernel start time. See
[contract](P3-SERVER-DEATH-TASK.md), [audit](P3-SERVER-DEATH-AUDIT.md),
[review](P3-SERVER-DEATH-REVIEW.md) and [result](P3-SERVER-DEATH-RESULT.md).

Complete serial locked/offline Cargo: **1088 passed/0 failed/15 ignored** in
153.17s (library712/binary142/integration234 across52 targets). Seven supporting
checks pass, decoder39/Python77/final docs265/0. Formatting and strict all-target
Clippy pass after review corrections. Parent targeted runs1/1 in3.81s and final
reviewed1/1 in3.73s. Worker restricted startup failed before new assertions;
its server log was removed by fixture cleanup, so the root cause is unproved.
Record it separately; no skipped acceptance counted as pass. Prior ACK42 and
history-writer intermittency remains unresolved, despite this full pass.
No Cargo deadline expired.

Only tests/p3a_default_open.rs changed executable test behavior this slice.
Prior application and pane-run/scrollback/client-lifetime source hashes match.
No production crash policy, dependency, wire/storage format or supervisor was
added. Canonical spec revision0.23 records that SIGKILL runs no graceful
cleanup or final flush; arbitrary descendants may survive. Cooperative-shell
exit does not establish orphan-free crash, in-flight-request recovery, provider
or physical reboot acceptance. Full Phase3 remains open.

Native Luna High workers completed exclusive implementation and independent
read-only audit/review. Parent specified, corrected/reviewed and verified; all
ownership is released. Native collaboration was the launcher; ms-rust remains
absent. Next reconcile the remaining broad Phase3 event/watch/launch/layout
checklist with current contracts/evidence and identify a concrete unmet runtime
requirement before new work. Human four-pane acceptance stays distinct; no
manual chores are handed to the user. Source graph is ephemeral; refresh as
needed before code exploration. Preserve all accumulated uncommitted work on
codex/p3-scrollback-integration based on9f3d8a0; no phase-completion commit or
push. Original source checkout untouched.

## Phase3 owner discovery and client lifetime delivered — 2026-10-23 (read first)

`pane list` exposes neutral server-issued generation:lease identity, caller
comparison, owner revision and raw server-clock deadline from the same sample.
No inferred agent/provider roles or authorization widening. A real private
outer-PTY regression SIGKILLs the exact live attach process, reattaches to the
same full target/child PID/output, checks one-shot input, then explicitly stops
the server and verifies child cleanup. Orderly terminal restoration is asserted
only for the second client. See [contract](P3-OWNER-IDENTITY-TASK.md),
[owner review](P3-OWNER-IDENTITY-REVIEW.md),
[lifetime review](P3-CLIENT-LIFETIME-REVIEW.md) and
[final evidence](P3-OWNER-LIFETIME-RESULT.md).

Complete serial locked/offline Cargo: **1088 passed/0 failed/15 ignored** in
156.25s (library712/binary142/integration234 across52 targets). Seven supporting
checks pass, decoder39/Python77/final docs261/0; formatting and strict all-target
Clippy pass after the final diagnostic assertion. Prior source/test hashes for
scrollback are unchanged. Earlier two full runs stopped at the existing
attach-host ACK42 timeout; a third stopped at existing history-writer startup
admission Full. Isolated and final complete reruns pass; no production fix or
resolved intermittency is claimed. Fixture corrections and raw-log locations
are preserved in the result note. No Cargo deadline expired.

Native Luna workers completed exclusive implementation/acceptance/review tasks;
parent reviewed, corrected fixtures and verified the combined tree. All file
ownership is released; no worker must survive to resume. Native collaboration
was the launcher; ms-rust remains absent. Source graph is fast/nonpersistent
and ephemeral; verify freshness before the next code exploration.

Next: specify/audit server-death cleanup and stale endpoint/context behavior,
then add bounded automated acceptance under its documented policy. Use the
[closure audit](P3-CLOSURE-AUDIT.md); preserve distinct human four-pane, in-flight
request, provider identity and reboot gates. Full Phase3 remains open. All
scrollback/pane-run/owner/lifetime work remains uncommitted on
`codex/p3-scrollback-integration` based on9f3d8a0; no phase-completion commit or
push. The original source checkout is untouched. Preserve this dirty tree.

## Managed pane run completed — 2026-10-23 (read first)

Delivered and parent-reviewed: `panelink pane run [--pane N] [--direction
left|right|up|down] [--cwd DIR] [--as NAME] [--json] -- COMMAND [ARG ...]`.
Raw Unix argv and invoking-client cwd/env feed the existing serialized schema7
split/embedded launch worker; no shell input substitution, new server route,
wire/storage format, dependency, manifest or vendor change. Exact anchor and
layout are pinned; INPUT negotiation, four-pane/geometry bounds and the existing
launch journal/deduplication remain authoritative. Applied means launched,
not ready/completed. No automatic resend or replacement key after uncertainty.

The independent review's two output findings were fixed: post-reservation
errors retain the key, and ack failure retains validated new target/revision
with unknown outcome. The error stores only compact validated metadata.
Parent added lost-ack/context-reopen/no-send bounds and deterministic local
ack-failure tests; strengthened real CLI geometry/context/failure/cleanup
assertions. See [accepted contract](P3-PANE-RUN-TASK.md),
[independent review](P3-PANE-RUN-REVIEW.md) and
[final evidence](P3-PANE-RUN-RESULT.md).

Final frozen-source full serial Cargo: **1084passed/0failed/15ignored** in148.24s
(library712/binary140/integration232 across51 targets). Seven non-Cargo checks
pass separately: decoder39/Python77/fingerprints/fmt/strict all-target Clippy/
build/docs256/0. Focused transport8/8 and parser/JSON9/9 pass; real CLI3/3 is
included in the full run. Pre-fix full runs1082 and1084 passed separately;
intermediate Clippy6/7 at large error size and temporary API/test compile
mismatches are preserved in the result note, not relabeled as passing runs.
No full Cargo deadline expired.

Three native GPT-6-Luna High workers had exclusive files; parent owned design,
review/integration and final verification. All ownership is released; no live
worker is needed to resume. Source graph refreshed fast/nonpersistent for
worktree7060 after freeze; it is ephemeral, so verify freshness next time.
The requested ms-rust skill remains absent. Previous scrollback source/tests
are hash-unchanged. The main source checkout is untouched.

Next: review/specify richer owner identity in discovery under existing
lease/target authority. Full Phase3, wider host/lifetime gates, provider identity
and physical reboot remain open. Changes remain uncommitted on
`codex/p3-scrollback-integration` based on9f3d8a0; no phase-completion commit
or push is claimed. Preserve all current work on continuation.

## Live-scrollback integration — 2026-10-23 (read first)

The integration branch `codex/p3-scrollback-integration` starts at `9f3d8a0`
and includes the separate live normal-scrollback patch from
`/Users/sahillamba/panelink`. That checkout's uncommitted work is preserved.
Managed-child metadata, observer renewal and workspace notifications remain
included. Parent reviewed the patch and a bounded GPT-6-Luna High reviewer
reported no actionable findings; native collaboration was the launcher. All
worker ownership is released. No dependencies, vendor source, manifest,
wire schema, or persisted format changed. The ms-rust skill remains absent.

Focused adapter tests pass 6/6; combined agent CLI/detach/workspace-event
integration tests pass 11/11. The seven non-Cargo verifier checks pass.
Final full serial Cargo:1064passed/0failed/15ignored in122.73s (library712,
binary123, integration229 across50 targets). Docs253/0 after final evidence
updates. The first full run was1063pass/1fail/15ignored: the unchanged final
shutdown fixture missed its second HUP marker. Its isolated rerun and final
full rerun pass without changes; cause remains unestablished, no fix claimed.
See [the integration evidence](P3-LIVE-SCROLLBACK-RESULT.md) and
[review](P3-LIVE-SCROLLBACK-REVIEW.md). Earlier source-checkout attach timing
failure and 180-second Cargo timeout remain historical failed/incomplete runs.

Next: specify/review managed `pane run`,
using explicit cwd/argv/client environment, the existing launch worker and
deduplication, and returned full targets. Do not implement this as text sent to
an existing shell. Richer owner identity and host/lifetime gates remain open.
Full Phase 3, provider identity and physical reboot acceptance remain open.
The integration remains uncommitted; Phase3 is unfinished and no phase-completion
commit is claimed. No push is authorized.

## Phase 3 managed-child metadata and observer renewal — 2026-10-23 (read first)

Completed and parent-reviewed: schema12/capability16 private current managed-child
cwd/executable queries, bounded isolated sampler/deadline/busy recovery,
exact-target authorization and publication revalidation, CLI sample/status
output, and same-lease workspace renewal after original-pane retirement.
The prior workspace-events slice is included in this checkpoint. User explicitly
authorized committing the completed verified slice; no push is authorized.
Branch: `codex/p3-integration-layout-events`; use Git history for commit identity.

Final full serial Cargo **1059passed/0failed/15ignored** in152.92s:
library709, binary123, integration227 across50 targets. Decoder39/Python77,
strict all-target Clippy, fmt, build, fingerprints and docs251/0 pass separately.
Initial default-parallel standard verifier7/8 stopped in the library at
707pass/2fail/4ignored (existing history-writer timing admission and immediate
post-spawn metadata sampling); this is preserved as failed evidence. The sampler
test now waits within a finite bound; the full serial run passes both.
See [final result](P3-PANE-METADATA-RESULT.md) and
[parent review](P3-PANE-METADATA-REVIEW.md) for commands, raw-log paths and limits.
All Luna ownership is released; no live agent is needed to resume.

Full Phase3 remains open. Next: review/integrate the separate live-scrollback
slice in `/Users/sahillamba/panelink`, which this continuation did not modify or
integrate. Preserve that checkout's attach timing and180s deadline evidence.
Managed pane run, richer owner identity, wider host/lifetime contracts, Phase4
provider proof and broader Phase5 recovery remain open. Ignored tests, logical
clock renewals, synthetic birth mismatch and private automated PTYs are not
human host, physical reboot, kernel PID-reuse or representative load proof.

## Historical Phase 3 baseline and layout notifications — 2026-10-23

The prior index blocker is resolved by integration commits `70f589c` and
`bd84625`. This continuation began clean on detached HEAD at `bd84625` and
created `codex/p3-integration-layout-events`. Scoped changes below remain
uncommitted; no push was made. The parent Phase 3 is not complete, so no
phase-completion commit is claimed. Historical merge reports below retain
their original failed commands, not the current index state.

Delivered and parent-reviewed: schema 11/capability 8 workspace layout
notifications, 64-record volatile journal, atomic snapshot/cursor reconciliation,
finite watches sharing the existing eight-watch cap, and agent CLI gap recovery
and full-target pinning. Pane screen/owner/exit events continue during quiet
workspace watches. No dependencies or persisted history format changed.
See [the complete result](P3-LAYOUT-EVENTS-RESULT.md),
[accepted contract](P3-LAYOUT-EVENTS-TASK.md), and
[independent review](P3-LAYOUT-EVENTS-REVIEW.md).

Final actual combined verifier: **8/8**. Root Cargo **1,036 passed / 0 failed /
15 ignored** (library691, binary119, integration226 across50 targets); decoder39
and Python77 pass; strict all-target Clippy, fmt, build and component fingerprints
pass; docs243/0. The real CLI gap test observes readiness, pauses only its test
observer, commits70 resizes, resumes and observes exactly one `layout_resync`
plus the quiet deadline. CLI target4/4. Four normally ignored Phase3 pathname
peer/authentication/descriptor-race/context cases pass separately. Ignored tests
and human Apple Terminal acceptance are not inferred passes.

The original restricted sweep reproduced 989 passed / 28 failed / 15 ignored.
Direct socket probes established EPERM only in the sandbox. All nine previously
failing integration targets passed in the permitted context before new product
changes:35/0/1ignored. Initial verifier compile/format failures during concurrent
edits are preserved in the result note and superseded by the final frozen-tree
run, not relabeled as passing executions.

User-authorized GPT-6 Luna Medium/High workers had exclusive files; parent
owned design, integration and independent verification. All ownership is
released; no live agent is needed to resume. The worktree graph was refreshed
fast/nonpersistent as `Users-sahillamba-.codex-worktrees-6730-panelink` after
source/test freeze. Source/tests remain authoritative.

Next: define and expose safe live pane cwd/executable metadata (currently null),
without arguments or environment values. Broader Phase3 live scrollback,
managed `pane run`, owner identity and host/lifetime contracts remain open.
The dedicated workspace observer does not proactively renew its30-minute
lease. Phase4 provider identity, Phase5 hierarchy/selection/reboot, human host
acceptance and active-agent load are not closed by this slice.

## Historical combined-tree merge checkpoint — 2026-10-22

The origin/main work and local stash work are combined in the working tree.
Parent reviewed the code/document resolutions and added core/socketpair
regressions for async growth to four panes and deferred multiple exits.
Final serial library675pass/0fail/4ignored; all-target check, strict Clippy,
fmt and component fingerprints pass. The full sweep's49 integration targets
report196pass/27fail/11ignored, with40 targets free of failures; binary118pass/
1fail at pathname-bind EPERM. Other server-startup/host/chooser assertions
remain failed, not passes. See [STASH-MERGE-RESULT.md](STASH-MERGE-RESULT.md)
for raw-run/rerun distinctions and index-resolution status. No commit or push
is part of this checkpoint. All Luna ownership is released.

Final build passes; decoder39/39 passes. Python76/77 passes, with its loopback
listener test denied by EPERM (direct loopback bind reproduces the denial).
Docs235/0, whitespace and conflict-marker checks pass. Attempting to mark the
five resolved paths with `git add` failed at `.git/index.lock` with Operation
not permitted; the worktree resolution is complete, but the index remains
unmerged until that same add runs where Git metadata is writable. No stash
was dropped and all reapplied/untracked work is preserved.

The upstream history reported verifier 8/8 and 993 passed / 0 failed / 15
ignored after merge commit 095bf45 plus four-pane, agent CLI, L16 relaunch,
L17 attach and L18 exit-status work. This was before stash integration. The
stash history reported library 641 passed / 4 ignored, focused integration
73 passed / 0 failed, and verifier 7/8 with binary 115 passed / 1 failed at
pathname-bind EPERM for async launch. These are separate prior evidence sets,
not combined-tree verification.

## Merge with origin/main + L16/L17/L18 — 2026-10-22 (read this first)

Merged origin/main (Wave1 usability, Wave2 async client worker and owner-tick
close, Herdr study, Subagents SA plan) as merge commit 095bf45. Docs
conflicts were resolved by keeping both histories. The uncommitted four-pane
work was then reapplied on top. Merge fixes:
- Remote code still sized for two panes: client screen_revisions, the server
  resize `failed`/`pending` arrays, and the ResizeReply pending bitfield. The
  wire field now carries bit i for pane i, and a test covers all 4 slots.
- Focus moves now use the remote's `set_focus` (focus generation).
- Remote saved-layout contract tests were updated to the 280-byte format plus
  a legacy-migration test. The byte-mutation contract found a real
  non-canonical decode: the one-pane current form now requires its root rect.

Three GPT-6-Luna (xhigh) workers ran in parallel:
- L16 relaunch (stopped at its timebox; parent finished it): see
  P3-L16-RELAUNCH4-RESULT.md. It also fixed a codec axis bug and per-pane
  lease scoping.
- L17 four-pane attach host test: P3-L17-ATTACH4-RESULT.md.
- L18 exit status (pane reply schema 10): P3-L18-EXIT-STATUS-RESULT.md.

Verifier 8/8; full suite 993 passed / 0 failed / 15 ignored. Not committed
beyond the local merge commit; nothing pushed. Apple Terminal four-pane
check still pending. Remaining Phase 3 gaps (layout events, cwd/command
metadata, live scrollback, `pane run`, owner identity) are open.

## Four panes + agent CLI integrated — 2026-10-22

The previous session ended with two worker slices left uncommitted and
unverified: L15 four-pane layout (P3-FOUR-PANES.md) and the agent pane/wait/
events CLI (P3-AGENT-CLI.md). On resume the parent found 4 binary-test
failures, 2 clippy errors, a consistently failing agent CLI test, a
four-pane test that never ran, and stale capacity-2 tests. Real bugs fixed:
nested-split divider validation, double chrome offset in the recovery view,
the regex matcher, and send/exit-wait races. See both notes. Verifier 8/8;
full suite 954 passed/0 failed/15 ignored. Not committed (slice, not a
completed phase). Four-pane Apple Terminal acceptance has not been run.
Phase 3 remains open.

## Phase 2 handoff: what closing did NOT prove — 2026-10-22

Phase 2 is ticked in ROADMAP.md for its scoped gates. These items are still
unproven or only partially evidenced; none blocks later phases, but do not
describe them as passed:

1. **Operator-driven host evidence is thin.** The nine outer-PTY host tests
   ran 9/9 (three serial runs plus a parent rerun), but from worker
   processes with no controlling terminal; they provision their own outer
   PTY. That is not a human-operated Apple Terminal session, and three runs
   are not a flake-rate claim (P2-HOST-ACCEPTANCE.md, "Limits").
2. **Manual Apple Terminal checklist was never itemized.** The operator said
   redraw, Unicode, vi, resize, paste and Ctrl-C were fine and daily use
   works, without per-item results or a host/version record
   (P2-OPERATOR-RUN.md). Only the OSC 52 clipboard-denial check has an
   explicit pass (clipboard unchanged, 2026-10-22).
3. **Ctrl-X live exit.** Ctrl-X was added as the local quit key (Ctrl-Q kept
   as an alias, but the host maps Ctrl-Q to a window-close confirmation). The
   decoder/dispatch tests pass; a live Ctrl-X exit with outer-shell
   restoration has no recorded operator confirmation.
4. **Redraw retest.** The incremental renderer replaced full-screen repaint
   on every change. Unit and parser-replay evidence exists
   (P2-DELTA-RESULT.md); a recorded Apple Terminal retest of the changed
   path does not.
5. **Restore-before-child-wait** is proven only by private-PTY tests
   (P2-RESTORE-ORDER.md), not by a human observing a slow child on a real
   host.
6. **Other terminals.** Only Apple Terminal on Apple Silicon was in scope.
   No other emulator was tried.

If a human pass is wanted, the checklist is in P2-EXPERIMENT.md and the
four-pane check (Phase 3) can be run in the same sitting. Phase 2 is not a
persistent-server or OpenCode-identity result; those are Phases 3 and 4.

## Phase 2 closed — 2026-10-22

User confirmed the OSC 52 clipboard-denial check in a live pane. A Haiku 5.5
worker ran the nine operator-only outer-PTY host tests: 9/9 in three serial
runs; parent rerun 9/9. Phase 2 boxes ticked. Committed; no push.

## Phase 6 complete (personal alpha scope) — 2026-10-22 (read this first)

User asked to complete Phase 6 and chose idle real-OpenCode agents (no
model calls) and a local build + install script release path. Three Luna
workers: L14 security review pack (clean); L13 release manifest/install
script (clean; two independent builds produced identical SHA-256); L12
real-OpenCode workload harness hung, parent stopped it and fixed the harness
(readiness-query deadlock, keepalive gaps, 30 s owner-lease expiry during
slow TUI startup, zombie-aware leak check), then ran A1/A2/B1/B2: 10 idle
OpenCode sessions usable, Panelink overhead recorded separately. Verifier
8/8 twice. Committed; no push. Remaining open: active-agent load, firm
approval/signing, and earlier phase gates (P2 host clipboard, P3 wider
contracts, P4 identity blocked upstream, P5 hierarchy/selection/reboot).

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

## Live-split launch/journal isolation completed — 2026-10-22

User approved the bounded architecture/build task following layout storage.
Parent accepted [ADR-LAUNCH-01](ASYNC-LAUNCH-DESIGN.md), orchestrated three Luna
high workers and integrated/reviewed the owner/worker/storage/runtime changes.
Live splits now perform preparation, journal IO and spawn off the owner through
one serialized worker. Owner adoption precedes final journal sync. Duplicate,
uncertain-result, disconnect, topology-busy and shutdown behavior is recorded
in [ASYNC-LAUNCH-RESULT.md](ASYNC-LAUNCH-RESULT.md).

Final parent checks: library641pass/4ignored; selected integration73pass/0fail;
verifier7/8 with binary115pass/1fail at existing pathname-bind EPERM. Strict
Clippy/fmt/build, decoder39, Python60 and component fingerprints pass. Final
docs216/0. The first normal-stack integration run exposed inline pending-state
stack growth; bounded boxes corrected it and the normal-stack rerun passes.
New gated tests prove real existing-pane input/read progress before release,
one child after exact retry/disconnect, singleton retention on cancellation,
and safe finalization after child exits. All worker ownership is released.

No wire/dependency/vendor/manifest changes or Git mutations. Initial startup
and legacy direct-core launch stay synchronous. Blocking calls cannot be
forcibly interrupted; bounded shutdown reports incomplete if its shared
deadline expires and makes no cleanup claim beyond process exit. Broad
Phase3/5/6, physical reboot and provider/host gates stay open. Follow ROADMAP.md
for the next phase; do not repeat this completed live-split slice.

## Bounded layout storage completed and reverified — 2026-10-22

User requested completion with Luna workers and deferred Git work. The store,
guarded writer, lifecycle capture and read-only CLI already existed in the
combined tree. Two Luna high reviewers checked the integrated boundaries;
parent fixed abandoned pending-shutdown cancellation and added its gated
regression. New acceptance covers uncertain last-pane close, explicit last
close, shutdown during phased close, and a single shared metadata deadline
with both writers blocked after child exit. Parent reviewed and independently
ran all new cases. See [P5-LAYOUT-COMPLETION.md](P5-LAYOUT-COMPLETION.md).

Final focused integration: 66 passed/0 failed/0 ignored. Library: 625 passed,
4 ignored. Combined verifier: 7/8; binary 115 passed/1 failed on the existing
pathname-bind EPERM. Decoder 39 and Python 60 pass; strict all-target Clippy,
fmt, build, component fingerprints pass. Final docs check: 214 files/0 findings.
The direct verifier initially selected an empty Cargo cache; the rerun used
the populated default cache through a runner wrapper, as documented in the
completion report. No dependencies/vendor/manifest changes or Git mutations.
All worker ownership is released.

Bounded layout storage is complete within this slice. Launch preparation and
launch-journal sync remain synchronous on the server owner and need a separate
architecture task. Full Phase 5/reboot/provider/host acceptance remain open.
Follow ROADMAP.md for subsequent phase work; do not repeat the storage build
or treat its completion as approval for concurrent launches.

## Wave2 scoped implementation integrated — 2026-10-22

User approved client responsiveness/high, server close progression/high and
saved-layout codec/medium. Parent reviewed both concurrency designs before
implementation, then reviewed/corrected source and independently verified it.
All three Luna owners released files. Preserve all dirty Wave1/planning/source
work; no active agent is required to resume. Ownership, parent corrections and
exact final evidence are in [WAVE2-RESULT.md](WAVE2-RESULT.md) and
[WAVE2-TASKS.md](WAVE2-TASKS.md).

Delivered: bounded client IO worker with host-owned UI, immediate saturated
CtrlX/restore, target-pinned input and delayed split focus; owner-tick close
with retained ledger/child ownership across dropped waiters, closing-input/
resize rejection, shared driver shutdown; strict historical160-byte layout
codec. No dependencies/vendor/manifests changed. No broad phase
completion. The Wave2 checkpoint is recorded in Git history. Async launch/journal work, layout storage/runtime, tabs/sidebar and
provider adoption remain outside this wave.

Final verifier7/8 after last guard: library569pass/4ignored; binary109pass/1fail
at known pathname bind EPERM; decoder39/Python45pass; strict all-target Clippy,
fmt/build/component fingerprints/docs191/0 pass. Independent selected26
integration tests pass, including real two-PTY sibling close, symmetric shared
shutdown with exact ESRCH absence and legacy history-driver regressions. Parent
client50 and codec8 tests pass. Logs are under wave2-evidence/. Root Cargo stops
at binary failure, so selected external tests were run separately. Unrun
delayed-fsync injection and complete host/provider/reboot acceptance remain
explicit limits, not passes or manual tasks assigned to the user.

At the time of this checkpoint, the next action was the bounded layout store. The incoming recovery slice has since implemented that store and the viewer/relaunch flow; the current next action is recorded in ROADMAP.md.
The ms-rust skill remains unavailable. Graph discovery succeeded early, then
parent transport returned Transport closed; direct source fallback was used.
No final refreshed-index claim. The recovery implementation arrived in the incoming branch; see the recovery sections below.

## Usability wave 1 — 2026-10-22

User requested 2–3 Luna agents high/medium on the recommendations. Parent
assigned U1 input/high, U2 help/medium, U3 bounded cleanup/high with exclusive
ownership; all have released files. Parent reviewed, integrated cleanup and
added real-PTY wire/input and continuous-writer cleanup acceptance. See
[task contract](USABILITY-WAVE1-TASKS.md) and
[integration result](USABILITY-WAVE1-RESULT.md) for actual checks and limits.
Input support and CtrlB-? help are implemented; larger async responsiveness,
mouse/clipboard/selection, hierarchy/tabs and provider observation remain open.
No dependencies/vendor/commit/push change. Parent final verifier7/8: library
555pass/4ignored, binary98pass/1fail at socket EPERM. Strict Clippy/fmt/build,
decoder39, Python45 and component sources pass. Independent privatePTY2 and
corePTY11 pass; multi-runtime1/close2 pass, pathname-close-runtime5 fail before
raw mode. Docs183/0 and whitespace pass. Source graph refreshed nonpersistent.
Preserve previous dirty docs; no broad phase completion or manual test handoff.
Next queued implementation remains the saved-layout codec; U3's larger async
design and remaining usability items are separate work.

## Herdr usability study — 2026-10-22

User explicitly requested a Luna study of Herdr for similar usability without
cloning. GPT-6-Luna xhigh delivered
[nine prioritized recommendations](HERDR-USABILITY-RECOMMENDATIONS.md), followed
by parent review/refinement. Existing two-pane splits/NAV/detach/history CLI
are distinguished from input/help/selection/navigation/status gaps. Proposed
delivery groups cover reliable interaction, navigation/layout/reading, then
verified attention/Subagents. No hierarchy decision or implementation adopted;
queued saved-layout work stays unchanged. Docs/whitespace checks passed.

Herdr documentation was read before the browser restriction was relayed. User
later opened the site in Chrome, but only tab metadata could be observed:
saved site permission still blocks page content. No screenshot, video-frame,
installed-app or hands-on visual review was completed. Parent independently
verified upstream repository metadata and Panelink's current input dispatch.

## Subagents feasibility attempt — 2026-10-21

User authorized S0 execution. [Results](SUBAGENTS-FEASIBILITY.md) and raw startup
JSON are preserved. Installed executable freshly reports OpenCode2.0.32;
SHA256/tag commit recorded. Fresh temporary XDG roots enable version/help,
but ACP initialize/private server and direct serve/stdio all fail before
readiness. Independent loopback and Unix binds return EPERM. No model prompt,
session creation, live worker or wrapper test ran. S0/P4 remain open; documented
parentID listing plus experimental per-session durable log provide a candidate
observer/reconciliation route, not runtime proof. No application code changed.
Current saved-layout next action remains; no manual verification task assigned.

## Subagents visibility planning — 2026-10-21

User requested automatic visibility of provider background/subagents in a tab
named Subagents for optional observation. Recorded R-17 and the proposed
[Phase SA plan](SUBAGENTS-PLAN.md). OpenCode V2 official docs expose candidate
child/session observation surfaces, but ACP's child activity stops after the
parent prompt ends and its private server is not an observer of arbitrary TUIs.
The spike must prove full background lifetime, exact ancestry, missed-event
reconciliation and no duplicate execution on the selected installed version.
User clarified scope: OpenCode and wrappers built on it; prove compatibility
per supported version. Read-only activity viewing is the proposed first slice;
interactive child TUI attachment needs separate proof and scope selection.
No runtime changes or new acceptance; current saved-layout next action remains.
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
pass;96 baseline Rust/test/manifest files and 13 reports have unchanged hashes.
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
and 13 prior raw/provenance reports have unchanged hashes. No new workload run.

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
