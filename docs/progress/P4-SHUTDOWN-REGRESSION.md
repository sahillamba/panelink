# Phase 4 shutdown fixture regression — 2026-10-24

The focused real-child shutdown regression failed once in the full run because
the second shell did not publish its HUP marker. The fixture had installed a
trap that invoked external `touch` and `/bin/sleep` commands. While a shell was
waiting on a foreground external sleep, signal delivery could interrupt or
delay the helper process and make the fixture report a missed HUP even when the
driver's shutdown behavior was correct.

The fixture now writes PID, readiness, HUP, and peer-missing markers with shell
builtin `printf`. Its trap waits for the other child's HUP marker using the
shell's timed builtin `read`, and the child remains in a timed builtin-read loop
so HUP is serviced by the shell without a foreground external process. The
40-iteration peer window, both peer-missing checks, direct-child ESRCH reap
assertions, and 800 ms–3 s shutdown-duration bounds are unchanged. The test
still fails if either HUP misses the peer window or either child remains alive.

Validation used the isolated Panelink Cargo cache and target directories. The
focused macOS arm64 integration test passed five consecutive serial runs:

```text
cargo test --locked --offline --test w2_server_shutdown -- --test-threads=1
5 runs: 1 passed, 0 failed each; individual test times 1.05–1.09 s
```

The codebase-memory service was unavailable (`Transport closed`); the diagnosis
came from inspecting the fixture and the current `MultiServerDriver::drop`
shutdown loop. This regression only validates the fixture's two-child HUP
ordering and process cleanup on the supported host. It does not expand the
scope of shutdown acceptance beyond the scenarios described in
`docs/progress/W2-SHUTDOWN-TEST.md`.
