# Phase 2 host probe result

The binary-private macOS/aarch64 host adapter now owns one disposable core session and one independently opened `/dev/tty`. The host loop uses the selected decoder and terminal renderer, keeps fixed-size input/frame buffers, limits input work between core pumps, coalesces resize updates, and never forwards unparsed host bytes. It keeps local Ctrl-Q available while draining after child exit, drops input-disabled replies, and fails closed on incomplete input, including a single-byte UTF-8 prefix. Typed character forwarding excludes C0/C1 control scalars.

Host restoration is cached and ordered before child shutdown in the resource cleanup path. It sends the fixed visual reset, restores saved termios, flushes already-queued input before and after termios restoration, keeps the separate TTY open while the child shuts down, then performs a final input flush and closes it. This protects bytes already queued at cleanup; it cannot suppress bytes an external producer sends after the final flush or process exit. The implementation saves exact termios and does not change standard-descriptor flags; the corresponding complete-process restoration assertions remain in the ignored runtime scenarios. ANSI visual modes restore conservative defaults because the prior private modes are not queried. Explicit runtime errors use `p2_probe::report_error`: a fixed-label line in a 128-byte stack buffer, sent through a separate nonblocking `/dev/tty` with a 100 ms write deadline and no inherited-stderr fallback.

The outer-PTY test helper now creates a new session, assigns the PTY with Rustix `TIOCSCTTY`, verifies `tcgetsid` against `getsid`, uses a synthetic environment, invokes `/usr/bin/tic` with a deadline, bounds PTY capture, and terminates/reaps a probe subprocess during test unwinding. It includes normal exit/control filtering, typed input and paste, Ctrl-Q, signal cleanup, child-exit drain, resize, paste overflow/input flushing, hangup, and output-stall cleanup scenarios. Nine scenarios are marked operator-only because this restricted command sandbox denies the probe’s separate `open("/dev/tty", RDWR|NONBLOCK|CLOEXEC)` with `EPERM` (`Operation not permitted`). The test-side controlling-session check succeeds, so the fixture establishes a controlling TTY; the device-open denial remains the blocker. Run those scenarios with `cargo test --locked --offline --test p2_host -- --ignored` in an environment that permits `/dev/tty` access.

The panic/drop unit test verifies saved terminal state after unwinding completes. It does not measure terminal restoration before the child’s HUP grace interval, and its HUP-ignore fixture has no readiness handshake; that runtime ordering evidence remains pending. Source cleanup order is explicit, but the outer-PTY output-stall scenario was not executed here. No Apple Terminal compatibility or manual shell/editor/paste behavior is claimed.

## Verification

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --bin panelink p2_probe`:

```text
running 8 tests
test p2_probe::tests::only_non_control_scalars_are_eligible_for_typed_key_forwarding ... ok
test p2_probe::tests::setup_control_write_stops_after_a_partial_hard_error ... ok
test p2_probe::tests::profile_caps_reject_before_any_host_resource_is_opened ... ok
test p2_probe::tests::incomplete_single_byte_utf8_prefix_fails_closed_after_inactivity ... ok
test p2_probe::tests::pending_timeout_clock_survives_a_completed_key_in_the_same_read_chunk ... ok
test p2_probe::tests::host_restore_flushes_already_queued_input_before_releasing_tty ... ok
test p2_probe::tests::unwinding_owner_drop_restores_saved_host_state ... ok
test p2_probe::tests::host_error_diagnostic_is_fixed_text_and_uses_a_bounded_write_budget ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.10s
```

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p2_host`:

```text
running 11 tests
test ctrl_q_restores_the_outer_terminal_and_stops_the_child ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test external_sigterm_is_polled_and_hangup_ignoring_child_is_killed_within_budget ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test host_hangup_exits_within_deadline ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test local_ctrl_q_remains_available_while_child_exit_output_is_draining ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test normal_child_exit_restores_termios_and_never_forwards_osc_controls ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test output_stall_diagnostic_and_child_cleanup_are_bounded_without_outer_drain ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test oversized_paste_drops_suffix_and_flushes_already_queued_outer_input ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test resize_is_applied_to_the_child_and_an_undersized_resize_restores_the_host ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test typed_keys_and_one_decoded_paste_reach_the_child_once ... ignored, operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access
test outer_pty_fixture_attaches_a_controlling_session ... ok
test non_tty_rejection_happens_before_child_side_effects ... ok

test result: ok. 2 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.18s
```

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` exited 0 with no output.

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings`:

```text
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
```

The device-dependent PTY scenarios remain unverified in this sandbox; the successful test counts do not include them.

Parent final review: original termios and stdio flags are now captured by the harness before spawning the probe, including the output-stall case. Normal rapid child exit no longer relies on observing a transient raw-mode interval. Each capture drain is capped at eight 8 KiB reads so a continuously writing probe cannot indefinitely postpone the caller's process/deadline check. After these harness corrections, the parent full suite passed: 66 application tests, zero failed, nine device-dependent scenarios ignored; fmt and strict Clippy passed. These source corrections do not turn the ignored scenarios into verified runtime behavior.
