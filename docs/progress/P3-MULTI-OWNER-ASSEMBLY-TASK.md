# Trusted multi-owner assembly — parent adoption contract

## Parent adoption amendments — 2026-10-18

Adopt the launch/connect/control/status/receipt/poll core below with these
corrections overriding conflicting proposal text. The module declaration is
macOS/aarch64-gated `pub mod multi_session;` and the trusted library type and
listed entry points are public, with redacted Debug and documentation of the
authorization/storage-lock obligations. This avoids shipping an unused private
core or suppressing dead-code warnings. It adds no socket/CLI route, and no
public method exposes concrete TargetState or the mutable ledger/registry.
Private field visibility and the two crate-private borrow seams stay as written.
Expose only with_ledger, adopting the caller's existing internal startup lease.
Omit the proposed new(generation,...) constructor below: an empty owner cannot
connect before a first target exists, so an empty ledger cannot bootstrap a
launch. Do not invent an internal key, expose open_lease or return a mutable
ledger to work around that cycle.

Preparation or journal preflight failure occurs before identity reservation and
does NOT spend a slot. Intent/spawn failures after reservation do spend the
identity. Test these separately; preserve existing coordinator behavior.
Control's preflighted handler takes connection as its first argument. Let that
handler perform the existing scope check rather than duplicate it outside;
after a successful exact lookup, always synchronize metadata after its result,
including denial. Global cached/rejected replies perform no target lookup or
metadata synchronization. This bounded library timing rule is adopted here;
runtime integration remains a separate review gate.

Add a result note P3-MULTI-OWNER-ASSEMBLY.md to exclusive ownership. No unused
convenience methods or accessor added solely for tests. Existing fake fixture
input-owner setup may validate control isolation for this slice; client Claim,
Release and Handoff routing are the next slice before functional runtime input.
Do not claim input ownership can yet be acquired through the new public owner.

The new core must not advertise Events while reconcile/scoped-watch routing is
deferred. Factor the existing private connect into a capability-mask helper,
preserving current StateOwner behavior with Read/Input/Events allowed. The new
owner permits Read/Input only; optional Events is omitted and required Events
fails before consuming a fresh lease. Resume cannot broaden a grant. This is
a conservative capability restriction for the isolated new core, not a wire
change or a reduction in the existing runtime's supported Events behavior.

Status: adopted with the parent amendments above. This document specifies a small owner assembly
slice. It does not add a socket route, CLI launch command, launch authorization
policy, child-stop policy, public library API, or multi-session runtime. The
existing `StartedServer`/`ServerDriver<StateOwner<PtyEffects>>` path remains
the running one-child path until the parent separately adopts runtime work.

## Current source boundary

The graph index for `Users-sahillamba-panelink` is ready. Graph search located
`StateOwner::{connect,handle_control,poll}`, `GlobalAuthority::{connect,
preflight_control,preflight_owner_mutation,require_target_scope}`,
`SessionHandler::{handle_control_preflighted,handle_owner_mutation_preflighted,
poll}`, `ManagedLaunchCoordinator::{execute,status,effects_mut}`,
`SessionRegistry::{next_target,reserve,attach_spawned,effects_mut}`, and
`StartedServer`. Direct source inspection confirms the graph view against the
current files.

The adopted foundations are:

- `src/server.rs`: private `GlobalAuthority` owns the one generation, input
  policy, `Ledger`, and `LeaseGrant` map. `LeaseGrant` includes a full pinned
  `TargetRef`; `GlobalAuthority::connect` requires exact target equality on
  resume and issues new global lease IDs for fresh connections.
- `src/server.rs`: private `TargetState<E>` owns one target's `InputOwner`,
  effects, `EventJournal`, and event metadata. `SessionHandler` borrows both
  authority and one target. Control preflight runs before its selected-target
  body. `encode_response` and `preflight_reply` are free functions.
- `src/managed_launch.rs`: `ManagedLaunchCoordinator<E>` owns the fixed
  two-slot `SessionRegistry`, launch results, and emergency retained effect.
  `execute` already calls global ledger duplicate/preflight before
  `next_target`, then prepare, journal preflight, ledger admission, reserve,
  Intent, spawn, attach, Spawned, Applied. It borrows the caller's ledger and
  journal. Its public `effects_mut` exposes only `&mut dyn EffectSink`.
- `src/session_registry.rs`: `next_target` is read-only; `reserve` spends one
  of IDs 1 and 2; failed/exited identities are not reclaimed. The only
  existing mutable handle accessor is the public trait-object `effects_mut`.
