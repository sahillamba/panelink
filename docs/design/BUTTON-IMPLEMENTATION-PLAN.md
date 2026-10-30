# Panelink button redesign implementation plan

Status: source implementation complete, 2026-10-31; canonical automated gate8/8.
Native host/theme/clipboard/touch/provider gates remain separate and open.
Progress and evidence: [execution state](../progress/button-redesign/PROGRESS.md).
Started: 2026-10-30.
Branch: `t3/terminal-button-design-research`.
Checkout: `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`.
Source baseline observed: `fdd8664d3085958430f1fbd91ec066d868c33050`;
planning/glossary files are uncommitted. Recheck before execution.

## Objective

Rethink Panelink's buttons so users can recognize actions as clickable before
interacting with them. Cover the whole existing control surface, rather than
changing only the pane Menu label. Produce a complete, actionable implementation
plan using the grilling and domain-modeling disciplines, with explicit decisions,
dependencies, file ownership, checks and remaining acceptance gates.

Inputs: [research](BUTTON-DESIGN-RESEARCH-2026-10-30.md),
[comparison](button-design-options.html),
[existing UX requirements](../UI-UX-PLAN.md),
[glossary](../../GLOSSARY.md), [roadmap](../../ROADMAP.md) and
[current handoff](../progress/SESSION.md).

## Authority and boundaries

The user requested research, then an implementation plan and a design interview.
Round 1 confirms option A, compact filled buttons, and explicitly includes
changes to underlying actions. Round 2 retains Close as process termination and
Detach as leaving processes running; improve existing actions first, with new
capabilities requiring explicit selection. Rounds 3–4 confirm layout, action
flows, interaction states, appearance, tab placement and local keyboard navigation.
Planning does not authorize
activation, server restarts, installation, pushing, or completion claims.

Existing confirmed requirements remain in force: terminal-hosted application,
project → workspace tab → pane hierarchy, four panes, keyboard/mouse access,
no permanent sidebar, no whole-pane dragging, and no new zoom expansion.
Close pane/tab ends the affected processes after confirmation; Detach keeps them
running. Preserve input ownership, nonreplayable operations, clipboard authority,
stable destructive targets and unresolved host acceptance throughout.

The current source tree and installed executable are separate identities. Source
observations here do not establish the behavior of the user's running build.

## Interview record

### Round 1 — confirmed direction and depth

1. **Visual direction:** adopt A, compact filled controls using the existing tab
   and pane-header rows? Recommendation: yes. Alternatives: B, outlined controls;
   C, a global action toolbar that uses an additional row.
   **User answer: Yes — A: filled buttons.**
2. **Depth of rethink:** include labels, grouping, padding/click targets,
   keyboard selection, pressed/unavailable/pending feedback and confirmations,
   while retaining what the underlying terminal/process actions do?
   Recommendation was yes.
   **User answer: Include changes to underlying actions.**
   Action semantics are therefore within the planning scope; the interview must
   identify the desired changes instead of assuming a purely cosmetic refactor.

### Round 2 — confirmed action boundaries

3. **Close versus detach:** should Close pane/tab continue ending the affected
   processes after explicit confirmation, with Detach keeping them running?
   Recommendation: yes; change wording/flows as needed without making close
   silently mean hide. **User answer: yes.**
4. **New capabilities:** should this plan improve existing action behavior and
   workflows, with any entirely new capabilities explicitly named before adding
   them? Recommendation: yes. Candidate additions for clarification include a
   launch picker for new tabs/splits and a searchable action palette; neither is
   accepted scope. **User answer: yes.** No new capability was named.

The glossary now records Close pane, Close workspace tab and Detach using these
confirmed meanings. No new ADR is warranted for retaining this existing contract.

### Round 3 — confirmed layout, menus, feedback and appearance

5. **Permanent controls:** retain one top tab row, filled Menu and close controls
   in each pane header, and contextual Back to live; place remaining pane actions
   in Menu rather than adding a permanent action toolbar? Recommendation: yes.
