## Chrome spacing step 1: centred pane-close dialog — 2026-10-31

User approved a three-step chrome spacing plan (centred close dialog; wider
header spacing at >=120 columns; blank row under the tab strip) with centred
buttons. Step 1 is done: `confirm_ui_geometry` returns a centred, bordered 9-row box
(<=52 columns, centred `[Close]` `[Cancel]`, key hint) on views of at least 11×44.
Smaller views keep the compact bottom prompt. Hit rectangles share the render
offsets; a new state test covers centring, label/hit agreement and fallback.
`p3_attach_four` now waits for "Close pane" instead of "(y/n)". Checks:
`RUST_TEST_THREADS=1 cargo test --no-fail-fast` all pass; fmt/clippy/build/docs
pass in `verify_non_ui.py`. Its Python step has 13
`test_opencode_bridge_plugin` listener timeouts, reproduced identically on the
unmodified tree (pre-existing, unrelated). Steps 2–3 pending; no install or push.

## Button redesign client installed — 2026-10-31

User explicitly requested installation. Rebuilt the verified dirty checkout and
atomically replaced `~/.local/bin/panelink` with version0.1.8; SHA-256 matches
the built artifact. Previous binary backed up. Running server identity/generation
compared equal before/after; no server restart or pane input. Existing attached
clients load the new UI after Ctrl-X and reopening `panelink`.
See [installation evidence](button-redesign/INSTALL.md). Earlier no-install
entries are historical. Native host/provider and broader phase gates remain open.

## Button redesign source complete — 2026-10-31

All BTN-00..06 source packets implemented in `t3/terminal-button-design-research`
(`t3-baecf477`). Final canonical8/8: Rust1626pass/0fail/20ignored, decoder39,
Python233run/1skip, docs464/0, fingerprints/fmt/strictClippy/build pass. Parent
source review and independent bounded safety review complete; in-bounds drag
and help release findings fixed with regressions. See [integration](button-redesign/BTN06.md)
and [execution state](button-redesign/PROGRESS.md) for exact logs, fixture changes,
source/runtime boundaries and remaining gates. Repository spec/help/usage skill
synchronized. No install, live user panes, server restart, push or commit.
Native host/theme/clipboard/touch/provider and broader phase gates remain open.
The earlier entries below preserve historical status and do not supersede this result.

## Button redesign implementation started

User confirmed the complete plan with "start implementation". Follow the
[execution state](button-redesign/PROGRESS.md): baseline/prototype and shared
control foundation first, then interaction and surface migration. Existing dirty
planning files preserved. No installation, server restart or push authorized.

## Button redesign plan ready for final confirmation — 2026-10-30

In `t3/terminal-button-design-research`, checkout `t3-baecf477`, all ten design
interview questions are answered. The [implementation plan](../design/BUTTON-IMPLEMENTATION-PLAN.md)
contains source pointers, control inventory, seven ordered packets with ownership,
interaction contracts and verification/rollout gates. Source baseline observed:
`fdd8664d3085958430f1fbd91ec066d868c33050`; planning/glossary changes uncommitted.
The user chose filled buttons and action-workflow changes, keeping Close versus
Detach semantics and limiting new capabilities to explicit choices. Final
whole-plan confirmation remains pending before implementation. Links and diff
checks only; no builds/tests/live panes/install/server restart/push. Prior product
and host acceptance gates remain open. Resume by obtaining whole-plan confirmation
then executing BTN-00 against freshly checked source/runtime identities.

## Code-health plan complete — 2026-10-30

All00–08 maintainability packets are implemented, reviewed and locally verified on
refactor/code-health-20261030. Source2b72e81831c9912d12a5cff878b5e34dacaeda03 passes
canonical8/8: root Rust1596/0/20ignored, decoder39/0, Python233run/1skip;
format, strict Clippy, build, component fingerprints and docs pass. Parent checked
the full log and finalized the evidence/checklists. See [execution state](code-health-20261030/PROGRESS.md),
[canonical report](code-health-20261030/INTEGRATION.md) and [navigation](code-health-20261030/NAVIGATION.md).
All leases released. Original checkout/installed runtime untouched; no push/install.
Native host/provider/reboot/real clipboard/performance/hosted CI gates remain open.
Earlier dated entries below are historical progress snapshots.

## Code-health packet05 verified; combined gate next — 2026-10-30

Frame assembly now uses immutable RenderView; modal labels and selection overlays
live with their state owners. Parent review, binary323/0, compositor43/0, private
PTY7/0 and strict checks pass. See [packet05 evidence](code-health-20261030/PACKET05.md)
and [navigation](code-health-20261030/NAVIGATION.md). All implementation packets
are verified;08 fresh combined canonical gate and final evidence remain pending.
No install, live panes or push.

## Code-health packet04 verified — 2026-10-30

CopyState owns frozen selection and clipboard request/result state; Interaction
owns pane modes, captured menus, confirmations and local modal consumption.
Parent completed the transition review after the delegated review stopped incomplete.
Binary323/0, compositor43/0, private PTY13/0 and strict checks pass. See
[packet04 evidence](code-health-20261030/PACKET04.md).05 rendering and combined08
remain; no install, live panes or push.

## Code-health packet03 verified — 2026-10-30

WorkspaceState owns tab catalog, picker/dialog/filter state and request-scoped
metadata/create completion rules. Full binary323/0, workspace tabs5/0, lifecycle10/0,
recovery chooser7/0 and strict all-target Clippy pass. Independent scoped review
has no material findings. See [packet03 evidence](code-health-20261030/PACKET03.md).
Parent continues04 pane interaction ownership;05 and combined08 remain pending.
No install, live pane use or push.

## Code-health packet02 verified — 2026-10-30

Private Viewports now owns pane cache/history/request/queue transitions with
immutable production access; App retains modal/selection/pointer coordination.
Existing state151/0, full binary323/0, protocol3/0 and strict all-target Clippy
pass; independent transition review has no material finding. A further fixture
header/old-modal readiness gap was guarded by restored-cursor barriers; workspace
PTY5/0 and final focused scenario1/0 pass. Original failure is retained, not
relabeled. See [packet02 evidence](code-health-20261030/PACKET02.md). No live install.

## Code-health packets01/07 verified — 2026-10-30

Shared host event dispatch preserves handler/effect order; parent host unit6/0
and workspace PTY5/0 pass. Three embedded multi-session test modules moved to
private children; parent verified exact rustfmt-normalized bodies, byte-identical
production prefix and85/0 scoped tests. Test names remain unchanged.
[Execution state](code-health-20261030/PROGRESS.md) records details and worker
report discrepancies. Packet02 is parent-owned after a no-edit worker handback.

## Code-health packet06 verified — 2026-10-30

Parent replaced the four-pane fixture's300ms/negative-status wait with expected
layout chrome and unique new-target focus checks using shared pane geometry.
Each previous captured screen is explicitly rejected; all three split completions
are observed. Private-PTY integration2/0, strict target Clippy, fmt and diff checks
pass. See [execution state](code-health-20261030/PROGRESS.md). No runtime changes.

## Code-health baseline verified — 2026-10-30

Fresh canonical verifier passes8/8 after the bounded positive-cancel fixture
repair; parent reviewed the independent report and exact marker/PID checks.
The original failing baseline log is preserved. Packet00 is complete; host and
server-test organization lanes are next. See [execution state](code-health-20261030/PROGRESS.md).
Product/native/provider/clipboard/reboot/performance gates remain separate.

## Code-health baseline fixture repair — 2026-10-30

New worktree baseline recorded 7/8 checks: the tab-create/rename/close fixture
could accept its first, still-captured close confirmation as the second one
after clicking Cancel. Parent added fresh child acknowledgment and same-PID
verification before reopening. Focused test passes; affected-target and canonical
verification remain pending. See [execution state](code-health-20261030/PROGRESS.md).
No production source, timeout contract, live runtime or installed binary changed.

## Code-health implementation started — 2026-10-30 (current worktree)

User authorized parent orchestration in the new
`refactor/code-health-20261030` worktree at `a1134ec4bc69e8a97b27ba2b0b61f2e446257440`.
The portable plan is now [durable in this worktree](code-health-20261030/README.md);
[PROGRESS.md](code-health-20261030/PROGRESS.md) records blockers, exclusive leases
and verification. GPT-6-Luna will run the fresh serial canonical baseline first.
No source changes, live pane use, installed-binary changes or push. Existing
product acceptance gates and historical results below remain separate.

## PR #13 CI fixture repair — 2026-10-29

Confirmed clean PR branch/head `t3/compact-modal-layouts` / `7a91f45`;
GitHub returned no review threads, reviews or comments. Hosted run40884497931
failed only cargo_test: `workspace_events_pin_filtered_pane_across_same_generation_replacement`
reported layout_changed, pane_closed and quiet, without output.

The original fixture allowed only 100 ms between output and pane close, while
the observer can wait 200 ms on workspace changes before polling pane events.
Closing removes that pane's cursor on topology reconciliation. A disposable
SIGSTOP/SIGCONT observer-delay experiment reproduced the exact hosted failure;
the unmodified focused test passed, establishing timing dependence rather than
a modal regression. Experiment-only signals were removed from the final tree.

The test now captures observer JSONL in a private file and waits boundedly for
an output event before closing the pane. A single submitted shell loop emits
synthetic output until a private stop-file is created, so observer startup needs
no fixed sleep. ObserverGuard bounds exit and cleans up on panic. The concurrent
observer has its own private context directory: an initial no-startup-sleep run
exposed context_directory_unavailable from shared-directory creation. No product
code, timeout contracts, live panes or installed executable changed.

Focused repaired test passes; strict Clippy for p3_agent_cli, fmt and diff checks
pass. Full serial p3_agent_cli: 8 passed/0 failed/0 ignored in 51.85 seconds.
Documentation check: 412 Markdown files/0 findings.
Local logs: /private/tmp/panelink-pr13-observer-delay-red.log and
/private/tmp/panelink-pr13-agent-cli-fixed.log. Fresh hosted CI remains required.

## PR #13 base-branch conflict resolution — 2026-10-29

Confirmed clean `t3/compact-modal-layouts` at `8a81dcf` matches PR #13.
Merged `origin/main` at `96e2a1c`, following existing merge-commit history.
The sole conflict was concurrent test additions in `src/ui_client/state_tests.rs`;
retained the compact/padded modal test and all incoming alternate-wheel fixtures
and tests. Modal geometry and main's scoped wheel delivery both remain intact.

Isolated CARGO_TARGET_DIR=/private/tmp/panelink-layout-client-target with
RUST_TEST_THREADS=1: canonical scripts/verify_non_ui.py passes all 8 checks
(root Rust, decoder, component fingerprints, fmt, strict all-target Clippy,
build, Python and docs). Python 233 tests/1 skip; docs 412 files/0 findings.
Ignored Rust and host/provider acceptance remain separate; no live install or
server restart performed for this merge. Verification log is local at
/private/tmp/panelink-pr13-merge-verification.log.

## Tab actions padding — 2026-10-29

Added two-column inner padding, blank top/bottom rows and heading separation,
a distinct Tab actions heading and keyboard footer. Uses nine rows when the
viewport fits; retains the four-row compact fallback. Pointer activation excludes
padding and uses the new action row offsets. Client suite 209/0, final focused
modal test 1/0, strict binary Clippy, build, formatting and docs (405/0) passed.
Skill frontmatter and local links checked. Installed the hash-verified debug
client at ~/.local/bin/panelink with a timestamped backup; no running client or
server restarted. Prior compact-modal install was also completed in this session.
No commit or push. Native visual acceptance remains open.

## Compact modal layouts — 2026-10-29

On `t3/compact-modal-layouts`, narrowed and centered tab actions (28 columns),
rename (56), tab close (44), All tabs (64, at most 12 rows), and bottom pane-close
confirmation (48). Shared dialog geometry drives rendering and pointer offsets;
chooser clicks outside its horizontal bounds cannot switch tabs. Existing pane
menu, help, swap and divider overlays already have bounded widths. Updated
README, SPEC and runtime skill. No live executable replaced, panes operated,
commit or push performed; native-host visual acceptance remains open.

Verification: isolated CARGO_TARGET_DIR=/private/tmp/panelink-layout-client-target;
client tests 209 passed/0 failed; strict binary Clippy, fmt and diff checks pass;
Markdown checker 405 files/0 findings. Skill quick_validate could not run because
Python lacks PyYAML; frontmatter and local Markdown links checked separately.

