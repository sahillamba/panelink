# Managed pane run implementation review

2026-10-23. Independent source review of `src/p3_cli.rs`'s `WorkspaceLink::run_pane`, `src/agent_cli/pane_run.rs`, and its raw-argument dispatch in `src/agent_cli.rs`, against `P3-PANE-RUN-TASK.md`. The codebase-memory lookup was attempted first but the connector returned `Transport closed`; source files and the accepted contract were used directly. No tests were run during this review.

## Findings and follow-up

- **Original P2 finding resolved:** `RunPaneFailure` now preserves the exact reserved operation key, and the CLI emits a pane-run error envelope with action, explicit outcome, operation, existing target, and fixed error/message. Transport, response-validation, and acknowledgement errors report `unknown`; an authoritative `NotAdmitted` rejection reports `failed`. The revised task contract documents null target and zero revision as unobserved. No argv, cwd, or environment values enter this envelope.
- **Follow-up P2 finding resolved:** `RunPaneFailure` now carries an optional validated `SplitResult`. The transport path sets it only after the response key and existing target match and the outcome is terminal, then attempts the durable context acknowledgment. If acknowledgment fails, the CLI preserves the observed new target and committed revision in its `unknown` error envelope; null/zero remain reserved for cases without a validated terminal result. The deterministic test removes the context pathname after reservation and request receipt, then checks that the exact key, child identity, and committed revision survive the failed acknowledgment. The context remains uncertain. No further actionable findings remain in this scoped review.

## Reviewed behavior

The parser dispatches `pane run` from the raw `OsString` argument vector before the legacy UTF-8 conversion. It treats the first `--` as the boundary, leaves subsequent command arguments literal, preserves Unix path and argument bytes, validates option UTF-8 and caller names, and checks the token and aggregate-byte bounds before opening the caller workspace. Relative cwd is resolved against the invoking process cwd.

The CLI captures the spatial target and layout revision, rechecks the same index, then opens a private per-caller/per-full-target run context. `WorkspaceLink::open_for_target` validates an existing context against the exact target; the launch method checks the negotiated INPUT capability and refuses unresolved or just-settled uncertainty before preparing a new operation. It queries layout again, requires the captured revision and full target to remain present, and computes geometry before encoding.

The split encoder validates the complete request size before reservation. The client reserves the durable key, sends the encoded request once, drops the payload before output, checks the returned key and existing target, and leaves uncertainty intact when response validation or acknowledgement fails. The server split authorization requires a live connection, INPUT capability, matching lease, and exact target scope without requiring input ownership. The existing launch worker prepares from the captured cwd/environment and journals launch intent; no shell command is inserted into an existing pane. Environment filtering excludes `PANELINK_*`, enforces the existing count/byte bounds, and diagnostics shown by the reviewed path use fixed messages without argv, cwd, or environment values.

## Evidence boundary

The parent reports that the real CLI target passed 3/3 before this follow-up correction; this review did not rerun tests. The parent says a corrected full run will follow, so the new acknowledgement-failure test and corrected tree still await that run. No host-terminal, broader lifecycle, crash-recovery, or provider-identity acceptance is established here. This source review does not claim pane-run task readiness or Phase 3 completion.

## Parent final verification

The final compact error representation carries only validated target/revision
metadata, preserving the reviewed behavior without the larger full reply value.
Final frozen-source serial suite1084passed/0failed/15ignored in148.24s; the
ack-failure transport regression and JSON test pass. All seven non-Cargo checks
pass separately, including strict all-target Clippy; docs256/0. See
[P3-PANE-RUN-RESULT.md](P3-PANE-RUN-RESULT.md) for distinct intermediate runs.
Broader acceptance gates remain open as described above.