6. **Action flows:** use a shared full-row pane menu for Menu/right-click; group
   Split/Resize, Copy/History and Close, with direct directional split choices,
   clickable resize adjustment/Apply/Cancel, and visible reasons when unavailable?
   Recommendation: yes. Keep global/tab actions clearly scoped separately.
7. **Interaction states:** activate once on release inside the original button;
   release outside, Escape or a changed target cancels; retain keyboard activation,
   obvious focus, truthful pending/disabled feedback and no hover requirement?
   Recommendation: yes. Existing selection/scrollbar/divider gestures remain
   distinct and must not leak input into child applications.
8. **Appearance:** neutral filled secondary buttons, one consistent accent for
   the primary/current action, and destructive color plus explicit wording in
   confirmations; preserve child terminal colors and support light/dark/reduced
   colors without special fonts? Recommendation: yes. Exact color values require
   cell-accurate prototype checks, not acceptance of browser CSS colors.

**User answer: yes to all (Q5–Q8).**

### Round 4 — confirmed final product details

9. **Tab controls:** project selector left, tabs in the middle, New tab and
   All tabs pinned right; separate close targets on visible tabs, with Rename
   exposed through a tab-actions menu? Recommendation: yes. Preserve selected
   tab and creation/overflow access at narrow widths; truncate names first.
10. **Keyboard navigation:** retain current global shortcuts; use arrows/Enter
    in menus, Tab/Shift-Tab between dialog controls, and Escape to cancel?
    Recommendation: yes. Keep ordinary shell Tab/Space unchanged, and preserve
    normal text editing inside rename fields. Cancel remains initially focused
    for destructive confirmations.

**User answer: yes (Q9 and Q10).**

### Final shared-understanding checkpoint

All ten product questions have answers; no unanswered product branch remains.
The user confirmed the whole outcome with "start implementation" and authorized
execution. Engineering defaults are implementation choices to validate against
source and the cell prototype, not additional product requirements. Any later
scope amendment must identify the actual new decision rather than silently
expanding this agreement.

## Design decision tree

- Filled visual direction and action-behavior scope — settled in round 1.
  - Header layout, action priority, compact forms and minimum usable widths.
  - Theme strategy, emphasis, reduced-color and ASCII behavior.
  - Press/release/cancel semantics, keyboard focus and activation.
  - Menu grouping, unavailable-action explanations and context preservation.
  - Confirmation targets, wording, initial selection and pending outcomes.
- Underlying action boundaries — settled in round 2.
  - Close/detach lifecycle retained; no new launch picker or action palette selected.
  - Specific creation, split, resize, copy and tab workflow changes.
- Layout, action flows, button feedback and appearance — settled in round 3.
- Tab control placement and local keyboard navigation — settled in round 4.
- Complete control coverage — initial source inventory verified.
  - Project/tab controls, overflow and choosers.
  - Pane headers, split/resize/copy/history and existing compatibility actions.
  - Dialogs, saved-layout/recovery surfaces, help/detach and empty states.
- Implementation design — documented below; prototype/baseline verification is
  BTN-00, not an unanswered user decision.
  - Shared render/hit geometry and button state ownership.
  - Integration with existing modal and event-priority rules.
  - Ordered packets, exclusive files and integration boundaries.
  - Regression tests, cell-accurate visual evidence and host acceptance.
  - Canonical spec, usage skill, examples and final evidence updates.

Interview frontier: empty. Final whole-plan confirmation: received.

Recompute the question frontier after each answer. Do not ask for facts that can
be verified from source, documentation or the environment. Record new answers
as they arrive; do not silently turn recommendations into requirements.

## Final plan completion criteria

Before marking this plan ready, provide the agreed final screen/control behavior,
a complete control inventory, chosen terminology, applicable ADRs, architecture
and symbol pointers verified against current source, dependency-ordered packets
with exclusive ownership, test and verification commands, representative
minimum/normal/overflow cases, rollout boundaries, and explicit open acceptance
gates. Identify whether a preparatory prototype is needed to settle any remaining
visual or input question. Implementation begins only after the interview frontier
is empty and the user confirms the shared understanding.

## Verified planning facts

