# BTN05 source handoff

Shared checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`, branch
`t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus existing parent dirty work.
No subagents, commits, push, install, live user panes or restart. No manifest,
protocol, server, root input, workspace lifecycle or canonical documentation edits.

## Implementation

`terminal/controls.rs::ControlStyle::write` now owns complete SGR emission for
pane headers, strip controls, history buttons and overlay spans. Existing full
color ANSI foreground/background pairs are retained. Reduced mode uses default
foreground/background plus reverse fill across the entire existing target at
rest. Secondary rest is reverse; selected/primary adds bold; destructive adds
underline. Focus adds bold+underline; pressed adds bold+italic+underline;
disabled uses italic; pending uses italic+underline (all retain reverse).
Labels, control positions and hit bounds are unchanged; no added marker can
spill outside padding. The palette's marker values remain available, but no new
marker text is emitted. Existing history pressed marker remains within its bound.

`Compositor::set_reduced_control_color(bool)` defaults false and invalidates
both committed-baseline validity and staged readiness on change. Same-value
calls preserve no-op behavior. Mode changes repaint the full host, including
chrome and active overlay, using existing writer failure/commit rules. No
snapshot mutation or rendering-time environment read was added.

The host reads presence of `NO_COLOR` (including empty value) and `TERM=dumb`
once during setup; route replacements inherit the compositor's mode rather
than rereading environment. Recovery setup also initializes the mode once.
No user configuration, CLI option, theme picker or dependency was introduced.
This is explicit ANSI styling, not native terminal theme detection.

Help's existing actionable title row now receives a shared secondary filled
span across exactly its existing full-row hit region. Remaining informational
help lines stay plain. Existing help text and keyboard escape paths are retained.

## Complete family audit

Paths below are relative to `src/`; symbol names identify exact entrypoints.
Shared compositor emission is `terminal/compose.rs::paint_overlay` for all overlay
spans and `terminal/controls.rs::ControlStyle::write` for all control families.

| Family | Render / geometry | Hit / capture or input | Audit outcome |
| --- | --- | --- | --- |
| Pane Menu / x | `terminal/compose.rs::pane_controls`, `paint_pane_controls`; `ui_client/state/render.rs::RenderView::render` | `ui_client/state.rs::pane_control_at`, `handle_header_event`, `valid_header_press` | Shared filled role/state style; exact padded bounds; preserved press validation |
| Pane action pages, Back/More/Cancel | `ui_client/state/interaction.rs::format_actions`, `actions_page_geometry`; `state/render.rs::render` modal spans | `interaction.rs::modal_controls_inner`, `modal_control_at`, `capture_action`, `handle_actions_menu_event` | Plain group/reason rows, disabled reasons; focus/pressed spans, no title fill |
| Split / saved-layout compatibility menu | `split_menu.rs::MenuState::overlay_lines`; `state/render.rs::render`, shared modal spans | `interaction.rs::modal_controls_inner` kind 4, `capture_action`, `handle_split_menu_event` | Existing actionable rows filled; saved-view action remains separate from relaunch |
| Pane Close confirmation | `interaction.rs::confirm_ui_geometry`; `state/render.rs::render` | `modal_controls_inner` kind 1, `capture_action`, `handle_confirm_event` | Destructive Close, focused Cancel, plain consequence; exact captured target preserved |
| Resize / swap / zoom-related actions | `interaction.rs::format_mode`, `state/render.rs::render` | `modal_controls_inner`, `handle_pane_mode_pointer`, `capture_action`, `handle_pane_mode_event` | Existing adjustment/Apply/Cancel controls use shared spans; statuses remain plain |
| Project, tab select / separate x, New, All, previous/next | `workspace_tabs.rs::layout`, `empty_layout`; `state/render.rs::render`, `compose.rs::paint_header` | `workspace_tabs.rs::TabsLayout::hit_test`; `state.rs::capture_strip_event` | Filled controls, primary selected, pressed/pending styles; gaps remain plain; inactive close guard unchanged |
| Tab actions / Rename / Close dialogs | `state/workspace.rs::format_dialog`, `workspace_controls`, `control_visual`; `state/render.rs::render` | `workspace_control_at`, `activate_control`; `state.rs::capture_workspace_event` | Independent control fills; pending Save/Close and disabled Cancel retained; editor text/status plain |
| Project / All tabs picker rows and Clear | `workspace.rs::picker_overlay_text`, `workspace_controls`, picker geometry; `state/render.rs::render` | `workspace_control_at`, `activate_control`, `capture_workspace_event` | Only actual route/Clear targets filled; filter/title/gaps plain |
| Empty workspace New / Detach | `state.rs::empty_action_geometry`; `state/render.rs::render` | `state.rs::surface_at`, surface capture in `capture_strip_event` | Shared role/state spans; pending New retained; no automatic spawn |
| History Back to live | `compose.rs::pane_history_controls`, `paint_history_controls` | `state.rs::surface_at`, surface capture in `capture_strip_event` | Removed independent hard-coded SGR; normal/pressed styles honor mode; history label/version plain |
| Attach help close row | `state/render.rs::render` help branch | `interaction.rs::handle_help_event` | Filled entire existing hit row; keyboard Esc/? preserved; input exception below |
| Startup saved-layout chooser | `default_open.rs::chooser_prompt`, `run_startup_chooser` | `read_startup_choice` | Preserved line-mode keyboard choices; bracketed key hints are not buttons |
| Offline saved history/checkpoints | `ui_client/recovery.rs::compose`, `run_frames` | `map_key` → `RecoveryUi::handle_local_key` | Preserved key-only viewer; no mouse targets or relaunch controls added |
| Detach/help/saved secondary actions | `interaction.rs::format_actions`, `modal_controls_inner` | `capture_action`, `handle_actions_menu_event` | Existing actions use common span states; Detach semantics unchanged |
| Nonaction chrome/status/copy feedback | `state/render.rs::render`, `compose.rs::paint_pane_label`, `paint_history_controls` | No button hit/capture | Plain status/title/gaps retained outside actual control spans; no fake button fill added |

