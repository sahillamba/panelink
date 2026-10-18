# Host PTY harness lifecycle correction

`ProbeHarness` now starts a small `/bin/sh` supervisor as the PTY's controlling session leader. The actual probe runs as its child in the same process group, with `<&0` explicitly preserving the PTY stdin that a noninteractive shell would otherwise redirect for an asynchronous command. The supervisor does not read from the terminal. It atomically publishes the probe PID and exit status through temporary files renamed with `/bin/mv`, then waits for a private release marker so the retained outer slave remains queryable after probe exit. The fixture directory is mode `0700`; control-file reads are regular-file-only and capped at 32 bytes. Partial, malformed, oversized and out-of-range control values fail distinctly from a not-yet-published file. CLI status is reconstructed with `ExitStatusExt::from_raw(code << 8)`; supervisor exit without a published probe status is an error.

The harness captures slave termios and descriptor flags before spawning. It keeps the outer master and slave open through assertions. Drop checks the published probe status first; if the probe is still running while the owned supervisor remains alive, it sends TERM to the supervisor's pinned process group and polls for status for at most 2.5 seconds. It then releases the supervisor and drains/discards the master in bounded batches (at most 64 KiB per poll) while allowing one second for exit. If needed, it closes the master, sends group KILL while the unreaped supervisor still pins the group ID, and performs a bounded one-second reap. A cached `Child::try_wait` status is returned without reusing the former PID as a process-group ID. Cleanup never signals a raw probe PID, avoiding a recycled-PID race between probe reap and status publication. The external-SIGTERM scenario still targets the published probe PID, after a child marker proves its HUP-ignore trap is installed and before probe exit is known. The supervisor waits for release with a 50 ms sleep per check instead of a busy loop.

Added a non-ignored regression that runs `/usr/bin/true` under this supervisor with the same `setsid`/`TIOCSCTTY` setup and no `/dev/tty` open. It waits for the child status, checks retained-slave termios and status flags while the supervisor still owns the session, then releases and reaps the supervisor. `/bin/true` is absent on this macOS host (`ls: /bin/true: No such file or directory`), so the fixture uses the equivalent absolute `/usr/bin/true` path. The former ENOTTY assertion is preserved; no error is swallowed.

A second non-ignored regression runs a child that emits 512 bytes, observes its exit without draining the PTY, then releases the supervisor. It asserts release cleanup consumed the queued output and reaped the supervisor. This specifically exercises post-assertion cleanup draining; the no-drain probe wait and its five-second output-stall deadline remain unchanged.

The oversized-paste scenario now sends the bracketed-paste opener with exactly 65,536 payload bytes first, then one short write containing the overflow byte, terminator and harmless command-shaped tail. It asserts the complete short write was accepted and sends no more input afterward. After the probe exit status is published, it checks for child-side line output and inspects the restored input queue while the supervisor is still alive, restoring the shared slave file-status flags before assertions. This scenario remains operator-only and was not run here.

The supervisor traps HUP, INT and TERM so a PTY hangup cannot make its session leader disappear before status publication. Its wait loop tolerates a wait interrupted by a trapped signal, checking whether the probe PID still exists before accepting the wait result. The output-stall scenario and its five-second outer test deadline are unchanged; the runtime output-stall cause remains for parent diagnosis. The HUP-ignoring fixture now uses a readiness marker after installing its trap, but the external signal/KILL scenario remains ignored and is not claimed as verified.

## Verification

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p2_host`:

```text
running 13 tests
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
test retained_slave_remains_queryable_after_supervised_child_exit ... ok
test release_drains_suppressed_child_output_and_reaps_supervisor ... ok
test non_tty_rejection_happens_before_child_side_effects ... ok

test result: ok. 4 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out; finished in 0.18s
```

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` exited 0 with no output.

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings`:

```text
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
```

All nine device-dependent outer-PTY cases remain unverified because the sandbox denies the probe's separate `/dev/tty` open with EPERM. The added regression establishes retained-slave queryability for a short supervised child; it does not test the probe's host-IO path, restoration, output-stall behavior, or Apple Terminal compatibility.