The read-only source audit identifies a material navigation constraint: the
codebase-memory index reports drift, so indexed symbols are starting points and
packet ownership must be based on exact current source. Current owners include
`src/terminal/compose.rs` for pane control/scrollbar geometry and painting,
`src/ui_client/state/workspace.rs` for tab dialogs and their input/presentation,
`src/ui_client/state/interaction.rs` for pane action capture, and
`src/ui_client/state.rs` for root event coordination. Do not assign overlapping
render/event files to workers on the strength of an older monolithic-state plan.

The [CI guide](../CI.md) defines the final local automated gate as:

```sh
RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py
```

This includes root Rust tests, decoder tests, component verification, formatting,
strict all-target Clippy, build, Python tests and documentation checks. Fetch the
locked root and decoder dependencies first when unavailable locally. The final
packets must add focused behavioral checks before this combined gate, rather
than treating a broad green result as sufficient button coverage.

No builds, tests, live pane operations, runtime installations or host acceptance
checks have been run during this planning interview. Apple Terminal behavior,
real clipboard, provider acceptance and other roadmap gates remain separate
from the automated gate. No ADR directory currently exists; create one only if
a settled decision meets the domain-modeling skill's consequential-tradeoff test.

## Intended experience

The application has a quiet terminal canvas with unmistakable actions in its
surrounding chrome. Neutral filled buttons have visible spacing; a selected tab,
a keyboard-focused control and the pane receiving terminal input are different
states. The focus marker stays with the pane header. Informational labels such
as Live, History, New output, Copied and the client version are not styled as
buttons unless an actual action exists.

Headers remain one terminal row. The project selector sits left, visible tabs
occupy the middle, and New tab/All tabs remain right-aligned. Visible tab close
targets are distinct from selection; Rename lives in tab actions. No extra
permanent pane toolbar is introduced.
Use side padding inside each button and include it in the click target. Preserve
space between targets. At narrow widths, discard optional context and truncate
titles before collapsing labels to compact forms. Never overlap hit regions or
give a hidden/clipped label an active invisible target. Derive bounds from the
existing layout limits; do not shrink child PTYs merely to fit verbose labels.

Pane Menu and right-click present the same captured pane's actions with full-row
targets, grouped by purpose. Group headings are not clickable. Directional split
choices show actual availability. Resize exposes decrement/increment, position,
Apply and Cancel. Keep supported legacy zoom/swap actions reachable without
promoting new zoom controls or introducing pane dragging.

Close confirmation names its captured target and consequence. Close pane ends
that pane's process; Close tab ends the tab's processes. Cancel is initially
focused. Detach leaves processes running. If state changes while a confirmation
is open, invalidate or reconcile using the existing target/revision contract;
never retarget a confirmation to a replacement pane or tab.

New tab still creates and focuses a managed ordinary shell. Successful split
focuses the new shell; failed/canceled split preserves focus. History keeps its
reading position under output; Back to live restores following. Selection and
copy remain intact, with truthful success/failure feedback. No new launch picker,
searchable action palette, or server/protocol feature is implied by this plan.

Preserve current global shortcuts. Within menus use arrows/Enter; within dialogs
Tab/Shift-Tab moves between controls and Escape cancels. Existing text-field
editing and ordinary shell Tab/Space stay intact. Keep menu keyboard selection,
dialog control focus, active tab and typing-pane focus visibly distinct.

## Engineering design — proposed implementation defaults

### Shared presentation and geometry

Introduce a small internal control model, with names finalized during source
review: semantic action identity, captured scope, label/compact label, visual
role, enabled/pending state, and a terminal-cell rectangle. Keep immutable
presentation separate from effect execution. A style role is not an operation
receipt, and a generated pane label is not an automation identity.

One layout result supplies both cell painting and hit testing. Adapt existing
`pane_controls`, history geometry, tab header hit regions and modal geometry
to this contract rather than layering independent button coordinates on top.
Use existing safe label/Unicode-width handling; truncate by terminal cells.
Do not import a GUI framework, icon font or styling dependency for this work.