Scrollbars, dividers and copy selection are existing gestures, not buttons;
their input/render rules remain unchanged. No existing actual control was found
in the line-mode startup chooser or key-only recovery viewer.

## Out-of-scope finding

`src/ui_client/state/interaction.rs:455` `handle_help_event` closes on left
**Down** anywhere in the title row (`:468`), with no release capture. This does
not satisfy the approved release/cancel rule. Fix requires input-owner work;
this task deliberately changes only presentation to match those existing bounds.
Parent should retain this as a real open input gate, not count BTN05 as full
button-design acceptance. No other owner's source was altered to fix it.

## Verification and evidence

All Cargo commands use `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`,
`--locked --offline`. Full stdout/stderr retained under `logs/btn05-*`.

| Command | Result | Log |
| --- | --- | --- |
| `cargo test --bin panelink ui_client::state::tests` | 169 passed, 0 failed | [state](logs/btn05-state.log) |
| `cargo test --lib terminal::` | 97 passed, 0 failed | [terminal](logs/btn05-terminal.log) |
| `cargo test --lib workspace_tabs::` | 11 passed, 0 failed | [tabs](logs/btn05-tabs.log) |
| `cargo test --test p5_workspace_tabs -- --test-threads=1` | 5 passed, 0 failed, 38.99s | [PTY](logs/btn05-p5.log) |
| `cargo test --lib btn05 -- --nocapture` | 2 passed, 0 failed | [cells](logs/btn05-cells.log) |
| `cargo test --lib overlay_controls_fill_exact -- --nocapture` | 1 passed, 0 failed | [overlay](logs/btn05-overlay-cells.log) |
| `cargo build` | passed | [build](logs/btn05-build.log) |
| `cargo clippy --all-targets -- -D warnings` | passed | [Clippy](logs/btn05-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn05-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn05-diff.log) |

The new actual renderer/vt100-parser fixture exercises every role and state
through full→reduced→full transitions for light (`30;47`) and dark (`37;40`)
initial SGR backgrounds. It checks cell background, reverse, bold, italic and
underline across entire spans, with plain gap/status attributes, repeated-mode
and repeated-frame no-op. These are synthetic initial SGR fixtures: they do not
change a host palette's default colors or establish native light/dark acceptance.
The separate chrome fixture checks selected strip, pane Menu and history cells,
pressed transitions, mode changes, failed writer refusal to commit, retry and
no-op. Existing overlay restoration/partial-write regression now runs in both
modes and compares complete restored parser cells against a fresh renderer in
the same mode. It retains style-only repaint, movement, clipping and failed-write
checks. Child snapshot assertions and all existing behavior assertions remain.

Development correction: the new fixture initially reversed the test helper's
(y,x) arguments; corrected to the intended row/column. The expanded restoration
fixture initially compared reduced output with a full-color fresh compositor;
fixed the oracle to use the same mode, strengthening to full cell equality.
Intermediate logs were overwritten; no retained red provenance is claimed.
State/PTY checks were rerun after the final style refinement; all listed checks
cover the settled source.

Graph-first lookup was attempted once and returned project not indexed; used
the task-authorized exact-source fallback, no retry/index loop. Memory search
found no relevant button implementation facts. ms-rust remains unavailable.

## Remaining gates

Parent owns canonical docs/skill/roadmap and the final non-UI verifier after all
source settles. Canonical provider WouldBlock remains **unclassified**. Palette
availability, tested default ANSI mappings, and native host acceptance are
separate: no Apple Terminal/light/dark host, clipboard, touch or provider
acceptance is claimed. Full source build does not activate the installed runtime.
