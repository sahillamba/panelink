# Synthetic ten-child PTY workload — 2026-10-18

This default-runnable macOS/aarch64 integration test exercises ten real
`TerminalSession::spawn` children concurrently through Panelink's production
private-PTY path. Each child is `/bin/sh` with an explicit temporary working
directory, `TERM=panelink-probe`, and a fixed 24x80 size. Each emits 100
numbered load lines and a count marker, then waits for its own readiness/input
handshake. After all ten READY markers are visible and their exit statuses are
still unobserved, the parent sends one distinct token per child. The children
verify their own token, emit an indexed ACK and final marker, and exit. The
test checks each child's READY/load/ACK/final output, normal exit, PTY EOF, and
absence of sibling markers.

Polling is round-robin across all ten sessions. A shared 15-second I/O deadline
starts at test entry and is checked between pump rounds; each phase also has a
7,500-round bound with a 2-ms interval. The deadline cannot preempt synchronous
PTY spawn syscalls or cleanup. The fixture uses the terminal core's 16-KiB
per-pump read bound and a fixed snapshot-text collection bound for its
ASCII-only output. Its temporary working directory is created with mode 0700.
The `ChildBatch` guard calls the existing bounded `TerminalSession::shutdown`
for every still-owned child on normal return or unwind; the working directory
is dropped after the child guard.

## One measured run

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p5_synthetic_workload -- --nocapture
```

Result: 1 passed, 0 failed, 0 ignored. The test reported:

```text
P5_SYNTHETIC_WORKLOAD children=10 load_lines_per_child=100 poll_rounds=5 pump_calls=50 pty_bytes_read=16840 elapsed_ms=63 test_process_peak_rss_start_bytes=2244608 test_process_peak_rss_end_bytes=7110656
```

The host was macOS 26.5.2 (Darwin 25.5.0), arm64; Xcode 26.6 build 17F113,
macOS SDK 26.5; Rust/Cargo 1.99.1. In the installed SDK,
`usr/share/man/man2/getrusage.2` defines `RUSAGE_SELF` as the current process
and `ru_maxrss` as maximum resident set size in bytes. The matching
`usr/include/sys/resource.h` declares `RUSAGE_SELF` as zero and `ru_maxrss` as
`long`; the existing `libc 0.2.191` binding uses the matching `rusage` field and
`getrusage` signature. The test samples the test process's high-water value at
entry and after child cleanup. These are cumulative process high-water
observations, not a delta or total Panelink overhead; they exclude child
process RSS and must not be added as an aggregate. The 63-ms duration is one
sample, not a performance threshold or benchmark conclusion.

## Scope and checks

The fixture uses synthetic shells, not OpenCode, and does not enable or claim a
ten-session server/API cap, ten actual agents, durable history, restart
recovery, or provider identity. It is an automated current-host workload check,
not evidence for a minimum macOS version or other platforms. It opens actual
private PTYs and fails if that host operation is unavailable; it is not ignored
or converted to a skip.

Other checks run:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
Finished successfully with warnings denied.

rustfmt --edition 2021 --check tests/p5_synthetic_workload.rs
Passed.
```

`cargo fmt --package panelink -- --check` currently reports formatting changes
in the separately owned, in-progress `src/managed_launch.rs`; this task did not
modify that file. The package-wide check must be repeated after its owner formats
it. No dependency or product-source changes were made.

Parent review independently reran the corrected fixture: **1 passed**, ten
children, 100 lines per child, 5 rounds/50 pump calls and 16,840 bytes. That
sample took 44 ms and reported process peak RSS 2,244,608 to 7,159,808 bytes.
It has the same scope and measurement limitations as the worker sample above.
