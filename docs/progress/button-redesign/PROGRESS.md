# Button redesign execution

```text
current-phase: BTN-00..06 source implementation and automated verification complete
status: complete for scoped source work; native/product gates remain open
branch: t3/terminal-button-design-research
baseline-head: fdd8664d3085958430f1fbd91ec066d868c33050
last-verified: canonical8/8; rootRust1626/0/20ignored; decoder39; Python233run/1skip; docs464/0
next-action: source ready for review; runtime activation remains separate
blocked-on: no source blocker; native-host/theme/clipboard/touch/provider gates not certified
activation: no install, user-pane operations, server restart or push performed
```

Authority: user said "start implementation" after the completed ten-question
[plan](../../design/BUTTON-IMPLEMENTATION-PLAN.md). All seven packets are in
scope. Existing dirty planning/glossary/roadmap files belong to the parent and
must be preserved. No other live source owner was reported at start.

## Packets

- [x] BTN-00 exact baseline, control inventory and terminal-cell prototype (final serial suite green; native palette gates separate)
- [x] BTN-01 shared pure geometry/styles and compositor adapters (focused parent checks pass; final combined gate still open)
- [x] BTN-02 captured release activation and keyboard navigation (focused checks; full combined gate still open)
- [x] BTN-03 pane workflows/dialogs (focused checks; final combined gate open)
- [x] BTN-04 tabs/project/chooser workflows (focused checks; inactive close requires select first)
- [x] BTN-05 remaining controls and consistency audit (parent help release fix and reduced-color checks pass)
- [x] BTN-06 combined verification, docs/skill synchronization and evidence

## Ownership

First worker may edit only `src/terminal/controls.rs` (if justified),
`src/terminal/mod.rs`, `src/terminal/compose.rs`, and new
`docs/progress/button-redesign/BTN00-01.md` plus cell evidence files under
`docs/progress/button-redesign/cells/`. Parent retains every other file,
integration decisions, roadmap/progress updates and commits. Worker must not
change root event behavior, dependency manifests, protocol/server code or tests
outside its compositor/helper ownership. Shared source packets run sequentially.

## Verification and evidence

Use an isolated Cargo target and synthetic fixtures; no installed runtime changes.
Record exact source identity, commands, actual result counts and failure logs.
Baseline canonical command: `RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py`.
Focused foundation gate: compositor/helper unit tests, formatting and strict
all-target Clippy when feasible. Parent spot-checks before accepting any packet.
Human host/clipboard/touch/provider gates remain separate. ms-rust is absent;
do not claim its use or compliance. Do not erase or skip a real failing assertion.

## Parent review of first worker

Not accepted yet: the adapter discards `layout_filled` and passes label widths
that cannot fit its 6/3-cell bounds. The style palette is unused, leaving the
buttons text-only. Existing appearance assertions need intentional updates to
the approved filled design, retaining behavioral/damage guarantees. Handwritten
cell examples do not prove renderer output. Baseline provider WouldBlock failures
are recorded, but pre-change provenance was not demonstrated; do not claim them
proved pre-existing. A bounded correction precedes BTN-02.

## Foundation parent completion

Parent corrected the partial renderer and independently passed terminal92/0,
strict all-target Clippy, fmt and diff. Actual parser-decoded cells and red/green
logs retained in [packet evidence](BTN00-01.md). BTN-00 baseline classification,
light/dark/reduced routing and overall acceptance remain open. Next worker owns
BTN-02 root interaction paths and relevant fixtures, with shared foundation
changes allowed only when needed for capture-state presentation.

## BTN-02 narrowed parent implementation

The broad BTN-02 worker reverted its experiment and retained label fixes only;
its151/0 state checks prove compatibility, not activation. Parent implemented
header Menu/close capture in App: Down stores exact target/revision/route/host;
Up inside same valid control dispatches once; drag-away/Escape/layout/modal
changes cancel. Opening release latch is consumed locally. Two new raw-event
regressions pass; whole existing state suite is142/9 because nine fixtures still
expect Down activation. Source is intentionally not accepted yet.
Next bounded owner: header capture/fixture migration and verification only, not
all modal/tab/keyboard BTN-02 work. Parent owns all roadmap/evidence acceptance.

## BTN-02a accepted bounded slice

Parent reviewed the wrapper and worker diff, independently passed four raw
header-capture tests and the real private-PTY pointer flow (1/0;13.83s). Worker
state155/0, terminal92/0, both private flows and strict checks are retained in
[BTN02a](BTN02a.md). This accepts header activation only, not all BTN-02.
Next sequential source owner: header pressed rendering in App RenderView and
compositor; no other capture/keyboard behavior changes in that task.

## BTN-02b accepted bounded slice

Parent independently passed raw header4/0, actual-color invalidation1/0 and
compositor partial-write/overlay1/0; reviewed immutable capture validation and
slot damage tracking. Worker full state156/0, terminal93/0, pointer1/0 and strict
checks recorded in [BTN02b](BTN02b.md). Header activation plus pressed rendering
are accepted. Whole BTN-02 remains open. Next sequential owner handles pane
action/split menu and confirmation release capture, local keyboard rules and
pressed presentation; tab/header/history captures remain later slices.