# Implementation session checkpoint — 2026-10-17

## Server0.1.8 activated — 2026-10-29 (current)

User explicitly approved server restart ending all running panes. `panelink stop`
confirmed stopped; `panelink recovery relaunch` returned Applied for the saved two
pane positions with fresh zsh shells. Codex/provider conversations not resumed.
`server-info` now confirms client/server0.1.8, shared dirty fingerprint
`f565cab6c33f3b53`, generation15642535525154316810, mask2047 including
`scoped_mouse_wheel`1024. Two live shells, vacant owners/revision1, verified.
User can run `panelink`, reopen/resume Codex and test actual scrolling. No live
input/provider calls, further source/build changes, commits or pushes. Runtime
capability activation is confirmed; real Codex/native-host acceptance remains open.
See [activation evidence](SCOPED-WHEEL-RESULT-2026-10-29.md).

## Scoped wheel0.1.8 installed, server activation pending — 2026-10-29

Canonical verifier passed8/8: rootRust1595/0/20ignored, decoder39/0,
components/fmt/strictClippy/build pass, Python233run1skip, docs412/0. Explicit
private wheel benchmark delivered20/20 and finalvisible20; Astra bounded source
review found no material findings. [Final evidence](SCOPED-WHEEL-RESULT-2026-10-29.md).

Installed standalone `~/.local/bin/panelink`0.1.8, dirty fingerprint
`f565cab6c33f3b53`, SHA256
`0de732e355163a6f1e8a2b6a0c308389542545d93cc55025d1efcd16e244a254`.
Backup0.1.7: `~/.local/bin/panelink.before-0.1.8-20261029-181325`.
Live server remains0.1.4 generation15642535525154316809, mask511; new scope
requires capability1024. Client update alone does not activate it. Restart ends
current panes, so await explicit user authorization before stopping the server.
No live input, restart, commit or push; Codex/native-host acceptance remains open.

## Scoped wheel source0.1.8 follow-up — 2026-10-29 (current)

User reports installed0.1.7 still slow. Astra identified six sequential RPCs and
six full flushes/report; real ContextFile measurements independently reproduced
~52-54 logical reports/sec for that component alone. Astra specified a single
capability-negotiated scoped operation. Luna delivered codec and partial server/
client packets; parent reviewed/corrected implementation and completed the core
transaction, negotiation, revision-boundary and fake-peer tests directly.
See [implementation result](SCOPED-WHEEL-RESULT-2026-10-29.md) for exact behavior,
checks, measurements, remaining acceptance and source changes.

Current source package0.1.8 (root lock entry only; no dependency change).
Focused codec23/0, handshake15/0, server/classification9/0 and clientscoped5/0.
One-process private receiver: burst20/20 final visible233.4ms, first126.9ms;
historical0.1.6 server/same new client+receiver: burst20/20 final588.9ms. These
are coarse fixtures, not real Codex acceptance or first-latency improvement.
Live read-only discovery recognizes pane2 Codex; exact READ-only mode query shows
alternate screen and mouse reporting both true. No pane input/transcript saved.

Full canonical verifier running `/private/tmp/panelink-scoped-wheel-verification.log`;
independent Astra review pending. All builds isolated in
`/private/tmp/panelink-codex-wheel-target`; installed client0.1.7 and old server0.1.4
unchanged. New server activation is separate because restart terminates panes.
No commit/push; phase host/provider gates remain open.

## Local scroll-fix install 0.1.7 — 2026-10-29

User authorized build/install. Package and root lock entry bumped to0.1.7,
README badge updated. Offline isolated debug build copied atomically to
`~/.local/bin/panelink` (standalone file), SHA256
`807bd881e81f81583b4fb8cc5cf85cfc1096f1cb585baf8953851a57c2f09318`.
Backup: `~/.local/bin/panelink.before-0.1.7-20261029-172159`.
Installed build-info: revision`daf28a63f062`, source fingerprint
`a801d78163935d32`, dirty=true (uncommitted wheel changes).

Post-bump library921/0/4ignored and binary317/0 passed. Opt-in exact-report
wheel fixture passed paced20/20 and burst20/20 (first99.2/92.3ms, coarse20ms
polling). Prior full canonical8/8 remains recorded below; no full rerun for
version-only change. Installed --version/build-info verified and SHA matched.
Server-info confirms the same server generation and old0.1.4 build, untouched.
Detach attached client with Ctrl-X, then run `panelink` to see v0.1.7 and test
scrolling. No server restart, child input, commit or push. Real Codex/native
host acceptance remains open. Usage skill installation guidance updated.

## Codex mouse-wheel repair — 2026-10-29 (current)

Branch `t3/improve-codex-mouse-scroll-speed`, starting at clean `daf28a63f062`.
User requested measurements and cost-effective Luna implementation. Parent
review/integration repaired the initial queue-clear/cancellation defects and
completed deterministic tests. Known alternate-screen wheel gestures now use a
direct typed delivery path; unsent FIFO cap32, one-second independent expiry,
cancel on input/view/target/mode/disconnect/error changes, per-event server
checks and no uncertain replay. Normal-history path stays separate; screen
polls can update during alternate-screen deliveries. No new dependencies or
wire changes. Detailed [plan/evidence](CODEX-WHEEL-2026-10-29.md) and
[original baseline](CODEX-WHEEL-BASELINE-2026-10-29.md).

Parent binary317/0 and strict all-target Clippy pass. Exact-report private PTY
fixture requires paced20/20 and burst20/20, observed first117.3/120.1ms during
broader verification (20ms polling). Initial baseline burst1/20; first-response
latency improvement is NOT established. Canonical verification passes8/8: root Rust1580/0/18ignored, decoder39/0,
component sources/fmt/strict Clippy/build pass, Python233run/1skip, docs407/0.
Opt-in wheel fixture run explicitly. Independent Luna source review found no
material issues. Final evidence is linked above.

All builds use `/private/tmp/panelink-codex-wheel-target`; no live-pane input,
install, restart, commit or push. Installed client0.1.6 remains clean daf28a63f062;
running server0.1.4 remains dirty10d81c24cdc2. Synthetic measurements do not close
real Codex, Apple Terminal or other phase gates.

## UI/UX UX-07 and UX-08 — 2026-10-28 (current, read first)

All eight packets are implemented on `t3/panelink-ui-ux-plan`. After UX-08 the
user asked to commit and to build/install locally (one commit; no push).

Header polish 0.1.6 (user feedback on the 0.1.5 screenshot: cramped, "All+"
not recognisable as a tab button): tab row now has a one-cell margin, two-cell
gaps, and labelled right controls `[<] [>]  [All tabs]  [+ New]` on hosts >= 60
columns (compact `[All]`/`[+]`/`+` below); pane header has a gap between
`[Menu]`, the focus marker and the label, and before `[x]`; the scrollbar column
stays blank until history exists (no full-height `#` column). Checks: lib
921/0/4, bin 309/0, compose 43/0, workspace_tabs 10/0, strict Clippy, fmt,
docs; serial integration p3_attach_four 2/0, p3_scroll 3/0, p3a_attach_host
5/0, p3a_default_open 7/0/1, p3a_runtime 1/0, p5_recovery_chooser 7/0,
p5_workspace_tabs 5/0. Installed the same way as 0.1.5: target/debug SHA-256
bf95c34a7e38d7955ca99c31902e906391af4bd6cca8af18cdf89a5a613e1cdf; previous
binary at `~/.local/bin/panelink-debug.before-0.1.6-20261028-132246`.

Local install 2026-10-28: package bumped to 0.1.5 (client badge `v0.1.5`).
Existing development-launcher install kept: `~/.local/bin/panelink` ->
this checkout's `bin/panelink` -> `target/debug/panelink` (locked/offline debug
build), SHA-256 a20a4f5d1c3ec021e6a8cc93adf3347514f63a7b04399b530a79e2a90c137856;
`panelink --version` reports 0.1.5. Previous 0.1.4 binary saved as
`~/.local/bin/panelink-debug.before-0.1.5-20261028-130854`. After the bump: lib
920/0/4 ignored, bin 309/0. The user's already-running server (pid 23083) and
attached clients keep the old code until the user detaches/reattaches (client)
and restarts the workspace (server); no restart, live-pane input or push done.

- UX-07 (Luna, resumed from the cancelled worker's partial edits in
  /private/tmp/panelink-wt-ux02a `ux/07-feedback`; 3-way merged against
  `ux/snap-after-06`): per-attempt clipboard results; source-pane `Copied` for
  1.5 s only after confirmed success, truthful failure labels, no copied text;
  pane-close confirmation with captured pane, "Ends this process", `[Close]`
  `[Cancel]`, Cancel default, Left/Right/Tab, Enter activates selection, `y`/`n`,
  opening event/repeat safe, stale target cancels; tab close also Cancel default;
  four direct split rows with reasons and paging; both UX-04 review findings
  fixed (disabled-item selection/reason; `[More]` hit column); persistent
  `Applying…` / `Outcome unknown` / `Disconnected` with mutations disabled.
- Parent integration fixes found by the full serial suite: UX-07 had dropped the
  host's confirmed-server-end exit (p3a_default_open 2 failures: "attach did not
  exit") and the partial/unknown tab-close keep-visible guard — both restored
  (`host.rs` server_ended exit; `keep_tab_visible_on_server_end`). Updated
  `p3a_attach_host` keyboard-menu step: Enter on the first row now splits
  directly, so the test checks the four rows and Esc instead.
- Docs: skill (`SKILL.md`, `references/cli.md`, `references/interactive.md`),
  README, SPEC updated (Kimi K2.6 draft, parent-corrected: restored dropped
  version-badge/80x240/input-ownership/extent/`Ctrl-B e` notes; UX-07 section
  describes implemented behaviour; README close rows). check_docs 402 files /0.

UX-08 automated verification (CARGO_TARGET_DIR=/private/tmp/panelink-ux-target,
host load from other sessions present):
- Canonical `RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py` run 1 failed
  cargo_test at `p3a_attach_host` (real regressions above, then fixed); run 2
  failed cargo_test at `p3a_attach_host::reattach_after_server_restart_replaces_dead_contexts`
  (2 context files instead of 1; passed 3/3 alone and 5/5 full-binary reruns —
  load-sensitive, NOT counted as a pass) and python_unittest (13
  `test_opencode_bridge_plugin` accept() timeouts, the same environmental
  failure recorded on the unchanged base; host Python 3.15 vs CI 3.11). Decoder,
  component sources, fmt, strict Clippy, build and docs passed both runs.
- Full locked/offline serial no-fail-fast Rust after the fixes: 1568 passed /
  0 failed / 17 ignored.
- Not done (open gates): Apple Terminal/native host, real clipboard, touch,
  UX-01 before/after timing re-measure with event/read/paint breakdown, any
  install or live-pane use. Python bridge-plugin suite needs a CI (3.11) or
  clean-host rerun. Snapshot branches `ux/snap-after-03`, `ux/snap-after-06` and
  worker branches `ux/03-focus-geometry`, `ux/04-menus`, `ux/06-tabs`,
  `ux/07-feedback` are local scratch (not for merge); worktrees under
  /private/tmp/panelink-wt-* and targets /private/tmp/panelink-ux*-target can be
  deleted once the user is satisfied.

## UI/UX implementation resumed — 2026-10-28

User moved orchestration to a new parent session (same checkout, branch
`t3/panelink-ui-ux-plan`), asked to leave everything uncommitted (no checkpoint
commit, no push) and allowed worktrees plus parallel cheap workers (GPT-6-Luna,
Claude Haiku 5.5, Kimi K2.6) via T3 `delegate_task` on the tescode ACP provider.
A live user server runs from this checkout's `target/debug/panelink`, so every
build uses a separate `CARGO_TARGET_DIR` under /private/tmp; no live panes used.

- UX-05c (Luna, main checkout): bounded client metadata worker
  (`RouteController::queue_create|queue_rename|queue_close`), `PreparedWorkspace`
  with exact request-id install, `App::new_workspace_empty` empty state with New
  tab / Detach (no Net worker, no spawn). Parent wired no-argument startup with
  zero visible tabs to the client empty state on a TTY (notice otherwise), and
  fixed a review finding: an unknown create outcome is held and only an explicit
  New tab re-dispatches the same intent ID (no auto-retry, no fresh intent).
  Parent also serialized `tests/p5_workspace_lifecycle.rs` fixtures (process-global
  fake runtime raced under the default parallel runner: 5/10 failed; now 10/0 x3).