- `src/server_runtime.rs` and `src/server_startup.rs`: `ServerDriver` and
  `StartedServer` still own one `StateOwner`; `StartedServer` drops the driver
  before storage releases the singleton lock.
- `src/session_protocol.rs`: requests are `Describe`, `Claim`, `Release`,
  `Handoff`, `OperationStatus`, `Receipt`, and `Detach`. The current
  `SessionHandler` implements all seven for one selected target.

The multi-owner module must build on the target-pinned grant and global
preflight changes. It must not reintroduce an unscoped lease or duplicate
authority state.

## Exclusive source ownership and module visibility

After parent adoption, edit only:

- new `src/server/multi_session.rs`;
- the narrow module declaration in `src/server.rs`;
- the consumed crate-private concrete borrow in `src/session_registry.rs` and
  `src/managed_launch.rs`.

Declare `pub(crate) mod multi_session;` from `server.rs`. Keep
`MultiSessionOwner` `pub(crate)` inside this child module; do not re-export it
from `lib.rs`, add runtime references, or expose `TargetState<E>` in a public
signature. This lets parent-owned in-crate tests/adapters use
`crate::server::multi_session::MultiSessionOwner` while external users still
cannot construct the owner.

The owner shape is exactly one global authority and one coordinator:

```rust
pub(crate) struct MultiSessionOwner<E: EffectSink> {
    global: GlobalAuthority,
    launches: ManagedLaunchCoordinator<TargetState<E>>,
}
```

Use one constructor for an existing ledger and a convenience constructor for
tests:

```rust
pub(crate) fn with_ledger(
    ledger: Ledger,
    allow_input: bool,
) -> Result<Self, ServerError>;
pub(crate) fn new(
    generation: u64,
    allow_input: bool,
) -> Result<Self, ServerError>;
```

`new` creates one `Ledger` then delegates. `with_ledger` rejects generation
zero, creates one `HashMap::with_capacity(MAX_ACTIVE_LEASES)`, and constructs
`ManagedLaunchCoordinator::new(ledger.generation())`; impossible coordinator
generation failure maps to `ServerError::InvalidConfiguration`. There is no
initial `TargetState` in this owner. Do not modify `StartedServer` to route its
startup child through this module in this slice.

## Concrete borrow seam, added only because this slice consumes it

Keep both existing public trait-object accessors unchanged. Add these narrow
crate-private methods and use them from this module's `connect`, `control`,
and `poll` implementations in the same change:

```rust
// SessionRegistry<E>
pub(crate) fn retained_mut(
    &mut self,
    target: TargetRef,
) -> Result<&mut E, RegistryError>;

// ManagedLaunchCoordinator<E>
pub(crate) fn session_mut(
    &mut self,
    target: TargetRef,
) -> Result<&mut E, RegistryError>;
```

`retained_mut` delegates to the same full five-field `index_for` identity
check used by `effects_mut`, then borrows the stored `E` only if a handle is
present. `session_mut` delegates to it. Neither can take, replace, remove, or
own a handle. Do not add a public `&mut TargetState<E>` accessor, mutable
registry accessor, slot reference, or `Option<E>` exposure. Do not add an
unused seam: tests and production-in-crate methods in this slice must call it.

Implement `EffectSink for TargetState<E>` by direct delegation of
`validate`, `read`, `input`, `poll`, `child_exited`, and `screen_revision`.
This is required for `ManagedLaunchCoordinator<TargetState<E>>::execute`,
`note_exited`, and its existing effect observation. It must not add buffering,
fallible work, or state transitions to the delegated sink calls.

## Pre-spawn target shell and post-spawn wrapping

Add a private `TargetStateSeed` holding all session allocations and
fallibly-created metadata except `E`:

```rust
struct TargetStateSeed {
    target: TargetRef,
    input_owner: InputOwner,
    journal: EventJournal,
    last_owner_observation: Option<OwnerObservation>,
}
```

Its `prepare(generation, target) -> Result<TargetStateSeed, ServerError>`
checks all five nonzero target fields and matching generation, constructs the
input owner and calls `EventJournal::new(generation)` before spawn. It records
the initial owner observation at this time. Its consuming
`attach<E: EffectSink>(self, effects: E) -> TargetState<E>` is infallible and
allocation-free: sample only `screen_revision()` and `child_exited()`, then
move all fields into `TargetState`; notification availability keeps the
existing rule that a nonzero screen revision and valid owner observation are
both required. Do not call `EventJournal::new`, allocate a map/vector, perform
target validation, or return an error after spawn.

The private owner launch method wraps the existing coordinator's callbacks so
the shell is created during the coordinator's pure preparation callback and
the spawned `E` is wrapped before the callback returns it to the coordinator:

