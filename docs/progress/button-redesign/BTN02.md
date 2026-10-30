# BTN-02 attempt — incomplete

2026-10-30, checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`,
branch `t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050`.

**BTN-02 is not implemented or ready for acceptance.** This attempt produced only
fixture label alignment with the parent's existing filled header foundation.
There is no external blocker established. Remaining implementation is substantial.

## Retained changes

- `src/ui_client/state_tests.rs`: four appearance/label lookups now use ` Menu `.
- `tests/p5_workspace_tabs.rs`: private pointer flow looks up ` Menu ` and ` x `.

All substantive assertions, process survival checks, Cancel checks and readiness
barriers remain intact. No event helpers or application behavior were retained.
No host/server/protocol changes, commits, installation, live user panes, restart,
push, or subagents. Existing dirty foundation/planning work is preserved.

## Rejected experiment

A temporary root capture deferred pane/tab/history chrome activation until release,
validated exact layout/route/catalog revision/host bounds, and kept divider/scrollbar
owners separate. It did not cover modal captures or keyboard/render focus states.
Its real state test run gave **139 passed, 12 failed**. It was reverted completely;
`src/ui_client/state.rs` has no diff. The experiment is not a delivered feature.
Logs retain the result, but not an applicable implementation patch.

The initial filter `ui_client::state_tests` matched **zero** tests, so it provides
no passing evidence. Correct filter is `ui_client::state::tests`.
After reverting the experiment, state tests gave **147 passed, 4 failed**; all
four failed on stale `[Menu]` appearance expectations. The first private pointer
flow also failed waiting for `[Menu]`. These failures were reproduced on the
restored root state in this checkout; no claim is made about earlier clean HEAD.

## Actual verification

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`.

| Command | Actual result | Log |
| --- | --- | --- |
| `cargo test --locked --offline --bin panelink ui_client::state_tests` | 0 tests; not evidence | `logs/btn02-state-initial.log` |
| `cargo test --locked --offline --bin panelink ui_client::state::tests` (experiment) | 139 passed, 12 failed | `logs/btn02-state-first-real.log` |
| same (root restored, old labels) | 147 passed, 4 failed | `logs/btn02-state-restored.log` |
| same (retained label updates) | 151 passed, 0 failed | `logs/btn02-state-labels.log` |
| `cargo test --locked --offline --lib terminal::` | 92 passed, 0 failed | `logs/btn02-terminal-final.log` |
| `cargo test --locked --offline --test p5_workspace_tabs workspace_pointer_controls_preserve_live_ptys_and_confirm_close -- --test-threads=1` (old labels) | 0 passed, 1 failed | `logs/btn02-private-pointer.log` |
| same (updated labels) | 1 passed, 0 failed | `logs/btn02-private-pointer-labels.log` |
| `cargo test --locked --offline --test p5_workspace_tabs workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe -- --test-threads=1` | 1 passed, 0 failed | `logs/btn02-private-tabs.log` |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | pass | `logs/btn02-clippy-final.log` |
| `cargo fmt --check` | pass | `logs/btn02-fmt-final.log` |
| `git diff --check` | pass | `logs/btn02-diff-final.log` |

Private flows above exercise current press semantics, **not release activation**.
They establish fixture compatibility only. No release cancellation regression was
retained, and none is claimed passed. Full canonical gate was not run. Previously
reported provider WouldBlock failures remain unclassified; no assertions were
altered concerning them.

## Next concrete implementation step

Implement a semantic capture identity and shared bounds for every local button,
including modal rows/buttons, with invalidation independent of child selection,
divider and scrollbar capture. Add explicit raw Down/Up regression cases before
changing fixture click helpers. Then route admitted releases through existing
exact target/revision effect paths, wire real pressed/focus presentation, implement
local dialog focus and repeat guards, and migrate private outer-PTY event helpers
to send real release bytes. Re-run the focused state/control/compositor/private
flows, formatting and strict all-target Clippy. Parent review and later canonical
acceptance remain required. Do not advance packet status based on this report.