- UX-03 (Luna, worktree /private/tmp/panelink-wt-int, branch ux/03-focus-geometry,
  merged by parent into the main working tree via 3-way file merge): shared
  pane-control/scrollbar/divider/content-point geometry in `terminal/compose.rs`;
  focused pane gets bold/cyan header, ASCII `>` marker and contrasting padding
  rail; child cells unchanged; wheel/scrollbar/divider keep typing focus.
- Combined check after both: strict Clippy, fmt, bin 275/0, compose 43/0,
  p5_workspace_tabs 4/0, p3_scroll 3/0, p5_workspace_lifecycle 10/0. One bin run
  had `clipboard::process_fixture_early_exit_is_a_redacted_write_failure` fail
  (passed 3 further full runs and alone) — load-sensitive, not counted as a pass.
- Parallel snapshot branch `ux/snap-after-03` (commit-tree of the working tree;
  not for merge) seeds worktrees: UX-04 in /private/tmp/panelink-wt-ux05a
  (`ux/04-menus`), UX-06 in /private/tmp/panelink-wt-ux05b (`ux/06-tabs`).
  UX-04/UX-07 research brief: /private/tmp/panelink-ux04-07-brief.md.
- UX-04 (Luna, `ux/04-menus`) merged: shared `PaneActionModel` for Menu /
  right-click / Ctrl-B s, divider clamp/cancel/release, split focus fenced by
  route. Parent updated two legacy tests to the new menu order. Review findings
  still OPEN: (M) clicking a disabled item does not persist selection/reason
  (`handle_actions_menu_event` -> `activate_local_action` disabled branch);
  (L) `[More]` hit region starts one column after its label.
- UX-06 (Luna, `ux/06-tabs`) merged: header +, overflow, All tabs search,
  rename/close dialogs, Ctrl-B + / t / w, per-route focus memory, private-root
  create/rename/close journey. Parent fixed review findings: inactive-tab close no
  longer sends `generation: None` (must open the tab first so the exact runtime
  generation is bound — deviation from "close inactive tab" in one step), and a
  late rename success only closes the matching saving rename dialog.
- Checks after UX-04+UX-06+fixes: strict Clippy, fmt, bin 297/0, workspace_tabs
  lib 9/0, p5_workspace_tabs 5/0 (serial; parallel 5/0 x2), lifecycle 10/0,
  tab_state 7/0, p3_scroll 3/0, p3_attach_four 2/0. Second snapshot branch
  `ux/snap-after-06` (before the UX-06 review fixes) seeds UX-07.
- RESUMED and finished 2026-10-28 (see "UX-07 and UX-08" below).
- (historical) PAUSED by user 2026-10-28 during UX-07. The UX-07 worker (worktree
  /private/tmp/panelink-wt-ux02a, branch `ux/07-feedback`, target
  /private/tmp/panelink-ux07-target) was cancelled mid-task; its partial,
  unverified edits (split_menu, clipboard, host, keys, state) remain uncommitted
  there. Resume: finish or restart UX-07 from those edits, 3-way merge into the
  main tree against `ux/snap-after-06`, fix the two open UX-04 findings, then
  UX-08 (full serial verifier, docs/skill/ROADMAP).

## UI/UX implementation — 2026-10-27

User confirmed the shared understanding and requested implementation with parent
orchestration using GPT-6-Luna and Claude Haiku 5.5 subagents (launcher:
`~/.opencode/bin/opencode run -m tesco-litellm/<model>`). Work proceeds in
UX-01..UX-08 order on `t3/panelink-ui-ux-plan`. The `tescode workflow custom
run` path failed with a LiteLLM virtual-key 401, so (per user choice) workers ran
via plain `opencode run -m tesco-litellm/gpt-6-luna` (Haiku 5.5 only for one
read-only audit; it hit a step-limit prefill error once). Writers used separate
git worktrees under /private/tmp with exclusive files; the parent reviewed,
fixed and merged them on local branch `ux/integration`
(/private/tmp/panelink-wt-int). User then asked not to start new agents; the
remaining packets are not started.

Status (2026-10-27, evening):
- UX-01 done as a baseline: `tests/ux01_history_latency.rs` (ignored benchmark)
  and [UX-01 baseline](UX-01-BASELINE-2026-10-27.md). Debug synthetic private PTY:
  wheel first change p95 about 20-36 ms; thumb seeks only on release; wheel and
  thumb on another pane stole focus. No event/read/paint breakdown yet.
- UX-02 implemented (02a protocol + 02b client), not yet accepted: optional
  capability 512 `history_cursor` carries the vt100 normal-history cursor on
  screen/history replies; the client keeps the reading position by cursor delta,
  shows History/New output/History truncated with a pane-local `[Back to live]`,
  never moves focus on wheel/scrollbar, drags continuously (one read in flight,
  latest wins) and discovers unknown extent at the current offset. Parent fixed
  two worker defects: the server's outgoing snapshot guard rejected cursor
  trailers (every UI reply dropped -> "connection lost"), and a thumb release
  before extent discovery was silently lost (now deferred and applied once).
  `p5_workspace_tabs` pointer step now waits for real oldest content.
- UX-05 a/b implemented, client half (05c) pending: approved
  [lifecycle design](../design/UX-05-TAB-LIFECYCLE.md); 32-byte
  `workspace-tabs.v1` sidecar hides only reserved (1,1); adapter
  create/rename/close with verified stop before metadata change; new CLI
  `workspace tab rename|close`; bare `panelink` opens the selected visible tab
  with the tab strip, or prints a notice and starts nothing when none remain.
- Not started: UX-03, UX-04, UX-05c, UX-06, UX-07, UX-08.

Integration verification (ux/integration, target /private/tmp/panelink-ux02b-target):
focused client 165/0, lib 912/0/4 ignored, p5_workspace_tabs 4/0, lifecycle 10/0,
strict Clippy, fmt, build, decoder, fingerprints and docs pass. Canonical verifier
run 1 failed: lib `shutdown_reports_kill_escalation_only_when_hup_was_ignored`
(passes 3/3 alone; load-sensitive) and 13 Python `test_opencode_bridge_plugin`
timeouts that also fail 13/14 on the unchanged base checkout (environmental).
`p3_detach interrupt_delivers_sigint_after_a_ready_marker` fails intermittently
on the unchanged base too (1 of 6).

The first full no-fail-fast run also exposed a real UX-05b regression: plain
startup created `lifecycle.lock`/registry files before the recovery chooser, so
quitting it was no longer side-effect free (`p5_recovery_chooser`). Parent fix
(commit 318b202 on ux/integration): route choice is read-only (bounded lock wait,
first run -> Default/Main); v1/sidecar initialization moved after the chooser
under the legacy lifecycle lock. p3a_default_open 7/0 (3 runs),
p5_recovery_chooser 7/0 (3 runs), lifecycle 10/0, routing 6/0, tabs 4/0.

Final full no-fail-fast Rust on ux/integration: 1507 passed / 8 failed / 17
ignored. All 8 are PTY HUP/shutdown-timing tests (kill escalation, p3_detach
SIGINT, p3b_close x2, layout_lifecycle drop, live_history_driver drop,
screen_checkpoint HUP, w2_server_shutdown); each passes when rerun alone. The
unchanged base on the same host in the same session: 1471 / 9 failed / 16
ignored with the same family plus one routing test, so these are treated as
host-load/environment failures, not regressions. They are not counted as passes.
The integrated changes are now staged (squash, uncommitted) in this checkout;
untracked: UX-01 test/evidence and the UX-05 design. Main-checkout recheck:
strict Clippy, fmt, client 164/0.
No commit to `t3/panelink-ui-ux-plan`, no install, no live pane use, no push.
Next: UX-05c (client empty state + tab metadata worker), then UX-03/04/06/07/08.

## PR 7 CI detach capture repair — 2026-10-27/28

User requested checkout and repair on `fix/wait-resize-full-screen` in
`/Users/sahillamba/panelink`, based on head `1c14aa8`. Hosted run40541338517
failed `p3a_default_open` at the `mouse disabled` assertion. The detach helper
now waits within its existing bound for the independently drained outer-PTY
capture to contain the restore bytes after attach exits. Exit-code and termios
checks remain. A temporary 300 ms delay before capturing those bytes reproduced
the original assertion failure; the corrected helper passed with the same delay.
The temporary delay was removed. Application behavior is unchanged.

The first local canonical verifier passed seven checks but stopped Rust testing
at `multi_startup::tests::opt_in_bridge_environment_reaches_only_the_started_child`:
shell redirection had created an empty file before printf wrote it. That test
now waits for the expected contents within its existing two-second bound.
The next run exposed another incomplete-output observation in the
agent-detection snapshot fixture: the prompt text could be parsed before the
final cursor move. The fixture now waits for child exit and PTY EOF before
copying its snapshot. Default-open tests pass7/0/1ignored after the detach fix.
Final canonical verifier passes8/8: root Rust1481/0/16ignored, decoder39/0,
Python233run/1skip, fmt, strict all-target Clippy, build, source fingerprints
and docs398/0. Agent-detection11/0 and bridge-environment1/0 also pass focused
checks. Full log: `/private/tmp/panelink-pr7-verification-complete.log`.
Builds use `/private/tmp/panelink-pr7-target` and Python3.11; no live binary
activation, commit, push, hosted rerun or phase completion.

## Wait reliability fixes — 2026-10-27 (current)

