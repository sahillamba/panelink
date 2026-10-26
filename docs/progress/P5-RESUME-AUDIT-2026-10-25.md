# Phase 5 resume audit — 2026-10-25

## Scope and method

Read `AGENTS.md`, the Phase 5 roadmap section, the top of `docs/progress/SESSION.md`, `README.md`, `docs/SPEC.md` (workspace, interaction, history/recovery, milestone and V-HISTORY/clipboard portions), `docs/PREBUILD.md`, and `docs/PANELINK-DISCOVERY.md`. Checked the working tree before inspection: branch `codex/p3-scrollback-integration`, HEAD `054ac3b`, clean, no unmerged index entries. The codebase-memory project `Users-sahillamba-.codex-worktrees-7060-panelink` was ready; its change detector showed divergence, so its fast index was refreshed with persistence disabled before code-symbol searches/snippets. No build, test, native UI, server, user-setting or Git mutation was run. The only repository write in this audit is this note.

`ms-rust` is still not installed per repository guidance. This audit does not claim Rust guideline compliance.

## Current state

The Phase 5 roadmap has three remaining gates: hierarchy plus keyboard layout operations; Apple Terminal mouse selection/copy/context-menu acceptance; and full V-HISTORY reboot/layout plus representative workload acceptance. Most prerequisite mechanisms already exist.

### Hierarchy and keyboard

The product choice is still pending in `docs/PANELINK-DISCOVERY.md`; this audit makes no choice about sidebar/tabs, multiple workspaces per project, or whether a terminal session is itself a workspace. Current product help accurately says the prototype has at most four panes and no tabs (`README.md`, Quick start / current limits).

The current client implements Ctrl-B navigation mode with directional focus, next-pane selection (Tab/BackTab/`o`), four-way split, menu, close confirmation, help, and saved-layout viewer (`src/ui_client/keys.rs:23-74`; `src/ui_client/state.rs:846-909`). Focus movement is local presentation state (`src/ui_client/state.rs:1028-1078`). Right-click and left-click are also implemented, described below. These are delivered capabilities, not outstanding work.

Still absent from the current UI contract are workspace/tab switching and the remaining M2 layout actions named by the spec: move, resize/divider adjustment, and zoom (`docs/SPEC.md`, M2 at the development-milestone table; keyboard contract in Proposed interaction contract). `map_key` accepts the documented limited key set and Ctrl-B prefix; there is no configurable binding map in this prototype (`src/ui_client/keys.rs:27-74`). Do not add a hierarchy model or bindings until the user answers the pending hierarchy question.

### Mouse, selection, menus

Implemented now: left click focuses; right click opens an original pane-relative menu; the menu owns input and supports keyboard/mouse choice; menu actions include four split directions, close, and saved-layout view; wheel routing supports live normal-history scrolling and inner applications that enable mouse reporting (`src/ui_client/state.rs:750-843,1125-1165`; `src/ui_client/keys.rs:76-110,140-157`; `src/split_menu.rs`; `README.md`, Low-level split panes). This is more than a menu design stub.

The remaining concrete feature gap is drag selection and copy-on-selection. `Event::Mouse` handling in `App::handle_event` handles button-down focus/menu and wheel only; it has no drag/release selection state. There is no local-client system clipboard path for user Copy or Paste in the current attach UI. The contract is specified, including Unicode/cell-width and ongoing-output behavior, but not implemented (`docs/SPEC.md`, Proposed interaction contract and Clipboard authority; `src/ui_client/state.rs:750-843`; `src/ui_client/keys.rs:140-157`). Further context-menu rows in the spec (new shell/OpenCode, rename, zoom, move, copy/paste) are proposals; treat them as required only where the pending product decision/confirmed requirements make them applicable. Do not expand menu scope by assumption.

A separate Apple Terminal check remains necessary for host event delivery and mouse-reporting interaction. Automated state/menu and PTY tests do not establish that the host lets Panelink receive the intended right-click/drag gesture. Phase 3's four-pane human host gate is also separate and remains open (`ROADMAP.md`, blocked-on summary; Phase 3 acceptance notes).

