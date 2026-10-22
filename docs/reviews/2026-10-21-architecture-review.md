# Panelink architecture and code-quality review

Reviewed 2026-10-21 at commit `8f14006e23520c30d13a8e289ecf4ce5866fbe0b`.
The working tree was clean before review. This report changes no application
code and does not advance the implementation roadmap.

Follow-up2026-10-22: the findings below describe the reviewed revision.
[Wave1](../progress/USABILITY-WAVE1-RESULT.md) caps the discard drain, and
[Wave2](../progress/WAVE2-RESULT.md) isolates active client IO and advances live
close across owner ticks. The launch/journal portion of finding1 remains open;
startup client connection and sibling IO serialization remain limitations of
the scoped finding2 fix. Runtime duplication remains technical debt. See the
dated integration reports for actual tests and unproved host/provider gates.

## Assessment

The core has deliberate safety contracts: shared operation admission,
nonreplayed input, explicit child environments, bounded protocol messages,
server-owned PTYs, sanitized screen rendering, and separated history storage.
The tests exercise many failure and uncertainty cases. The main architectural
weakness is accumulated synchronous orchestration in the server and interactive
client. Those boundaries need attention before increasing pane counts or
claiming predictable responsiveness.

Three substantial findings follow. Style, naming, small helpers, unfinished
roadmap features, and already documented history-loss limits are excluded.
These findings come from source inspection; this review did not reproduce
worst-case latency or a continuously saturated cleanup hang.

## 1. High: closing or launching one pane blocks progress for every pane

