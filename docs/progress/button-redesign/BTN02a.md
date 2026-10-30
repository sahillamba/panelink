# BTN-02a bounded header capture — verified

2026-10-30; checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`;
branch `t3/terminal-button-design-research`; HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050`, shared dirty source.

This slice is complete for parent review. It does not complete BTN-02.
The parent's HeaderPress/handle_event wrapper and dispatch_event router remain.
Worker added captured input-focus generation validation and migrated the nine
state fixtures to explicit header Down/Up. No global dispatcher auto-release
was added; gesture and feed remain raw. Modal press activation is unchanged.
Existing appearance updates and all unrelated dirty work were preserved.

## Review and behavior

Down captures the exact TargetRef, Menu/close action, layout revision, workspace
route and host dimensions without invoking effects. Release validates those
values, current control hit and focus generation before dispatching the existing
admission path once. Focus changed away and back invalidates the press. Modal,
route-switch, pointer-seek and pointer-divider ownership exclude header admission.
A mouse event during capture is consumed locally; drag-away or wrong-button
release permanently cancels that capture. Escape cancels locally; focus loss
and other nonmouse events discard capture before ordinary dispatch. Opening
release is consumed by the wrapper and clears the interaction opening latch.
Exact-target comparison rejects replacement at the same geometry/revision.

Two additional raw-event tests cover replacement target, focus loss, focus
changed away/back, wrong-button release, and duplicate release over the close
confirmation. Both parent raw tests remain raw and pass. Fixture migrations
retain confirmation, focus, child-input, action availability and edge assertions.
The fixture which reopens confirmation now explicitly releases both header clicks.

Private PTY helpers already emit real SGR `M` Down and `m` Up; no helper mutation
was necessary. Their readiness barriers, Cancel and process survival assertions
remain intact. Both real private flows pass with header release activation.
No installed runtime, live user pane, server restart, compositor change, other
surface capture, keyboard redesign, commit, push or subagent was used.

## Completed gates

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`.
Logs are full stdout/stderr under `logs/` relative to this report.

| Command | Result | Log |
| --- | --- | --- |
| `cargo test --locked --offline --bin panelink ui_client::state::tests` | 155 passed, 0 failed | [state](logs/btn02a-worker-state.log) |
| `cargo test --locked --offline --test p5_workspace_tabs pointer_controls_preserve_live_ptys_and_confirm_close -- --test-threads=1` | 1 passed, 0 failed | [pointer](logs/btn02a-worker-private-pointer.log) |
| `cargo test --locked --offline --test p5_workspace_tabs workspace_tab_create_rename_close_keeps_identity_and_cancel_is_safe -- --test-threads=1` | 1 passed, 0 failed | [tabs](logs/btn02a-worker-private-tabs.log) |
| `cargo test --locked --offline --lib terminal::` | 92 passed, 0 failed | [terminal](logs/btn02a-worker-terminal.log) |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | passed | [Clippy](logs/btn02a-worker-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn02a-worker-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn02a-worker-diff.log) |

The intermediate migration run had 152 pass / 1 fail: the second header opener
in the confirmation geometry fixture still lacked Up. Its full log is retained
in [first state run](logs/btn02a-worker-state-first.log). The final run includes
the explicit second release and two new regression tests. No assertion was
removed or weakened to bypass a real application failure.

Pressed rendering, other surfaces, keyboard behavior and canonical combined
acceptance remain with the parent. These private-fixture checks do not establish
installed-runtime or host/provider acceptance. ms-rust was not used.
