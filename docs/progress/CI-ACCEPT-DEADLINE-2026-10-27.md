# CI handshake failure: accepted-connection deadline race

## Scope and checkout

Investigated [run 40511639904, job 122215128926](https://github.com/sahillamba/panelink/actions/runs/40511639904/job/122215128926).
The failed head is `b97e2f7cceed75cd8436ab13542ec81dafeef311`.
Work is isolated on `codex/ci-pane-run-handshake` in
`/private/tmp/panelink-ci-40511639904`. The original main checkout is at
`bd84625` with two untracked user files; neither was changed.

Session workflow: Matt Pocock's `diagnosing-bugs` skill, upstream
`59db609eb1e18aaf59b63284d0e52255aff0c63c`, installed outside the repo in a
temporary session folder. It is development guidance, not a shipped dependency.

## Observed failure and reproduction

Hosted macOS 15.7.9 / arm64 failed the canonical verifier's root Rust check in
`pane_run_uses_invocation_context_literal_argv_and_returns_quick_exit_identity`.
`pane list` returned `control_unavailable`, `cause=transport_failed`,
`stage=handshake`, `transport_stage=reply_read`. Library tests had reported
907 passed / 4 ignored, binary tests 252 passed, and the failing target reported
2 passed / 1 failed. Later integration targets were not run by that fail-fast
Cargo invocation.

On local macOS 26.5.2 / arm64, the exact unchanged test passed once, then
failed on repetition 8 of an up-to-20-run loop. The local failure was the same
handshake failure class but at `request_write`, not `reply_read`.
Command:

```sh
RUST_TEST_THREADS=1 cargo test --locked --offline --test p3_pane_run -- --exact pane_run_uses_invocation_context_literal_argv_and_returns_quick_exit_identity --nocapture
```

A reduced `/usr/bin/true` launch passed 30 repetitions. Another 20 original
runs and 20 runs backgrounding only the owned private server passed. Temporary
fixed-label tracing for capacity, worker read errors, and listener errors did
not capture a second natural handshake failure. A later traced series passed
24 runs, then its next build failed from local disk exhaustion, not a handshake.
These successful repetitions do not establish a flake rate or a hosted pass.

## Confirmed defect and repair

`run_serve` uses a 1 ms accept slice. `PrivateListener::accept` previously
consumed a socket with the nonblocking kernel accept, then checked the deadline.
If preempted across expiry, it returned TimedOut and dropped that accepted
socket. The owner treats slice timeout as normal; the waiting client instead
sees a broken handshake.

A controlled regression injects an already-dequeued socket at an expired
post-accept boundary, without sleeping or relying on scheduler timing. With the old behavior it fails because the client observes EOF. The
repair retains at most one dequeued stream across expiry and returns the same
timeout. The next nonexpired call authenticates that same stream before
returning it. An already-expired next call leaves the pending stream untouched.
Concurrent accepts return WouldBlock rather than waiting on the one-slot state;
poisoning fails closed. The spawn gate still covers descriptor creation only.
No deadline, authentication, capacity guard, wire contract, operation identity,
input replay rule, dependency or ignored-test setting is relaxed.

This defect can produce the observed symptom. The hosted failure itself has
no accept-boundary trace, so its attribution to this particular race remains
an inference pending a fresh hosted run.

## Verification

The new regression failed with old behavior (peer read returned EOF), then
passed after the repair. The original three-test `p3_pane_run` target passed.
An initial regression build had a fixture-only Debug-bound compile error;
the assertion was corrected without exposing AuthenticatedStream internals.
A subsequent build exhausted disk space. Only this checkout's generated
incremental cache was removed; subsequent local checks use
`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
Temporary vendor formatting from `cargo fmt --all` was restored in full.
Temporary diagnostics and reduced tests were removed.

Independent GPT-6-Luna High review found no material correctness/security
issue; it inspected the diff, gate, listener, authentication and deadline paths
and ran whitespace validation, without running tests. Its test-timing note was
resolved by replacing a 10 ms entry deadline / 20 ms sleep with direct injection
at the post-accept staging helper. Parent mutation verification of the final
regression fails with the old check-before-retain order (EOF) and passes with
retain-before-check, both in under 0.02 seconds of test execution.

Canonical verifier exited 0: **8/8 checks passed**. Root Rust result totals:
**1,480 passed, 0 failed, 16 ignored** across 79 result groups; decoder:
**39 passed**. Python: **233 run, 232 passed, 1 opt-in skip**. Fingerprints,
formatting, strict all-target Clippy and build pass. Docs: **393 files, 0 findings**.
The broad root test run used the equivalent pending-stream repair with the
initial timed regression; during its integration targets, the test seam was
refactored to the deterministic staging helper. Final-source transport rerun:
**40 passed, 0 failed, 2 ignored**. Later canonical Clippy/build validated the
final source. The original pane-run target also passed in the broad run.

Logs:

- `/private/tmp/panelink-ci-40511639904-verification.log`
- `/private/tmp/panelink-ci-40511639904-transport.log`

Final docs/whitespace checks were rerun after this evidence refresh.
No hosted rerun, push, live binary activation, workspace restart, or phase
completion is implied. Existing host/provider/reboot/workload gates remain open.