```rust
pub(crate) fn execute_launch<P, PE, SE, J, Prepare, Spawn>(
    &mut self,
    payload: &[u8],
    now_secs: u64,
    journal: &mut J,
    prepare: Prepare,
    spawn: Spawn,
) -> Result<LaunchResult, ManagedLaunchError>
where
    J: LaunchJournalPort,
    Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
    Spawn: FnOnce(P) -> Result<E, SE>,
```

Internally call `self.launches.execute(payload, now_secs,
&mut self.global.ledger, journal, ...)`. The adapted prepare closure first
calls `TargetStateSeed::prepare(self.global.generation, target)` and then the
caller `prepare(request, target)`; any preparation error is mapped by the
coordinator to its existing fixed `PreparationFailed`. It returns
`(P, TargetStateSeed)`. The adapted spawn closure calls the caller's
`spawn(P)`; on `Err`, it returns without a child and the seed is dropped. On
`Ok(effects)`, it immediately returns `Ok(seed.attach(effects))`. The
coordinator then attaches that `TargetState<E>` before any Spawned/Applied
journal write. Keep the coordinator's global duplicate lookup and
`preflight_admission` ahead of target ID preflight, preparation, journal
preflight and spawn. No launch authorization policy is added here: this
crate-private method is a trusted core seam, not a callable API route.

The one `GlobalAuthority.ledger` is used for launch keys, control operations,
status and receipts. Do not copy it into the coordinator or sessions. The
coordinator's fixed operation entries and `SessionRegistry` are not substitute
ledgers. Keep exact payload conflicts global across targets and operation
kinds, including the launch/control collision case. A duplicate launch
returns the coordinator's retained result and never calls prepare, journal,
or spawn. Accepted terminal input is never replayed.

## Target-pinned connect and exact control dispatch

Add these private-owner methods; neither returns a concrete session borrow:

```rust
pub(crate) fn connect(
    &mut self,
    target: TargetRef,
    hello: &SessionHello,
    now_secs: u64,
) -> Result<(Connection, Welcome), ServerError>;

pub(crate) fn handle_control(
    &mut self,
    connection: &Connection,
    payload: &[u8],
    now_secs: u64,
) -> Result<Vec<u8>, ServerError>;
```

Connect order: call `hello.validate()` first; borrow the exact retained target
with `session_mut`; map `InvalidTarget` and `EffectsUnavailable` to
`ServerError::UnknownContext`; then call the single
`GlobalAuthority::connect(&TargetState, hello, now_secs)`. Preserve its
capability validation before issuing a fresh ID, checked expiry time, global
lease counter, and resume checks. Each fresh connect to a different target
consumes a new lease. A resume succeeds only for the exact full target saved
in that lease grant; target mismatch is `UnknownContext` before `renew_lease`
and leaves the old scope/expiry unchanged. No lease permission broadening.

Control dispatch must preserve this exact order and response mapping:

1. `decode_request_payload(payload)`; malformed/unknown schema is
   `Err(ServerError::Control(error))` as today.
2. `global.preflight_control(connection, &request, payload, now_secs)` before
   any registry or target lookup. `require_live_connection` failures propagate
   `Err(UnknownContext)`. Key-lease or selected-capability failure is
   `RequestPreflight::Rejected(Unauthorized)`, encoded by
   `preflight_reply` as the existing schema response. Exact in-progress and
   completed admissions are `Known` and encoded as their existing status
   response. Changed bytes and ledger expiry/generation errors propagate as
   `Err(ServerError::Protocol(error))`. `Continue` reserves nothing and is
   not effect authority.
3. For `Rejected`/`Known`, return the `preflight_reply` bytes immediately;
   do not call `session_mut`, read a target, synchronize metadata, admit, or
   invoke a sink. This is what lets global idempotency win before an exact
   target lookup.
4. For `Continue`, call `launches.session_mut(request.target)` using the full
   five-field target. `InvalidTarget` or `EffectsUnavailable` returns the
   existing encoded `Rejected(StaleTarget)` for `request.key`; other registry
   invariant errors map to `Err(ServerError::Internal)`. Do not return raw
   registry details.
5. With that exact `TargetState`, call
   `global.require_target_scope(connection, session.target)`. `Unauthorized`
   returns encoded `Rejected(Unauthorized)` without validation, new admission,
   or effect. `UnknownContext` propagates. Then construct the borrowed
   `SessionHandler` and call `handle_control_preflighted(request, payload,
   now_secs)` directly; it retains current target, sink-validation, input-owner
   candidate, `Ledger::admit`, effect, and completion order. Do not call the
   public wrapper and repeat preflight. Always call the handler's existing
   `synchronize_metadata()` once after this selected-target body, including
   stale/scope rejection after lookup; a global Known/Rejected result above
   has no selected target and therefore does not synchronize one.

