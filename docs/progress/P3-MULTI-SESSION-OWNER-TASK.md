# Multi-session owner refactor plan — proposal for parent adoption

Status: source-backed design proposal only. It does not implement or claim
multi-session dispatch, select a product hierarchy, enable a launch/control
endpoint, or resolve pending authority policy. The currently integrated
`StateOwner<E>` still owns one `TargetState<E>`. The next bounded refactor must
preserve that runtime and public API until the parent adopts the integration
and its gates pass.

## Parent adoption checkpoint — 2026-10-18

Only stage 1 (private authority extraction and borrowed handler, preserving the
one-session facade) is adopted for implementation. All later stages and policy
options remain proposals. Keep the types private to server.rs for now; future
module visibility/accessors require their own review. No expanded grants,
schema migration, stop behavior, or runtime routing is authorized by this slice.

Stage-1 exclusive files: src/server.rs and P3-BORROWED-HANDLER.md. Keep constructor
allocation/error order and all existing semantic assertions. Public StateOwner
signatures and const accessors stay unchanged; Debug stays redacted without
requiring E: Debug. GlobalAuthority::connect takes an immutable TargetState
parameter for its existing notification-availability calculation (no new sink
poll/side effect). SessionHandler can forward authority checks to private
GlobalAuthority methods; do not duplicate those checks or admission logic.
Keep global maintenance inside the moved single-target poll body for this
compatibility stage; later multi-target scheduling must centralize it once per
tick. No input/lease/time/ownership order may change.

Run the exact server::tests:: filter, existing p3_socket_detach and
p3_managed_launch integration targets, strict all-target Clippy, package fmt,
docs/diff checks. No mirrored-layout tests or ignored tests are added. Parent
reviews the complete diff and runs the integrated verifier. No commits.

Clarifications for later work: EventJournal::new currently takes only generation;
its target binding is supplied by the containing session and event push/full
target checks. Schema5 adds cursor-session checks at the owner boundary; do not
claim the current journal constructor alone enforces that namespace. Receipts
change ledger retention metadata but never authorize process/input effects.

## Source boundary and objective

The just-completed private split in `src/server.rs` defines the per-target
group: `target`, `input_owner`, `effects`, `journal`, notification availability,
screen revision, owner observation, and child-exit observation. `StateOwner`
currently owns those fields alongside generation, `allow_input`, one `Ledger`,
and the `LeaseGrant` map. `connect`, control/session admission, polling, and
response construction currently coordinate both sets directly.

The target design separates the shared values into exactly one
`GlobalAuthority` and passes both domains explicitly to a borrowed
`SessionHandler`. It does not emulate a session switch by replacing fields in a
single `StateOwner`.

```rust
struct GlobalAuthority {
    generation: u64,
    allow_input: bool,
    ledger: Ledger,
    grants: HashMap<u64, LeaseGrant>,
}

struct SessionHandler<'a, E: EffectSink> {
    global: &'a mut GlobalAuthority,
    session: &'a mut TargetState<E>,
}
```

`TargetState<E>` remains private. `SessionHandler::new(global, session)` is a
private/crate-private borrowing seam; its methods never retain either
reference. Do not add `Deref`/`DerefMut`, unsafe aliasing, cloned ledgers or
grant tables, mutable active-target selectors, or extraction/replacement APIs
for retained effect handles.

## Method movement and compatibility wrappers

Move shared state and its operations into `GlobalAuthority`:

- Move `generation`, `allow_input`, `ledger`, and `grants` from `StateOwner`.
- Move `connect` unchanged in signature and validation order. Its internal
  capability calculation takes the target state as an explicit immutable
  argument because notification availability and `screen_revision()` are
  session data; `allow_input`, lease creation/renewal and the grant map remain
  global.
- Move the body of `complete_startup` to a crate-private global-authority
  method. Keep startup completion outside client dispatch.
- Move `require_live_connection`, `active_input_grant`, and generation/ledger
  access to `GlobalAuthority`. Keep `require_capability` as a fixed check or
  inline it; do not change its selected-capability semantics.
- `available_capabilities(&TargetState<E>)` combines global `allow_input` with
  that session's metadata support exactly as today.

Move target behavior into `SessionHandler<'_, E>` with the signature style
shown below. Its implementation receives only the selected target state and
the one shared authority borrow:

```rust
fn handle_control(&mut self, connection: &Connection, payload: &[u8], now_secs: u64)
    -> Result<Vec<u8>, ServerError>;
fn handle_session(&mut self, connection: &Connection, payload: &[u8], now_secs: u64)
    -> Result<SessionReply, ServerError>;
fn reconcile(&mut self, connection: &Connection, target: TargetRef,
    max_bytes: u32, max_lines: u16, now_secs: u64)
    -> Result<ReconcileState, ServerError>;
fn read_events(&mut self, connection: &Connection, target: TargetRef,
    cursor: EventCursor, limit: u16, now_secs: u64) -> Result<EventPage, ServerError>;
fn poll(&mut self, now_secs: u64) -> Result<(), ServerError>;
```

Move the corresponding inner methods and private helpers with those methods:
`handle_control_inner`, `handle_session_inner`, `handle_read`, `handle_input`,
`handle_owner_mutation`, `session_info`, `require_watch_authority`,
`synchronize_metadata`, `record_event`, `rejected`, `status_response`, and
`encode_response`. Keep the current branches and ordering intact. In
particular, `handle_control_inner` and `handle_owner_mutation` consult the
global `ledger.existing_admission(key, exact_payload, now)` before comparing
the request target to `session.target`; do not turn a known global duplicate
into a stale-target response or dispatch it again. Input and owner mutation
candidate state remains prepared before ledger admission and committed only
after successful admission. All input remains synchronous and non-replayable.

Keep `StateOwner<E>` as the existing one-session façade, with private fields
`global: GlobalAuthority` and `session: TargetState<E>`. Preserve every current
public method signature and behavior by forwarding through short-lived
`SessionHandler` borrows. `target`, `owner_snapshot`, `effects`, and
`effects_mut` remain direct accessors on the façade. `effects_mut` retains its
existing concrete `&mut E` return for compatibility; the coordinator's
multi-session API separately keeps concrete owned handles hidden.

The constructors retain their current public signatures:
`StateOwner::new(generation, target, effects, allow_input)` and
`StateOwner::with_ledger(ledger, target, effects, allow_input)`. Validate
generation and the full target before constructing either owner. Preserve the
with-ledger behavior for the reserved internal launch key, and preserve
constructor failure mapping. Build the global ledger/grant capacity and the
session input owner/journal in the same order that avoids publishing partial
state. Do not add a second `Ledger` or `HashMap` to each session. Keep redacted
Debug output and avoid an `E: Debug` bound.

## Multi-session ownership seam

The later parent-adopted owner can own
`ManagedLaunchCoordinator<TargetState<E>>` beside its single
`GlobalAuthority`. `ManagedLaunchCoordinator::execute` already owns its
registry, admits through a borrowed `&mut Ledger`, and immediately attaches a
successful spawn result before lifecycle journal writes. Its public
`effects_mut(target)` intentionally returns only `&mut dyn EffectSink`; that
must remain the public non-replacement boundary.

`TargetState<E>` can implement `EffectSink` by delegating `validate`, `read`,
`input`, `poll`, `child_exited`, and `screen_revision` to its contained `E`.
This lets the existing coordinator poll and observe the owned target without
exposing its concrete handle. Add one narrow **crate-private** accessor seam
to `ManagedLaunchCoordinator`, reviewed by the parent before integration:

```rust
pub(crate) fn session_mut(
    &mut self,
    target: TargetRef,
) -> Result<&mut E, RegistryError>;
```

It delegates to a crate-private `SessionRegistry` concrete mutable borrow and
performs the same full five-field `TargetRef` validation as `effects_mut`.
Neither accessor removes, replaces, or returns ownership of `E`; neither is
exported publicly. The owner selects a target from a validated request, gets
that exact mutable `TargetState`, then constructs
`SessionHandler::new(&mut self.global, session)`. The global and session borrows
are disjoint struct fields. Do not expose a public mutable registry, slot,
`Option<E>`, or take/replace handle method to accomplish this.

Before each child spawn, prepare every target-owned allocation and
fallible-initialization step: `InputOwner`, `EventJournal::new(generation)`,
bounded journal storage, and the target shell/metadata. The launch preparation
closure can carry this shell with the already-resolved launch inputs. After the
spawn closure produces `E`, move `E` into a `TargetState` through an infallible
assembly operation, sample only the existing infallible initial metadata
accessors, return that value to `ManagedLaunchCoordinator`, and attach it
before any `Spawned`/`Applied` journal record. There must be no fallible
allocation, target validation, or journal initialization between successful
spawn and coordinator attachment. If attachment reports its existing
invariant error, retain the returned `TargetState<E>` in the coordinator's
existing emergency slot and latch unhealthy; never drop an acquired child.
Do not write event metadata or claim a completed launch before this order.