### Saved history and layout

Delivered and documented:

- Bounded normal-buffer rows (1,024 rows / 8 MiB per session), latest normal and alternate visible-screen checkpoints, gap/reset metadata, read-only history list/show/search, clear, end retention and safe orphan-temp cleanup (`docs/progress/P5-SAVED-HISTORY.md`).
- Two-slot historical layout codec/store/writer, current 280-byte format with supported legacy 160-byte migration, lifecycle observation, read-only `layout show`, startup chooser/viewer, and explicit `recovery relaunch` that recreates fresh shells in saved spatial topology and records old-to-new links (`docs/progress/P5-LAYOUT-COMPLETION.md`, `P5-RECOVERY-CHOOSER.md`, `P5-RECOVERY-RELAUNCH.md`, `P5-RECOVERY-LINKS.md`). Historical targets are never live dispatch identities; provider conversations and old processes are not resumed.
- Fresh-process history acceptance already passes: `tests/p5_history_recovery.rs` drives a real PTY, persists a normal row plus alternate screen B after observing A then B, then reads with fresh CLI processes. It proves A is unavailable when not checkpointed, B and retained content are labeled/read, no stored effects/canary are replayed, no workspace starts, and reads do not mutate storage (`docs/progress/P5-RESTART-ACCEPTANCE.md`).
- Explicit fresh-shell topology relaunch already has five passing isolated integration cases for one through four panes, both two-pane orientations, three/four-pane nested spatial order, fresh context and links, refusing invalid/empty/currently-running cases. The user also reported the documented manual relaunch/viewer checks working (`docs/progress/P5-RECOVERY-RELAUNCH.md`).

The remaining V-HISTORY work is not to rebuild these components. The gap is integrated acceptance across both durable streams and the restart/recovery path, with all limits visible. The history test does not exercise saved-layout restoration; the relaunch test does not exercise actual saved scrollback/checkpoint recovery alongside topology. Layout and history are separate files, not an atomic multi-file snapshot (`docs/SPEC.md`, Scoped saved-layout/restart design and Meaning of saved history). Neither test represents a physical Mac reboot or filesystem power-loss test.

### Representative workload

There are two kinds of evidence, neither a representative Phase 5 pass:

- The earlier single-store synthetic benchmark used 24 operations per case and explicitly had no numerical budget (`docs/progress/P5-HISTORY-THROUGHPUT.md`).
- The later actual-store live benchmark has two recorded runs of paced, burst and shutdown two-pane synthetic workloads. It captures control/input timing, rows/checkpoints and retention limits, but does not exercise ten sessions or a human host path. Wide cases hit the configured byte cap and report gaps; burst reported a sticky history failure in both final runs, and active shutdown producers were incomplete. The evidence preserves those outcomes and does not diagnose them as a specific queue defect (`docs/progress/P5-LIVE-HISTORY-BENCH.md`).

The smallest workload follow-up is first to agree on measurable history-loss, capture-lag, error and UI/control-latency budgets, then extend/repeat the release workload to the supported session/workspace shape and representative output rates. Current four-pane/no-tabs UI limits mean a ten-agent idle P6 observation is not a substitute for active-output history workload acceptance. Do not call the existing synthetic observations a budget pass.

## Smallest next slices

