# BTN-02b header pressed rendering

2026-10-30; shared checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`, branch `t3/terminal-button-design-research`, HEAD `fdd8664d3085958430f1fbd91ec066d868c33050` plus preserved dirty work. Bounded implementation complete for independent parent review; BTN-02 overall remains open.

Immutable App capture validation feeds RenderView and per-slot PaneChrome.header_pressed (Menu/close), then staged SlotMeta. Canceled captures, revision/route/host/focus-generation changes, missing/replaced targets and modal/gesture ownership suppress pressed presentation. Rendering does not mutate input state or infer operation results. Existing header activation wrapper is unchanged.

The compositor selects semantic Pressed independently of pane focus. Chrome state participates in pending/committed damage comparison; changes repaint complete filled bounds including blank padding without repainting identical child content. An unchanged committed frame emits no bytes. Existing overlay exclusions and restoration apply. Failed/partial writes cannot commit the staged state; retry repaints against the last accepted frame.

New state regression checks actual vt100 background colors for both buttons, all padding, no action before Up, unchanged-frame no-op, drag-away/Escape/Up restoration, revision, replacement target, focus generation, host size, route-switch and direct route identity invalidation. Existing four raw header regressions and process/input assertions remain intact. New compositor regression covers child exclusion, both immediate and partial writer failure, successful retry/commit/no-op, modal covering the button and normal restoration after modal removal.

## Verification

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`. Full stdout/stderr logs:

| Command | Result | Evidence |
| --- | --- | --- |
| `cargo test --locked --offline --bin panelink ui_client::state::tests` | 156 passed, 0 failed, 0 ignored | [state](logs/btn02b-state.log) |
| `cargo test --locked --offline --lib terminal::` | 93 passed, 0 failed, 0 ignored | [terminal](logs/btn02b-terminal.log) |
| `cargo test --locked --offline --test p5_workspace_tabs pointer_controls_preserve_live_ptys_and_confirm_close -- --test-threads=1` | 1 passed, 0 failed, 0 ignored; 14.55s | [pointer](logs/btn02b-pointer.log) |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | passed | [Clippy](logs/btn02b-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn02b-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn02b-diff.log) |

[Parser-decoded rendered Menu cells](cells/btn02b-pressed.txt) show white foreground/blue background on every cell, including both padding spaces. These were produced by the compositor regression, not handwritten render examples; the test also emits the decoded evidence when run with `--nocapture`.

An initial all-target check failed on an incorrect TargetRef import path; corrected to control_protocol before the final gates. [Initial check log](logs/btn02b-check.log) retained. No substantive implementation was reverted and no process/input expectation weakened.

No subagents, install, live panes, restart, commits or push. No modal/tab capture or keyboard redesign. No protocol/server/manifest or other planning documentation changes. Canonical combined verifier, installed-runtime, Apple Terminal, touch, clipboard and provider acceptance were not run or claimed. ms-rust is absent and was not used. Parent retains independent review and next packet admission.
