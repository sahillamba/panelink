# Phase 6 timing-sensitive test robustness

Status: test-only hardening completed for the named reproduced flakes. All
results below were run on macOS aarch64 with Rust/Cargo 1.99.1, using
`CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l10`.
Synthetic load was eight `/bin/bash` `yes >/dev/null` workers in one process
group; each driver killed and waited for that group in `finally`. Cargo commands
used `--locked --offline`.

## Changes and preserved assertions

| Test | Root cause | Test-only change | Correctness assertion retained |
| --- | --- | --- | --- |
| `server_startup::tests::explicit_startup_commits_metadata_and_keeps_singleton_while_child_lives` | A parallel test process can inherit the dropped flock descriptor during fork-to-exec. The test already had the same bounded retry pattern as `server_storage::reopen_after_drop`. | Verified the existing 2-second retry under load; no additional edit was needed. | A live server still excludes another opener; after it drops, the next open advances the generation to 2. A competing real lock still returns `WouldBlock` after the retry bound. |
| `history_capture::tests::full_count_queue_retains_unsubmitted_end_and_finalizes_after_timeout` | The test's intentional block/gate could be starved by unrelated scheduled workers before the backend signaled entry or completed its reserved-end finalization. | Expanded only gate-entry and finalization observation deadlines from 3 to 30 seconds. The `shutdown(Duration::ZERO) == TimedOut`, full-queue admission, unsubmitted-end diagnostic, and finalized-end diagnostic assertions are unchanged. | Queue saturation still leaves the end metadata unsubmitted at shutdown timeout; after releasing the backend, its accepted end is finalized. |
| `history_writer::tests::executing_final_stage_can_commit_after_timeout` | This target did not reproduce a failure in this run. It already uses an entered/released condition-variable gate. | The shared receipt/lock wait helpers were extended to 30 seconds to tolerate load. Explicit lock-retention and late-commit assertions remain. | Timeout remains the shutdown outcome; the blocked final stage later commits, does not record cancellation, and does not rewrite the timed-out shutdown result. |
| `multi_runtime::tests::blocked_history_writer_does_not_block_pty_input_or_socket_control` | Fixed short receive/progress deadlines caused socket reads to expire during loaded scheduling; an additional screen-query loop can safely observe more than one snapshot while waiting for the launched pane to render its ready marker. | Expanded test-local socket/deadlock deadlines and overall progress bounds; retained the 250 ms owner-tick latency assertion; polling loops use increasing request IDs and bounded intervals. | While the backend remains deterministically blocked, PTY input reaches the shell, split control completes, and the new pane's screen contains `ASYNC-SPLIT-READY`. No request or input is replayed. |
| `default_open::tests::lock_probe_detects_a_held_storage_lock` | No failure reproduced. A one-shot post-drop open could overlap another process's fork-to-exec descriptor inheritance. | Added a bounded 2-second test-only retry for `WouldBlock` after release. | The probe detects the held lock and refuses open while held; after drop the test waits for openability. Non-`WouldBlock` errors are not retried. |
| `tests/p5_screen_checkpoint.rs::hup_final_output_is_captured_or_reported_incomplete_before_pty_close` | The test started a foreground `/bin/sleep` in the shell's process group after printing readiness. Group HUP raced child exit and shell trap handling. | Changed the post-readiness fixture to block in the shell's own `read`, with a comment explaining the signal-order issue. | Before PTY close, either EOF is observed with `P5-FINAL-ON-HUP` in the screen, or incomplete capture is reported without claiming EOF; child exit and closed-PTY assertions remain. |

The root causes for the startup and default-open retries are the bounded,
documented fork-to-exec lock window. The HUP test failure was fixture process
group ordering, not a product behavior change. The socket EOF was in the
test's follow-up client path after short deadlines; the revised test allows
scheduler delay and continues to assert actual control progress and rendering.

## Baseline reproduction and post-change repetitions

Baseline repeated runs were under the eight-worker synthetic load. For the two
Rust library tests, a full `--lib -- --test-threads=12` run was repeated 20 times;
for the two bin/integration tests, the named target was repeated 20 times.
Target results are failures / 20:

| Test | Baseline under load | After changes under load |
| --- | --- | --- |
| server startup singleton reopen | 0/20 in the full lib run; parent retry also verified in isolation 20/20 | 0/20 in isolated post-change reruns |
| history capture full-queue end finalization | 0/20 in the full lib run; isolated baseline not separately run | 0/20 isolated post-change reruns |
| history writer final-stage commit-after-timeout | 0/20 in the full lib run; isolated baseline not separately run | 0/20 isolated post-change reruns |
| blocked history writer / PTY input / socket control | 13/20 full lib runs failed at the second client's `read_frame` with `UnexpectedEof: socket closed before the frame was complete`; 12/20 in an earlier baseline pass | 0/20 isolated post-change reruns; full lib post-change repeats below passed the named test each time |
| held storage lock probe (bin) | 0/20 isolated target runs and 0/20 full bin-suite runs | 0/20 full bin-suite runs |
| HUP final output screen checkpoint (integration) | 8/20 target runs failed because EOF was observed without the trap marker | 0/20 target runs |

The post-change full library suite was also repeated 20 times under load. The
named `multi_runtime` test passed 20/20, but six suite runs failed in other,
unnamed-at-task-start writer tests (`real_store_reserved_known_and_unknown_ends_are_conservative`
and `backend_panic_fails_current_receipt_and_closes_writer`). Investigation
showed an immediate-submit race against worker initialization in the panic test,
and a test that attempted maintenance lock acquisition while it still owned the
storage handle. A deterministic `BlockThenPanic` gate now ensures batch
admission precedes the panic; the real-store test drops its storage handle and
waits for the worker-held lock to release before opening the store. The two
additional cases each passed 20/20 isolated under load after those corrections.
They are outside the six named flaky tests but were found during required full
suite repetitions.

