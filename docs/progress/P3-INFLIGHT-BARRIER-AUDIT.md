# P3 in-flight input client death barrier audit

Status: design audit only; no source or test changes. Recommended acceptance
fixture: one new `tests/p3_inflight_client.rs` integration test, with no
production hook and no dependency change.

## Finding

The existing acceptance covers adjacent pieces, but not a real input CLI
process dying at a deterministic post-effect/pre-response point. In
`tests/p3_detach.rs`, `logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay`
calls `StateOwner::handle_control` in-process, receives its response bytes,
then drops them and the context. `tests/p3_socket_detach.rs`,
`socket_loss_status_reconciliation_and_detach_preserve_one_server_owned_pty`,
uses a real framed Unix socket and drops the client stream after writing the
Submit frame, but it does not own or kill a `panelink control submit` process.
Its server driver then continues until the socket worker closes. Both tests
already verify operation status, durable context reconciliation, same-child
identity, and one application marker.

The real control CLI has the needed contract: `control_input` calls
`ContextFile::reserve()` before `exchange`; `reserve` durably records the exact
uncertain `OperationKey`; `handle_reserved_response` acknowledges only after
matching the response key; `status SEQUENCE` reports that key's status without
clearing context; `resync` queries the exact uncertain key and clears it only
for a terminal or NotAdmitted status. `ContextFile::open_existing` reopens the
same locator and `context().uncertain()` exposes the durable key. These are
enough to test process death and recovery without changing application code.

## Deterministic fixture

Add one test in `tests/p3_inflight_client.rs`, following the isolated real
server/process setup in `tests/p3_pane_run.rs` and the bounded marker polling in
`tests/p3_socket_detach.rs`:

1. Start one real `panelink serve --allow-input` fixture and its `/bin/sh`
   child. The child prints `READY:<pid>`, counts only a unique `TOKEN` line,
   prints `GOT:<pid>:<count>`, and handles a later unique `CHECK` line by
   printing `CHECK:<pid>:<count>`. Keep fixture state, home, terminfo and
   context in a private temporary root, as the existing real CLI fixtures do.
2. Use the ordinary `panelink control init`, `describe`, and `claim` CLI calls
   directly against the real server directory. Save the exact target,
   generation, lease, owner revision and context path. Confirm the ready child
   PID before input.
3. Create a second private directory containing a test-owned private
   `control.sock` listener. Run only the submitting real CLI process with this
   directory as `--directory` and the same existing `--context` path. The
   proxy accepts it with `AuthenticatedStream`, connects upstream with
   `AuthenticatedStream::connect(real_server_directory, deadline)`, and
   relays the ordinary Hello/Welcome and bounded Request/Response frames.
   Decode the Submit request to remember its key; relay it unchanged. Decode
   the matching upstream response and require
   `ResponseBody::Status(Completed(Applied))`. Signal that exact key through a
   bounded channel, but hold that response frame without writing it to the
   submitting client.
4. On that channel barrier, immediately send SIGKILL to the owned CLI child and
   reap it, then tell the proxy to drop the held response and close both
   streams. Do not wait for the CLI's two-second `CLIENT_DEADLINE`; that timeout
   is why the test kills as soon as the barrier arrives. Assert the child did
   not exit successfully. Reopen the exact context with
   `ContextFile::open_existing`; assert its uncertain key equals the captured
   Submit key, record `next_sequence`, and drop the handle before invoking
   another CLI command (the file lock is exclusive).
5. Run `control status <sequence>` against the real server and require the
   same key with `completed` / `applied`. Reopen the context and verify it is
   still uncertain. Then run `control resync`; require that same key and
   terminal status, reopen again, and verify uncertainty cleared while
   `next_sequence` is unchanged. No Submit payload or duplicate key is sent in
   this recovery path.
6. Reconcile the same full target through the real control CLI and require
   `READY:<pid>` and exactly one `GOT:<pid>:1`. Send one distinct `CHECK` input
   through the real CLI and reconcile until `CHECK:<pid>:1` appears; require
   exactly one `GOT:<pid>:1` and the original PID. This ordered marker proves
   the shell consumed the one token once and had no delayed duplicate. Stop
   the server and reap all fixture processes in bounded cleanup.

The proxy may accept connections on a small listener loop and handle each
connection on its own bounded worker so the test can issue observation CLI
commands while the Submit response is held. The setup commands can connect
directly to the real server directory, so only the Submit CLI needs the proxy.
This avoids replacing or mutating the real server endpoint. Use the existing
`PrivateListener`, `AuthenticatedStream`, `Frame`, `MessageKind`, and
`decode_response_payload` interfaces; preserve request IDs and relay full
frames. A bounded channel barrier replaces timing sleeps. Use an explicit
cleanup path to release the proxy worker even when an assertion fails.

## Evidence boundary and feasibility

The matching upstream `Completed(Applied)` response is the deterministic
server-side completion barrier; because the proxy captures it before
withholding it, the test is not merely observing an admitted or still-pending
operation. The child's `GOT` and later `CHECK` markers independently confirm
the effect reached the child exactly once. The killed process is therefore
known to have sent its request and the server is known to have completed it;
this scenario does not claim to kill a client before request delivery or
between admission and completion. A separate pending-operation barrier would
be needed to make that different claim.

The approach is feasible with current APIs and framing. `control.sock` is the
fixed endpoint under the directory passed to the CLI; `AuthenticatedStream`
validates the private directory/socket and same-user peer. The proxy must use
private directory/socket permissions so normal CLI validation succeeds. The
existing integration tests are gated to macOS arm64; this acceptance should
follow that gate and report host/environment limitations honestly. No test was
run for this read-only design audit.
