# Phase 5 abrupt server-death recovery acceptance — 2026-10-26

## Scope

Added `tests/p5_crash_recovery.rs`, a default-runnable Apple Silicon/macOS
integration test using the real `panelink serve` executable and its private
Unix pathname listener. The fixture uses mode-0700 private `PANELINK_HOME`,
home, cwd, env, and terminfo directories, plus a deterministic cooperative shell.
It keeps the scenario to one pane, which exercises the minimum composed
layout, history, and generation recovery contract.

Before server death, the test independently polls and decodes the on-disk
committed one-pane layout, normal scrollback rows, and normal/alternate screen
checkpoints. The fixture scrolls a unique normal-row marker into history,
briefly displays alternate A, overwrites it with alternate B, and waits for
B's committed latest checkpoint before returning to normal. It submits one unique input canary;
the cooperative child records its side effect exactly once. The test then
validates the exact server PID against its owned child PID, effective UID,
and captured kernel start time, sends SIGKILL, and reaps that process.

Fresh CLI processes run offline recovery view, historical history show, and
history search. They must leave storage byte-identical, avoid launching the
configured shell, and produce output without ESC or OSC 52 control data.
Explicit `recovery relaunch` then exercises stale-socket recovery, requires
the saved one-pane topology, and checks a new full target/generation mapping
plus the fixture cwd, home, and private environment canary.
Historical history remains addressable under the old full target and its
committed files remain byte-identical. The original input side effect remains
exactly once and is not replayed into the fresh shell.

## Verification status

The implementation owner ran formatting/diff checks only. Parent reviewed and
executed the test serially with the isolated target directory:

```sh
CARGO_TARGET_DIR=/private/tmp/panelink-phase-acceptance-target \
  RUST_TEST_THREADS=1 cargo test --locked --offline --test p5_crash_recovery
```

Result: **1 passed, 0 failed, 0 ignored**, 3.17 seconds, at
`/private/tmp/panelink-p5-crash-r3.log`. Strict all-target Clippy passed.
The test is default-runnable and contains no environment-controlled skip.
Full verifier results are in [the continuation note](P3-P5-RESUME-2026-10-26.md).

Independent review found the original best-effort replacement stop could
remove state before exit was confirmed. Parent now captures the replacement
PID/kernel start identity, verifies UID before fallback signalling, requires
normal stop and exact-process exit in the test, and preserves fixture files
if exit cannot be confirmed during unwind. Follow-up review found no material
issues; final affected target passes **1/0 in3.46 seconds** at
`/private/tmp/panelink-p5-crash-r4.log`.

Parent corrected fixture assumptions before the passing run: macOS tic may
use the `70/` terminfo directory; normal readiness must use the last visible
filler rather than the already-scrolled history marker; the alternate disk
poll waits for B; human `panes` output closes the target with `)`; and the
cleanup stop command must capture/reap its subprocess. The initial terminfo
failure and subsequent relaunch-output parse failure are preserved in
`/private/tmp/panelink-p5-crash-r1.log`, `-r2.log`, and `-backtrace.log`.

## Evidence limits

This test covers abrupt process death after independently confirmed committed
files. It does not prove physical reboot, power-loss durability or its loss
bound, filesystem/controller behavior, native Apple Terminal interaction,
host clipboard behavior, or cleanup of arbitrary detached descendants. One
pane is the exercised topology; it is not full Phase 5 completion.
