# P3 killed input-client acceptance review

Review scope: `tests/p3_inflight_client.rs` against
`docs/progress/P3-INFLIGHT-CLIENT-TASK.md`. Source review only; no source edits,
test runs, or commits by this reviewer.

## Result

The fixture now tests the requested post-completion/pre-client-response
boundary with a real `panelink control submit` process. The barrier is strong:
the proxy decodes the exact Submit target, owner revision and text, forwards the
original frame once, then requires the upstream response to match its request
ID and operation key with `Completed(Applied)`. It retains that response frame
without writing it to the client and only then notifies the test. The test
checks the CLI is still alive, sends SIGKILL to that owned `Child`, reaps it,
and verifies the exit signal.

The durable recovery evidence also matches the task. `ContextFile` is reopened
after the killed process releases its lock; generation, lease, exact uncertain
key and the one-step sequence advance are checked. The read-only `status`
command reports `Completed(Applied)` and leaves the context uncertain. `resync`
then clears that exact key without advancing the sequence; a second resync
reports clear. No duplicate Submit is sent. A separate observer sees the same
full target and child PID, the private append record contains one token, and an
ordered `CHECK` through the recovered caller reports a handled count of one.

The earlier lifecycle concerns are addressed in the current source snapshot:
the serve `Child` is stored in the fixture before startup polling, so an early
assertion reaches fixture cleanup; `Fixture::drop` uses non-panicking bounded
SIGTERM/SIGKILL polling instead of the panic-on-timeout wait helper; normal
shutdown signals and reaps that exact `Child` and checks the endpoint was
removed. The normal success path calls `release_and_join(true)`, which requires
the proxy worker to finish and join. On assertion unwind, `Proxy::drop` signals
cancellation and release, waits up to five seconds, and only detaches as a last
bounded fallback. The proxy's accept, frame I/O and held-response waits have
explicit deadlines, and the owned submit child is dropped before the proxy,
which closes its client stream. The normal acceptance path therefore proves
full worker termination; the exceptional fallback remains a cleanup guard for
a worker that violates those deadlines.

The worker's restricted run failed at server startup with
`listener_permission_denied`, before reaching the proxy or acceptance
assertions. Parent-run attempts then reached the repeated-resync assertion and
later the shutdown path; those brittle assertion/cleanup issues were corrected
in the current source. The corrected parent targeted rerun subsequently passed1/1 in0.84s; its
combined verification is recorded separately in P3-INFLIGHT-CLIENT-RESULT.md. The initial listener denial is an
environment-blocked run, not a pass or a failure of the scenario. No remaining
source-level barrier, context-recovery, counter, identity, SIGKILL, or bounded
wait gap was found in this review.

## Scope limits

This proves a client killed after the server emitted a terminal Applied
response and before the proxy delivered it. It does not exercise client death
before request delivery or while the server still reports `InProgress`. It
also does not establish arbitrary descendant cleanup after forced server kill;
the test only claims explicit stop and bounded owned-process cleanup.

Full Phase 3 remains open. The parent targeted run used macOS arm64 with socket/PTY access; this reviewer
ran no tests. Do not promote its result to other platforms or untested
request stages.