The startup retry was already in the parent-edited working-tree baseline, so
its pre-change 0/20 result verifies rather than estimates behavior before that
edit. The two history cases passed every baseline full-library run; isolated
post-change runs provide their explicit 20/20 counts.

## Verification status

Executed repetitions and summaries:

- Baseline full library suite, 20 runs under load: 13/20 suite runs failed on
  `multi_runtime`; every history/startup target passed in all observed runs.
- Post-change isolated `multi_runtime`, 20 runs under load: 20/20 passed.
- Post-change isolated history targets (history capture, writer late final
  stage, writer worker panic, and real-store reserved ends), 20 runs per target
  under load: all four were 20/20 passed.
- Post-change startup singleton target, 20 runs under load: 20/20 passed.
- Post-change full binary test suite, final 20 runs under load: 20/20 suites
  passed (101 passed each); lock-probe target passed 20/20.
- Post-change HUP integration target, 20 runs under load: 20/20 passed.
- Full post-change lib suite under load: the first attempt had 14/20 suite
  passes; six exposed the two additional writer-test races, now corrected.
  Additional isolated post-fix tests passed 20/20 each. A final 20-run full
  library suite passed 20/20 (637 passed, 0 failed, 4 ignored in each run).
- The lock-probe retry helper is test-only under `#[cfg(test)]`; final bin
  suite repetitions after this cleanup passed 20/20.

## Final validation record

After formatting the one changed `src/multi_runtime.rs` layout, targeted
`cargo fmt --check --package panelink` exited 0 with no output. Strict
`cargo clippy --locked --offline --all-targets -- -D warnings` exited 0:

```text
Checking panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.38s
```

Workspace `cargo fmt --check --all -- --check` is not clean because untouched
vendored `vendor/vt100/src/*.rs` files differ from the active rustfmt output.
The first check also found and was followed by fixing the changed
`src/multi_runtime.rs` formatting. No vendored files were reformatted or edited.

The required three final verifier runs emitted these summary JSON lines:

```json
{"checks": [{"name": "cargo_test", "status": "passed"}, {"name": "crossterm_input_decoder_tests", "status": "passed"}, {"name": "component_sources", "status": "passed"}, {"name": "cargo_fmt", "status": "passed"}, {"name": "cargo_clippy", "status": "passed"}, {"name": "cargo_build", "status": "passed"}, {"name": "python_unittest", "status": "passed"}, {"name": "docs_check", "status": "passed"}], "component_verification_only": true, "host_provider_gates": "not_certified", "milestones_complete": false, "setup_status": "valid"}
{"checks": [{"name": "cargo_test", "status": "failed"}, {"name": "crossterm_input_decoder_tests", "status": "passed"}, {"name": "component_sources", "status": "passed"}, {"name": "cargo_fmt", "status": "passed"}, {"name": "cargo_clippy", "status": "passed"}, {"name": "cargo_build", "status": "passed"}, {"name": "python_unittest", "status": "passed"}, {"name": "docs_check", "status": "passed"}], "component_verification_only": true, "host_provider_gates": "not_certified", "milestones_complete": false, "setup_status": "valid"}
{"checks": [{"name": "cargo_test", "status": "passed"}, {"name": "crossterm_input_decoder_tests", "status": "passed"}, {"name": "component_sources", "status": "passed"}, {"name": "cargo_fmt", "status": "passed"}, {"name": "cargo_clippy", "status": "passed"}, {"name": "cargo_build", "status": "passed"}, {"name": "python_unittest", "status": "passed"}, {"name": "docs_check", "status": "passed"}], "component_verification_only": true, "host_provider_gates": "not_certified", "milestones_complete": false, "setup_status": "valid"}
```

Verifier run 2's aggregate Cargo check failed only in
`tests/p3_detach.rs::interrupt_delivers_sigint_after_a_ready_marker`; its
captured summary was `test result: FAILED. 2 passed; 1 failed; 0 ignored; 0
measured; 0 filtered out; finished in 3.07s`. The same command targeted at
that test immediately afterward passed:

```text
running 1 test
test interrupt_delivers_sigint_after_a_ready_marker ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.04s
```

This is outside the named L10 tests and was not changed. Verifier outcome is
therefore **2/3 complete passes; one run had an unrelated intermittent PTY
signal test failure**. The verifier correctly reports
`"component_verification_only": true`, `"host_provider_gates": "not_certified"`,
and `"milestones_complete": false`; this test work does not certify those
gates.

## Parent review — 2026-10-21

Reviewed diff: all `src/` edits are inside `#[cfg(test)]` modules or test-only
helpers; no production semantics changed. Parent independently ran the
verifier three times after the worker finished: 2/3 passed 8/8 (983 passed /
0 failed / 15 ignored); one run failed `tests/p3_detach.rs
interrupt_delivers_sigint_after_a_ready_marker` ("PTY output marker timed
out" while waiting for the shell's READY marker under load). Parent changed
only that test file's `wait_for_screen` marker wait from the shared 3 s IO
deadline to a 10 s marker deadline; signal/claim/status assertions are
unchanged. The test passes in isolation. Repository binary rebuilt
(`cargo build --locked`) after the slice.
