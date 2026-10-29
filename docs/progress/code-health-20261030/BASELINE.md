# Ticket 00 baseline evidence

**Status:** baseline captured; parent review required. This is test00 evidence only. No product phase, host/provider, reboot, native-terminal, or performance acceptance is closed.

## Checkout and ownership

- Worktree: `/Users/sahillamba/.t3/worktrees/panelink/refactor-code-health-20261030`
- Branch: `refactor/code-health-20261030`; base/HEAD: `a1134ec4bc69e8a97b27ba2b0b61f2e446257440`.
- Parent-owned dirty docs at start: `ROADMAP.md`, `docs/progress/SESSION.md`, and untracked `docs/progress/code-health-20261030/`. No source files were edited by this worker. Original `/Users/sahillamba/panelink` was not accessed for changes.
- Baseline build target: `/private/tmp/panelink-code-health-20261030-baseline-target`. Full canonical log: `/private/tmp/panelink-code-health-20261030-baseline.log`.
- Graph orientation: fresh fast, non-persistent index of this exact worktree (12,081 nodes, 75,620 edges). The prior status lookup said “project not found or not indexed”; the fresh indexing succeeded.

## Canonical verification

Command, exactly as run:

```sh
RUST_TEST_THREADS=1 CARGO_TARGET_DIR=/private/tmp/panelink-code-health-20261030-baseline-target python3 scripts/verify_non_ui.py > /private/tmp/panelink-code-health-20261030-baseline.log 2>&1
```

Exit code: **1**. Fresh run completed in about 5m50s. `cargo test` failed in `tests/p5_workspace_tabs.rs`: `workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe` panicked at line 548 because visible control `"[Close tab]"` was missing. Captured screen showed the renamed tab row and shell prompt, but no expected close control. Group result: **4 passed, 1 failed, 0 ignored**. Do not start structural edits until parent reviews this baseline failure.

Relevant successful Rust test groups from the log include library **931 passed / 0 failed / 4 ignored** and binary **323 passed / 0 failed / 0 ignored**. Opt-in/private groups include ignored tests for latency, fixture child, pathname socket/device access, and real shell configuration; ignored tests were not run. Individual Cargo integration groups and totals are present in the complete log. Decoder suite: **39 passed**. Python: **Ran 233 tests**, **OK (skipped=1)**. Docs checker: **425 Markdown files, 0 findings**. Component source verification, fmt, Clippy, and build all passed. Summary JSON reported `cargo_test=failed`; decoder, component sources, fmt, Clippy, build, Python, and docs passed; host/provider gates not certified; milestones incomplete; setup valid.

Tool versions: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`, `Python 3.14.6`.

## Source and test inventory

Counts are current checkout physical lines, including inline test modules; separately compiled test-only modules are identified independently. This avoids treating the first `cfg(test)` declaration as the end of production code.

| File | Lines | Layout / anchors |
| --- | ---: | --- |
| `src/ui_client/state.rs` | 7,593 | `App` state and transitions; `apply_scroll_update` around line 2,328, copy/selection around 3,200–3,400, `handle_event` at 3,460, workspace tab operations around 1,400–1,700, scrollbar and selection handling around 5,400–6,050, render at 6,700. Inline test-only helpers/modules occur at multiple points; separately compiled `src/ui_client/state_tests.rs` is 6,080 lines. |
| `src/ui_client/host.rs` | 1,557 | Host run/restore and routing; `run_inner` 355, `restore_then_cleanup` 518, host event/routing helpers 642–833, `event_loop` 834; inline tests begin 1,414. |
| `src/server/multi_session.rs` | 9,934 | Multi-session authority and dispatch, metadata authorization/publication around 423–467; several embedded test modules begin around 3,865, 7,062, 8,828, and 9,933. Separate test children: `async_split.rs`, `close_tests.rs`. |
| `src/multi_runtime.rs` | 5,152 | `dispatch` graph anchor, route/session/watch dispatch; separate `launch_tests.rs` and `metadata_tests.rs`; inline test modules also present. |
| `src/p3_cli.rs` | 6,129 | Parsing at 204–515, control/runtime dispatch from 515 onward, operation/receipt and watch flows, inline tests from about 4,069. |
| `src/terminal/compose.rs` | 4,418 | compositor/frame render path, validation and overlay painting; inline tests from 2,148. |

Graph-confirmed high-level anchors include `App::handle_event` (cognitive complexity 390; graph’s source line estimate is unavailable), `App::apply_scroll_update` (cognitive 83), `multi_runtime::dispatch`, and the compose test/render helpers. Direct source inspection confirmed current coordinates. These counts are inventory, not architecture recommendations.

## Existing regression map for planned packets

- **Scoped wheel negotiation and no replay:** existing state tests cover alternate-screen wheel routing, negotiated operation path and stale reply invalidation; `alternate_screen_wheels_queue_fifo_with_coordinates_and_direction`, `alternate_wheel_capacity_and_individual_expiry_are_bounded`, and `alternate_wheel_queue_cancels_on_input_and_view_changes` cover fresh-only FIFO order, coordinates/direction, capacity 32 and one-second expiry, cancellation, and no replay after uncertain/failure results. Preserve optional capability 1024 behavior and server authorization.
- **Cross-pane scheduling:** UI history/wheel completion tests exercise pending reads, stale epochs, focused/unfocused panes, and retained visible content; preserve application-wide admission gates and per-pane in-flight ordering/priority. Server/multi-session and multi-runtime tests cover ownership, operation admission, exact targets, dispatch and metadata authority; inspect focused cases from existing test names/source before moving them.
- **Metadata identity/routing:** `metadata_result_for_another_route_is_ignored` and workspace create/rename/close completion cases assert request and route matching, reconciliation and partial visibility. Keep metadata identity tied to exact route/request.
- **Selection/copy invalidation:** `selection_drag_is_frozen_local_and_cancelled_by_layout_change`, keyboard copy frozen-view tests, `keyboard_copy_cancels_before_buffer_route_or_modal_identity_changes`, and metadata-wrap invalidation cases cover frozen content, Unicode/wrap identity, one-shot completion, and cancellation on route/buffer/layout/focus changes.
- **Modal defaults and geometry:** `close_confirmation_defaults_to_cancel_and_enter_closes_only_after_selection`, modal opening-event/input isolation cases, compact tab-modal geometry and outside-click rejection, plus compose modal geometry tests protect cancel defaults, event ownership and hit/render agreement.
- **Restoration and output commit:** host tests around restoration and private PTY tests in `p3a_attach_host`/workspace scenarios cover ordered host restoration. Compositor commit tests (`*_rendered_committed`, incremental/full-render comparisons) protect output baselines advancing only after accepted writes. Treat process/host lifecycle as separate from server-owned PTYs.

Focused command map for later packets remains as in `EXECUTION.md`: UI state suite, host suite/private-PTY targets, compose library tests, `p3_attach_four`, and server/multi-session library suite. None were separately run here; the canonical verifier ran its prescribed suite once.

## Post-edit checks and limits

- `git diff --check`: passed (empty output).
- `python3 scripts/check_docs.py`: passed, `Checked 425 Markdown file(s); 0 finding(s).`
- No test reruns, fixes, opt-in tests, installed Panelink invocation, live pane activity, or provider/native/performance acceptance were performed. The verifier failure is preserved as the baseline blocker for parent review.