That last distinction is intentional for the unrouted internal slice: the
existing one-target `StateOwner` wrapper currently synchronizes its known
session after every control call. The multi-owner contract returns a global
cached result before selecting a target and leaves metadata synchronization to
the bounded owner poll. The parent must review this observable timing change
before any runtime adopts the method.

Other exact mappings within step 5 remain those of `SessionHandler`: a new
ledger capacity error is `Rejected(Capacity)`; sink validation errors retain
their `RejectionCode`; invalid input-owner transitions retain
`owner_rejection`; sink outcomes are completed once and reported using the
existing status/read payload; a response encoding or protocol invariant error
propagates unchanged. No path calls `Ledger::admit` for stale target or
out-of-scope grant.

## Global status, receipt, polling, and expiry

Add internal global session helpers that never look up a `TargetState`:

```rust
fn operation_status(
    &self, connection: &Connection, key: OperationKey, now_secs: u64,
) -> Result<Vec<u8>, ServerError>;
fn receipt(
    &mut self, connection: &Connection, generation: u64, lease: u64,
    through_sequence: u64, now_secs: u64,
) -> Result<Vec<u8>, ServerError>;
```

Preserve `SessionHandler::handle_session_inner` mapping exactly. Both first
require a live context (`UnknownContext`). Status then requires Read
capability; a key generation or lease mismatch encodes `Rejected(Unauthorized)`;
`Ledger::status` failures propagate as `ServerError::Protocol`; valid status
encodes the existing status body. Receipt has no added capability requirement;
generation/lease mismatch is `Err(ServerError::Unauthorized)`, ledger failures
propagate as Protocol, and success encodes Completed(Applied). Both are
lease-scoped: one target's connection cannot inspect or retire another lease's
results, even though both keys live in the same global ledger. They never
lookup a session or invoke a sink.

Split current `SessionHandler::poll` so global and target maintenance are
separate without duplicating either:

```rust
fn GlobalAuthority::expire(&mut self, now_secs: u64); // ledger once, grants retain once
fn SessionHandler::poll_session(&mut self, now_secs: u64) -> Result<(), ServerError>;
pub(crate) fn MultiSessionOwner::poll(&mut self, now_secs: u64)
    -> Result<(), ServerError>;
```

Keep `StateOwner::poll` behavior by calling global expiry once and then its
one target's `poll_session`. Multi-owner `poll` calls global expiry exactly
once, snapshots at most two immutable records into a fixed stack array, and
visits each record with `has_effects` once. Each selected target expires its
own `InputOwner`, makes exactly one bounded sink `poll`, and synchronizes its
own metadata. It then records `Exited` through `ManagedLaunchCoordinator::note_exited`
when the retained sink reports exit; the existing handle remains owned. Do
not add a per-target ledger/grant expiry pass. Continue visiting the second
target if the first session poll reports an error, then return the first
session error (or `Internal` for an impossible registry transition). Within
one target, preserve the existing rule that an input-owner expiry error
prevents that target's sink poll, while metadata synchronization still runs.

This is a private bounded core method only. No `ServerDriver`/worker changes,
watcher scheduling, sleep, new thread, or stop endpoint are part of this
slice.

## Session request cases outside the first slice

The existing one-target handler already implements these; do not accidentally
route them to a default target or broaden the new owner while implementing
launch/connect/control plus global status/receipt:

| Request | First-slice rule | Existing behavior to preserve when separately routed |
| --- | --- | --- |
| `OperationStatus` | Implement globally, with the lease/capability/error order above; no target lookup. | It observes only the caller lease's key in the shared ledger. |
| `Receipt` | Implement globally, with the identity/error order above; no target lookup. | It retires completed results only for the named caller lease; no dispatch. |
| `Describe` | Defer target routing. Do not guess from registry order. | Live context, Read capability, then exact pinned grant target before returning that target's info. |
| `Claim` / `Release` / `Handoff` | Defer target mutation routing. Do not select a target before global mutation preflight. | Live/key/capability preflight and exact cached result first; full target stale check; pinned scope; candidate owner transition; admit; complete; commit candidate. Handoff also checks recipient's active grant is pinned to the same target. |
| `Detach` | Defer to worker/runtime attachment handling; it has no process target. | Live context then `SessionReply::Detached`; no sink, lease mutation, or child stop. |
| Reconcile / event watch | Defer all watches/runtime work. | Existing target, grant, capability and bounded-cursor rules; schema-5/session migration remains separately gated. |