The constructor allocates the global grant map/ledger once, constructs the
empty two-slot coordinator for the same nonzero generation, and retains the
existing `ServerStorage` singleton lock for the entire owner lifetime. The
runtime still owns the lock lifecycle; no second lock or generation is made
inside either owner. If an integrated owner struct directly contains the lock,
declare child-owning coordinator fields before the lock field so normal field
drop order releases retained child handles before releasing the singleton
lock. Preserve `ServerDriver::drop` ordering: stop/join socket workers and
settle retained watch replies before dropping any child-owning state. A failed
second-session attach cannot drop the handle or unlock storage while it remains
retained.

The current constructor's one initial effect sink remains one child until the
parent adopts a factory path that allocates its shell before the child spawn.
Do not silently turn `StateOwner::new` into two-session startup or change the
current CLI/runtime. A later `MultiSessionOwner` constructor/adoption is a
distinct, parent-reviewed integration step.

## Identity, admission, storage, and polling invariants

- Retain `SESSION_CAPACITY == 2`, the registry's fixed IDs 1 and 2, and its
  no-removal/no-reuse rule. Failed, Unknown, Applied and Exited identities all
  spend their slot. Exited children remain owned tombstones. No reset or
  capacity growth is introduced.
- There is one generation-wide operation ledger. Same-key/different-byte
  conflicts remain global even when payloads address different targets or
  launch/control kinds. Exact duplicate lookup precedes mutable target owner,
  capacity, preparation and spawn preconditions. Receipts and status remain
  observations; no path retries PTY input or replays stored side effects.
- Use one in-memory `EventJournal` per `TargetState`, each initialized with the
  same generation and its exact full target. Events keep the existing schema-3
  payload with full `TargetRef`; no event text or terminal bytes are journaled.
  The new schema-5 scoped-watch codec already carries `{generation, session,
  sequence}`. Runtime, worker, and CLI adoption must validate its session
  against the full target. Keep schema 4 usable only for a generation that has
  spent one session ID; once a second ID is allocated, return one fixed
  upgrade-required failure for schema-4 reconcile/watch. This migration rule
  is a proposal for parent adoption, not active runtime behavior.
- Preserve one server-wide cap of eight pending finite watches and the current
  one-second maximum wait. Keep the owner loop nonblocking. Each owner tick
  polls each live retained child at most once and examines at most eight
  pending requests total. If the driver needs an order cursor, rotate the
  starting session/request on successive ticks so one target's ready watches
  cannot starve the other target. Do not multiply the cap by session count or
  add a sleeping thread per watch. Continue to recheck live connection and
  exact target authority when resolving a pending watch.

## Explicitly unresolved policy gates

These are product/authority decisions for the parent and user; this refactor
must not decide them by implication:

- **Grant scope on future targets and resume:** today a `Connection` and grant
  are generation/lease scoped because there is one target. Decide whether a
  lease is bound to one target at issuance, explicitly granted across selected
  targets, or governed by another reviewed rule. Do not let a currently valid
  lease automatically authorize a future target, and do not make `connect`
  resume broaden an old grant. Until scope is adopted, multi-target dispatch
  must fail closed.
- **Launch authority:** the managed-launch coordinator is a trusted core and
  assumes authorization happened before `execute`. Decide which global lease,
  capability, or separate server authority may create a second target. Do not
  infer that input permission (`allow_input`) implies launch permission.
- **Stop authority:** no client stop command or child-stop policy is added by
  this plan. Decide who may stop a selected child, how it is acknowledged and
  deduplicated, and whether server shutdown alone owns cleanup. Existing
  handles remain retained; no public `shutdown`/remove operation is proposed
  here.
- **Target hierarchy:** the registry's workspace/pane/session mapping remains
  a prototype fixture only. This plan preserves full target identity checks
  without adopting that mapping as product hierarchy.
- **Schema-4 migration:** the single-session-only legacy behavior above needs
  explicit parent adoption with worker and CLI changes. Existing runtime stays
  schema-4 as currently implemented until that integration is reviewed.

## Exclusive implementation stages after adoption

Each stage is a proposed exclusive assignment. Parent reviews the API before
the next stage; workers do not commit or edit shared roadmap/manifest files.

1. **Authority extraction, `src/server.rs` only:** introduce `GlobalAuthority`
   and `SessionHandler`, preserve the `StateOwner` façade and all existing
   public signatures. Keep the one-child runtime. Move only state and methods
   listed above. Parent reviews field/API boundaries and runs the full current
   regression suite before integration proceeds.
