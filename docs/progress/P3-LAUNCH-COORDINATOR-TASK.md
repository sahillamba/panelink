# Trusted launch transaction core — parent contract, 2026-10-18

This adopts a bounded prerequisite for milestone 2; it does not enable a socket
command, launch capability or CLI. The real pathname accept/exec acceptance is
still blocked by bind EPERM. The runtime remains one child. Existing credentials,
leases and authority must eventually be checked by the single global owner
before calling this trusted core. No per-session ledger or grant table is added.

Exclusive future owner files: new `src/managed_launch.rs`, new
`tests/p3_managed_launch.rs`, and `P3-LAUNCH-COORDINATOR.md`. Parent owns exports,
shared docs and review. Target current macOS/aarch64 only, reusing existing
`LaunchJournal`, `Ledger`, schema-6 codec and `SessionRegistry`. No dependencies.

## Interface and order

A coordinator owns the two-slot `SessionRegistry<E>` and at most two fixed
operation-key/result entries. It borrows the caller's ONE generation-wide Ledger
for each call. Constructor requires a nonzero generation. Provide fixed,
redacted errors and Debug; do not print/persist payloads, arguments or env.
Expose immutable target/record/effects observations and constrained EffectSink
access as the registry permits, without removing/replacing owned children.

Define a small journal trait with `preflight_launch(key, session)` and
`record(LaunchRecord)`, implemented for the real LaunchJournal by delegating to
its existing methods. Fake journals inject failures but are not durability proof.

Execute receives exact schema-6 launch payload, monotonic logical time, mutable
global ledger, journal, a fallible pure preparation callback and a fallible
spawn callback consuming its prepared value. Decode the payload, require its key
generation == coordinator generation == ledger generation, then consult
`Ledger::existing_admission` before mutable capacity or preparation checks.
A duplicate uses the stored operation/result entry and never calls prepare,
journal or spawn; changed bytes conflict globally. No target is disclosed unless
it belongs to that exact known key. Expiry/retirement errors remain ledger errors.

For a new operation, preflight registry capacity and the exact next target;
prepare the request (may resolve cwd/environment/executable but performs no
process effect); preflight journal key and full three-record headroom. Then
admit the exact payload in the global ledger. Reserve the preflighted target and
store its key/result entry immediately, before durable Intent or spawn. Everything
is synchronous on one owner turn, with no interleaving between these steps.
A parent-approved registry `next_target` observation can be added separately by
its owner; it shares reserve's capacity/overflow validation and does not mutate.

Write/flush Intent through the journal BEFORE invoking spawn. Mark registry
Intent only after successful write. Intent failure: mark Failed, complete ledger
Failed and cache Failed with reserved target; no spawn. Spawn failure: attempt
Failed journal record, mark Failed, complete/cache Failed; a genuine failed
spawn callback guarantees no acquired child. Never retry for the same key.

A successful spawn callback immediately moves E into attach_spawned before any
journal/result operation. If an unexpected attach invariant fails, retain E in
an emergency owned field (bounded to one) and return fixed internal failure;
never silently drop a successfully spawned child on the error path. This is a
trusted-core invariant fault, not a retry instruction.

Record Spawned then Applied. Any post-spawn write failure marks registry Unknown,
retains its handle, completes/caches Unknown with target, and never sends Applied.
On full success, mark Applied, complete/cache Applied with target. A vanished
response receiver has no effect here: returning/dropping a result cannot drop
the coordinator or replay spawn. Ledger-completion invariant errors must retain
all child ownership and expose uncertainty, never execute another spawn.

Status is observation only, requires original key/ledger validation, returns the
stored target/result or fixed unknown-key error; it never invokes callbacks.
No automatic process execution on restore or recovered journal metadata.

## Tests and evidence

Unit fake-journal/spawner counters must cover duplicate before/after result
loss, changed raw payload, cross-kind key collision in the global ledger,
capacity including failed/exited identities, wrong generation/expired lease,
prepare failure, journal preflight rejection, Intent write failure, spawn failure,
Spawned/Applied write failures, and no secret diagnostics. Check exact call order
and child-handle drop counters, not just final enum values. No callbacks run for
duplicates or validation/admission failures. All new semantics require tests.

A separate private-PTY integration test uses the real TerminalSession spawn path,
explicit controlled environment/cwd and real journal in a private fixture.
Two launches emit distinct child markers; repeated exact launch payload after
simulated reply loss returns same target/PID and does not produce another marker.
Changed payload conflicts, a third ID is rejected, both children remain owned
until explicit bounded cleanup/drop. Test real child retention on injected
post-spawn journal failure with an observable child acknowledgment; do not count
fake callback counters alone as live spawn proof. Use deadlines and RAII cleanup.
This needs no pathname socket, outer /dev/tty, account or manual verification.

Verify focused locked/offline module and integration tests, strict all-target
Clippy, package formatting, docs checker and diff. Parent reviews source and runs
integrated checks. Report actual results and limitations; no commits or shared
file edits. Runtime integration and capability decisions remain later work.
