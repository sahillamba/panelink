# Phase 3 killed input-client acceptance — bounded contract

2026-10-23. Parent design under the user's next-slice request. Preserve all
prior uncommitted work. Use native Luna High for exclusive test implementation
and independent review, with parent verification. No new input/retry policy,
production hook, wire/storage format or dependency is authorized by this task.

## Boundary

Test a real short-lived `panelink control submit` process killed after the
server has produced a validated terminal Completed(Applied) response, but
before that response reaches the client. The request is still in flight from
the caller's perspective; the server operation is already terminal. Do not
claim this covers death before admission or while the owner is still applying
input. Existing socket-loss/logical lost-ack tests cover related boundaries.
The concrete new evidence is OS process death plus durable uncertain-context
recovery without resending the payload.

## Test design

Own ONLY new tests/p3_inflight_client.rs. Reuse existing transport/context
APIs and fixture patterns; avoid changing production code or existing tests.

- Private mode0700 fixture, isolated terminfo/runtime/proxy directories and
  explicit env/cwd. Start the actual serve binary with allow-input and a
  controlled /bin/sh loop on its real PTY. Disable echo, write readiness/PID,
  count a unique token and write one private append record for each handled
  token. A separate CHECK command reports total count. Never use echoed input
  as the effect barrier. Bound process/startup/output waits and diagnostics.
- Initialize and claim a persistent control context directly against the
  real server. Record full target, generation/lease and owner revision.
- Bind one test-owned PrivateListener in the proxy directory. Accept the real
  submit CLI via AuthenticatedStream, connect to the real server, and relay
  its frames without changing payload/key/request ID. No subscriptions or
  Event frames are needed. Detect the encoded Command::Submit, capture its
  key/target and forward it exactly once. Validate the matching response is
  terminal Completed(Applied). Hold that response in memory; never write it to
  the caller. Notify the parent through a bounded channel with the exact key.
- The parent waits for that protocol barrier, verifies its submitting child
  is still running, SIGKILLs only that owned Child and reaps it with a signal
  exit. Do not SIGSTOP it or widen its timeout: normal control exchange is
  bounded, so promptly signal once the response is held. A timeout/ordinary
  exit is a test failure, not equivalent to SIGKILL evidence. Proxy teardown
  is bounded and drops held bytes/streams without sending the result.
- After the killed process releases the context lock, reopen the same file
  and assert its uncertain key equals the captured submitted key; next
  sequence advanced exactly once for that operation. Do not reserve a new key
  or resend to establish the effect. Observe the controlled child's token
  marker/private count and same PID/full target using a separate observer.
- Reopen through the actual direct `control resync` CLI: read-only status of
  that exact uncertain key returns terminal Applied and clears uncertainty.
  Generation/lease and next sequence stay unchanged. Another resync reports
  clear and sends no input. The private append record still contains one
  handled token. Explicit CHECK through the now-recovered caller reports1;
  verify no repeated token marker, same child/target and live server. Stop the
  server explicitly; observe cleanup without claiming arbitrary descendants.

All mutable resources need bounded failure cleanup. Use Child handles for
signals, never arbitrary persisted PIDs; proxy waits/channels must terminate
on assertion unwind. Capture outputs within fixed bounds, no unbounded read
or thread join. No token/credential contents in production logs; fixture
payloads are synthetic.

## Verification and scope

Worker runs only:
`cargo test --locked --offline --test p3_inflight_client -- --test-threads=1`
with private sockets/PTYs permitted, and reports actual output. Sandbox bind
failure before assertions is not a pass. Parent independently reviews/reruns,
then complete locked/offline serial Cargo and seven supporting checks after
source freeze. Use 600s full-suite limit separately from the standard180s
verifier; preserve every failure/timeout and ignored status.

No human four-pane host, server-death/in-flight owner cleanup, new-generation
retry, provider or reboot recovery claim. FullPhase3 stays open. Parent owns
shared docs and corrections; workers do not commit. No push authorization.