2. **Concrete borrow seam, `src/session_registry.rs` and
   `src/managed_launch.rs` only:** add crate-private target-validated mutable
   access to the retained concrete session value. Keep public immutable and
   trait-object accessors unchanged. Parent reviews visibility and handle
   retention, then runs registry and managed-launch suites.
3. **Owner assembly, new `src/multi_session_owner.rs` plus `src/lib.rs` only:**
   add the parent-adopted owner around one `GlobalAuthority` and one
   `ManagedLaunchCoordinator<TargetState<E>>`; retain one-child compatibility
   adapter. Integrate no socket route, stop command, or expanded grant scope
   unless those are separately approved. Parent owns export review.
4. **Runtime routing, `src/server_runtime.rs` and the explicitly assigned
   worker/client files only:** route complete target identities through
   `SessionHandler`; adopt schema-5 watches and the agreed schema-4 gate;
   preserve the shared eight-request cap and bounded nonblocking tick. This
   stage is not authorized until parent adopts authority and wire decisions.
5. **Integration and evidence, named integration tests and one new progress
   report only:** add the scenarios below, run all focused and integrated
   checks, and report blocked pathname-socket acceptance separately. Parent
   performs final review and phase/commit decision.

## Required automated evidence for the later implementation

The plan calls for tests exercising behavior, not tests that merely mirror the
field refactor. Keep deterministic counters, exact event order, unique child
markers, deadlines, and RAII cleanup. Do not use ignored tests as passes.

| Area | Required scenarios |
| --- | --- |
| Unit: shared authority | Two sessions receive leases from the same ledger; one session's operation is visible to global status/receipt; global lease/operation caps and expiry remain global; no independent per-session ledger or grant map exists. |
| Unit: target routing | Full five-field mismatch cannot access a target; each exact target invokes only its own sink; stale occupant, wrong generation/session and invalid session ID fail before effects. |
| Unit: deduplication | Exact duplicate is resolved before target-owner/capacity/prepare/spawn checks; changed bytes conflict globally across targets and across launch/control key kinds; status and receipt never invoke a sink or spawner. |
| Unit: ownership and input | Each target has independent owner revision/pending input; one pending input does not pin another target; each input sink call occurs once; lost response does not replay; launch failure retains a tombstone and does not reuse session ID. |
| Unit: launch transaction | Two launches allocate IDs 1 and 2; third is rejected after Failed/Exited/Unknown slots too; assert Intent-before-spawn and attach-before-Spawned/Applied; each injected write failure preserves the correct handle/outcome; attach failure retains the child. |
| Unit: scoped watch | Session A and B both at event sequence 1; an A cursor is rejected for B; schema-4 works before the second ID is spent and gets fixed upgrade-required rejection after; resync/timeout never advances the other session cursor. |
| Unit: polling bound | Eight shared pending watches are accepted, the ninth rejected; no tick blocks; both children get at most one bounded poll per tick; request/session rotation prevents starvation; shutdown settles each pending reply once. |
| Socketpair integration | Route authenticated requests for two target IDs through the existing owner-driver/socketpair path; verify independent output/input/owner state and full-target stale rejection; disconnect and reconnect do not kill or replace either retained child. |
| Private-PTY integration | Spawn two real children with distinct markers and controlled environments/cwds; exact launch retry after reply loss yields the original target/PID and one marker; changed bytes conflict; third launch is rejected; post-spawn journal fault leaves the child responsive and owned; bounded explicit cleanup/drop reaps both under deadlines. |
| Storage/lock integration | Two owner starts cannot overlap one private directory; generation increments before owner construction; a second session ID is never reset/reused in one generation; lock stays held until worker shutdown and child-owning coordinator drop. |

The existing pathname listener/exec acceptance remains a separate gate: a
socketpair test is not evidence of successful private-path binding under the
current sandbox. Keep launch/control routes disabled until the parent records
all required gates and explicitly adopts the integrated owner.

## Repository references

- [State owner split](P3-OWNER-STATE-SPLIT.md) and
  [its task contract](P3-OWNER-STATE-SPLIT-TASK.md)
- [Launch coordinator contract](P3-LAUNCH-COORDINATOR-TASK.md) and
  [registry behavior](P3-SESSION-REGISTRY.md)
- [Managed launch plan](P3-MANAGED-LAUNCH-PLAN.md), especially cursor/storage
  and owner-set proposals
- [Scoped watch codec](P3-SCOPED-WATCH-CODEC.md) and
  [watch delivery evidence](P3-WATCH-DELIVERY.md)
- Source: `src/server.rs`, `src/managed_launch.rs`, `src/session_registry.rs`,
  `src/server_runtime.rs`, `src/server_storage.rs`, and `src/watch_protocol.rs`
