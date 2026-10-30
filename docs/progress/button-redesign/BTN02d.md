# BTN02d — incomplete; local keyboard subset retained

2026-10-30, shared checkout `/Users/sahillamba/.t3/worktrees/panelink/t3-baecf477`,
branch `t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus existing parent changes.

**This task is not complete. Do not accept this as BTN02 capture coverage.**
There is no demonstrated external blocker. The worker did not implement the
requested remaining pointer-capture work. Parent review and further implementation
are required before style migration.

## Retained changes

Only `src/ui_client/state/workspace.rs` and two appended tests in
`src/ui_client/state_tests.rs` were changed by this worker, in addition to this
report and `logs/btn02d-*`. Existing parent changes were preserved.

Tab actions cycle locally with Tab/Shift-Tab (including decoded BackTab).
Action letter shortcuts require Press rather than Repeat. Rename has three
local focus positions: editor, Save, Cancel. Tab and Shift-Tab cycle these;
Enter saves from editor/Save and cancels from Cancel, Escape cancels. Editing,
Space and paste affect the draft only while the editor is focused. Focused
Save/Cancel uses uppercase labels with unchanged geometry. Tab-close accepts
BackTab alongside existing navigation, retaining initial Cancel focus and
pending guards. No global shell key mapping changed.

Two regressions cover reverse/forward action focus, repeat shortcut refusal,
rename editor Space, noneditor Space/paste consumption, Cancel activation,
single pending save, and no child key delivery. Existing tests continue passing.

## Still required

All requested remaining capture surfaces are unchanged: tab strip, project/All
tabs chooser rows, tab actions, rename Save/Cancel and close buttons, Back to
live, empty New tab/Detach. These still use their earlier input behavior.
No workspace capture identity/epoch, bounds validation, pressed rendering,
empty-host routing change, raw Down/Up fixture migration or requested stale
capture regression matrix was added. The empty host currently calls
`handle_empty_workspace_event` directly without the live worker router; empty
capture must remain independent of live layout validity. Do not translate a
captured release into a fresh coordinate lookup on a changed catalog/dialog.
Accepted header and pane-modal capture and presentation were not edited.

No new cell capture was produced; existing terminal tests passing is not new
visual evidence. Full canonical verification and host/provider acceptance were
not run. The canonical provider WouldBlock failure remains unclassified.

## Actual verification

All Cargo commands used `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target`.
Full stdout/stderr logs are linked below. These checks establish compatibility
of the retained keyboard subset, not completion of the requested task.

| Command | Actual result | Full log |
| --- | --- | --- |
| `cargo test --locked --offline --bin panelink ui_client::state::tests` | 161 passed, 0 failed, 0 ignored | [state](logs/btn02d-state.log) |
| `cargo test --locked --offline --lib workspace_tabs::` | 10 passed, 0 failed, 0 ignored | [tabs](logs/btn02d-workspace-tabs.log) |
| `cargo test --locked --offline --lib terminal::` | 93 passed, 0 failed, 0 ignored | [terminal](logs/btn02d-terminal.log) |
| `cargo test --locked --offline --test p5_workspace_tabs -- --test-threads=1` | 5 passed, 0 failed, 0 ignored; 39.59s | [private PTY](logs/btn02d-p5.log) |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | passed | [Clippy](logs/btn02d-clippy.log) |
| `cargo fmt --check` | passed | [fmt](logs/btn02d-fmt.log) |
| `git diff --check` | passed | [diff](logs/btn02d-diff.log) |

An intermediate test compilation failed because the added synthetic KeyEvent
omitted its `state` field. It was corrected before the final state run. The
intermediate log was overwritten; only final full logs are retained.

Graph discovery initially used the incorrect `project_name` argument; using
`project` resolved the registered checkout and located the workspace handlers.
Exact source was then inspected. No refreshed-index claim. Memory lookup found
no relevant button-redesign entry and supplied no implementation facts.

No subagents, commits, push, install, user-pane operations, restart, dependency,
server/protocol, roadmap, global skill or other documentation changes. ms-rust
was not used. Parent independently reviews the retained subset and owns completion.
