# Phase 3 pre-admission client-death acceptance

2026-10-23. This is the next bounded acceptance slice, specified from the
existing client-context and operation-ledger contracts. It adds automated
evidence only: no production hook, input/retry policy, provider identity,
wire/storage format or dependency change.

## Contract boundary

Exercise a real `panelink control submit` process after its persistent context
has reserved the operation key and after a test-owned authenticated proxy has
decoded and validated the exact Submit frame, but before forwarding that frame
to the real server. At this barrier the server has not received or dispatched
the Submit. Assert the exact owned CLI Child is still running, SIGKILL and reap
that Child, then release the proxy by dropping the held frame and streams. Do
not SIGSTOP, wait for a client timeout, forward the held Submit, or infer the
barrier from elapsed time. The killed Child must be the one started by the
fixture and must exit by signal. This requires no production hook.

## Required observations

- At the proxy barrier, capture the Submit operation key and verify the target
  matches the fixture's original full target. The proxy must not write the
  Submit frame upstream.
- After the killed client's context lock is released, reopen the same context
  and verify its uncertain key is exactly the captured key and its next
  sequence advanced once. This is caller-local durable state, not evidence of
  server admission.
- Through the still-live active lease, query status for that exact key. The
  server ledger must report `NotAdmitted`; `NotFound` is not the protocol
  result for this valid, above-high-water key. Resync that same context and
  verify it clears uncertainty with unchanged generation, lease and next
  sequence. A repeated resync remains clear and sends no Submit.
- Use a separate explicit CHECK against the original controlled shell to
  verify its unique token count remains zero, and verify the original target
  and child PID remain live. Do not use terminal echo as a side-effect
  assertion. Client context, server status and shell effect are separate
  evidence sources.
- Stop fixture-owned processes explicitly and check their observed cleanup.

`ContextFile::reserve` persists uncertainty before transport exchange.
`GlobalAuthority::operation_status` reads the lease-scoped ledger, where an
unknown sequence above the active lease high-water mark is `NotAdmitted`.
`ContextFile::reconcile` clears uncertainty on that terminal non-admission
result; it preserves uncertainty for `InProgress`. The proxy barrier proves
the server never received the Submit, so it makes no claim about server-side
validation or dispatch. `SessionHandler::handle_input` clones the owner,
validates, admits the ledger key, commits the owner pin, applies input,
completes the ledger entry and clears the pin. A future in-process acceptance
is needed to pause between server dispatch and ledger admission without adding
a production hook.

## Scope limits and implementation ownership

This acceptance covers death after durable key reservation and before server
delivery/admission. The existing `P3-INFLIGHT-CLIENT-RESULT.md` separately
covers death after terminal `Completed(Applied)` is held before caller
acknowledgment. Neither proves death after server dispatch but before
admission, death after admission while the live owner pin is pending or the PTY
is applying input, server-crash cleanup for an in-flight owner, human Apple
Terminal four-pane acceptance, provider identity/recovery, or reboot recovery.
Keep those as separate gates; do not add any automatic resend or new operation
key.

Implementation should be fixture-only in `tests/p3_inflight_client.rs`, using
the existing private listener, authenticated transport, bounded channels and
owned Child cleanup. Parent review must verify the exact decoded Submit is
captured before any upstream write, the held frame is dropped on success and
failure cleanup, and status/resync/check traffic is served directly by the
real server. Client-context, server-status and shell-effect observations must
remain independent sources.