No new error enum or wire response is needed for deferred cases because this
slice adds no generic `handle_session` entry point and no runtime caller. The
parent must separately choose an explicit fixed rejection/error if a later
runtime exposes unsupported cases during staged rollout.

## Required tests for this slice

Add unit tests under the new private child module. Keep the current 33
`server::tests`, `p3_socket_detach`, and `p3_managed_launch` tests intact; they
remain regression checks for the unchanged one-child runtime. Do not add
ignored tests or count an unrun private-path socket check as a pass.

1. **Construction and identity:** one `GlobalAuthority` and coordinator share
   the same nonzero generation; both target slots are allocated only through
   coordinator launch; target IDs 1 and 2 are distinct; a third fails even if
   an earlier record is Failed/Exited; zero/wrong generation constructors
   return the stated fixed errors.
2. **Launch ordering and retention:** fake callbacks record pure prepare,
   journal preflight, Intent, spawn, Spawned, Applied order. The target seed is
   made in prepare before spawn. The returned `TargetState<E>` owns the exact
   spawned handle before post-spawn records; injected Spawned/Applied write
   failure reports Unknown but keeps the handle responsive and retained.
   Preparation/Intent/spawn failures retain a spent tombstone and never retry.
3. **Global dedup:** exact launch retry after dropped result returns the same
   key/outcome/target and calls neither prepare nor spawn; altered bytes and a
   cross-kind control/launch key conflict globally. Status/receipt never call
   prepare, spawn, or an effect sink. No payload/argument/env appears in
   Debug/error output.
4. **Fresh target-pinned connect:** connect to target 1 then target 2 issues
   successive IDs in one ledger and grants different full targets. Resuming
   target-1 lease on target 2 returns `UnknownContext`, does not renew it, and
   cannot change its pinned target. A target-1 connection sent a new target-2
   control request receives encoded `Unauthorized` with no admission/effect.
5. **Control dispatch order:** an unauthorized/missing connection fails
   before ledger mutation; denied capability returns encoded Unauthorized; an
   exact known status is returned without target lookup; changed bytes produce
   the existing `Protocol(OperationConflict)` before lookup; new stale target
   returns encoded StaleTarget; correct target outside grant scope returns
   encoded Unauthorized without admission; correctly scoped Read and Input
   hit only their selected sink. Assert full five-field identity and
   dedup-before-target-precondition behavior.
6. **Global status/receipt:** operations from both targets are present in the
   one ledger; each lease can query/receipt only its own sequence range;
   invalid status identity is encoded Unauthorized, invalid receipt identity
   is `ServerError::Unauthorized`, and expired/unknown ledger classifications
   are preserved. No target borrow or process effect is needed.
7. **One global expiry, per-target progress:** expire global ledger and grants
   once per owner tick, expire each target's input owner independently, and
   increment each retained sink's poll counter exactly once. Inject one
   target's poll failure and prove the other target is still visited. Exited
   handles remain retained after their registry state advances.

No socketpair/private-PTY test is required for this isolated owner slice:
pathname and runtime behavior are deliberately untouched. The existing real
private-PTY launch integration remains the process-effect evidence for the
underlying coordinator; a new assembly test should use deterministic fake
journal and sink callbacks. Parent decides whether a later source-backed
private-PTY owner integration is required before runtime adoption.

## Checks, blockers, and parent gates

The graph index was available and the named current source symbols were
confirmed by graph search; source inspection supplied the exact contracts.
The pathname bind/accept proof remains a separate environment gate and is not
changed by this proposal. The current startup/runtime integration remains one
child. No tests are claimed for the proposed type because this is a contract
document, not an implementation.

After adoption, the implementation owner should run focused new module tests,
the exact `cargo test --locked --offline --lib server::tests::` filter,
`cargo test --locked --offline --test p3_socket_detach`,
`cargo test --locked --offline --test p3_managed_launch`, strict all-target
Clippy, package fmt, docs checker, and `git diff --check` using the writable
offline Cargo cache. Parent reviews and runs the integrated verifier. The
parent owns source adoption, exports, runtime/API gates, evidence integration,
and any phase-completion commit.

Related contracts: [global request preflight](P3-GLOBAL-PREFLIGHT-TASK.md),
[pinned grants](P3-PINNED-GRANTS.md), [owner assembly proposal](P3-MULTI-SESSION-OWNER-TASK.md),
and [launch coordinator](P3-LAUNCH-COORDINATOR-TASK.md).