Evidence: [owner queue](../../src/multi_runtime.rs#L531),
[close transaction](../../src/server/multi_session.rs#L1741),
[PTY stop](../../src/server_pty.rs#L1042),
[split dispatch](../../src/multi_runtime.rs#L1002), and
[journal writes](../../src/server_storage.rs#L469).

`MultiServerDriver::tick` polls the PTYs, then executes queued requests
synchronously. A close invokes `effects.stop()` inside that request. The stop
waits through HUP/KILL cleanup windows on the same thread, so the surviving
pane receives no normal polling, input dispatch, or screen-query service
throughout that work. The runtime explicitly allows about two seconds for
this stop. A busy sibling can fill its PTY buffer while the owner is occupied.

Splits have the same architectural exposure to slower operations: the owner
opens/loads the launch journal, prepares the launch, spawns the child, and
records lifecycle changes synchronously. Each journal record calls
`sync_all` and `F_FULLFSYNC`. Filesystem calls have no wall-clock deadline.
Moving history writes into a worker has not isolated these other storage calls.

Increasing the close reply deadline only helps the requesting worker wait;
it does not let the other panes make progress. The interactive client's
ordinary 400 ms exchange deadline is substantially shorter than a close's
allowed stop window, creating a path to unrelated pane disconnections.

Recommendation: keep state changes serialized, but represent stopping and
launching as in-progress transactions. Advance child-stop phases across ticks;
perform potentially blocking preparation/journal work through bounded owned
jobs and return results to the owner. Preserve Intent-before-spawn ordering,
single ownership of acquired children, deduplication, and unknown outcomes.
Verify that a HUP-ignoring pane can close while its sibling continues handling
input, output, and control queries.

## 2. High: interactive rendering and input share blocking network and disk work

Evidence: [host event loop](../../src/ui_client/host.rs#L385),
[network polling](../../src/ui_client/net.rs#L268),
[request exchange](../../src/ui_client/conn.rs#L374),
[key reservation](../../src/ui_client/conn.rs#L429), and
[durable context write](../../src/client_context.rs#L398).

The host loop calls network operations directly before rendering. Polling can
perform maintenance, layout fetches, resize, ownership claims, watches, and
screen fetches sequentially. Each exchange has its own timeout, rather than
one total UI-loop budget. A slow pane therefore delays both local interaction
and displaying already available updates from another pane. Explicit split
and close actions permit still longer synchronous waits.

Keyed input also reserves its key by writing and fully flushing the client
context before transmission. Acknowledgment writes and fully flushes it again.
These operations run on the host loop. A 400 ms socket deadline does not bound
`F_FULLFSYNC`, so delayed storage can stall typing, local navigation, detach-key
recognition, rendering, and signal handling beyond the advertised exchange
budget. Normal successful input batches pay this durability cost too; its
actual latency on the target Mac was not measured here.

Recommendation: give connection/context work a separate bounded owner and
deliver results to the host loop through bounded queues, or implement
incremental nonblocking exchanges. Keep the durability barrier before send and
never replay uncertain input. The host loop must remain able to render,
recognize detach, and restore the terminal while those operations are pending.
Test with delayed socket replies and an injected slow context flush, including
two panes where only one is delayed.

## 3. Medium: a cleanup drain can bypass the shutdown deadline

Evidence: [deadline loop](../../src/terminal/mod.rs#L1195) and
[output discard loop](../../src/terminal/mod.rs#L1219).

`wait_bounded` calls `discard_queued_output` before it checks the deadline.
The discard helper repeatedly reads 4 KiB chunks until EOF, an error, or
`WouldBlock`; successful reads have no byte, iteration, or time limit.
A child that keeps the PTY readable can therefore postpone the deadline check
and the later SIGKILL escalation. Nonblocking reads bound each individual
read's waiting behavior, not the total work of this loop.

This affects ordinary `TerminalSession::shutdown` and Drop/probe paths using
discard mode. Explicit multi-pane close selects the separate final-output
mode, whose drain performs one bounded read per iteration; this finding does
not assert that explicit close uses the unbounded discard helper.

Recommendation: cap bytes/reads per cleanup iteration, then check the shared
deadline and child status before continuing. Retain the output draining needed
for macOS controlling-TTY teardown. Add a deterministic continuously readable
reader regression plus a real child that ignores HUP and writes continuously;
verify that escalation and return still occur within the cleanup allowance.

## Code-quality concern to address during those changes

The clearest avoidable duplication is the parallel single-child and multi-pane
runtime infrastructure. [server_io.rs](../../src/server_io.rs#L311) and
[multi_runtime.rs](../../src/multi_runtime.rs#L1584) each implement worker
handshakes, request sequencing, schema/reply validation, owner submission,
deadlines, and frame writes. Their corresponding drivers also repeat queue and
pending-watch management. Changes to transport/lifecycle guarantees must be
reviewed in both paths.

The multi-pane runtime's own module documentation identifies it as a parallel
runtime. Consolidate shared worker/envelope/deadline mechanics when addressing
the findings above, with schema-specific dispatch kept explicit. This warrants
a focused refactor; a broad rewrite or a new general framework is not justified
by this review. Large file size alone was not treated as a finding, particularly
where files include extensive tests.

## Verification and limits

Fresh checks from this review:

- `cargo test --locked --offline --lib -- --test-threads=1`:
  **551 passed, 0 failed, 4 ignored**.
- `cargo test --locked --offline --bin panelink -- --test-threads=1`:
  **94 passed, 1 failed, 0 ignored**. The failure was
  `default_open::tests::endpoint_state_classifies_files_and_sockets`, at
  `src/default_open.rs:1195`, where pathname socket creation returned
  `EPERM` / `Operation not permitted`. This matches the documented sandbox
  restriction; it is not counted as a pass.

Review concentrated on server ownership/dispatch, lifecycle cleanup, the
interactive connection/event loop, launch durability, and history
capture/writer/store boundaries. A separate read-only history review was
delegated to GPT-6-Luna at xhigh, with parent assessment of the findings.
That scoped review found no independently confirmed high-impact history defect.
The code graph was used for discovery; exact source was used to validate
behavior because some graph call edges resolve shared names incorrectly.

This was not an exhaustive line-by-line review, a dependency/advisory audit,
a full integration-suite rerun, or host/provider compatibility certification.
Previously recorded history payload loss, bounded best-effort shutdown,
missing saved-layout implementation, and unproved provider recovery remain
existing limitations rather than newly discovered findings.

Priority: isolate blocking work from the server and host loops, then fix the
discard-loop bound. Preserve the existing safety contracts during that work.