## BTN-02c accepted bounded slice

Parent inspected captured modal identity/target/revision, opening latch changes
and row/button geometry; independently passed modal-filter3/0 and disabled-action/
confirmation-cycling1/0. Worker full state159/0, split31/0, terminal93/0 and both
private flows1/0 retained in [BTN02c](BTN02c.md). Next source owner implements
remaining existing tab/chooser/history/empty button captures and local keyboard
focus. Styling/grouping and final source-wide consistency remain later packets.

## BTN-02d keyboard subset and BTN-02e parent strip capture

BTN02d incomplete worker retained only local dialog keyboard focus; pointer work
was not delivered. Parent implemented tab-strip capture with empty-host support
and strict catalog/layout/action validation. [BTN02e](BTN02e.md): state162/0,
strict Clippy/fmt/diff pass; no new private fixture run yet. Next owner handles
strip pressed rendering plus Back to live and empty-body capture only. Tab
dialogs/chooser rows remain explicit next slice.

## BTN-02f accepted bounded slice

Parent inspected semantic surface capture/revalidation and committed history
damage/style resets; independently passed surface regression2/0 and direct/root
empty regression1/0. Worker state165/0, tabs10/0, terminal93/0, private target5/0
and strict checks retained in [BTN02f](BTN02f.md). Tab dialogs and chooser rows
are the remaining BTN-02 capture work; broader filled styling/grouping remains
BTN-03/04, not implied complete by these input checks.

## BTN-02g accepted; input slice complete

Parent reviewed workspace semantic action/bounds, UI epoch/catalog/dialog/filter
stamp, runtime/focus/host validation and empty-host priority. Independently ran
full state suite167/0. Worker tabs10/0, terminal93/0, serialPTY5/0 and strict checks
retained in [BTN02g](BTN02g.md). BTN-02 focused acceptance is complete; final
canonical/host acceptance remains open. Next owner is BTN-03 pane style/grouping
plus reusable per-control filled overlay styling; no tab layout work concurrently.

## BTN-03 accepted after parent bound fix

Reviewed modal geometry enumeration, grouped pagination and actual per-control
fills/cached spans. Parent pane1/0, spans1/0 passed; found silent truncation above
32 styles. Raised bounded span capacity to twice MAX_ROWS and added pre-output
count/row/coordinate validation with regression. Parent overlay tests2/0, strict
all-target Clippy/fmt/diff pass. Logs logs/btn03-parent-*. Worker state168/0,
terminal94/0, split31/0, serialPTY5/0 in [BTN03](BTN03.md). No runtime acceptance.
Next sequential owner: BTN04 tab strip/individual close targets and filled
workspace controls using shared style spans; no overlapping source writer.

## BTN-04 accepted bounded source

Parent reviewed strip disjoint Select/Close bounds, pinned controls, shared
workspace fills and safe Close(route) routing. Independently passed BTN04 state
1/0 and width22..240 close/active/create/overflow geometry1/0; worker full
state169/0 tabs11/0 terminal95/0 serialPTY5/0 and strict checks in [BTN04](BTN04.md).
Inactive close retains explicit select-first runtime guard; no automatic switch
or unknown-generation stop. BTN05 owns remaining control/theme audit and
canonical source/help/usage-skill synchronization; combined verifier still open.

## BTN-05 parent closure and safety review

Reviewed reduced-mode pure setter and shared SGR emission; parent actual mode
fixtures2/0 passed. Fixed the audited help Down activation using shared
help-control bounds and captured release; full state170/0 passed. Independent
read-only safety review found in-bounds workspace Drag(Left) canceled capture;
parent fixed it with a passing raw regression. Reviewer reports no remaining
material findings (source review only). Canonical source/README/help/spec/UX
notes and repository usage skill/references synchronized; skill frontmatter and
17 local links validated. Documentation checker463/0 before final evidence docs.
Full canonical verifier now running; do not infer pass until its final summary.

## BTN-06 initial full gate and correction

Initial full verifier7/8: library941/0, binary342/1 (old in-bounds-drag cancellation
expectation). All seven support gates passed. Parent preserved full log and
changed only that cancellation scenario to cross the control right edge;
positive in-bounds release separately tested. Corrected matrix1/0; fresh full
verifier running. [Integration evidence](BTN06.md) records review/doc/runtime
boundaries. No acceptance inferred from the still-running process.

## Final parent result — 2026-10-31

All scoped source packets complete. Final canonical verifier **8/8**, root Rust
**1626/0/20 ignored**, decoder39/0, Python233run/1skip, docs464/0; component
fingerprints/fmt/strict Clippy/build pass. [Integration record](BTN06.md) and
[full final log](logs/btn06-canonical-verified.log). Parent source review and
bounded independent safety review complete, one material in-bounds-drag finding
fixed and regression-tested. Historical red results above are preserved evidence,
not current failures. Repository usage skill/frontmatter/local links synchronized.

Changes remain uncommitted in this checkout. No broad Phase3/5 completion or
phase-completion commit is claimed while their native acceptance gates remain
open; no additional checkpoint commit was requested. Runtime installation,
server restart, live user input and pushing were not performed. No ms-rust use
or native light/dark/clipboard/touch/provider certification is implied.
