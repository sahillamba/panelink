# Panelink UI/UX outcome and implementation plan

Status: confirmed by the user on 2026-10-27 and implementation requested
(parent orchestration with GPT-6-Luna / Claude Haiku 5.5 subagents).
Packet progress is tracked in [SESSION](progress/SESSION.md); no packet is complete
until its acceptance evidence is recorded.
Date: 2026-10-27. Planning baseline: `t3/panelink-ui-ux-plan`, HEAD `b97e2f7`,
checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-320eb3c2`.
Recheck checkout, branch, diff and executable identity before implementation.

## Purpose and authority

Deliver an independently designed Panelink experience inspired by Herdr's
accessible terminal interactions. The user's actual priorities are **fast, smooth
pane-history scrolling, reliable divider/scrollbar gestures, fully clickable
confirmations and better tabs**. Four panes are enough. Whole-pane dragging,
zoom expansion and a permanent sidebar are excluded from this redesign.

This document supersedes the original seven-packet draft. The
[interview record](design/UI-UX-INTERVIEW.md) preserves all answers and corrections;
[GLOSSARY.md](../GLOSSARY.md) defines the shared terminology. Confirmed product
choices below are distinct from proposed engineering defaults and future checks.
No wording here claims these improvements have shipped or closes Phase 3/5.

Read [AGENTS.md](../AGENTS.md), [README](../README.md), [SPEC](SPEC.md),
[discovery](PANELINK-DISCOVERY.md), [PREBUILD](PREBUILD.md), [ROADMAP](../ROADMAP.md),
[current session](progress/SESSION.md) and the
[runtime skill](../.agents/skills/panelink/SKILL.md) before starting.
Existing safety, dependency and phase-order constraints remain in force.

## Final user experience — confirmed requirements

| Area | Required outcome |
| --- | --- |
| History | Wheel scrolling feels fast and smooth. Scrollbar dragging updates the displayed history continuously while held. |
| Reading position | Incoming output leaves the history reading position fixed; show New output / Back to live. |
| Return to live | Clicking Back to live resumes following output. Typing in the focused pane also returns that pane to live and sends the input normally. |
| Focus | Scrolling a pane, dragging its scrollbar and adjusting dividers preserve typing focus. |
| Focus appearance | A stronger focused border/header and explicit marker distinguish the pane receiving keyboard input; other panes keep normal text colors. |
| Dividers | Resizing is easy to target and predictable; retain a keyboard/menu alternative. |
| Copy | Preserve the existing satisfactory text-selection behavior; visibly indicate successful copying. |
| Pane menus | Menu and right-click expose the same selected actions: split, resize, copy/history and close; no new zoom controls. |
| Input methods | Every everyday action has visible mouse controls and equivalent keyboard access. Confirmation never requires typing a command. |
| Tabs | Visible create, switch, rename and close; a single top row, overflow navigation and searchable All tabs. Keep active tab and + reachable. |
| New tab | + creates one ordinary shell inside Panelink and switches to it. It receives the same management, history, controls and lifetime treatment as other panes. |
| Split | Successful split focuses the new shell pane. Failed/canceled split preserves previous focus. |
| Tab close | Button confirmation explains that closing ends that tab's processes; Close tab and Cancel are both clickable. |
| Last tab | Show an empty workspace with New tab and Detach. Never automatically spawn a replacement shell. |
| Layout | Project → workspace tabs → split panes; maximum four panes per workspace. Top tabs and compact pane headers, no permanent sidebar. |

[Conceptual screen and flow](design/pane-ux-concept.html) illustrate the direction.
They are explanatory HTML, not an application screenshot, cell-accurate prototype
or host acceptance evidence. Validate actual terminal cells in UX-01.

## What exists and what still needs work

Source and repository evidence were inspected; no live panes, native Terminal
interaction, Cargo tests or installed binary changes occurred in this planning
session. Installed behavior must be reproduced against its own client/server
identity. A documented button does not establish that the user's current build
makes it reachable or easy to use.

| Existing source baseline | Required work |
| --- | --- |
| One-cell pane margins, Menu/x, version badge | Stronger focus treatment and consistently reachable controls |
| Separate pane-actions and right-click split menus | Shared action availability, target capture and presentation |
| Normal-history reads and extent cache | Continuous thumb-drag reads, responsiveness diagnosis, stable history under output |
| Divider local preview and commit | Reliable hit-testing, clear preview/cancel feedback and retained focus |
| Drag-release copy, keyboard copy | Visible success/failure feedback without changing selection semantics |
| Opt-in project/tab strip and chooser | In-client create/rename/close, overflow, managed-shell creation, empty state |
| `WorkspaceIndex::rename_tab` pure model | Wire through validated persistent mutation and in-client text field; not currently a rename CLI command |
| Workspace stop/remove CLI and lifecycle guards | Safe confirmed tab-close transaction and last-tab/legacy compatibility |
| Existing Swap and zoom | Preserve compatibility; no new whole-pane drag, swap redesign or zoom work |

Codebase-memory starting points, then read exact current source:

- `src/ui_client/state.rs`: `App::handle_event`, `ActionsMenu`, `PaneMode`, history
  results, pointer captures, focus and route state; `state_tests.rs` for behavior.
- `src/ui_client/`: inspect current effect adapters and queueing before assigning
  files; separate scroll-read scheduling from nonreplayable input effects.
- `src/terminal/compose.rs`: `pane_controls`, `scrollbar_geometry`,
  `Compositor::render_with_chrome`, damage and successful-write commit.
- `src/pane_layout.rs`: actual padded geometry and feasible split/resize bounds.
- `src/split_menu.rs`: `MenuState`, menu layout and input capture.
- `src/workspace_tabs.rs`: header, route and chooser presentation.
- `src/workspace_index.rs`, `src/workspace_index/`: `rename_tab`, `remove_tab`,
  model/codec/store, bounded registry and revision-safe persistence.
- `src/workspace_cli.rs`, `src/default_open.rs`: existing create/select/open,
  stop/remove, legacy Default/Main and lifecycle locking.
- Existing fixtures: `tests/p5_workspace_tabs.rs`, workspace routing and pane
  operation/private outer-PTY tests. Discover matching current test names first.

References: [pane layout](PANE-LAYOUT.md),
[pointer controls](progress/P5-POINTER-CONTROLS-2026-10-27.md),
[interactive usage](../.agents/skills/panelink/references/interactive.md),
[tab integration](progress/P5-TAB-INTEGRATION-2026-10-25.md),
[routing evidence](progress/P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md).

## Original design and visual rules

Herdr's official documentation describes mouse-accessible panes, menus, divider
resizing and selection, with keyboard equivalents. Those principles informed the
research; its source, assets, exact styling, hierarchy, keymap and wording are
not a template to copy. Official pages were read on 2026-10-27:
[quick start](https://herdr.dev/docs/quick-start/),
[keyboard](https://herdr.dev/docs/keyboard/),
[configuration](https://herdr.dev/docs/configuration/).
The [older study](HERDR-UI-STUDY.md) is historical documentation evidence, not
hands-on or screenshot validation.

Panelink's direction is a quiet terminal canvas with explicit actions:

- Use existing margins; do not add a toolbar row inside every pane. The tab strip
  may use its existing global row. Child terminal appearance stays intact.
- Header priority: Menu/close hitboxes, focus marker, safe concise pane label,
  optional context. Omit optional context and truncate labels before controls
  overlap. Safe generated pane labels are not stable automation IDs.
- Focus changes chrome contrast and a marker, not child text brightness. Use
  semantic colors with light/dark/reduced-color and ASCII fallback checks.
- Show concise History, Applying, Disconnected and Outcome unknown states. No
  provider completion inference, animated decoration or recurring shell status commands.
- Proposed copy indicator: short-lived Copied label in the pane margin after
  confirmed success. Show Copy failed/Clipboard unavailable truthfully. Never
  include copied content in the indicator or logs. Exact duration is a prototype
  detail; it must not cover child content or block input.
- Preserve client version/build inspection. Changing badge placement requires
  updating its current specification and usage references.
- Terminal content minimum is 5×20, padded pane minimum 7×22, divider one cell.
  An equal split requires 45 outer columns or 15 outer rows. Derive availability
  from layout code, not duplicated menu arithmetic or historical unpadded examples.
- Root/render bound remains 80×240. Test a larger host without exceeding it.
  A viewport too small for the layout preserves processes and reports that limit.

## Scrolling and divider contracts

### Smooth history

Treat responsiveness as the first engineering priority, not a cosmetic label.
Record a before baseline for wheel, trackpad-translated wheel and thumb dragging
under idle and sustained synthetic output. Separate event-to-local-feedback,
read round trip, result application and frame paint. Inspect queue ordering,
background polls, extent discovery and rendering before choosing a repair.
No single cause or numeric improvement is established by this interview.

Wheel input targets history under the pointer and preserves keyboard focus.
Thumb dragging changes visible history continuously while held. Keep the last
valid screen visible while a read is pending. Preserve the pointer's grab offset
and exact target; no jump when grabbing the middle of the thumb.

Proposed scheduling: one active bounded history read and one replaceable latest
intent per relevant view. Coalesce fast motion and cancel/ignore obsolete replies
using target, generation, route, history epoch and request ordering. A response
for old geometry or an older requested position must never repaint the latest
view. Keep reads finite; do not enqueue every motion. Tune bounds from evidence.
This applies to read-only history, never to terminal input or destructive mutations.

Extent can be unknown or a conservative lower bound. Do not label it an exact
total or issue an oldest-row probe before every wheel read. Extent discovery
must not make thumb interaction stall unnecessarily. Preserve the distinction
between normal history and alternate-screen application mouse scrolling.

When new output arrives during history reading, hold the reading position using
stable retained-row identity where available. Show New output without forcing a
jump or claiming an unread count that cannot be proven. If retention evicts the
anchor, show History truncated and clamp to the oldest surviving position.
Back to live resumes following; typing does so for the focused pane only and
is delivered once through existing ownership and no-replay rules. Scrolling
another pane cannot redirect that input.

### Divider resizing

Render and hit-test the same divider geometry, distinct from scrollbar and child
content. A clearly visible affordance may use chrome cells but cannot invisibly
steal child content. Press captures target, revision, divider and geometry.
Motion previews locally and clamps to legal subtree sizes. Release applies one
absolute resize; unchanged press/release applies zero. Do not resize every PTY
on every pointer event. Keyboard/menu controls use the same constraints.

Esc, focus loss, host resize, route/layout change, target replacement and relevant
buffer invalidation cancel capture and paired resize mode. A later Enter cannot
commit a canceled value. Scrolling and resizing retain typing focus. No whole-pane
movement gesture is added.

## Menus, selection and pane close

Menu, right-click and Ctrl-B s share captured target/revision, availability,
action labels and reasons. Four split directions stay visible; unavailable actions
explain size, capacity or server compatibility. Proposed ordering: Split → Resize
→ Copy mode / Back to live → separator → Close pane. Keep existing direct keys.
No new zoom entry or drag-to-move feature. Existing backend shortcuts are not
removed merely because they are outside this redesign.

Overlay painting and hit-testing share a single page/geometry result. Opening
press/release cannot activate an item. Click outside dismisses and is consumed;
it cannot also affect content behind it. Esc/Back/Cancel remain available.
All actions and disabled reasons remain reachable at small sizes using paging
or a full-content overlay. Never require hover or sub-cell pointer precision.

Explicit pane-content click selects typing focus. Opening an explicit pane menu
may select that pane as a proposed prototype default; scrolling/divider/scrollbar
interactions do not. Modal events, paste, repeats and releases cannot leak into
child input. Layout-changing actions use the captured full target and revision,
not subsequently changed focus. Successful split focuses its returned new shell
only after committed success and only if the initiating route remains active.

Preserve wrap-aware drag-release and keyboard selection. Freeze the source;
new output must not alter copied text. Existing cancellation on route, buffer,
geometry or modal change stays. Successful user-gesture copy triggers feedback;
clipboard denial/unavailability is not success and never authorizes programmatic IO.

Pane Close shows its captured pane, Ends this process, and clickable Close/Cancel;
the last pane explains its workspace consequence. Proposed safe initial selection
is Cancel; preserve explicit keyboard confirmation without accepting opening-key
repeats. No keyboard action is required for the mouse flow. No stale confirmation
may close a replacement process. Help and Detach have visible global routes;
Detach explicitly keeps processes running.

## Tab interaction and lifecycle contract

Use a single top tab row: current project, tabs, +, overflow/All tabs controls and
workspace actions as space permits. Keep active-tab indication and + reachable;
use a compact chooser when labels cannot fit. Scroll controls and All tabs
navigate without starting/stopping work. Search filters names with bounded text,
clear empty results and mouse/keyboard selection. Tab drag-reordering is not in scope.

Create: + admits one creation intent, opens one managed shell tab, then switches
to that exact route once ready. Use existing explicit launch environment and
configured working directory; never infer cwd from terminal text. Proposed names
are unique Shell N within the project, editable through Rename. At the current
model's eight-tab limit, explain capacity; increasing that limit is separate work.
New tabs do not spawn agents automatically. Show Opening tab while pending and
reconcile uncertain launch results without duplicate creation.

Switch: retain existing process/history/ownership separation between tab servers.
Remember each tab's focused pane and viewport when valid; cancel in-progress
selections/gestures during route change. Do not forward buffered input to a newly
selected tab. A stale async response cannot steal focus or apply to another route.

Rename: open a mouse-reachable text field with Save/Cancel and keyboard editing;
validate the existing name bounds. Rename changes display metadata, never process,
route IDs, cwd or ownership. Persist through current store revision checks; on
conflict preserve edits and report it rather than silently overwriting a peer.

Close: capture the exact tab route and expected registry/runtime identity. Explain
that its processes will end, with Close tab/Cancel buttons. Cancel changes nothing.
After confirmation, stop only that route under existing lifecycle guards. Verify
termination before treating it as closed; partial/unknown stop stays visible with
truthful status. Removing a tab from presentation cannot stand in for stopping it.
Keep history/layout files under existing retention; close does not clear history.
When other tabs remain, choose a deterministic adjacent tab as a proposed default.
When none remain, show New tab and Detach, with no replacement process created.

**Required compatibility design:** source currently rejects removing the last tab
of an active project (`ActiveProjectCannotBeEmpty`); CLI removal protects reserved
Default/Main (1/1), while plain panelink still targets it. Stop, remove and hide are
different effects. UX-05 must design and review empty/closed presentation state,
registry persistence and reopen behavior before wiring Close. Preserve the reserved
legacy route and prevent unregistered runtimes. A stopped compatibility record may
remain stored while absent from the open-tab UI; do not falsely call it running or
restart it merely by rendering the header/empty screen. Choose a versioned,
bounded representation and migration only after reviewing the store/codec.
Never call remove blindly or weaken process/namespace validation to make the UI work.

The intended final UI includes top tabs for ordinary Panelink startup, not only a
special opt-in command. UX-05 must explicitly reconcile legacy headerless attach
and raw/probe paths, preserving CLI targeting and binary/server capability behavior.
An unsupported server gets an honest limitation, not an automatic stop/restart.

## Effects, safety and failure feedback

| Result | Presentation and permitted follow-up |
| --- | --- |
| Known success | Commit authoritative returned state; clear pending status |
| Known rejection | Preserve prior valid view; explain concise reason |
| Stale target/layout/route | Cancel preview/intent, refresh, require fresh explicit action |
| Old server | Mark unavailable; no automatic destructive compatibility repair |
| Retained owner | Explain blocked input; no takeover or implicit release |
| Partial or unknown effect | Keep uncertainty visible; reconcile existing operation only, never fresh-key replay |
| Disconnect | Mark cached view stale/disconnected and disable mutations until reconciled |
| Registry write conflict | Preserve names/intent; reload and reconcile against revision |

Server owns PTYs and authority; UI owns local previews. Short input batch locks do
not protect unfinished human drafts. No optimistic success, forced owner release,
input replay, provider readiness guarantee or restored-conversation claim.
Keep secrets, pasted text, selected text and sensitive environments out of metadata
and diagnostics. Automated tests use private roots and verified process cleanup;
never probe live user panes for this redesign without explicit task scope.

## Implementation packets and dependencies

Eight packets replace the original sequence. Work blockers-first. Exclusive file
ownership is mandatory for delegated work; parent owns architecture, integration
and verification. Current repository routing allows Luna for suitable bounded
work. Do not assign concurrent writers in state.rs/state_tests.rs, compose.rs,
split_menu.rs or workspace routing files. No new dependency is assumed.

### UX-01 — Reproduce and freeze evidence baselines

Blockers: none. Owner: parent for baseline design; bounded evidence helper allowed.
Inputs: this plan, current source/help, executable and server identity.
Deliverables: private reproducible wheel/drag/close journeys, synthetic cell captures,
client/server fingerprints, event/read/paint timing breakdown, and identified queue
and hit-test bottlenecks. Include focused/unfocused history, nested dividers and
small/large hosts. Diagnose the reported keyboard-only confirmation against the
selected build rather than assuming source support resolves it.
Acceptance: exact commands/geometry/results and cleanup recorded; no borrowed
historical test totals, claimed speedup or disturbed live workspace. Record an
engineering baseline and proposed responsiveness budgets before UX-02.
Likely ownership: new focused fixture/evidence files and read-only source analysis.

### UX-02 — Smooth wheel and continuous scrollbar history

Blockers: UX-01. Likely ownership: client history state/effect scheduling, matching
state tests and one private outer-PTY history fixture; compositor only if needed.
Implement bounded latest-intent scheduling, continuous thumb view, preserved
reading anchors, stale-result rejection, truthful extent and New output/Back to live.
Acceptance: faster-motion latest position wins; no obsolete reply regression,
blank flashing, unbounded queue or eager repeated extent probe. New output and
retention truncation behave explicitly. Input and alternate-screen behavior remain
separate. Measure before/after under identical idle and output loads.

### UX-03 — Focus chrome and shared pointer geometry

Blockers: UX-01, UX-02. Likely ownership: compose.rs, client pointer/focus state
and their tests. Build shared visible/hit geometry for pane controls, thumb and
dividers; strengthen focused header/border plus marker. Scrolling/adjustment keeps
typing focus. Keep inactive child colors and satisfactory selection behavior.
Acceptance: exact boundary hits, pointer-to-content coordinates, narrow names,
Unicode and ASCII fallbacks; no PTY content-size regression or focus theft.
Preserve damage/commit semantics: only successful host writes advance baseline.

### UX-04 — Divider and consistent menu journeys

Blockers: UX-03. Likely ownership: state.rs/state_tests.rs, split_menu.rs and
shared geometry helpers. Use one bounded action/availability model for pane Menu,
right-click and keyboard. Implement reliable divider capture/preview/apply/cancel.
Acceptance: unchanged gesture zero effects, changed release one absolute resize;
all invalidations cancel capture plus mode; outside click consumed; every item,
disabled reason and Cancel reachable in short viewports; no modal input leakage.
No new whole-pane drag, zoom control or speculative large state refactor.

### UX-05 — Tab lifecycle, persistence and empty-state support

Blockers: UX-01; implementation begins after UX-04 if sharing client files.
Likely ownership: workspace_index model/codec/store, workspace_cli/default_open
lifecycle adapter, route state and tests. Parent designs the shared interfaces.
Specify create/rename/stop/close outcomes, reserved route compatibility, closed/open
presentation and final-tab empty state. Reuse existing rename model and lifecycle
locking; do not shell out to invented commands. Review any versioned persistence
change/migration, bounded size, crash recovery and lock order before adoption.
Acceptance: one create effect per intent; CAS-safe rename; close verifies exact
runtime stopped; partial/unknown stops stay visible; last tab can show empty state
without spawn or illegal remove; legacy Default/Main stays registered; no history
loss. Ordinary startup reaches the tab UX without silently changing CLI targets.

### UX-06 — Complete mouse and keyboard tab controls

Blockers: UX-05 and UX-03; serialize client ownership after UX-04.
Likely ownership: workspace_tabs.rs, client route/header state, focused tests and
private tab integration fixtures. Add +, select, Rename Save/Cancel, Close tab /
Cancel, overflow previous/next, searchable All tabs, and New tab/Detach empty state.
Acceptance: managed shell lifecycle and focused new route; every control reachable
with both input methods; active tab/+ never lost on overflow; stale tab selection
and async replies cannot affect replacement routes; no buffered input leak.
Reuse bounded current text-editing contracts; keep eight-tab capacity unless
separately reviewed. No sidebar or drag-reordering.

### UX-07 — Copy, pane close and failure feedback

Blockers: UX-04, UX-06. Likely ownership: client status/confirmation/clipboard
feedback and compositor/client tests. Preserve text selection and show Copied only
after success. Finish mouse-only pane/tab confirmations, consequence text, safe
cancel defaults and pending/rejected/disconnected/unknown states.
Acceptance: no clipboard contents logged, no success after denial, no destructive
opening-event repeat/click-through, no stale occupant close, no uncertainty hidden
by transient success feedback. Help/Detach remain visibly reachable.

### UX-08 — Integrated verification and durable handoff

Blockers: UX-02 through UX-07. Parent owns final review and integrated checks.
Run the matrix below plus current canonical non-UI verification in [CI.md](CI.md).
Fix failures; preserve ignored/blocked evidence honestly. Update SPEC, discovery,
pane layout, README, ROADMAP, SESSION and the repository Panelink skill/references
in the same implementation task. Validate the skill and Markdown links.
Deliver before/after measurements, synthetic captures, exact build identities,
commands/results, cleanup, limitations, and next task. Never substitute private
PTY tests for Apple Terminal, real clipboard, touch or physical reboot evidence.
Phase completion/commit rules remain; no push authorized by this planning request.

## Acceptance matrix — future checks, not results

| Journey / boundary | Required observation |
| --- | --- |
| Wheel/thumb, idle and continuous output | Timing breakdown; fast latest position, bounded work, no obsolete repaint |
| Output while reading history | Same retained anchor; New output visible; eviction explicitly clamped/labeled |
| Scroll B while typing focus is A | Focus marker/input remain A; B history moves; Back to live targets its own pane |
| Typing while focused pane is in history | That pane returns live, input delivered once; no input replay |
| Divider drag | Accurate preview; zero unchanged/canceled effects; one valid release mutation |
| Host focus/resize, layout/route change, buffer/occupant replacement | Capture and mode canceled; no delayed Enter commit |
| Menu/right-click/keyboard | Same actions, reasons, captured targets and safe cancellation |
| Copy during output; clipboard failure | Stable wrapped text; Copied only on success; failure without text disclosure |
| New tab and split | One managed shell per accepted intent; correct committed focus and lifetime |
| Rename conflict and bad/long names | Bounds, Save/Cancel, no silent overwrite, stable route identity |
| Close tab/pane, active/inactive/last | Button-only confirmation; exact process termination; no accidental input |
| Last tab, legacy Default/Main | Empty state, zero automatic spawn, safe retained compatibility metadata |
| Overflow/max capacity | One row; active/+ accessible; All tabs search; capacity explained |
| Rapid double click/repeat, pending create/close | No duplicate process/effect, no click-through or stale confirmation |
| Disconnect, old server, uncertain delivery/stop | Honest state, reconciliation, no takeover or fresh-key replay |
| 7×22 outer minimum, below minimum, 80×24, 120×40, 64×269 host | Safe bounded geometry, short menus, readable valid layouts, no forced process stop |
| Light/dark/reduced color and Unicode/wide cells | Clear focus and warnings, no inactive-content dimming, aligned controls |
| Detach/reattach, rename/closed-state reload | Lifetime preserved, metadata truthful, no terminal side-effect replay |
| Existing explicit Swap/zoom/CLI paths | Compatibility retained without adding redesign features or changing legacy targets |

Use parsed VT cells, exact effect counts, bounded deadlines and readiness markers.
Screenshots alone cannot prove authority, targeting or process termination.
Avoid arbitrary sleeps, raw ANSI matching and tests tied only to helper structure.
Track private child identity and confirm teardown before fixture deletion.

Proposed engineering evaluation: instrument p50/p95 input-to-feedback, history
round-trip and paint separately; use identical before/after scripted motion and
sustained output, record duration/sample counts/host/binary/load. Investigate a
p95 local response target of 100 ms, but validate and publish agreed measurable
budgets in UX-01 rather than calling this a user-selected release threshold.
No timing, RSS or throughput improvement is established by this plan. Human-host
compatibility, real clipboard, reboot/provider recovery and representative workload
budgets remain separate gates. Automate permitted checks and record blocks;
do not hand the user an obligatory manual acceptance checklist.

## Scope boundaries and reversible delivery

Retain four panes, current renderer limits, independently implemented Rust logic,
server-owned PTYs, explicit launch environments and existing input authority.
No permanent sidebar, native desktop client, pane drag, tab drag reorder, zoom
expansion, new provider SDK, autonomous orchestration or guaranteed recovery.
Existing explicit Swap/zoom compatibility is distinct from redesign deliverables.
Additional history search, word-selection and link-opening behavior are follow-ups.

Keep each packet reviewable/revertible. Presentation rollback must not stop live
processes, erase history or replay input. Tab-state changes need versioned migration
and rollback analysis; do not force old binaries to parse incompatible records.
No ADR has been created merely to restate UI preferences. If UX-05 chooses a
consequential persistence/compatibility trade-off, record why in a scoped ADR.

## Next-agent start

First check whether final shared-understanding confirmation is recorded in the
interview. If not, keep this as a reviewable planning outcome, not approved build
scope. After confirmation and an implementation request, read current ROADMAP and
SESSION, claim UX-01's files and reproduce the chosen binary's behavior privately.
Do not start with cosmetic menus while bypassing scrolling measurements, and do
not start a sidebar. Keep all decisions/evidence durable so no chat history is needed.
