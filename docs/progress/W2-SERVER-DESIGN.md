# Wave 2 — nonblocking per-pane close design

Status: parent-approved for Stage 2 on 2026-10-22. This note records the
implementation constraints. Scope is the live multi-pane server close path;
async launch and interactive-client IO remain separate slices.
The scoped slice is now implemented; baseline descriptions/API sketches below
record the design stage. Exact adopted APIs and final evidence are in source,
W2-SERVER-RESULT.md and WAVE2-RESULT.md.

## Problem and target

`MultiServerDriver::tick` polls the owner and then dispatches queued requests.
Schema-9 close dispatch currently calls `MultiSessionOwner::execute_close`,
which invokes `EffectSink::stop` synchronously. A HUP-ignoring child can hold
that thread through the HUP and KILL windows, preventing sibling PTY polling,
input, and control requests. The fix keeps all layout, registry, ledger,
history-capture and lifecycle mutations on the existing owner thread, and
turns only the stop wait into work advanced by later ticks.

The live route will issue HUP at admission, poll child status on each owner
tick, issue KILL once at the one-second HUP deadline if still running, then
poll for at most the one-second KILL window. Time comes from an injected
monotonic `Instant` source in the state-machine API so tests can advance it
without sleeping. There is no wait/sleep loop on the owner. A tick has a fixed
close-advance budget (at most each active close receives one nonblocking child
status poll and one phase transition); the existing live pane bound is two.
PTY output polling continues independently before and after close advancement.

## Proposed owner API and state

Keep the legacy synchronous `execute_close` entrypoint for existing direct-core
callers and tests. Add an explicitly phased API to `MultiSessionOwner<E>`:

```rust
begin_close(payload, now, clock) -> Result<CloseAdmission, CloseError>
advance_closes(clock) -> Vec<CloseCompletion>
close_status(connection, key, now) -> Result<CloseStatus, ServerError>
```

`CloseAdmission` is either `Terminal(CloseResult)` for an exact retained
duplicate or immediate validation rejection, or `InProgress { key }` after
successful ledger admission. It never returns `Applied` until `CloseResult`
has been finalized. `CloseStatus` is an internal enum:
`NotAdmitted`, `InProgress`, or `Terminal(CloseResult)`. The existing
`OperationStatus` ledger result remains authoritative for retry/status.

The owner holds a bounded map/vector of at most
`MAX_RETAINED_CLOSE_RESULTS` transactions, sharing the existing 4096-result
capacity with retained terminal results. Each in-progress record owns the
exact request bytes/key, validated target and captured revision, session slot
identity, phase/deadline, and single terminalization guard. It does not move or
take the child/effects out of `SessionRegistry`: the registry remains the sole
owner of every acquired `TerminalSession` while the close progresses. A second
close for that target is rejected/observes existing operation status according
to the current stale/already-closed rules; exact same-key/same-payload retry
observes the admitted operation, and same-key changed payload conflicts.

Admission order stays compatible with the current core: decode; exact-payload
duplicate lookup; close result capacity preflight; ledger `admit`; validate
layout revision and target; send HUP exactly once. Validation rejection is
recorded as the same terminal `CloseResult`/ledger `Failed` as today. If HUP
cannot be sent or child status cannot be queried, finish with `Unknown` while
retaining the child slot and layout. No cancellation path may drop an acquired
child. Once `begin_close` returns `InProgress`, completion is owner work and
survives client disconnect or worker timeout.

Suggested close phases are `HupSent { deadline }`, `KillSent { deadline }`,
and completion. Every advance first polls child exit. An observed process exit
before or after escalation records `Exited` or `Escalated` as appropriate.
At the HUP deadline, a still-running child gets one process-group KILL; after
the KILL deadline, one final nonblocking poll decides `Unknown` if exit is not
confirmed. Process exit is a `Child::try_wait` fact; PTY EOF is an independent
stream fact and is never used as proof that the process exited. Final PTY
capture drains only under the existing bounded policy and cannot delay sibling
progress. Existing bounded cleanup/Drop is not run on a live owner tick.

On confirmed exit, retain current successful ordering: final capture and
lifecycle `note_exited`; collapse the layout if a sibling exists; retire the
stopped pane/session; apply the survivor's absolute resize; then complete the
ledger and retain exactly one `CloseResult`. If resize fails, preserve the
current `Partial` outcome and `survivor_resize_pending` behavior. For the last
pane, keep its tombstone and existing bounded server linger/shutdown path.
`Unknown` leaves layout and child ownership intact and records one terminal
unknown result. Later natural exit follows the current `poll` auto-collapse and
history/lifecycle path. Duplicate delivery of a completion or repeated owner
ticks cannot collapse, retire, resize, or complete the same operation twice.

## Runtime response and wire behavior