Proposed new helper: `src/terminal/controls.rs` for pure control geometry and
semantic presentation types, exported internally through `src/terminal/mod.rs`.
Validate whether the existing compose types can be reused before creating it.
Keep compositor write/damage ownership in `compose.rs`; avoid a broad renderer
extraction. The shared types may describe disabled reasons, not discover action
availability by issuing I/O from painting code.

### Captured interaction

Add client-local button press state where root mouse priority is coordinated.
A press captures action, target/scope, layout revision and geometry validity;
it does not execute, claim terminal input or send child bytes. Paint pressed
feedback immediately. Release activates at most once inside the same valid
target. Release outside, Escape, lost focus, route/layout change, target removal,
or a modal ownership change clears the press. Conservatively cancel a capture
when its validity cannot be established. No replay after reconnect or uncertainty.

Captured menu rows obey the same release rule. The press opening a popup cannot
also select its first row on release. Dragging across unrelated buttons does not
transfer the captured action. Read-only history actions remain outside terminal
input ownership. Existing selection, scrollbar and divider captures retain their
own rules and event priority; do not globally rewrite all pointer gestures.

Keyboard activation uses the same admission/effect path as pointer activation,
without requiring a synthetic pointer press. Prevent repeat-driven duplicate
destructive/mutation submission and preserve existing pending-operation guards.
Pending feedback follows real operation state. An unavailable action cannot
execute; its reason remains discoverable by menu selection or concise help.
Outcome unknown remains explicit and does not become a retry button.

### Rendering, theme and accessibility

Keep filled backgrounds confined to chrome/overlays and restore terminal style
after each run. A fill or state change invalidates the complete button bounds,
including spaces, and erased/moved controls restore correctly. Maintain the
existing successful-write commit rule for compositor caches and dirty state.

Prototype neutral/action/danger/disabled/focus roles in dark, light and reduced
colors. Do not assume host theme reporting is available. Prefer existing host
capabilities; any required theme override must be proposed explicitly before
adding a setting. In reduced-color mode use structural markers/reverse video
and text rather than relying on hue. No hover dependency, hand-cursor promise,
emoji width assumptions, animation requirement or global key interception.

## Complete control inventory and migration owners

| Family | Presentation and behavior to migrate | Current source starting point |
| --- | --- | --- |
| Pane header | Menu, close, focus marker and title priority | `src/terminal/compose.rs`, `src/ui_client/state.rs` |
| History chrome | Back to live; status remains noninteractive | `src/terminal/compose.rs`, viewport/root coordination |
| Pane actions/split | Shared grouped full-row menu; actual disabled directions | `src/split_menu.rs`, `src/ui_client/state/interaction.rs` |
| Resize/swap/legacy zoom | Consistent controls, cancellation and compatibility | `src/ui_client/state/interaction.rs`, `src/pane_layout.rs` |
| Close pane | Exact-target confirmation, Cancel focus, pending result | interaction state, root admission and render adapters |
| Project/tab strip | Project chooser, tab selection, create, close and overflow | `src/workspace_tabs.rs`, `src/ui_client/state/workspace.rs` |
| Tab dialogs | Actions, rename field, Save/Cancel, close consequence/pending | `src/ui_client/state/workspace.rs` |
| Choosers | Full-row targets, selection/filter editing and cancel | `src/workspace_tabs.rs`, `src/ui_client/state/workspace.rs` |
| Empty workspace | New tab and Detach with distinct emphasis | workspace state/render paths |
| Copy/help/detach | Existing effects, truthful copy status, visible dismiss/action routes | copy/interaction state and root effect adapters |
| Saved layout/recovery | Migrate existing saved-view action in pane menus; preserve key-only/line-mode contracts elsewhere | `src/ui_client/state/interaction.rs`, `src/split_menu.rs`, `src/default_open.rs`, `src/ui_client/recovery.rs`, `src/recovery_chooser.rs` |