User asked for waits that detect a long agent task finishing without endless
polling. A live marker wait on a tescode/OpenCode pane exposed two client bugs;
see [P3-AGENT-CLI.md](P3-AGENT-CLI.md#wait-reliability-fixes--2026-10-27).
`wait` no longer ends on geometry-only layout changes of its pinned target, and
`wait --text` searches the whole visible screen instead of the top 24 rows.
Client-only change; the running server needs no restart.

Live: with installed fingerprint d993350e8c254fa7 a visible marker on row 28 and
footer text both timed out and resize waits returned stale_target. After the
rebuild (fingerprint 418efd9a5f476fc6, dirty) row-28 and row-37 text matched at
once against the same running server. `~/.local/bin/panelink` resolves to the
debug launcher, so `cargo build` replaced the PATH CLI. A live host resize during
a wait was not repeated; the new integration test covers it.

Checks: new regression test fails on old code for each bug and passes; fmt,
strict Clippy, build, docs and component checks pass. Full Rust suite with
--no-fail-fast: 1478 passed, 2 failed (`control_unavailable` handshake during
pane close/pane run, the known intermittent issue); both binaries passed 3/3
reruns, as did an intermittent `p3_inflight_client` failure. Python unittest
has 13 `test_opencode_bridge_plugin` socket-accept errors that reproduce on the
unmodified baseline with local Python 3.15 (CI pins 3.11). Tescode-launched
OpenCode stays unrecognized (PTY-proxy wrapper); recognition is a separate
proposal. No commit or push.
## Pane UI/UX plan — 2026-10-27

User requested a detailed plan and then invoked grill-with-docs. All six rounds
are answered; the [interview](../design/UI-UX-INTERVIEW.md) records decisions and
clarifications. [UI-UX-PLAN.md](../UI-UX-PLAN.md) is consolidated into eight packets:
baseline, smooth history, focus/geometry, dividers/menus, tab lifecycle, tab UI,
feedback and integrated verification. Final shared-understanding confirmation
remains pending; this planning task requests no application implementation.
Confirmed: four panes, full tabs, managed new shells, stable/continuous history,
focus-preserving gestures, button confirmations, copy feedback, no sidebar/pane
drag/zoom expansion. Last-tab empty state and reserved-route compatibility are
explicit future lifecycle work, not current capability. GLOSSARY.md added.
Planning baseline HEAD b97e2f7 in t3-320eb3c2. Current source and official Herdr
docs inspected; bounded Luna read-only design review incorporated. No application
changes, Cargo tests, live pane actions, installation, commit or push. Documentation
validation is recorded in the task response. Existing phase gates remain open.

## CI accept-deadline repair — 2026-10-27 (isolated checkout)

Run40511639904 failed `p3_pane_run` during a pane-list handshake. An unchanged
local repetition reproduced the handshake failure class. A controlled transport
regression proves that accept-slice expiry could drop an already-dequeued peer;
a one-slot pending stream now preserves it for the next live slice while retaining
timeouts, kernel authentication and bounded admission. Attribution of the hosted
failure to this race remains inferred pending a hosted rerun. See
[reproduction, repair and verification](CI-ACCEPT-DEADLINE-2026-10-27.md).

Work is isolated at `/private/tmp/panelink-ci-40511639904` on
`codex/ci-pane-run-handshake`, based on failed head `b97e2f7`. The original root
checkout and live workspace are unchanged. Canonical verifier passes 8/8:
root Rust1480/0/16ignored, decoder39/0, Python233run/1skip, supporting checks
and docs393/0. Final-source transport40/0/2ignored; independent source review
found no material issue. The final deterministic regression is old-fail/new-pass.
No commit, push, activation or phase-completion claim.

## v0.1.4 live activation — 2026-10-27 (current)

User explicitly approved restarting and ending current pane processes. Installed
panelink stop exited0 and ended the two old panes in generation15642535525154316808.
Default open selected a fresh shell at the saved-layout prompt; no saved input
or layout was replayed. New generation15642535525154316809 has one live zsh shell
in /Users/sahillamba/panelink. UI displayed v0.1.4; the temporary verification
attachment was detached with Ctrl-X, preserving the shell/server.

Live server-info reports client and server version0.1.4, revision10d81c24cdc2,
source fingerprint1edf2c210c2de12d and selected capability mask511, including
agent observations and build identity. Agent list succeeds; the fresh plain
shell correctly remains unrecognized. Input ownership is none while attached.
Server-info succeeds after detach. No input was sent to the shell and no provider
was launched. Server-side feedback corrections are activated. Run panelink to
reattach. No source change, commit or push in this activation turn.


## Agent feedback corrections — 2026-10-27 (current)

User requested all five prioritized feedback improvements. Version 0.1.4 adds
mode-aware bracketed submission, fixed control/ownership diagnostics, bounded
bottom reads, styled/cursor-checked Codex empty-composer recognition and separate
client/server build identity. See [contracts and evidence](AGENT-FEEDBACK-FIXES-2026-10-27.md).
A private real Codex 0.164.0 local /status reproduction confirms the raw-input
failure and bracketed acceptance with zero tokens; private Panelink end-to-end
acceptance also passes. Broader final verification/install details are recorded
in the linked evidence. Existing user panes/server remain running, with no
input sent or server restart performed. No commit or push.

Final serial checks: library 907/0/4 ignored, client 252/0, agent CLI 7/0,
recognized 2/0, shared 2/0, input PTY 3/0. Strict Clippy, formatting, whitespace,
eight Markdown files and optimized release build pass. Earlier fixture startup
failures and a test-created disk-space failure remain in the linked record;
private package copies were removed and the final probe uses --no-daemon.
One authenticated connection now serves each bounded agent wait. The installed
artifact's real Codex /status probe passes with zero tokens and no daemon.
Installed source fingerprint 1edf2c210c2de12d; SHA256
878f5e9595675cc7bf6426ed5fc1adf5b6fd4a74677c064c01b2669cbac8b070.
Previous 0.1.3 binary retained. Current server returns identity unsupported;
server-side activation still requires explicit permission to restart/end its
current pane processes. Existing panes were preserved.


## Live activation — 2026-10-27

User explicitly approved restarting and ending both existing shells. Installed
0.1.3 `panelink stop` returned workspace stopped; default open started a fresh
shell from /Users/sahillamba/panelink after its saved-layout prompt. Generation
advanced from 15642535525154316807 to 15642535525154316808. The saved layout was
not replayed; current workspace has one fresh shell. The temporary verification
UI displayed v0.1.3 and was detached with Ctrl-X, preserving the server/shell.

Installed agent list succeeds both while attached and after detach. The plain
zsh shell correctly reports recognized:false/kind:unknown; no real provider was
launched. Pane list while attached reports input_owner:none, and bounded pane
read succeeds. No child input or provider/model prompt was sent. Existing-server
activation is complete; real Codex/OpenCode recognition/state acceptance remains
separate. Root main stays clean. Run panelink to reattach the updated client.

## Shared pane input and recognized agents — 2026-10-27 (current)

User selected shared per-operation input and included recognized-agent commands.
The bounded implementation is installed as optimized 0.1.3; see
[contracts, verification and activation boundary](HERDR-WORKFLOW-IMPLEMENTATION.md).
Idle UI clients no longer reserve input. pane submit claims, submits text+Enter
once and releases after confirmed delivery. agent list/get/read/prompt/wait use
native foreground process refs and conservative screen markers; new schema14 /
capability128 requires a matching server. Legacy retained claims remain exclusive.

Final serial verification: library898/0/4ignored, client247/0; agent CLI7/0,
recognized private fixtures2/0, shared two-attachment tests2/0, input PTY2/0.
Pointer outer-PTY passes after known-extent distinction fixes return-to-tail;
three other outer-PTY scenarios passed before this small correction. Clippy,
fmt, whitespace, ten-document links and release build pass. Initial failures,
review correction and coverage limits are preserved in the evidence above.

Current older server has two live panes; read-only inspection returns unsupported
for recognized agents. No user-pane input, detach or restart performed. Reattach
new UI for scoped shared input; restart/new server needed for agent observations.
Restarting existing workspace ends its shells and awaits user approval. Root
main clean and safety stashes retained. No phase-completion, commit or push.

## Scroll responsiveness and Codex socket access — 2026-10-27 (prior checkpoint)

User reported slow scrolling and sandboxed Codex control_unavailable. User then
requested reading AGENT-API-PROPOSAL.md and understanding Herdr before selecting
input-control policy. Handoff design remains unselected: no Allow agents, global
yield, forced takeover, new exact-target command or negotiated composite submit
was implemented. The overbroad UI automation child was cancelled before edits.
[Herdr source study](HERDR-INPUT-CONTROL-2026-10-27.md) records the inspected model.

Local 0.1.2 removes the optional oldest-history range probe from ordinary wheel
reads and suppresses new background poll admission while a scroll is pending.
Normal history's cache-miss critical path uses one read rather than two. Explicit
read-only seeks still discover the extent, matching target and screen revision.
Parent review fixed maximum-offset caching to use the requested offset before
reply-pattern shadowing, and ensured a conservative extent cannot cap future
oldest-history seeks or scrollbar range discovery. Alternate-screen mode checks,
input ownership, operation keys, bounded queues and no-replay rules stay intact.
An already queued/in-flight poll can still delay scrolling; child TUI wheel
processing and the 16 ms render cadence remain. Zero lag or a numeric end-to-end
latency budget is not established. A 15-sample debug CLI read baseline (median
38.98 ms, max44.92 ms) is recorded in
/private/tmp/panelink-scroll-access-debug-baseline.json; it is not wheel latency
and no before/after wheel performance claim is derived from it.

Codex 0.164.0 :workspace with the user's stable temporary context reproduced
control_unavailable. Adding the exact legacy control.sock permitted discovery.
The named panelink profile in ~/.codex/config.toml extends :workspace and grants
writes only to the existing Panelink agent-contexts directory plus that socket;
network_proxy is enabled, with no allowed external domains. Default-profile
sandbox discovery and a bounded pane read pass; a direct unrelated external
connection remains denied. Original config retained at
~/.codex/config.toml.before-panelink-20261027-142414. Existing Codex sessions
must load/select the updated profile through their host; editing defaults does
not mutate a running session's policy. Other routes need their exact endpoints.

Permission-denied socket/directory failures now return control_permission_denied
rather than grouping them as control_unavailable. Unauthorized input/claims
return input_not_authorized with owner inspection/release guidance. Read access
is verified, while writing to attached UI-owned panes remains blocked by the
existing leases until the user chooses the ownership design. No input was sent
to the user's panes and no server was restarted.

Parent verification: binary242/0; compositor37/0; locked offline optimized build,
strict lib/binary Clippy, root cargo fmt and whitespace checks pass. Socketpair
request-order tests cover cache misses/revisions, explicit read-only extent
queries and no background poll between pending/follow-up scroll. State tests
cover conservative range navigation. The final installed release passes sandboxed
list/read; forcing :workspace without the socket produces the new specific
control_permission_denied. A full canonical verifier and fresh Apple Terminal
wheel-to-display acceptance were not run.

Installed ~/.local/bin/panelink now links directly to this worktree's optimized
target/release/panelink; --version is panelink0.1.2. Previous launcher preserved as
~/.local/bin/panelink.before-0.1.2-20261027-143404. Binary SHA-256:
6ece06c1bf2c999a9d795733a0d7f860ed701f7025e98998d00cc01ab2f21845.
A fresh attach loads v0.1.2; existing attached clients remain their original
processes. The debug bin/panelink launcher remains a separate development path.
Skills and docs reflect installed behavior and the unresolved handoff choice.
No commit or push requested.

## Local install and corner version — 2026-10-27 (prior checkpoint)

User selected latest main plus the transferred local changes for installation.
This worktree/branch `codex/main-local-changes-20261027` was fast-forwarded
from bd84625 to origin/main at 10d81c2. The old scrollback code, tests and
skill/research files were already integrated upstream; newer implementations
were preserved. The unique agent API proposal and its documentation links
remain proposed. Original changes remain recoverable in stash
95e4d0856e28ab68ef4e674b00c4e2e16812c07d and the additional pre-install stash;
original main checkout remains clean at bd84625.

Package version is now 0.1.1. The padded compositor draws `v0.1.1` in the
bottom margin of the pane touching the viewport's bottom-right corner.
PTY/content geometry and hitboxes are unchanged; modal overlays take priority
and damage restoration redraws the badge. Unpadded recovery views omit it.
The badge identifies the client package, not the running server or Git SHA.
README, specification and usage skill describe these limits.

Installed via the existing development-launcher workflow: `~/.local/bin/panelink`
now resolves to this worktree's `bin/panelink` and built `target/debug/panelink`.
Previous launcher retained as `~/.local/bin/panelink.before-0.1.1-20261027-133756`.
The broken global skill link was repointed to this worktree's repository copy,
with its previous target retained as a backup symlink. Installed `--version`
reports `panelink 0.1.1`; binary SHA-256 is
5929ea4931b53762d96890801837b1567cac7f6e361c196b1566c993109dbcfd.
Existing servers and attached clients were not restarted; a fresh attach loads
the updated presentation.

Verification: compositor 37 passed/0 failed; client state 92 passed/0 failed;
locked offline build and strict lib/binary Clippy passed. Parsed VT100 cells
cover placement, unchanged frames, split panes, placeholders, modal restoration
and partial-write recovery. The first new-test run had one blank-cell fixture
expectation failure (empty cell text versus spaces); corrected and rerun.
An initial client-test filter matched zero tests; the corrected module filter
ran the 92 cases above. No full product, Apple Terminal or reboot acceptance
is claimed. Bundled skill validator could not import PyYAML; exact frontmatter
was checked with the standard library and affected Markdown links were checked.
Root-package `cargo fmt --check`, whitespace and eight affected Markdown checks
pass. An extra `cargo fmt --all --check` also examined vendored code and
reported existing vendor formatting differences; those sources were unchanged.
No commit or push was requested.


## PR 3 CI socket readiness repair — 2026-10-27 (current)

Hosted run40449868873 at ec09eb0 failed the wide-terminal attach regression
with connection loss; the other seven verifier checks passed. A focused test
with only the verified fixture server at background priority fails with the
old socket sleep loop and passes with descriptor readiness waiting. The
original deadlines, shell identity, controls/prompt and detach assertions stay
intact. Independent review is clear; focused transport39/0/2ignored passes.
Final canonical verifier8/8: rootRust1441/0/15ignored, decoder39/0,
Python233run/1skip, fingerprints/fmt/strictClippy/privatebuild/docs388zero.
The first combined attempt's unrelated CLI connection failure and lint failure
remain documented; no claim of a cause for the intermittent CLI failure is made.
Publishing the repair to PR3 triggers a new hosted run, whose result is pending.
See [diagnosis and evidence](PR3-CI-SOCKET-READINESS-2026-10-27.md).

## Consolidation for PR — 2026-10-27 (current)

User authorized combining main's dirty files with the installed 7060 worktree,
then checkpointing, pushing and opening a PR against main. Main's source changes
were already integrated; unique skill/research/handoff docs are now imported.
The newer 7060 implementation and its open phase gates are preserved. Canonical
combined-tree verification passed8/8: root Rust1439/0/15ignored, decoder39/0,
Python233run/1skip, plus fingerprints/fmt/strictClippy/build/docs387zero.
Imported skill validation, its four Markdown links, agent YAML and report
checks pass. See [the consolidation record](WORKTREE-CONSOLIDATION-2026-10-27.md).
This is a user-authorized checkpoint for PR, not full-phase completion.
The original main checkout and installed binaries remain unchanged.

## Agent usage skill — 2026-10-27

User requested comparison with all Herdr APIs/commands. Added the
[documented-surface comparison](HERDR-API-COMPARISON-2026-10-27.md) and
[per-entry JSON mapping](HERDR-API-COMPARISON-2026-10-27.json), using current
official Herdr docs and the installed 7060 Panelink checkout. Inventory covers
106 CLI paths (including one alias),86socket methods,5terminal-stream input
commands and8cross-cutting flags/selectors; arguments are not separate commands.
Parent checked help/source and a bounded read-only Luna inventory. No Herdr
installation, runtime test or application change is part of this comparison.

User requested solution options for visible-pane control. Added
[the proposed ownership ADR](AGENT-INPUT-CONTROL-OPTIONS.md), comparing per-pane
explicit handoff, workspace automation mode and activity-based handoff.
Recommendation: workspace automation mode, built on a bounded per-pane first
increment. No selection, implementation or new runtime check is claimed.

Control follow-up: user reported working reads but `input_failed` on claims.
Parent traced the installed 7060 client's `Net::poll` claiming every pane and
renewing owners every10seconds for a30-second TTL. The server rejects another
owner with Unauthorized; `owner_error` hides that as generic `input_failed`.
One live claim on the right pane reproduced the error and left its owner
unchanged. No text was sent. Skill troubleshooting now distinguishes this
confirmed UI/automation ownership gap from sandbox access. Application handoff
behavior and specific error reporting remain unfixed; this task changes docs
only and preserves other worktrees' dirty application work.

Follow-up: user reported Codex failing with `context_directory_unavailable`,
then `control_unavailable` after setting an explicit temporary context. Parent
reproduced both errors using the installed Codex `:workspace` sandbox profile.
Keeping the writable context and allowing the exact legacy control socket
made read-only pane discovery succeed (three panes). Normal local discovery
and a bounded right-pane read also succeeded. Added skill connection guidance
covering both permissions, stable contexts, Codex approvals and spatial pane
selection. No pane input or ownership mutation was sent, and no global Codex
configuration was changed. This is connection/discovery evidence, not provider
recovery or full host acceptance. Skill validation passed; six affected Markdown
files passed link/hygiene checks and `git diff --check` passed.

User requested a maintained skill teaching agents when and how to use Panelink.
Added [.agents/skills/panelink/SKILL.md](../../.agents/skills/panelink/SKILL.md)
and CLI workflows, linked from README and required maintenance in AGENTS.md.
The local `~/.codex/skills/panelink` installation links to this repository copy.
PATH currently resolves to the separate 7060 checkout; examples use this main
checkout's source baseline and require executable/state-root discovery before
acting. Source/help review and Markdown checks cover the documentation; this
task does not supply new application/runtime acceptance evidence. Existing
live-scrollback work and its pending verification remain as recorded below.

## Wide-terminal attach crash repair — 2026-10-27

User reported immediate `internal error (terminal restored)` at host size
64x269. Reproduced on an isolated private PTY at widths above240. The legacy
attach renderer used physical host dimensions while its fixed header caches
hold240columns; staging an absent header panicked at compose.rs:674. Both
legacy and tabbed attachment now use the existing bounded screen viewport.
No server restart, live pane input or live workspace mutation was performed.

Regression at64x269 and100x300 fails against the old path with the exact
269-versus240slice panic and passes with the repair, covering initial,
unchanged and resize renders. Client147/0; default-open outer-PTY7/0/1ignored,
including64x269startup/detach/reattach with exact target preservation; strict
all-target Clippy, debug build, fmt and diff checks pass. The first new PTY
test failed because its read-only control context lacked initialization; the
fixture was corrected and the complete target rerun. FullRust was not rerun
for this follow-up. Installed launcher still resolves to7060's rebuilt debug
binary. Native Apple Terminal acceptance awaits the user's retry.

## Pointer and keyboard controls — 2026-10-27 (current)

User selected the installed 7060 worktree for simpler panel controls. Parent
orchestrated Luna renderer and input owners with exclusive files; the input
owner reached its usage limit before verification, and parent completed the
saved edits directly. Independent source reviews covered chrome/backend and
input; parent fixed the reported buffer-switch cancellation defect. No worker
owns files now. Existing scrolling/workload/release changes were preserved.

Delivered: per-pane Menu/x in margins, captured-target actions with short-host
paging, explicit mouse Close/Cancel, divider drag preview and one-shot commit,
menu minus/plus/Apply/Cancel, scrollbar track paging and thumb seek on release,
NAV PageUp/PageDown/Home/End, clickable Help exit and pointer return from copy
mode. Pointer history uses a distinct read-only backend command even if server
modes change. Existing exact-target/revision, modal isolation, minimum-size,
ownership and uncertainty guards remain. No wire or dependency change.

Final locked/offline serial no-fail-fast Rust: 1,437 passed, 0 failed, 15 ignored
across77resultgroups. Parent client146/0, compositor36/0 and workspace outer-PTY4/0
pass; final strict Clippy/fmt/debug build/fingerprints/docs383zero pass. Fixture
corrections and prior failures are tracked in
[the evidence](P5-POINTER-CONTROLS-2026-10-27.md).
The runtime usage skill remains installed from its canonical main repository
copy; its new interactive reference explicitly names this newer build.

The development launcher still resolves to 7060's debug binary. Existing
attached processes/workspaces were not restarted or activated. Reattaching the
rebuilt client loads the UI while server-owned shells continue. Native Apple
Terminal, real clipboard and touch-hardware acceptance remain separate; no
broader phase completion, release artifact update, commit or push is claimed.

## Scrolling usability repair — 2026-10-27 (current)

User reported flashing loading views while scrolling and requested scrollbars.
Parent repairs preserve cached content during history reads, coalesce bounded
normal-history wheel motion, and draw incremental indicators in right padding.
Alternate-screen input remains unqueued; optional extent sizing is best effort.
Independent review's valid-viewport rejection finding is corrected and follow-up
review clear. Library880/0/4ignored, binary221/0, history6/0 and outerPTY3/0 pass.
Final compositor32/0 passes after equivalent lint cleanup; strict all-target
Clippy and formatting pass. Two fresh release builds match; refreshed arm64
`release/panelink-release` hash c0af2dd88dce4c4dc3e14d16ed46a85dbba715aab3218f0dda81eedf1c893576
is independently verified and help smoke passed. Installed/running workspaces
were left alone. No agents or commands remain active.
See [scrolling evidence](P5-SCROLLING-UX-2026-10-27.md). Previous acceptance and
workload changes remain dirty/preserved; no full-phase completion or push.

## Combined workload measured — 2026-10-26 (current)

User selected ten idle OpenCode sessions plus shell/build output, with no
model calls. VariantC now completed two serial60-second release observations
with all10 real TUIs ready, two shell panes and72acknowledgedprobes per run.
Both completed55fixturebuilds during the interval, passed workload correctness
flags, reported no sticky history failure and confirmed owned group cleanup.
The paced shell retained1024rows with an explicit retention gap. Valid empty
normal histories for alternate-screen TUIs are preserved honestly.

PATH OpenCodev2.0.32 needs its verified `--standalone` private-server mode;
default background-service port collisions caused the first four startup
failures. Preliminary run5's positive-row assertion failure remains recorded.
Parent corrected empty-row validation, measured-end counters/output gates,
post-settlement sticky diagnostics and review findings around verified group
identity/cleanup errors. Review's version-probe root leak is also fixed.
Final targeted/all-target strictClippy, release example build and formatting
pass; raw report invariants/resource summaries independently checked. Source
metadata-only shell-kind correction follows the two observations; see limits
in the evidence rather than inventing another full run.

Panelink harness RSS sampled maxima varied60.11/345.19MiB; cause unresolved.
OpenCode echo p95 was133.469/119.883ms, shell ack p95 18.775/19.502ms. Numeric
latency/loss/resource budgets remain unset. Next bounded work is RSS diagnosis
and budget selection, preserving wide-burst final-payload loss and the separate
host/clipboard/physical-reboot gates. No active agents or workload commands
remain. Prior acceptance changes remain dirty/preserved on the continuation
branch; no push or full-phase completion commit.
See [combined workload](P5-COMBINED-WORKLOAD-2026-10-26.md) and
[evaluation plan](P5-WORKLOAD-ACCEPTANCE-PLAN-2026-10-26.md).

## Acceptance continuation — 2026-10-26 (current)

This section supersedes older active-owner and pending-run lists below.
The clean starting point was `1060668`, with the hosted arm64 check passing.
Current branch: `codex/p5-acceptance-continuation`, worktree `7060`.

Added `tests/p5_crash_recovery.rs`: actual private pathname `serve`, committed
one-pane layout and normal/alternate history independently decoded before
exact owned-server SIGKILL, read-only byte-preserving offline inspection,
explicit fresh-generation relaunch, historical mapping and input nonreplay.
Final cleanup captures PID/start identity, confirms exit before removing
files and preserves the fixture on uncertain cleanup. Independent review's
cleanup finding was corrected; follow-up review found no material findings.

Restored `live_history_bench` after pane padding made its old geometry invalid.
Schema3 uses24x80 and24x238 actual PTYs within current outer size limits;
schema1/2 historical240-column reports remain strict and readable. One release
report validates; all48 probes overlap output. Paced capture has no failure;
burst and active shutdown report `final_payload_dropped_bytes` and gaps.
No performance budget pass is claimed.

Canonical verifier8/8 at `/private/tmp/panelink-phase-acceptance-verifier.log`:
root result totals1402pass/0fail/15ignored, decoder39, Python233run/1skip,
fingerprints/fmt/strictClippy/build/docs pass. The root test binaries preceded
the final cleanup-only amendment; supporting checks used that amendment and
the final affected crash target separately passes1/0 in3.46s (`-crash-r4.log`).
Earlier focused orderly/startup cases3/0 and UI proxies5/0 also pass. All Cargo
outputs use `/private/tmp/panelink-phase-acceptance-target`.

Native Terminal binding was explicitly denied by the computer-use tool; no
bypass, clipboard operation or physical reboot occurred. Phase3 human host,
Phase5 native copy/clipboard/physical reboot and representative workload
criteria/combined harness remain open. Existing no-model-call preference
remains in force. No active agents or verification commands remain. Changes
are uncommitted on the continuation branch; no full-phase completion commit,
push or runtime activation. See [continuation evidence](P3-P5-RESUME-2026-10-26.md),
[crash recovery](P5-CRASH-RECOVERY-2026-10-26.md) and
[workload refresh](P5-WORKLOAD-REFRESH-2026-10-26.md).

## Verified pane-operation integration — 2026-10-26 (current)

This section supersedes all active-owner and pending-run lists below. No
agents or verification commands remain active. Bounded pane-location swap,
ancestor-divider preview/absolute commit and server-authoritative zoom/unzoom
are implemented and pass automated integration.

Full locked/offline serial Rust: **1399 passed, 0 failed, 15 ignored**,75 result
groups at `/private/tmp/panelink-p5-pane-full-r1.log`. Strict Clippy then found
lint issues and large worker result variants; parent simplified expressions
and boxed those payloads with existing bounded byte reservations. Final client
**123/0** and outer-terminal **3/0** pass after that cleanup, at
`panelink-p5-pane-client-final.log` and `panelink-p5-pane-ui-pty-final.log`.
The separate actual4PTY protocol continuity fixture passes1/0 at
`panelink-p5-pane-pty-r7.log`. Focused reruns do not invent a second full-suite
pass; the integration note preserves earlier compiler/test/Clippy failures.

All seven supporting checks pass: decoder39/0, component source fingerprints,
rootfmt, strict all-target Clippy, isolated build, Python232run/231pass/1opt-in
skip, and final docs374/0. Logs `/private/tmp/panelink-p5-pane-` with
`decoder-final`, `fingerprints-final`, `fmt-final`, `clippy-r2`, `build-final`,
`python-final`, `docs-final` suffixes. Integration evidence:
P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md.

Independent review found and the parent verified the pane-exit refresh fix
through actual socket-pair Net::refresh_layout requests. Other parent repairs
include last-pane zoom cleanup, effective resize retry, pending-screen
composition, fixture focus/pane-local geometry and same-lease idle resume.
No terminal input or uncertain mutation was replayed.

Phase3 human four-pane Apple Terminal and Phase5 native host/real clipboard,
physical reboot/layout recovery and representative workload gates stay open.
Scoped pane-operation checkbox is complete; the full phases are not. No
phase-completion commit or push. Active target/debug/panelink remains the
previous binary; no installed runtime activation, clipboard operation or
physical reboot occurred. Preserve all dirty source and unrelated .DS_Store.

## Pane operations verification in progress — 2026-10-26 (current)

All workers released source/tests. Outer terminal target passes3/0 at
`/private/tmp/panelink-p5-pane-ui-pty-r2.log`; parent removed two unused fixture
bindings afterward. This includes tab navigation, clipboard-free copy and
actual pane keyboard controls. Client123/0 includes a parent socket-pair
full refresh-path regression after exited-query-target removal. Actual4PTY
protocol fixture1/0 remains separate evidence.

Full source-frozen serial Rust run active:
`/private/tmp/panelink-p5-pane-full-r1.log`. No full result claimed yet.
Supporting checks passed: Python232run/231pass/1opt-in skip, rootfmt and
component fingerprints. Remaining decoder/strictClippy/build/finaldocs run
serially after Cargo completes. Final integration note:
P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md.

Active target/debug/panelink SHA remains
3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e.
Isolated /private/tmp verification binary SHA:
4b6965f17efa08f3f557eedd92fd1c2913f9e549ef92b3e193911513a65e4105.
No current active agents. No clipboard, physical reboot, activation, commit or
push. Phases3/5 remain open for distinct host/clipboard/reboot/workload gates.

## Pane client acceptance and exit-refresh correction — 2026-10-26 (current)

Parent focused client120/0 at `/private/tmp/panelink-p5-pane-client-ui-r3.log`.
Fixed borrowed KeyEvent/fixture borrow compiler issues and a pending-resize
render guard that supplied a wrong-sized snapshot to compositor; regression
covers divider preview and pending zoom with old actual snapshots.
Actual4PTY protocol acceptance passes1/0 in2.84s at
`/private/tmp/panelink-p5-pane-pty-r7.log`. Confirms stable PIDs/state/fullrefs,
stty dimensions, hidden output, root resize, unzoom, dedup/conflict and
unnegotiated denial. Earlier marker-width, key-count, idle socket and helper
compile failures remain recorded. Same-lease resume before final idle-client
checks preserves sequences without replay. Not a native/outer UI pass.

Independent client review found an exit-refresh defect: view validation
required the querying target to remain in the layout, rejecting a legitimate
workspace view after that pane exits and blocking sync away of the stale
connection. Current exclusive Luna High/default owners:
- `resume-20261026-pane-client-transport-r2` completed and parent extended
  with actual socket-pair Net::refresh_layout regression; client123/0.
  Only net.rs and transport note;
  generation/workspace scope validation and network refresh regression.
- `resume-20261026-pane-ui-pty-r2` active after r1 parent result2pass/1fail
  at premature split-focus/whole-row marker assumptions. R2 corrects
  pane-local parsed checks, padded sizes, axis arrows and true-divider detection.
  Only tests/p5_workspace_tabs.rs and new
  P5-PANE-UI-PTY note; real outer-terminal keyboard controls acceptance.
- `resume-20261026-pane-docs-r1` completed and parent reviewed; only README/SPEC/DISCOVERY; documented
  prototype controls and distinct proof/restore/manual gates. Parent clarified
  that1356 full-tree copy evidence predates pane operations; docs372/0.

No worker Cargo/build/tests. Parent full serial Cargo/supporting checks after
all source/test owners release; historical1356 full-tree baseline predates
pane changes. Source offer64 enabled only on multi-session path after reviewed
owner/runtime library871/0/4ignored; no installed binary activation, real
clipboard, reboot, commit or push.

## Live pane integration and client UI — 2026-10-26 (current)

Owner runtime_helper12/0 and full library871/0/4ignored passed after parent
corrections; independent Luna source review found no material owner/runtime
issues. Its gaps were explicit zoom split/close regression cases and live
unnegotiated refusal. Client transport released; parent exported module and
added temporary host result plumbing. Focused client109/0 passes with dead-code
warnings for controls not yet wired. No strict Clippy pass on this slice yet.

Parent enabled cap64 in multi-session offer and shared grant-builder mask
only; single-session offer stays unchanged. Actual4PTY r2 stopped at missing
grant-mask selection (fixed); r3 negotiated successfully but timed out at a
later socket exchange. Backtrace recheck located the oversized post-divider state marker. Parent
shortened markers to fit narrowed panes. r4 progressed past swap/divider then
failed encoding the long hidden-output producer (InvalidKeyCount). Parent
helper now chunks MAX_KEYS with fresh keys per chunk and final Enter only;
rerun pending UI source release. Logs at
`/private/tmp/panelink-p5-pane-pty-{backtrace-r3,r4}.log`.
Logs `panelink-p5-pane-pty-r{2,3}.log` preserve failures; no live pass claimed.

Current exclusive Luna High/default owner `resume-20261026-pane-client-ui-r1`:
state.rs/state_tests.rs/keys.rs/host.rs/worker.rs and new UI note only. Implements
captured swap chooser, local divider preview/absolute commit, authoritative
zoom display plus geometry/input mapping, exact result matching and tests.
No worker Cargo/build/tests. Net/conn/protocol/server/PTY files released.
Parent continues fixture diagnosis in separate files; serialized Cargo resumes
after UI release. No activation, clipboard, reboot, commit or push.

## Pane owner correction and client transport — 2026-10-26 (current)

This section supersedes active owner lists below. No current full-tree pass
is claimed beyond the historical keyboard-copy checkpoint1356/0/15ignored.
Geometry27/0, wire6/0 and actual pathname startup2/0 passed separately.
Parent reviewed completed owner/runtime/PTY source. Initial combined build
failed at nonexhaustive OperationClass runtime match; parent now matches exact
schema13 Request variants. `--lib pane_` passed58/0, but broader
runtime_helper_tests passed9/failed2: failed-resize retry used a single-pane
no-size-change fixture, and unzoom unnecessarily resized already-correct PTYs.
Logs `/private/tmp/panelink-p5-pane-owner-r{1,2}.log` and
`panelink-p5-pane-owner-all-r1.log`. Both failures remain open pending fixes.

Parent r2 run9pass/3fail exposed the outer collapse-loop early return on
last-pane exit plus fixture sequencing and equal-size assumptions. Parent
fixed these directly. All runtime_helper_tests now12/0 at
`/private/tmp/panelink-p5-pane-owner-all-r3.log`; full library passed871/0/4ignored at
`/private/tmp/panelink-p5-pane-lib-r1.log` (17.37s). No live offer yet.
Independent read-only review `resume-20261026-pane-owner-review-r1` active.

Current T3 GPT-6-Luna High/default owners, all no Cargo/build/tests:
- `resume-20261026-pane-owner-r2` completed, parent fixes above: server/multi_session.rs, pane_layout.rs,
  tests/p5_pane_operations.rs and owner note; effective pending versus
  underlying observation, no-op resize, exit cleanup and boundary coverage.
- `resume-20261026-pane-pty-r2`: only new real-PTY fixture and its note;
  correct pinned screen/split scope, PID readiness, move ownership,
  pending metadata comparisons and hidden-output synchronization.
- `resume-20261026-pane-client-transport-r1`: conn/net/worker/worker tests,
  new client pane_operations.rs and transport note; bounded async transport,
  explicit authoritative view and all-target polling. Parent owns module
  export, host result matching and subsequent App/keys/render integration.

Capability64 is still not offered by production owner connect. Parent enables
only after owner/runtime focused checks and then runs real4PTY acceptance.
No push, phase completion, activation, clipboard operation or reboot.

## Verified tabs and keyboard copy; remaining pane operations — 2026-10-25 (current)

This section supersedes status/owner lists below. Current source-frozen tree
passes **1356 Rust/0 failed/15 ignored**, 71 result groups:
`/private/tmp/panelink-p5-copy-full-final.log`. Focused client108/0,
compositor25/0 and clipboard-free actual PTY2/0 also pass. All seven supporting
checks pass: decoder39, fingerprints, fmt, strict all-target Clippy, isolated
build, Python232run/1opt-in skip, docs363/0 after final evidence updates.
Logs `/private/tmp/panelink-p5-copy-{decoder,fingerprints,fmt,clippy,build,python}-final.log`
and `panelink-p5-copy-docs-r1.log`. Final documentation recheck: 363 files, zero findings; the plan date had trailing whitespace, now corrected.

No real clipboard operation occurred. The first PTY fixture stopped at its
initial caret wait; corrected input uses only no-space/no-y redirection and
unanchored Enter while copy mode owns input. Rendered mode-exit barriers
precede all normal shell commands, including after Escape. See the copy
result and PTY evidence for prior failed attempts and parent corrections.

Remaining audit confirms accepted M2 pane move, interactive divider resize and
zoom are still required outcomes; existing split/focus/close/viewport resize
must not be relabeled as those features. Plan-r1 completed. Current exclusive
T3 GPT-6-Luna High/default owners:
- `resume-20261025-pane-geometry-r1` completed; parent focused geometry27/0
  after fixing assertion Debug requirements and split-menu exhaustive mapping.
  Logs `/private/tmp/panelink-p5-pane-geometry-r{1,2}.log`; only `src/pane_layout.rs` and new
  `P5-PANE-GEOMETRY-2026-10-25.md`; pure exact-target swap and absolute
  revision-bound ancestor-divider adjustment. No zoom or snapshot shape change.
- `resume-20261025-pane-contract-audit-r1`: only new
  `P5-PANE-OPERATIONS-CONTRACT-2026-10-25.md`; all source read-only, exact
  protocol/root authority/zoom representation review before server/client work.

- `resume-20261025-history-startup-acceptance-r1` completed; parent actual
  pathname startup2/0 after fixing test Metadata permission accessor.
  Logs `/private/tmp/panelink-p5-history-startup-r{1,2}.log`; only new
  `tests/p5_history_cleanup_startup.rs` and
  `P5-HISTORY-STARTUP-ACCEPTANCE-2026-10-25.md`; actual private pathname
  startup cleanup/capture fixture, existing source read-only.

Contract audit completed and parent reviewed it. Parent refined wire: new
schema13 module, existing status responses plus view query, explicit confirmed
PTY sizes, legacy pending-size meaning unchanged. See contract's parent
refinement, which supersedes its initial reply/size proposal.
- `resume-20261025-pane-wire-r1` completed; parent exported module, corrected
  actual-size content upper bounds and verified wire6/0. Only handshake, new pane-operations codec,
  new wire integration test and evidence; capability64 vocabulary only until
  runtime implements it. Parent owns lib export and dispatch.

Current owner `resume-20261025-pane-owner-r1` exclusively edits
server/multi_session.rs, pane_layout.rs, new tests/p5_pane_operations.rs and
P5-PANE-OPERATIONS-OWNER-2026-10-25.md. Implements exact-ledger owner mutations,
zoom revision/effective PTY resizing and view; runtime capability offer remains
disabled until parent dispatch integration.
Runtime owner `resume-20261025-pane-runtime-r1` completed; parent reviewed
strict classify and repeated capability gating; unit tests await owner release. It exclusively edited
multi_runtime.rs and a new runtime note: schema13 classifier/dispatch and
response validation, capability-gated through the same serialized worker.
It consumes the fixed owner APIs; parent enables the server offer only after
both owners release and integration checks pass. No client source owner yet.
PTY acceptance owner `resume-20261025-pane-pty-r1` exclusively creates
`tests/p5_pane_operations_pty.rs` and its evidence note. Actual 4PTY
identity/size/hidden-output continuity and unnegotiated refusal; no existing
source edits. Requires integrated capability offer before parent execution.

No worker Cargo/build/tests; parent serial verification after source release.
The existing tree already retains proportional first/total cell spans, so
reuse that behavior rather than assuming resize recreates equal halves.
Graph search returned Transport closed; direct-source fallback used.
Audit also recommends a real pathname
startup/temp-cleanup fixture to replace the historical listener blocker.

Phase3/5 remain open for those M2 outcomes and separate human Apple Terminal,
real clipboard, physical reboot and representative workload gates. No phase
completion commit or push. Active binary SHA remains
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`;
isolated verified copy checkpoint binary SHA is
`a63fba9afb409fc9b656ba1398b0e4c5a9adbc694b033463eeb7c0799f311817`.
Preserve existing dirty work and unrelated .DS_Store files.

## Verified workspace-tab checkpoint; keyboard copy active — 2026-10-25 (current)

This section supersedes active-owner/status lists below. Workspace-tab
integration is independently verified: full locked/offline serial Rust
**1344passed/0failed/15ignored**, 71 result groups, at
`/private/tmp/panelink-p5-tabs-full-final.log`. Complete binary target190/0
and immediate two-project/tab private-PTY switching passes3/3 without the
temporary shell-response probe. Registry Locked reads now wait boundedly on
the route worker instead of reporting missing registration; corrupt, unsafe
or missing data still fails closed. Temporary trace hooks are removed.

Supporting checks pass: decoder39/0, fingerprints match, root fmt, strict
all-target Clippy, isolated build, Python232run/231pass/1opt-in skip and
Markdown359/0. Logs `/private/tmp/panelink-p5-tabs-{decoder,fingerprints,python}.log`
and `panelink-p5-tabs-{fmt,clippy,build,docs}-final.log`. Earlier history receipt
timeout, compile failures, immediate-switch failures and probe-assisted passes
remain recorded in the tab integration/contention notes; none were waived.

Current exclusive T3 GPT-6-Luna High/default owner:
`resume-20261025-keyboard-copy-r2` completed ownership of keys.rs, state.rs, selection.rs,
state_tests.rs, terminal/compose.rs and new keyboard-copy result note only.
Implement the reviewed bounded visible-view keyboard-copy plan; no worker
Cargo, dependencies, real clipboard, commits or activation. Parent owns shared
docs/integration and serial verification after release. The1344/0 tab
checkpoint does not cover these in-flight keyboard-copy edits.

R1 source released; parent client compile found a Rect/Option mismatch and
borrowed temporary checkpoint buffers in new tests. Focused compositor run
24pass/1fail at wide-caret continuation style. Logs
`/private/tmp/panelink-p5-copy-{client,compose}-r1.log`. R2 also addresses
copy-mode recovery probing, status overlays hiding the caret and protocol
repeat/release events leaking after copy exit. No copy-mode pass yet.

Parent focused copy-mode checks now **108 client/0 failed** and
**25 compositor/0 failed**, `/private/tmp/panelink-p5-copy-client-r5.log` and
`/private/tmp/panelink-p5-copy-compose-r2.log`. Parent added the missing event
kind import, removed an unused decode, corrected fixture byte order and
anchor-toggle/key-flush setup, and checked the header's actual existing style.
The repeat latch now clears on an explicit new Press, allowing a new gesture.
Current owners: `resume-20261025-copy-pty-r2` owns only
tests/p5_workspace_tabs.rs and new keyboard-copy PTY note (no clipboard write);
`resume-20261025-copy-docs-r1` completed README/SPEC/DISCOVERY; parent reviewed
the documented keys, local-input scope, repeat limitation and open host gates. Parent will run
serial integration/full/supporting checks after both release their files.

PTY r1 parent run failed before its input phase at an incorrect hardware
cursor-based caret wait; workspace navigation passed. Parent additionally
found the proposed touch command unsafe for this test because spaces/y are
copy-mode controls. That phase was never reached, so no clipboard operation
occurred. R2 must scan inverse pane cells, use a no-space/no-y redirection and
wait for rendered mode exit before sending normal shell text (including after
lone Escape). Log `/private/tmp/panelink-p5-copy-pty-r1.log`.

Active target/debug/panelink SHA remains
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
Isolated verified tab binary SHA:
`0be9185913f7647a9c36dfa6cc53a9c1d7bf62e64f59189d0deef2c1503cf3c0`.
Git remains based on054ac3b; no commit/push. Preserve unrelated .DS_Store files.
Phase3/Phase5 remain open: human Apple Terminal/four-pane/clipboard proof,
physical reboot and representative workload gates are separate.

## Verified selection and workspace CLI batch — 2026-10-25 (current)

User resumed after freeing storage. The scoped registry/routing and selection
batch is implemented; Phase3/Phase5 remain open. Read
[P5-RESUMED-INTEGRATION-2026-10-25.md](P5-RESUMED-INTEGRATION-2026-10-25.md)
for scope, prior failures, review corrections and evidence classes.

Parent full locked/offline serial Rust run: **1304passed/0failed/15ignored**,
70 result groups, `/private/tmp/panelink-p5-parent-full-final.log`. After
style-only lint fixes, client74/0, live wrap transport1/0 and focused host9/0
plus routing6/0 pass. All seven supporting checks pass: decoder39, fingerprints,
repository fmt, strict all-target Clippy, isolated build, Python232run/231pass/
1opt-in skip, Markdown348/0. Logs include `panelink-p5-parent-focused-final.log`
and `panelink-p5-parent-supporting-pass.log`. Earlier six-of-seven verifier
runs remain preserved; no failure was counted as a pass.
No real clipboard, physical reboot or user configuration change was performed.

All implementation/review owners from the previous batch are completed.
The header/model foundation owners have also released their files. Parent
focused runs now pass **24 compositor/0 failed** and **8 model/0 failed**:
`/private/tmp/panelink-p5-header-parent-r3.log` and
`/private/tmp/panelink-p5-tab-view-parent-r3.log`. The r2 run exposed swapped
row/column coordinates in two modal fixtures, an allegedly empty project
that still had a tab, and a Unicode fixture exceeding the registry name
limit. Parent corrected those test setups; no runtime behavior changed.
The final switch contract r2 is reviewed: no metadata IO during installation,
nonjoining cancellation/cleanup, and asynchronous selection save after install.

Current exclusive Luna High owners (no worker Cargo):

- `resume-20261025-tab-registry-contention-r1` completed: default_open.rs and
  routes.rs plus new contention note. Parent traced the immediate return
  failure to route validation reporting missing registration while the
  selection metadata worker can hold the exclusive registry lock. Preserve
  StoreError::Locked and retry only that read failure with a bounded off-host
  wait; corruption/unsafe/missing data must still fail closed. Temporary
  navigation/error tracing has been removed from source and fixture.

Post-contention parent results: complete binary target **190/0** and immediate
live workspace switching **3 consecutive passes**, without the shell-response
probe. Logs `panelink-p5-tabs-binary-final-r3.log` and
`panelink-p5-tabs-contention-live-{1,2,3}.log` under `/private/tmp`. Parent
corrected a callback result mismatch and test generic inference errors caught
by compilation; no source-only worker result counted as a pass. Current full
serial run: `/private/tmp/panelink-p5-tabs-full-final.log` (in progress).

Full rerun r2 passed **1342/0/15ignored**, 71 result groups, including the
probe-assisted live fixture. First r1 library timeout remains recorded.
Removing the probe reproduces the live failure (acceptance-r4 and trace repeat
2), so that pass is not immediate-readiness proof. The fixture now requires
alternate-screen mode and no longer has the shell-response probe. Parent
will rerun serial verification after the contention owner releases files.

Parent current results: client **98/0** (`panelink-p5-tabs-client-r4.log`),
live workspace fixture **1/0** with the one-time destination shell response
probe (`panelink-p5-tabs-acceptance-r3.log`), strict all-target Clippy passes
(`panelink-p5-tabs-clippy-r2.log`). First Clippy found four style issues;
parent corrected those without behavior changes. Python232run/1skip and
component fingerprints pass. Current full run r1 stopped at a preexisting
history receipt gate timeout (853pass/1fail/4ignored); isolated recheck passes
without source changes and full r2 is running. Preserve both logs. First
docs check ran during the docs owner edit and found its not-yet-created link
target; rerun after that owner releases. No combined final pass yet.

- `resume-20261025-tab-docs-r1` completed: README/SPEC/DISCOVERY and new tab
  integration note only; describe opt-in behavior and honest evidence limits.
- `resume-20261025-copy-mode-plan-r1` completed: new keyboard-copy plan only;
  read-only source design for the next bounded Phase5 selection slice.

- `resume-20261025-tab-live-debug-r1` completed: host/state/keys/state_tests and
  p5_workspace_tabs.rs, plus new debug note. Real acceptance failed twice at
  post-picker Tools-to-Editors return (fixture line768). Logs
  `/private/tmp/panelink-p5-tabs-acceptance-r{1,2}.log`; r2 has backtrace. No
  source cause established. A one-time Tools shell response marker now gates
  the next shortcut, giving the next parent run a more precise failure point.
- `resume-20261025-tab-metadata-r3` completed: routes.rs and controller note only.
  Fix potential lost catalog wake with rendezvous channel, admit bounded
  save/refresh jobs reliably, and add injected stalled-metadata tests.

Previous owners below have released files. Parent client suite now **93/0**,
`/private/tmp/panelink-p5-tabs-client-r2.log`. First run91pass/2fail: transition
test string mismatch and snapshot fixture passed inner instead of outer size.
Parent corrected those and grouped decoder with compositor/output to retain
the seven-argument event loop. The live acceptance failures remain unresolved.

- `resume-20261025-tab-state-r2` completed: state.rs, state_tests.rs, keys.rs and its
  state note. R1 source complete; parent review requests checked row-zero
  subtraction, parsed-cell rendering tests, viewport-aware picker scrolling,
  prefix cleanup and freeze-safe catalog refresh/recovery/flush seams. Parent
  confirms the added update_workspace_catalog/take_catalog_refresh_request
  APIs and corrected two new test setups (missing pane layout and help still
  open before an intended picker action). Serial validation remains pending.
- `resume-20261025-tab-controller-r2` completed: default_open.rs, host.rs, new routes.rs
  and controller note. R1 complete; parent requests freeze-aware resize polling,
  explicit busy admission across running work, cancellation preservation,
  grouped host route state and deterministic stalled-save/cleanup acceptance.
  Parent owns ui_client.rs exports and the nonblocking worker detach helper.
- `resume-20261025-tab-acceptance-r2` completed: new p5_workspace_tabs.rs and acceptance
  note only. First source is complete; parent review found tab-name-only waits
  could return before switching because both names remain in the strip. R2
  must observe selected vt100 cells, correct a temporary-string borrow and
  align picker evidence/cleanup. Worker reports these corrected; parent
  compile/runtime review remains pending.

Parent added the workspace-aware ui_client module wrapper and routes export,
plus WorkerHandle::try_detach_nonblocking using try_lock and try_send only.
The new worker regression holds the accounting lock and fills its budget to
verify detach admission returns locally without waiting. Scoped formatting
and whitespace checks pass; no Cargo run while source owners are active.

The following foundation assignments are historical and completed:
Parent reviewed the completed `P5-WORKSPACE-TAB-UI-PLAN-2026-10-25.md` and
started these exclusive Luna High tasks (shared checkout, no worker Cargo):

- `resume-20261025-header-compositor-r2`: only `terminal/compose.rs` and new
  header result note. Add fixed bounded `HeaderCell` and optional persistent
  header render API, with old APIs unchanged and commit-after-write semantics.
- `resume-20261025-tab-view-r2`: only new `src/workspace_tabs.rs` plus view
  note. Pure typed route/header geometry/hit/navigation model, no IO/TargetRef.
  Parent added its `lib.rs` export. It consumes the shared HeaderCell API.
- Switch contract r1 completed, code read-only. Parent review remains before
  controller implementation; r2 subsequently resolved nonblocking selection
  persistence and cancellation/adoption semantics.

First header run22pass/1fail: raw-contiguous Project assertion mismatched its
per-cell renderer. Parent also found full-clear header loss and removal/modal
layering issues; r2 corrects them with actual-screen assertions. Model first
compile stopped at shadowed layout function in tests; r2 also corrects a
Unicode width disagreement between scalar-derived Glyph and HeaderCell after
variation selectors. These initial failures preceded the passing r3 component
runs above. Logs are
`/private/tmp/panelink-p5-{header,tab-view}-parent.log`.

Review integration and execute focused checks only after owners release files;
the1304/0 baseline above does not cover in-flight UI integration. Parent
refinement: prepare the destination off the event loop while the old route
stays visible with child input frozen; on preparation failure it remains the
actual active route and reports failure (it was never retired). On success,
restore/cleanup old client state and adopt a fresh destination attachment;
never retarget unsent or uncertain input. No in-client tabs are enabled yet.
The graph connector again returned Transport closed; direct reads are the
documented fallback. No new top-level thread, activation or checkout.

Git remains based on `054ac3b` with the prior and new dirty work retained;
no phase-completion commit or push. The active `target/debug/panelink` SHA
remains `3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
The isolated checked binary SHA is
`d57ef321a8bac05019732b3a793ce92ac3a9429fd50d86652b3fc338dac6ca9f`.
Preserve unrelated newly observed untracked `vendor/.DS_Store` files.

Remaining gates: in-client workspace tab UI/switching, keyboard copy mode,
human Apple Terminal four-pane/drag/clipboard evidence, physical reboot and
representative workload acceptance. Continue independent implementation with
Luna; do not count private-PTY/fake-clipboard/restart tests as those gates.

## Storage recovery and orchestration resumed — 2026-10-25 (current)

User freed storage and authorized continuation. This section supersedes the
active-owner list below; older assignments are historical. The first parent
build failed with ENOSPC in a new temporary target. The three T3 Luna r1
tasks ended with generic `Provider turn failed`; none produced a result or
accepted verification. No source rollback was performed.

The r2 tasks below completed. Current live T3 GPT-6-Luna High owners:

- `resume-20261025-selection-r3`: same client files as r2; correct parent
  review findings: restore before clipboard cleanup (including unwind),
  preserve alternate-screen selection on ordinary revisions, verify import
  and correct binary-target evidence commands.
  Completed; parent corrected a remaining callback mutable-borrow conflict,
  restored the worker-test helper import and removed an unused assignment.
  Fresh binary client74/0; private-PTY four-pane2/0, scroll2/0 and attach5/0
  (including70 attach/detach cycles). Logs `panelink-p5-parent-client-r4.log`
  and `panelink-p5-parent-host.log` under `/private/tmp`.
- `resume-20261025-routing-fix-r2`: main/default_open/workspace_cli and its
  result note only; fix first compile errors, read-only list root validation,
  canonical macOS alias handling and ambiguous stop/remove state guards.
  Completed; parent additionally corrected the missing-namespace remove/open
  race by acquiring a materialized per-route lifecycle lock before metadata
  removal. Server/context symlink validation is also enforced. See result note.
- `resume-20261025-routing-acceptance-r2`: routing integration test plus new
  acceptance note only; actual two-server route/stop isolation and legacy
  preservation, bounded fixture cleanup. No production edits or Cargo.

The first routing implementation completed but its parent compile failed at
Result inference, nonexistent EndpointState::Missing and route_exists tail
iterator lifetime; see `/private/tmp/panelink-p5-parent-components-r3.log`.
That attempt also caught an in-progress host closure-borrow error belonging
to the active selection r3 owner; no integrated acceptance pass is claimed.

Completed r2 ownership, now released:

- `selection`: client state/tests/host/selection/clipboard/module only; finish
  frozen drag and attached-only asynchronous clipboard lifecycle.
- `store`: workspace index/model/codec/store and their tests/evidence only;
  review atomic persistence, stable identities and additive migration.
- `routing-design`: new routing-plan note only; code read-only.

Parent exclusively runs Cargo serially using the existing
`/private/tmp/panelink-p4-bridge-store-target` cache and
`CARGO_HOME=/private/tmp/panelink-p4-cargo-home`. Workers must not build new
target trees. Free space recovered from1.7GiB to13GiB during restart.
Fresh compositor unit checks:19 passed/0 failed. Component integration compile
found the preexisting selection `mouse_button` import and mutable-borrow
errors in `state.rs`; no component acceptance is claimed from that attempt.
Logs: `/private/tmp/panelink-p5-parent-{compose,components}.log`.
Initial parent store unit check failed at the post-rename fault assertion:
`self.fault.take()` at the pre-rename check consumes an `AfterRename` fault
before its intended injection point. Store owner must preserve it until the
matching point. Actual log: `/private/tmp/panelink-p5-parent-store-unit.log`.
Parent fixed the fault consumption after the store worker released its files.
Fresh store unit checks now2pass/0fail, including symlink/hardlink temporary
substitution refusals; log `/private/tmp/panelink-p5-parent-store-unit-r2.log`.
UI reply codec unit checks freshly pass21/0.
Additional parent checks: actual live wrap transport1/0, new negotiation and
exact-target reply-guard regression1/0, Python232run/231pass/1opt-in skip,
component fingerprints pass; Markdown344/0 before new routing note.
Client binary target freshly73pass/0fail (r2 source, before r3 changes), log
`/private/tmp/panelink-p5-parent-client.log`. Component retry r2 stopped at the missing
`mouse_button` import while the client owner was editing; the borrow error
was already corrected. Do not rerun integrated Cargo until owners release
their files. Source changes are uncommitted; no phase-completion claim.
Full library-only parent serial run:841passed/0failed/4ignored in18.79s,
`/private/tmp/panelink-p5-parent-lib.log`. This excludes the binary and
integration targets; source-frozen combined verification still required.
The refreshed code graph later returned `Transport closed`; source-read
fallback is in use. Keep all Phase3 human-host and Phase5 reboot/workload
gates open; no push, activation or real clipboard action.

## Phase 3 and Phase 5 resumed — 2026-10-25 (current)

User explicitly requested completing both phases after Phase4 commit `054ac3b`.
Confirmed hierarchy: **Project → workspace tabs → split panes**. Tabs ARE
workspaces; no nested second tab layer. Discovery/spec record this decision.
Parent orchestrates GPT-6-Luna High with exclusive ownership and independent
review. Phase3/5 remain open; Phase4's scoped completion remains intact.

Completed and independently checked by parent:

- Phase3 private-PTY four-pane/killed-client target:2 passed in11.39s. The
  sole explicit remaining Phase3 gate is human Apple Terminal four-pane proof.
- Selection model:13 passed, including parent-requested correction preserving
  spaces across soft wraps, stale-view rejection, frozen source and byte bounds.
- Composite layout/history recovery:1 passed in2.94s. Orderly process/store
  restart evidence only, not physical reboot or abrupt death.
- Clipboard worker:7 passed in1.01s using fake/in-memory sinks only. No real
  clipboard accessed. Host lifecycle is not wired; unused-code warnings remain.
- Snapshot-wrap metadata:3 PTY tests passed; checkpoint loader clear/unchanged
  saved-byte regression:3 passed. UI codec:21 passed, strict legacy/negotiated
  extension validation; existing checkpoint format unchanged.

Current active exclusive owners:

- `p35-20261025-wrap-transport-r5`: handshake/server/multi-session/runtime and
  client conn/net/worker/state metadata paths completed, including the legacy
  response-guard fix identified below. Parent reviewed the fix and reran the
  previously failing focused p3a attach case via its existing test executable:
  1 passed in3.52s, including1002 checks; no skip. A Cargo invocation first
  failed at the in-progress store module declaration, not this acceptance.
  Repeat final source-frozen Cargo acceptance after component completion.
- `p35-20261025-host-acceptance-diagnose-r6`: diagnose failed outer-PTY
  p3a_attach_host acceptance; edits only attach fixture tests/new evidence note,
  production readonly. Host-mode r5 completed unit checks host3/keys4, but its
  outer-PTY run had2 failures and was interrupted; focused retry lost connection.
  This change has NOT passed native acceptance. Concurrent model compile errors
  were also reported; separate those from actual fixture/runtime failures.
- `p35-20261025-workspace-codec-r7`: new nested pure registry codec/tests and
  module declaration only completed; parent independently reran6/0. Model9/0
  also independently passed. `p35-20261025-workspace-store-r8` now owns new
  store module/tests and declaration only: private lock/atomic registry saves,
  allocation watermark/revision checks and additive legacy initialization.
- `p35-20261025-selection-compositor-r8`: additive frozen selected-cell
  highlight/render API and tests in compose only, no client/state edits.
- `p35-20261025-selection-client-resume-r10`: now owns state/state_tests/host and
  clipboard lifecycle plus optional private selection helper/module declaration.
  Connect drag begin/extend/release, immutable source, cancellation/highlight and
  attached-only async clipboard service; no production clipboard tests.
  Prior r9 failed with generic provider-turn error after partial App/helper
  edits, without tests/report or host lifecycle wiring. Parent inspected and
  preserved the partial files, then assigned this fresh bounded continuation
  with original brief and remaining work. No rollback or pass claim for r9.

Other prior audit/model/review/composite/design/codec tasks are completed.
Parent reviewed workspace-tab design and selected one existing server/storage/
context domain per tab with additive legacy-default migration; no TargetRef
reinterpretation. Future tab removal requires stopped-workspace validation,
not hiding/orphaning live processes. This is architecture direction, not a
runtime feature already delivered. No project-root discovery/sidebar implied.

Next: review transport and host-mode components, implement client drag state,
Parent identified a concrete transport review issue while transport r5 owns
the file: `multi_runtime::snapshot_matches` still invokes legacy screen/history
decoders, which reject the new marked trailers. This can drop the connection
at the outgoing-reply guard before a client receives its first screen. Fix
the guard using the connection's negotiated capability and add negotiated vs
legacy delivery tests before rerunning host acceptance; do not weaken fixture
assertions. The diagnosis task itself confirmed only pre-marker connection loss.
frozen-cell highlight and copy-on-release lifecycle; then workspace registry/
routing/tab UI slices after model review. Workload budgets and physical reboot
acceptance remain separate gates; never substitute process tests for them.
No push, activation, physical reboot, user configuration or actual clipboard
change has been performed. Phase-completion commit authorization remains
subject to required checks; no incomplete phase completion claim or commit.

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

## CI setup — 2026-10-24

The user selected standard GitHub-hosted runners after confirming the repository
is public. Live GitHub API reads confirmed `sahillamba/panelink` is public,
Actions is enabled, `main` points to `bd84625`, and branch protection is not
enabled. The local checkout started clean with no unmerged index entries;
older merge/index warnings below are historical.

Added PR/main/manual macOS arm64 CI, exact Rust 1.99.1 pin, locked fetches,
separate download/build caches, cold test compilation, canonical serial
verification, failure summaries and 14-day log artifacts. Official Actions
are SHA-pinned. Weekly Dependabot PRs cover Actions and root Cargo without
automatic merging. See [CI.md](../CI.md) for operating details and limits.

Initial local verification passed 7/8 checks. Root Rust stopped with
674 passed/1 failed/4 ignored because
`history_writer::tests::empty_batch_initializes_history_and_same_epoch_retry_only_syncs_begin`
submitted its second batch before the bounded one-slot queue was available
(`SubmitError::Full`, not a timeout). An isolated rerun passed. The fixture
now waits for the first commit before the second submission; both committed
receipts, exact storage-call sequence, drained shutdown and empty saved history
remain asserted. No production logic changed.

Final local verifier: **8/8 checks passed**, root Rust **1,017 passed / 0 failed /
15 ignored**, decoder **39 passed**, Python **77 passed**, strict Clippy, fmt,
build, component fingerprints and docs **236 files / 0 findings**. The final
run used `RUST_TEST_THREADS=1`, `CARGO_TARGET_DIR="$PWD/target/ci"`, and
`RUSTUP_TOOLCHAIN=stable` (the installed stable is exactly rustc 1.99.1) with
approved execution outside the restricted sandbox for real socket/PTY access.
The named 1.99.1 toolchain manifest was verified downloadable from Rust's
official distribution server; installing that named toolchain on a fresh
GitHub runner remains part of the first hosted baseline. The final pass does
not erase the initial queue-admission failure or certify the ignored cases.

Workflow validation with checksum-verified actionlint 1.7.12 passed. Summary
rendering was exercised for pass, failure and missing-verifier-output cases;
Dependabot YAML, documentation links and component fingerprints passed.
The bounded documentation task and independent read-only workflow/test review
used a GPT-6-Luna subagent via Codex collaboration (no external launcher).

The user authorized committing and pushing this setup on 2026-10-24.
Publication and first hosted verification follow this checkpoint. Branch
protection and release publication have not been changed. After publication,
confirm the first hosted baseline and require **macOS arm64 verification**
before merging. This CI does not certify ignored host cases, Apple Terminal,
reboot or provider acceptance.

## Combined-tree merge checkpoint — 2026-10-22 (read this first)

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
