# Phase 6 measurements — partial — 2026-10-22

Phase 6 remains OPEN. The ten-pane harness was repaired and two sequential
release observations are recorded below; these are synthetic proxy measurements,
not the SPEC ten-agent acceptance and not budgets.

## Build/resource (completed)

`scripts/measure_build.py`, two runs, each in a fresh private
CARGO_TARGET_DIR, on Apple Mac16,8 (M4 Pro, 12 cores, 24 GiB), macOS 26.6.1,
Rust 1.99.1. Raw: [p6-raw/build-measurements.json](p6-raw/build-measurements.json).

| Metric | Run 1 | Run 2 |
| --- | --- | --- |
| Clean release build | 11.39 s | 6.74 s |
| Incremental rebuild after touching src/lib.rs | 2.14 s | 1.75 s |
| Release binary | 2,069,888 B | 2,069,888 B |
| Copy after `strip` | 2,069,944 B | 2,069,944 B |

The first clean build likely included cold-cache effects; two samples are
observations, not a budget. `strip` did not reduce size (the release binary
is already effectively stripped); recorded as observed.

## Ten-pane synthetic workload — completed bounded proxy observations

`examples/ten_pane_workload.rs` ran twice sequentially in release on the current
Apple Silicon macOS host. Each run started five independent two-pane workspace
and storage domains in one process, with one paced synthetic producer child per
pane, history and saved layout enabled, and a 30-second measured interval.
Input ownership is claimed and confirmed per pane; the harness uses the
working live-history benchmark's `q` + Enter input and child acknowledgment
marker protocol. The driver also sends read-only layout queries under 800 ms
to keep its bounded one-second idle connection alive. Input is never retried.
Raw per-workspace latency/resource samples are in
[run 1](p6-raw/ten-pane-run1.json) and
[run 2](p6-raw/ten-pane-run2.json).

**SYNTHETIC proxy only:** this is not ten OpenCode agents, is not the SPEC
R-03 ten-agent acceptance, and establishes no latency/resource budget. CPU is
the sum of `ps %cpu` for the workload process plus ten producer children, sampled
once per second; RSS is their summed resident bytes. CPU values may exceed 100%
on multi-core systems. Each row below reports the range across the five
independent workspace results where per-workspace metrics apply; CPU/RSS are
aggregate process-group observations.

| Run | Owner tick p50 / p95 / max (µs; per-workspace range) | Child ack p50 / p95 / max (µs; per-workspace range) | RSS min–max (MiB; aggregate) | CPU mean; min–max % (aggregate) | History / layout flags | Shutdown (ms; per-workspace range) | Child/server cleanup |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 101–114 / 151–178 / 713–7,224 | 39,691–41,405 / 47,270–49,504 / 47,270–49,504 | 29.57–43.50 | 28.64; 12.10–36.50 | 5/5 true / 5/5 true | 113–255 | Verified stopped: true |
| 2 | 98–112 / 129–160 / 1,706–8,738 | 89,037–97,842 / 126,067–138,386 / 126,067–138,386 | 29.37–46.36 | 25.35; 15.00–32.40 | 5/5 true / 5/5 true | 277–283 | Verified stopped: true |

The harness verified every producer PID exited after each run; a subsequent
`ps -axo pid,ppid,stat,command` process listing contained no workload child or
Panelink server process. Per-second RSS and CPU records are preserved in raw
JSON. The two observations differed materially in child-ack latency and shutdown
time, so they remain observations only; do not infer a workload budget from
them.

The connection failure in the prior run came from the runtime's bounded
one-second idle read timeout, not the child stdin reader or acknowledgment path.
Periodic read-only layout queries keep the synthetic client attached while
retaining nonreplayable input semantics. Both corrected full runs completed all
ten child acknowledgments per workspace (five probes per pane), confirmed
history/layout storage flags, and verified cleanup.

## Tests

`python3 -m unittest tests.test_phase6_scripts` OK.

## Verification and known intermittent tests

Three consecutive verifier runs after this slice: run 3 passed 8/8 (983
passed/0 failed/15 ignored); runs 1-2 each failed one timing-sensitive lib
test (`multi_runtime::tests::blocked_history_writer_does_not_block_pty_input_or_socket_control`
with a socket EOF) under high machine load (load average up to ~13). On the
committed baseline (d9b2d78, no slice changes) six lib runs reproduced
intermittent `server_startup` singleton-reopen (2/6) and `history_capture`
end-intent timeout (1/6) failures. The parent added a bounded fork-to-exec
reopen retry to the `server_startup` test (same pattern as
`server_storage::reopen_after_drop`). These flakes are recorded, not hidden;
a dedicated test-robustness task is recommended.
