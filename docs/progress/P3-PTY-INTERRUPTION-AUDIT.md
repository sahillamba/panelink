# Phase 3 admitted input and PTY interruption audit

2026-10-24. Read-only source/test audit plus this new evidence note. No source,
test, or shared planning file was changed. The codebase-memory index was used
first for discovery; source and tests below are authoritative for the current
checkout.

## Finding

An admitted Submit/Text/Paste is executed synchronously on the serialized
server-owner path. The request is first validated, admitted into the operation
ledger, and pinned to the current input owner; then `EffectSink::input` runs,
its terminal outcome is recorded, and the pin is cleared. Closing a client
socket can discard that worker's reply receiver, but it does not cancel this
already-admitted owner call. This is consistent with the ledger contract that
accepted work may complete after lease expiry and that exact duplicate payloads
observe the existing result instead of executing again.

`PtyEffects` atomically queues one bounded request and drains it within a
100-ms window. Each PTY pump accounts for the bytes actually written. At exit,
the unwritten suffix is discarded in every case: all bytes written maps to
`Applied`, a nonzero prefix maps to `Partial`, zero written maps to `Failed`,
and loss of trustworthy progress maps to `Unknown`. The outcome is terminal
and retained against the exact operation key, so reconnect/status can
reconcile it and an exact duplicate cannot replay the input. A caller must not
interpret `Failed` as proof that no side effect occurred for other operation
types; this precise zero-byte claim applies to this PTY drain path, while
`Partial` and `Unknown` remain explicit uncertainty states.

## Existing evidence

- `src/server/multi_session.rs:2094-2124` shows owner pin candidate, validation,
  ledger admission, pin commit, synchronous effect, terminal ledger completion,
  and pin clear in that order.
- `src/server_pty.rs:881-918,920-971,1039-1045` contains input dispatch,
  atomic enqueue, the bounded drain, suffix discard, outcome mapping, and the
  `EffectSink` entry point. `src/server_pty.rs:70-71` sets the drain and poll
  windows.
- `src/protocol.rs:555-562,573-601` checks exact payload identity for a known
  operation and permits accepted operations to be completed with a terminal
  result.
- `src/terminal/unit_tests.rs:144-205` deterministically exercises the
  lower-level queue contract: partial prefix consumption, `WouldBlock`
  preservation while the attempt is live, and suffix discard/disable after a
  hard error or zero write. This injects writer results; it does not claim a
  real PTY delivered a partial operation.
- `tests/p3_socket_detach.rs:725-757,778-851` uses a real PTY shell, drops the
  Submit acknowledgment, reconnects, checks terminal `Applied`, reconciles the
  same context, sends an explicit exact duplicate, and verifies one handled
  token with a subsequent ordered shell check. It proves disconnect and
  no-replay reconciliation for an operation that completes; it does not kill
  the client while the PTY drain is still running.
- `tests/p3_detach.rs:400-470` separately checks an `Applied` interrupt against
  a live PTY child and observes the child's signal output/exit.
- `src/multi_runtime.rs:2981-3160` has an existing socketpair test showing a
  disconnected worker does not cancel a server-owned close transaction. It is
  close-path coverage, not input/PTy-write evidence.
- The closure audit and prior result notes distinguish post-completion
  killed-client, pre-delivery killed-client, and the handler validation barrier
  from the still-open admitted owner-pending/PTy interval; see
  `docs/progress/P3-CLOSURE-AUDIT.md`,
  `docs/progress/P3-INFLIGHT-CLIENT-RESULT.md`, and
  `docs/progress/P3-POST-DISPATCH-FEASIBILITY.md`.

## Acceptance recommendation and limits

No additional independently owned test file is justified for this slice.
The lower-level deterministic writer test already covers prefix/suffix
accounting, the real-PTY socket-loss scenario covers terminal completion and
no-resend reconciliation. There is no delivered deterministic test of the
production server waiter's lifetime while the owner is pending. A previously
considered synthetic `StateOwner`/test-channel case was removed because it did
not exercise production waiter code; it is not evidence for this boundary.
No replacement waiter-loss test was delivered, so this remains an explicit
evidence gap.

A real-PTY test cannot deterministically force a chosen partial byte count or
kill a client at the PTY drain boundary using the current public APIs. A large
request, non-reading child, timing delay, or socket observation may produce
backpressure on a particular run, but cannot establish a stable point between
the first and final PTY write. An in-test barrier would require a production
hook or a separately designed in-process fixture; the existing CLI fixture
runs the production server in another process. Do not add a production hook or
claim partial-write/client-death acceptance from timing or synthetic writer
results.

Any future test would need to exercise the production waiter path after the
owner admits a unique key, then drop the requesting worker while owner work is
pending and verify owner completion plus same-key deduplication without a
second effect. A separate real-PTY case would still need a deterministic
OS-level mechanism to prove a partial prefix reached the child before
asserting `Partial`; current timing/backpressure alone is not such a
mechanism.

This audit does not claim real-client death during an admitted PTY write,
deterministic real-PTY partial delivery, production waiter survival while
owner work is pending, arbitrary child-read behavior, or provider recovery.
Human Apple Terminal four-pane acceptance also remains an open evidence gate;
this note records that status without assigning a manual task. The existing
integration and synthetic queue evidence remains scoped as stated above.
