# P3A outer-PTY observation refactor — 2026-10-28

| Field | Value |
| --- | --- |
| Checkout | `/Users/sahillamba/.t3/worktrees/panelink/t3-bc7b2da2` |
| Branch | `t3/shared-pty-test-fixtures` |
| Base/head at start | `0baf25eefbf396b2ad76f2b237bece1cde5377ed` |
| PR | Parent-reviewed refactor prepared for an independent PR against `main`. |

## Changes

Added `tests/support/outer_pty.rs` and moved both P3A test consumers to it.
The support client owns outer-PTY/session setup, status and release supervision,
bounded capture (read errors and overflow are explicit), exit observation,
termios checks, input, and bounded cleanup. Each consumer still builds its
scenario-specific shell command, environment and argv. The 64x269 launch and
full-stderr-pipe launch remain explicit in their scenarios.

Added a delayed-drain regression: a gated capture worker cannot consume the
child's marker; the test observes the attach exit first, releases the drain,
then waits for the captured marker. This uses observable conditions and bounded
cleanup. Default-open detach still waits for actual restore bytes after exit;
attach-host retains its stalled-server latency and termios assertions and now
waits on restore bytes instead of a fixed post-exit delay. The ignored
real-environment smoke also uses the shared harness.

## Verification

- Focused serial P3A tests on the final tree: `p3a_attach_host` 6 passed;
  `p3a_default_open` 8 passed, 1 ignored (`real_environment_smoke`). The
  delayed-drain regression passed in both integration binaries.
- `cargo fmt --check`: passed.
- Strict all-target Clippy with isolated Cargo target: passed.
- `scripts/check_docs.py`: 399 Markdown files checked, 0 findings.
- `scripts/check_component_sources.py`: both `vt100` and `crossterm-input`
  matched on the final tree.
- Final canonical `verify_non_ui.py` run: 7 of 8 checks passed. `cargo_test`
  failed in `tests/p3_agent_cli.rs` at
  `workspace_events_pin_filtered_pane_across_same_generation_replacement`
  (`tests/p3_agent_cli.rs:854`): the expected pane-output event was absent;
  captured events were `layout_changed` sequence 3, `pane_closed` for index 1,
  then `quiet` at cursor 2. The test target reported 7 passed, 1 failed in
  45.63 seconds. This test and its source were not changed; baseline
  attribution is unverified, and the failure was not retried. Decoder,
  component-source, formatting, strict Clippy, build, Python (233 run, 1
  skipped), and docs checks passed in that invocation.
- The earlier canonical run passed `cargo_test` but the component-source step
  reported a `vt100` `hash_mismatch`: an earlier `cargo fmt --all` invocation
  had temporarily reformatted vendored files. Those files were restored, and
  `check_component_sources.py` passed afterward and in the final canonical run.
- All Cargo work used
  `CARGO_TARGET_DIR=/private/tmp/panelink-outer-pty-refactor-target`,
  `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, and
  `CARGO_PROFILE_TEST_DEBUG=0`. Python checks used
  `/Users/sahillamba/.local/bin/python3.11`.

Product behavior and acceptance boundaries are unchanged. No dependencies,
launcher artifacts, live workspaces, roadmap or session handoff were changed.

## Parent review and final checks

Parent source review restored the previous empty-cell-as-space row matching,
made the attach-host restoration check wait for the final `1049l` marker before
asserting all preceding restore bytes, and removed busy yielding while the
regression gates its drain. The ignored smoke test checks its second-pane
output as a screen substring rather than as an unsplit root row.

The delayed-drain regression now also checks spaced text in the content area.
Its first extended fixture incorrectly placed the text in the excluded left
margin; that assertion failed, then passed after moving the marker into the
content area. Final serial focused suites: attach-host 6 passed; default-open
8 passed, 1 existing ignored. The real-environment smoke remains unexecuted.
The parent separately reran all 8 pane CLI tests successfully, including the
earlier event-test failure; this does not establish its baseline cause.

Parent logs: `/private/tmp/panelink-pty-parent-focused.log`,
`/private/tmp/panelink-pty-parent-final-focused.log`,
`/private/tmp/panelink-pty-parent-regression.log`, and
`/private/tmp/panelink-pty-parent-verifier.log`.

Parent canonical verifier exited 0: 8/8 checks passed. Its Cargo suite began
before the final spaced-marker test extension; final focused P3A suites and
all-target Clippy/build then validated the final source. Its deliberately short
PATH omitted optional Python test prerequisites, so Python reported 233 run
with 21 skips; that count is not treated as complete Python scenario coverage.
The follow-up with the normal PATH passed all 233 Python tests with only the
existing opt-in skip (`/private/tmp/panelink-pty-parent-python.log`). Final
docs and whitespace validation passed after this evidence update.