Source inspection distinguishes actual buttons from other input surfaces:
`default_open::run_startup_chooser` and `ChooserTerminal::read_choice` use a
line-mode startup prompt; `ui_client/recovery.rs` is a read-only local viewer with
`RecoveryUi::handle_local_key` and `map_key`, using an unpadded compositor. They
are not existing clickable button surfaces. Preserve those contracts and key
hints; do not silently add mouse activation, graphical-looking inert labels or
new process relaunch buttons. The pane menu's saved-layout action is in scope.
Provider recovery commands retain existing CLI authority. Packet 00 verifies
this classification against the execution checkout before assigning files.

## Ordered implementation packets

These are planned tasks, not active assignments. No packet is complete until
its checks and limitations are recorded. Execution begins after final plan
confirmation. Parent owns shared integration, evidence and all commits.

### BTN-00 — exact baseline and terminal-cell prototype

- Recheck branch, HEAD, dirty files, current module boundaries, dependency
  availability, and separately identify any executable used for runtime evidence.
- Enumerate every current visible control, renderer, hit tester, event owner,
  keyboard route and fixture; verify the chooser/recovery classification above.
- Capture current control cells at normal/minimum widths. Build disposable
  cell-accurate representative frames for filled Menu/close, tabs, Back to live,
  one action menu and a close confirmation using synthetic content.
- Verify the 7×22 padded pane minimum, four-pane layout, tab overflow and large
  host bounds against current source. Record any label that needs a compact form.
- Own planning/evidence fixtures only; no installation, live user pane input or
  server restart. Compare prototype outcomes to the agreed direction before
  migrating all controls. If visual tradeoffs require a product change, return
  that specific choice to the user.

### BTN-01 — pure control layout and semantic styles

- Depends on 00. Own new pure helper, terminal module export, compositor geometry
  adapters and focused unit tests. Keep root behavior unchanged in this packet.
- Define shared cell bounds, roles and compact-label policy; migrate geometry
  contracts first. Establish normal, focused, pressed, unavailable and pending
  render states and safe style restoration.
- Checks: exact half-open edges, gaps, minimum-width clipping, Unicode labels,
  nonoverlap, padding-as-target, filled blanks, damage restoration, overlay
  exclusion and failed-write cache preservation.

### BTN-02 — one-shot pointer and keyboard activation

- Depends on 01. Own `src/ui_client/state.rs`, interaction state and relevant
  `state_tests.rs` cases. Parent integrates any host event changes only if needed.
- Route existing buttons through captured press/release and shared admission;
  preserve modal priority and all other gesture/child input paths.
- Checks: valid release once, release outside, Escape, missing release, target
  disappearance, layout/route changes, focus loss, changed modal, repeat input,
  pending guard, disabled activation and no child-input leakage. Exercise a
  replacement pane at the same spatial index as a stale destructive target.

### BTN-03 — pane actions and dialogs end to end

- Depends on 02. Own `src/split_menu.rs`, pane interaction/render integration,
  associated compositor changes and client tests, with no concurrent editor of
  those files. Deliver header → Menu/right-click → action → feedback paths.
- Migrate directional split, resize controls, close confirmation, copy/history
  and legacy compatibility actions. Maintain exact captured scope and existing
  lifecycle semantics. Verify inactive-pane menu actions do not accidentally
  redirect later typing or close the currently focused sibling.
- Checks: mouse/keyboard equivalence; disabled reasons; Cancel default; closing
  one pane while preserving sibling process identity; successful/failed split
  focus; applying/canceling resize; unchanged history and copy behavior.

### BTN-04 — tabs, project controls and dialogs end to end

- Depends on 03. Round-4 product choices are confirmed. Own `src/workspace_tabs.rs`, workspace state,
  tab render integration and tab fixtures. Shared root/compositor files remain
  sequentially leased to this packet, not edited by another lane.
- Migrate strip controls, tab actions, rename, creation/close, project/All tabs
  choosers, overflow and empty state. Preserve route/revision capture and current
  unknown-create reconciliation; clicking twice must not create duplicate tabs.
- Checks: active/new/overflow reachability, close-vs-switch hit separation,
  rename editing and saving failures, pending close, switching/deleting targets
  while dialogs are open, final-tab empty state, and no auto-spawn.

### BTN-05 — remaining control surfaces and consistency audit

