# P2 host acceptance: operator-only outer-PTY tests (task H1)

Date: 2026-10-22. Scope: execution-only run of the nine `#[ignore]`d
outer-PTY host tests in `tests/p2_host.rs`. No source or test files were
edited. Acceptance result is recorded from the commands below only.

## Environment

- macOS: 26.6.1 (build 25G76), Apple Silicon (aarch64).
- Rust: rustc 1.99.1 (58cf8f9c3 2026-10-13); cargo 1.99.1 (5f94df478 2026-08-27).
- Build flags: `--locked --offline`; `CARGO_TARGET_DIR` set to a private
  per-worker target directory under the system temp area.
- Controlling terminal of the worker shell: `tty` printed `not a tty`.
- `/dev/tty` (device node present, mode `crw-rw-rw-`): an explicit
  `os.open('/dev/tty', O_RDWR)` from the worker failed with
  `[Errno 6] Device not configured` (ENXIO), because the worker process has
  no controlling terminal. The earlier `#[ignore]` reason text names EPERM
  from a restricted sandbox; the failure observed in this run was ENXIO, not
  EPERM. The tests passed in this worker despite the failed `/dev/tty` open,
  which suggests they provision their own PTY; this was not verified in
  source.

## Commands and results

Logs are kept outside the repo under the worker's temp directory
(`.../T/opencode/h1logs/`).

1. Ignored tests, run 1 of 3 (serial):
   `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`
   Result: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 6.15s` (exit 0).

2. Ignored tests, run 2 of 3 (same command):
   Result: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 5.75s` (exit 0).

3. Ignored tests, run 3 of 3 (same command):
   Result: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 5.70s` (exit 0).

4. Non-ignored tests, once (serial):
   `cargo test --locked --offline --test p2_host -- --test-threads=1`
   Result: `test result: ok. 4 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.49s` (exit 0).
   The 9 ignored tests were skipped here, not run, and are not counted as passes.

Test names and per-run status for the nine operator-only tests:

| Test | Run 1 | Run 2 | Run 3 |
| --- | --- | --- | --- |
| ctrl_q_restores_the_outer_terminal_and_stops_the_child | ok | ok | ok |
| external_sigterm_is_polled_and_hangup_ignoring_child_is_killed_within_budget | ok | ok | ok |
| host_hangup_exits_within_deadline | ok | ok | ok |
| local_ctrl_q_remains_available_while_child_exit_output_is_draining | ok | ok | ok |
| normal_child_exit_restores_termios_and_never_forwards_osc_controls | ok | ok | ok |
| output_stall_diagnostic_and_child_cleanup_are_bounded_without_outer_drain | ok | ok | ok |
| oversized_paste_drops_suffix_and_flushes_already_queued_outer_input | ok | ok | ok |
| resize_is_applied_to_the_child_and_an_undersized_resize_restores_the_host | ok | ok | ok |
| typed_keys_and_one_decoded_paste_reach_the_child_once | ok | ok | ok |

Per-run counts for the ignored runs: passed 9, failed 0, ignored 0 (runs
1-3). The non-ignored run: passed 4, failed 0, ignored 9.

The four non-ignored tests that passed in the non-ignored run were
`non_tty_rejection_happens_before_child_side_effects`,
`outer_pty_fixture_attaches_a_controlling_session`,
`release_drains_suppressed_child_output_and_reaps_supervisor`, and
`retained_slave_remains_queryable_after_supervised_child_exit`.

## Failures

None. No test failed in any run, so the isolated five-repeat rerun step
(step 3 of the task) was not needed and was not performed.

## Process hygiene

After each run, `ps` was checked for `p2_host`, `panelink`, and `pty-probe`
processes. None belonging to these test runs remained. Unrelated processes
belonging to other sessions were present (an opencode worker, a codegraph
server, and a shell); they were not started by this task and were left
untouched.

## Conclusion

Within this worker session, all nine operator-only outer-PTY tests passed
in three consecutive serial runs (27 of 27 executions), and the four
non-ignored tests in the same binary passed once. These are real executions
(`ignored, ... ok` status), not skips.

Limits of this evidence:

- Executed from a worker process without a controlling terminal, not from
  an operator's Apple Terminal session. The tests passed without a usable
  `/dev/tty`, so they appear to provision their own outer PTY, but this run
  does not prove behavior under a human-operated terminal.
- Three runs in one session. This is not a stability or flake-rate claim.
- This does not close any Phase 2 gate by itself. The roadmap's open Phase 2
  items (clipboard sentinel in full Apple Terminal, etc.) are tracked
  separately in `ROADMAP.md`.

## Parent verification — 2026-10-22

Worker model: claude-haiku-5-5 (user-selected). Parent independently reran
`cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`:
`9 passed; 0 failed; 0 ignored`. The ignore reason (sandbox EPERM on
/dev/tty) no longer applies in this environment; the tests create their own
outer PTY. Together with the user's 2026-10-22 OSC 52 clipboard check, the
Phase 2 host gates are ticked in ROADMAP.md.