1. **Hierarchy decision (user dependency; no code slice yet).** Record the user's answer in the canonical planning documents when authorized by the parent. Then derive the minimal model and keyboard commands from that choice. Until then keep existing pane navigation/split/close behavior and avoid selecting tabs/sidebar/workspace semantics.
2. **Keyboard/layout implementation (after decision).** Add only the missing operations required by the selected hierarchy: workspace/tab activation if chosen, and move/resize/zoom if in the accepted M2 scope. Preserve stable full targets, focus behavior and in-flight operation rules. Acceptance: real attached multi-pane navigation and each accepted operation, including resize/minimum-size, process continuity, and layout persistence observation. Existing split/focus/close are regression coverage, not new implementation tasks.
3. **Selection/copy interaction.** Implement a bounded client-local selection model over rendered cells and history coordinates, with drag start/extend/release, wrapped-line and Unicode cell-width extraction, defined output/resize/buffer-switch behavior, and copy-on-release under the existing clipboard authority contract. Preserve inner-app mouse usability and do not enable programmatic clipboard requests. Acceptance: automated pure selection cases and attached PTY cases for Unicode/wrapping/ongoing output, then Apple Terminal host acceptance for actual gestures, exact clipboard text, failures and mouse-reporting pass-through. Expand context-menu actions only after scope is resolved.
4. **Composite fresh-process V-HISTORY.** Add one isolated acceptance scenario that seeds a multi-pane saved topology and real history (including normal rows, alternate A→B between checkpoints, checkpoint gaps, and at least one old-to-new relaunch link), stops/destroys the server process, then uses fresh CLI/server processes to explicitly inspect/relaunch. Assert topology/order and history sources agree where records are committed; unavailable overwritten A is stated; old historical IDs never dispatch; no provider/process/terminal side effects replay; corrupt/missing source behavior remains independent; storage is unchanged by read-only paths. Run separately for orderly stop and abrupt process death where feasible. This is reproducible process/store restart evidence, not a machine reboot or power-loss claim.
5. **Separate human host/reboot gate.** On the agreed Apple Silicon Mac/Apple Terminal baseline, perform a controlled physical reboot acceptance after the process-level scenario passes: observe startup choice, explicit relaunch, pane topology, history labels/gaps and terminal restoration. Record OS/Terminal versions and raw outcomes. Do not use a human reboot result to replace automated restart/fault coverage, or count a skipped host test as a pass. Existing Phase 3 four-pane host acceptance stays a separate gate.
6. **Representative history workload.** Define numeric budgets before declaring pass; preserve current diagnostics and raw samples. Extend actual-store workload from two panes to the selected hierarchy/session shape (up to the accepted 5–10 agent goal), with paced/burst/active-stop cases and foreground control/input latency. Diagnose or explicitly budget the observed burst failure/gaps and incomplete shutdown workload. Report local storage throughput, capture lag/loss bounds and UI/control impact separately.

## Proposed exclusive file ownership

Parent owns canonical `ROADMAP.md`, `docs/SPEC.md`, `docs/progress/SESSION.md`, final integration, and shared existing UI/runtime modules. After the hierarchy answer, assign bounded owners as follows; workers should not edit the parent-owned files or commit:

- Hierarchy model/keyboard: one owner for a new `src/workspace_navigation.rs` plus `tests/p5_workspace_navigation.rs`; parent integrates changes to `src/ui_client/state.rs`, `src/ui_client/keys.rs`, `src/ui_protocol.rs`, and runtime ownership files only if the selected model requires them.
- Selection/copy: one owner for new `src/terminal_selection.rs` plus `tests/p5_terminal_selection.rs`; parent owns wiring through `src/ui_client/host.rs`, `src/ui_client/state.rs`, clipboard adapter/policy and any dependency decision. Keep clipboard commands and OS interaction out of the selection model.
- Composite restart acceptance: one owner for new `tests/p5_history_reboot.rs` and its private fixture helper under `tests/support/`; no production edits unless the test exposes an integration defect, then return the finding for parent assignment.
- Workload acceptance: one owner for a new release example/report validator and only its new raw report files; parent sets/approves budgets and owns canonical evidence/roadmap integration.

These are proposed task boundaries, not assignments or product decisions.

## Verification status and blockers

This audit ran no build/test/verifier, host interaction, server, physical reboot, benchmark, Git mutation, or native configuration change. The prior scoped results above are evidence from their linked records, not checks rerun at HEAD `054ac3b`. The pending hierarchy answer blocks implementation choices; Apple Terminal gesture and physical reboot acceptance require a human host; representative workload pass criteria require agreed budgets. The `ms-rust` skill remains uninstalled. No Phase 5 completion is claimed.