- Depends on 04 and the inventory completed in 00. Own remaining attach help,
  saved-layout/menu and feedback presentation in interaction/render/copy state;
  audit `src/default_open.rs` and `src/ui_client/recovery.rs` against the key-only
  classification without changing their runtime contracts. No unspecified worker
  may modify shared roots.
- Apply agreed action/style/selection rules to all remaining actual buttons.
  Audit every family in the inventory for labels, disabled/pending treatment,
  escape routes and documented keyboard access. Leave informational statuses
  plain. Preserve separate saved-view and process-relaunch semantics.
- Checks: bounded terminal sizes, recovery refusal/cancel paths, correct target
  retention, unaffected saved-history viewing and no implicit relaunch/replay.

### BTN-06 — combined verification and documentation

- Depends on 05. Parent owns integration and affected canonical docs/skill files.
- Update `docs/SPEC.md`, affected `docs/UI-UX-PLAN.md` sections, README/help,
  `.agents/skills/panelink/SKILL.md` and its affected references to match the
  implemented behavior. Mark prototypes/pending host evidence accurately.
- Record focused results, cell captures, full non-UI verifier output and any
  explicit host observations. Validate Markdown links and usage-skill structure.
- Update roadmap/handoff with the final state and remaining gates. Source commit
  follows existing parent commit authorization only after its required checks;
  pushing and runtime activation remain separate user-requested actions.

## Execution dependencies and ownership

Default order: 00 → 01 → 02 → 03 → 04 → 05 → 06. These packets overlap central
Rust files, so do not parallelize their source mutations by surface alone.
Bounded read-only audits or independent fixture preparation can run in parallel
only with explicit exclusive file ownership and parent integration. Re-read the
actual checkout before executing; the code-health worktree's history does not
prove that this branch has identical symbols or a verified baseline.

## Verification matrix

| Dimension | Required cases |
| --- | --- |
| Geometry | One/two/four panes; minimum pane; narrow tab strip; overflow; 80×240 render bound and larger host; safe wide labels |
| Button states | Rest, keyboard focus, pressed, disabled, pending, failure and uncertain outcome |
| Input | Mouse click/drag-away, release without press, keyboard activation/cancel, repeated keys, paste in rename, shell Tab/Space |
| Invalidations | Route, target, layout, focus, buffer/gesture ownership and modal changes while pressed/open |
| Appearance | Dark/light/reduced color, ASCII forms, full background clearing, overlay restoration and style reset |
| Effects | One invocation, correct target, no child leakage, no implicit input claim/replay, pending duplicate suppression |
| Lifecycle | Close/cancel target, sibling survives, final tab empty, detach keeps processes, no implicit relaunch |
| Existing UX | Stable history, Back to live, split focus, resize/scroll focus preservation, selection/copy feedback |

Representative commands for existing test locations inspected during planning
(not executed in this task; augment rather than replace existing coverage):

```sh
cargo test --locked --lib terminal::compose
cargo test --locked --bin panelink ui_client::state_tests
cargo test --locked --test p5_workspace_tabs workspace_pointer_controls_preserve_live_ptys_and_confirm_close
cargo test --locked --test p5_workspace_tabs workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe
RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py
git diff --check
```

Confirm exact binary/target filters at execution; a command reporting zero tests
does not satisfy a gate. Serialize timing-sensitive private-PTY runs as documented.
Use injected events and controlled private fixtures for cancellation/target
regressions; avoid sleeps as readiness proof and retain real failing assertions.

## Completion and rollout

Completion requires all inventory families migrated, focused regressions passing,
canonical checks passing, representative cell frames inspected, and source/help/
skill documentation synchronized. Report host checks separately; do not count
unrun Apple Terminal, touch translation, real clipboard or provider acceptance
as passed. No promise of a hand cursor or native graphical button rendering.

This plan changes client UI and existing action workflows. Server/wire/storage
changes require a demonstrated need and an explicit scoped plan amendment; do
not bundle them preemptively. Preserve existing dirty work. A new client build
does not install itself, identify the running server or justify restarting it.