Do not add a schema-9 in-progress result variant. The schema-9 `CloseResult`
remains terminal-only. Represent a runtime pending close explicitly in internal
dispatch/worker response types, never as a generic `Applied` response. The
worker's finite reply wait sends the schema-9 result only after owner
completion. If its wait expires or the client disconnects, the admitted key
stays `InProgress`; the owner continues the close independently. A status query
for that key remains pending until a terminal schema-9 result is available. No
generic schema-1 “accepted” reply may be mistaken for terminal close success.

Keep close requests/status and event delivery on the current runtime plumbing.
The worker waits in one bounded interval for the explicit pending-close
response and does not poll `Child` or mutate owner state. It may return/close
after timeout without cancelling the transaction. The owner advances the close
on each normal tick. Internal `WorkerResponse`/runtime types may be extended
without wire change. Stop and report if implementation requires changing
`src/close_protocol.rs` or another unassigned protocol file.

Ledger/dedup behavior remains: exact retry does not signal again, key conflict
does not affect the child, in-progress status is not completion, and terminal
status/result remains available subject to existing lease/result retention.
No input is ever replayed. At close admission, clear or reject any pending
input for that target immediately; all further input and resize dispatch for a
target with an active close are rejected until it is retired. Sibling input and
resize continue normally. A resize queued before close is applied before HUP
admission according to the owner request queue; after HUP admission target
resize is suppressed. Survivor resize occurs only after collapse and follows
the existing layout revision transition.

## Failure and boundedness policy

- Capacity is preflighted before ledger mutation/HUP; exhaustion rejects with
  the existing fixed capacity label and leaves the session untouched.
- All active-close metadata counts against the existing retained-result cap;
  no unbounded auxiliary queue is added. Terminal results remain subject to
  existing result bounds and close-status semantics.
- A signal error, `try_wait` error, or unconfirmed exit becomes terminal
  `Unknown`; preserve the handle and target. Do not retry HUP/KILL implicitly
  after an error or client retry.
- No wait, drain loop, or cleanup/Drop path may run on a live owner tick. A
  verified already-reaped handle may be retired with the existing bounded
  output drain only if it performs no deadline sleep. If that invariant cannot
  be proved, retain ownership and stop for parent review; do not add a generic
  cleanup framework.
- Keep shutdown changes proportionate: mark new commands unavailable, initiate
  HUP for each live owned child, advance all stop phases concurrently using
  shared deadlines, and retain ownership until each bounded attempt completes.
  Do not serialize two full one-second waits or add a generic cleanup
  framework. Unknown exits remain unknown in lifecycle/status.
- Asynchronous launch preparation, journal sync isolation, and eliminating
  all synchronous filesystem work from split dispatch are not included here.
  Any blocking journal work encountered on close completion is an explicit
  parent review risk; this slice must not add new such work.

## Tests and verification plan for stage 2

Deterministic core tests with fake clock/effects must prove: admission sends
one HUP; status is `InProgress`; exact retries do not signal twice; payload
conflict is rejected; child exit before one second avoids KILL; at one second
KILL is sent once; exit during the second window completes as escalated;
unknown after two seconds retains child/layout; ledger/result terminalizes
once; stale/no-layout/capacity failures preserve existing results; collapse,
retire, capture finalization, lifecycle update, resize-pending outcome and
history ordering match current behavior; repeated ticks do not duplicate side
effects; input and resize to the closing target stop immediately while sibling
traffic remains enabled; process exit and PTY EOF are observed independently.

Runtime regression must use two real PTYs (or socketpair-backed owner fakes if
real PTY creation is sandbox-blocked): pane A traps/ignores HUP; pane B has a
continuous writer and accepts input. During A's full HUP interval, repeatedly
read B output, send B input, and issue pane/status queries; assert progress
before advancing fake time to KILL. Then assert A is reaped or honestly
reported unknown, B remains readable/writable, the close key has one terminal
result, and retry never launches/signals again. A second test holds a client
worker past its finite response deadline and proves the server-owned close
continues to terminal status without a false applied acknowledgment. Shutdown
coverage must prove both children receive HUP before the shared deadline,
concurrent escalation, and bounded teardown under the existing limits.

Run focused unit/integration tests, `cargo fmt --check`, strict Clippy and the
permitted project verifier as parent directs. Record each actual command/result
and sandbox-blocked listener/PTY cases separately; no ignored or blocked case
is a pass. Keep existing sources and uncommitted Wave 1 work intact.

## Risks and unresolved review points

The main risk is fitting progress replies/status into current worker plumbing
without conflating generic operation `InProgress` with schema-9 terminal
results. Parent review must check worker channel ownership, request ordering,
last-pane linger, disconnect cleanup, capacity accounting, and whether a
reaped `TerminalSession` destructor or final history capture can block. Schema
9 has no terminal-pending result. This proposal explicitly leaves the codec
unchanged and does not claim that the current client will stay responsive while
waiting; interactive-client network isolation remains separate. Async launch
and journal durability remain open findings. No provider recovery, saved-layout
behavior, history replay, or workspace hierarchy is included.
