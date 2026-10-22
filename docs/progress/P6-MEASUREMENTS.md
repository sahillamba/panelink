# Phase 6 measurements — partial — 2026-10-21

Worker: GPT-6-Luna (xhigh), interrupted once by machine sleep + LLM auth
broker timeout, resumed, then stopped by the parent after ~50 minutes stuck
on the ten-pane workload driver (17 failed patch attempts). Parent recorded
the completed parts honestly. Phase 6 remains OPEN.

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

## Ten-pane synthetic workload (NOT completed)

`examples/ten_pane_workload.rs` (five independent two-pane workspaces, two
paced producer children each, history + layout enabled) compiles after
parent fixes (a type mismatch and removal of a retry that resent input
under a new request ID, which would violate the nonreplayable-input
contract). A parent release run exited 1 after 4.5 s: every pane's child
probe acknowledgment failed. No processes leaked (checked with pgrep).
No workload numbers exist; the example's probe/ack logic needs a separate
debugging task. This would in any case be a synthetic proxy, not the
SPEC ten-agent acceptance.

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
