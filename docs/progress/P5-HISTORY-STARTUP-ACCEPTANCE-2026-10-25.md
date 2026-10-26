# P5 pathname startup history-cleanup acceptance — 2026-10-25

## Scope

Added `tests/p5_history_cleanup_startup.rs` as a dedicated actual-runtime
acceptance fixture. It starts the production `serve` command against a private
pathname server directory and a compiled private terminfo entry. It does not
call a cleanup helper in place of startup.

The startup source establishes the required call order: `run_serve` calls
`prepare_history_store` and only then `driver.enable_history`. The preparation
opens the history store, runs `cleanup_temporary_files`, and prunes expired
history. The startup JSON reports `history_capture` only after this setup.
The integration test observes the corresponding runtime behavior as follows:

1. Seed one exact writer-format orphan in the private `server/history/` path.
2. Start the real pathname server and wait on a bounded startup-JSON channel.
3. Require `history_capture: enabled` and verify the orphan is gone before
   releasing the child shell.
4. Release the shell through a fixture-owned gate file and require its unique
   post-readiness marker to appear in a validated committed normal-screen
   checkpoint.

The marker/checkpoint check proves that the actual server-installed capture
path persisted output produced after readiness. The source call order plus the
observable pre-release deletion and post-readiness capture establish the
startup cleanup/capture contract. The fixture does not instrument an internal
timestamp for the writer constructor.

## Fail-closed case

The refusal case seeds both a valid candidate and an otherwise recognized
regular temporary with mode 0644. The store requires candidate files to be
private, single-link regular files, so the unsafe mode makes startup cleanup
fail during preflight. The expected runtime response is `state: running` with
`history_capture: unavailable`. Both candidates and their bytes remain
unchanged, demonstrating that preflight refusal happens before unlinking a
safe sibling. This matches the specified policy: an unsafe history leftover
disables history capture while the shell server continues.

## Safety and fixture bounds

- All test state lives below a unique mode-0700 directory in `/private/tmp`;
  seeded history files use mode 0600. Terminfo output also stays below that
  private root. The fixture removes only its own root.
- Terminfo compilation writes only under that root. The server receives a
  private runtime directory, explicit environment, and fixture-owned shell.
- Startup and committed-capture waits are bounded and condition-based. The
  child-output gate is released only after the server's startup JSON arrives.
- Shutdown first sends SIGTERM to the server PID returned by this fixture's
  `Command`; a bounded SIGKILL fallback targets that same PID only.
- No clipboard, activation, dependencies, source/runtime semantics, commits,
  or pushes are involved.

## Verification status

The requested Cargo/build/test commands were not run by the test owner. The
parent can run the proposed focused command after reviewing the new files:

```sh
cargo test --locked --offline --test p5_history_cleanup_startup -- --test-threads=1
```

This is a proposed dedicated target, not evidence of a pass. The test file is
macOS/aarch64-gated and performs a real pathname listener bind; record any
host restriction as blocked evidence rather than a test pass. No phase or
roadmap checkbox is changed here.

## Parent execution — 2026-10-25

The first build failed because the test called Metadata::mode without its
platform trait in scope. Parent used the already-imported PermissionsExt via
`metadata.permissions().mode()`. Initial failure remains at
`/private/tmp/panelink-p5-history-startup-r1.log`.

The corrected actual pathname runtime target passed **2 tests, 0 failed,
0 ignored** with locked/offline isolated Cargo home/target and serial threads:
`/private/tmp/panelink-p5-history-startup-r2.log` (5.98 seconds).
Both normal cleanup/capture and unsafe-temp capture refusal ran; the listener
permission blocker from the older environment did not recur. This resolves
that narrow historical blocker, not physical reboot or power-loss durability.
The test introduces no runtime semantic changes or clipboard operation.
