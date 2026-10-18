# Restore-before-child-wait observation

Status: synthetic private-PTY observation passes for explicit cleanup and unwind-driven `Resources::drop`. This does not complete the Apple Terminal manual gate.

The new `cfg(test)` fixture in `src/p2_probe.rs` owns a private openpty master/slave pair and a real `TerminalSession`. Its child installs `trap '' HUP` before writing a fixed readiness marker, then execs `/bin/sleep 30`. The test reads the child PID from the known marker, sends HUP only to that private child process group, waits briefly, and confirms the child remains alive. It then enters raw/visual modes on the private host slave and verifies the exact setup bytes on the master.

A worker owns `Resources` while it performs either explicit `cleanup()` or an injected unwind that exercises `Resources::drop`. The main test thread observes the cloned slave termios and drains the master. It requires the complete 31-byte `RESTORE_SEQUENCE` and every saved termios field to match before cleanup has finished, while the child PID is still alive and less than the one-second HUP grace has elapsed. The worker must later finish within a bounded receive deadline, and the recorded cleanup duration must be at least one second. The explicit path also checks the collected child status; the unwind path checks the child no longer exists after Drop completes.

This fixture uses only private PTYs and a child-scoped signal through Rustix. It does not access `/dev/tty`, alter the agent's standard descriptors or environment, install process-global signal handlers, or print terminal content. Every readiness, output, and cleanup wait has a deadline; the worker retains ownership of the resource guard if an assertion fails.

## Verification

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --bin panelink p2_probe::tests:: -- --nocapture --test-threads=1
```

Actual result:

```text
running 12 tests
restore_order_path=explicit; restore_observed_ms=1; child_cleanup_ms=1012; exact_sequence_bytes=31; child_alive_at_restore=true
restore_order_path=unwind-drop; restore_observed_ms=1; child_cleanup_ms=1013; exact_sequence_bytes=31; child_alive_at_restore=true
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 5.02s
```

The other ten tests in this filter include the existing bounded unread-PTY regression and host restoration checks. `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` exited 0 with no output.

I also temporarily reversed the two cleanup operations in `Resources::cleanup` and reran the explicit-order test. The test failed as intended after the observer received the reset sequence and termios, with `child was no longer alive when host restoration was observed`; the original restore-first source was immediately restored. This negative mutation is not present in the working tree.

The synthetic result establishes ordering for the source-level guard with a surviving HUP-ignoring child. It does not demonstrate `/dev/tty` access, the host probe subprocess supervisor, or behavior in Apple Terminal. The remaining operator/manual host checks stay pending.

## Parent review and integrated check

Parent independently ran both new cases together: explicit restoration observed at 1 ms / cleanup 1,015 ms; unwind restoration at 1 ms / cleanup 1,013 ms; both saw the exact 31 bytes while the child was alive. These are observations from that run, not latency guarantees. Parent hardened readiness parsing to require a complete trailing `:READY-END` before interpreting the numeric child PID, preventing a partial PTY read from selecting a numeric prefix. The unwind completion probe now requires `ESRCH` specifically rather than accepting any liveness-probe error. Production cleanup order/semantics remain unchanged.

After those corrections, parent ran `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline -q`: exit 0, **83 passed / 9 ignored** (48 library + 20 binary + 4 host fixture/preflight + 11 real-PTY; 0 doctests). `cargo fmt --check`, `cargo clippy --locked --offline --all-targets -- -D warnings`, and `git diff --check` also exited 0. No dependencies, lockfile or vendored source changed. No commits/pushes; owner released files. This closes the scoped automated ordering observation; Apple Terminal redraw retest and interrupted manual acceptance remain pending.
