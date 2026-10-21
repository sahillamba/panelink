# P5 read-only restart acceptance — 2026-10-21

## Verified scope

`tests/p5_history_recovery.rs` adds one bounded macOS/Apple Silicon integration
scenario for the read-only history portion of V-HISTORY. It runs a real private
PTY `TerminalSession` through `PtyEffects` and the public `HistoryStore` API,
then invokes the freshly built `panelink history show` and `search` commands in
separate processes. All files and the shell used by the test live beneath a
mode-0700 fixture in `/private/tmp`; the user's `PANELINK_HOME` is never read.

The PTY first emits enough normal rows to scroll `NORMAL-ROW-03` into retained
scrollback and blocks at a marker. The test advances the same raw
`TerminalSession` through alternate screen A, checks its visible marker, then
clears and replaces it with B and waits for a marker after a synthetic OSC 52
canary. Only after B is visible does the test construct `PtyEffects` and enable
capture. This gate ordering makes A-before-B a real observed transition while
ensuring no checkpoint writer can capture A. It then returns to normal screen,
captures the final normal checkpoint, finishes and stops/reaps the child, and
drops the writer/store handles before running the CLI.

The fresh CLI reads the retained normal row and separately labeled latest
normal and alternate checkpoints. The alternate checkpoint and CLI contain B;
they do not contain A. CLI stdout and stderr contain no ESC byte, OSC 52
introducer, or synthetic canary payload. The fixture configures `SHELL` as a
marker script that exits immediately if the CLI takes the workspace-launch
path. The marker remains absent, the storage tree is byte/mtime-identical
across each read, and no server PID, socket, lock, generation, or log appears.
The PTY only emits a synthetic clipboard request; the test performs no system
clipboard access. Marker waits and fresh CLI subprocesses have six-second
deadlines; a timed-out CLI process group is killed and reaped.

## Verification

Commands and actual results:

```text
$ cargo test --locked --offline --test p5_history_recovery -- --test-threads=1
test fresh_history_cli_reads_only_committed_rows_and_latest_checkpoints ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; finished in 0.60s
exit code: 0

$ cargo fmt --check
exit code: 0
```

No dependency or production-code changes were made for this acceptance slice.
The test is target-gated because `PtyEffects` is available on Apple Silicon
macOS only.

## Remaining acceptance gaps

This proves a stopped PTY's committed store can be read by fresh CLI processes;
it is process/store restart-read evidence, not an actual machine reboot test.
It does not prove layout restoration, process or provider conversation resume,
capture of output transitions that the PTY engine never observed, recovery of
older overwritten alternate-screen content, five-second checkpoint cadence,
partial-tail repair, or representative disk throughput. Post-rename and
failed-end-sync retry coverage is delivered separately in
[P5-RECOVERY-FAULTS.md](P5-RECOVERY-FAULTS.md). Full V-HISTORY and Phase 5
remain open; current integrated failures are in [SESSION.md](SESSION.md).
