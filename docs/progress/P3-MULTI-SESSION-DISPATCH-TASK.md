# Multi-session request dispatch — follow-on contract

## Parent adoption — 2026-10-18

Adopt this bounded routing slice after the assembly owner releases its files
and parent review passes. Exclusive files are src/server/multi_session.rs and
new P3-MULTI-SESSION-DISPATCH.md. Preserve the existing parent/assembly work;
no server.rs, runtime, codec, registry or integration-test edits. All seven
existing variants are routed as below. Public Claim followed by control input
must be exercised on both targets; first-slice fixture owner setup is not a
substitute. Global cached replies intentionally select no target and sync no
metadata; selected-target bodies synchronize exactly once even on errors.
Tests may replace the entire fake coordinator with an empty same-generation
one to prove cached/status paths do not look up a target. Use fake handles only
for that test-only operation; do not add removal/replacement APIs. No grant
expansion, launch/stop authority, Events advertisement or watch adoption.

**Status: adopted with the amendments above; implementation follows assembly review.** This task
continues the bounded owner assembly in
[P3-MULTI-OWNER-ASSEMBLY-TASK.md](P3-MULTI-OWNER-ASSEMBLY-TASK.md). It routes
the existing seven `SessionRequest` variants through one
`MultiSessionOwner::handle_session`; it changes no wire schema, socket/runtime
path, launch policy, child-stop policy, or journal schema. The existing
one-child `StateOwner` runtime remains unchanged until parent adoption and its
gates pass.

## Source-checked boundary and signature

The graph-first search of `Users-sahillamba-panelink` located
`SessionHandler::handle_session_inner`,
`GlobalAuthority::{preflight_owner_mutation,operation_status,receipt,
require_target_scope}`, and
`SessionHandler::handle_owner_mutation_preflighted`. Direct source inspection
confirmed the current behavior in `src/server.rs`; the seven request variants
are declared and encoded in `src/session_protocol.rs`.

The proposed public entry point is:

```rust
pub fn handle_session(
    &mut self,
    connection: &Connection,
    payload: &[u8],
    now_secs: u64,
) -> Result<SessionReply, ServerError>;
```

It belongs on the public `MultiSessionOwner` from the amended assembly
contract. It decodes once with the existing `decode_session_request`, uses
existing `SessionReply`, `ServerError`, `RequestPreflight`, `preflight_reply`,
and `encode_response`, and makes no new public concrete-target API. Current
`GlobalAuthority::operation_status` and `GlobalAuthority::receipt` already
exist as private methods in `src/server.rs`; a child module
`server::multi_session` can call them. There is no existing helper to select a
grant's pinned target: for `Describe`, read the copied `LeaseGrant` from
`global.grants` only after `require_live_connection` succeeds. Do not add a
public accessor or infer the target from coordinator order.

`SessionHandler::handle_session_inner` currently combines decode, live check,
and one-target dispatch. Multi-owner dispatch must not call that generic
method after independently preflighting: it would repeat global checks and
would bind authority to an already-selected session. Instead it dispatches
the decoded request and calls the existing narrow methods for the selected
target. `SessionHandler::handle_owner_mutation_preflighted` takes
`connection` first and performs the full-target and grant-scope checks itself.
That method's private visibility is usable from its sibling child module; no
new borrow seam is needed for this dispatch task. Use the consumed
`ManagedLaunchCoordinator::session_mut(TargetRef)` seam from the assembly
contract to borrow exactly one concrete `TargetState<E>` internally. Never
return that borrow from the public method or owner.

## Dispatch contract

Decode exactly once. Its `SessionProtocolError` converts through `?` to
`Err(ServerError::Session(error))`; do not create a second wire decoder. The cases below describe
all post-decode behavior and response shape.

### `Describe`

1. Call `global.require_live_connection(connection, now_secs)`; propagate
   `UnknownContext` for missing, expired, wrong-generation, or capability-
   widened connections.
2. Require `CAPABILITY_READ`; return `Err(ServerError::Unauthorized)` if
   absent, matching the existing handler.
3. Look up `connection.lease` in `global.grants`, copy its pinned full
   `TargetRef`, and return `UnknownContext` if the entry is absent. Do not
   inspect `ManagedLaunchCoordinator::next_target`, pick a first/live record,
   or accept a target supplied by the request (there is none).
4. Borrow only that exact target with `launches.session_mut(target)`. Map
   `InvalidTarget` and `EffectsUnavailable` to `UnknownContext`; map an
   impossible registry invariant error to `Internal` as in assembly.
5. Construct `SessionHandler::new(&mut global, session)`, get
   `session_info()`, and return `SessionReply::Info`. Call
   `synchronize_metadata()` exactly once after this selected-target body,
   including a selected-target `session_info` error. Do not sync on any
   earlier global denial. The existing session-info behavior, including
   `UnsupportedOwner` for a human holder, remains unchanged.

This preserves the pinned-grant scope: a lease observes only the target to
which it was issued. It does not make that lease a general launch or stop
authority.

### `Claim`, `Release`, and `Handoff`

For each request, construct the same `OwnerMutation` currently constructed in
`handle_session_inner`, preserving all request fields. Then:

1. Call `global.preflight_owner_mutation(connection, key, payload, now_secs)`
   before any full target lookup. It performs live-context validation,
   INPUT-capability/key generation and lease checks, and exact global ledger
   admission lookup in that order.
2. Convert `Rejected` or `Known` immediately with
   `preflight_reply(key, decision)`, returning `SessionReply::Payload`. Do not
   query the registry, synchronize a target, run scope/handoff checks, admit,
   or invoke effects for these cached/denied cases. Propagate preflight
   `UnknownContext`, `Protocol(OperationConflict)`, expiry, and other ledger
   errors unchanged.
3. For `Continue` only, use the request's full five-field `TargetRef` in
   `launches.session_mut(target)`. Map an invalid/stale/unavailable target to
   the existing encoded `Rejected(StaleTarget)` payload for this key; map
   impossible registry errors to `Internal`. No `Ledger::admit` has occurred
   yet.
4. Construct `SessionHandler` with the exact borrowed target and call
   `handle_owner_mutation_preflighted(connection, key, target, mutation,
   payload, now_secs)`. Do not duplicate target scope or recipient checks:
   this existing method checks exact target, then the caller grant's target
   scope, then transitions a candidate owner. Handoff additionally requires
   the recipient to have a live INPUT grant pinned to this same target.
   Existing rejection codes and order remain authoritative.
5. Synchronize the selected target's metadata exactly once after the handler
   result, including stale/scope/transition denial after the target was
   selected. Never synchronize on steps 1–3 rejection/cache paths.

The existing handler retains the remainder of the operation order: candidate
transition; `Ledger::admit`; complete as `Applied`; then commit the candidate
owner. Capacity maps to `Rejected(Capacity)`, transition failures retain
`owner_rejection`, and protocol failures propagate. Do not replay input or
change duplicate/conflict semantics. In particular, a byte-identical retry
is resolved globally before a stale target precondition, while changed bytes
under the same key conflict globally before target lookup.

### `OperationStatus` and `Receipt`

Delegate without selecting any target:

```rust
SessionRequest::OperationStatus { key } => global
    .operation_status(connection, key, now_secs)
    .map(SessionReply::Payload),

SessionRequest::Receipt { generation, lease, through_sequence } => global
    .receipt(connection, generation, lease, through_sequence, now_secs)
    .map(SessionReply::Payload),
```

Keep the existing helper mapping exactly. Both helpers require live context.
Status then requires READ; a key from another generation/lease returns the
existing encoded `Rejected(Unauthorized)`, and ledger errors propagate as
`ServerError::Protocol`. Receipt has no added capability check; identity
mismatch returns `Err(ServerError::Unauthorized)`, ledger errors propagate,
and success encodes `Completed(Applied)`. Neither helper performs registry
lookup, target metadata synchronization, or process effects. Do not create a
default target to satisfy the old `SessionHandler` shape.

### `Detach`

Call `require_live_connection` and return `SessionReply::Detached`. It has no
target lookup, metadata sync, lease mutation, sink call, or child stop. This
is only the existing logical reply; socket attachment teardown remains the
runtime's responsibility. A lease does not grant authority to stop a child.

## Scope exclusions and authority constraints

Do not route reconcile, event watch, runtime worker operations, child stop,
launch, or journal schema migration in this change. In particular, do not
silently expand an existing pinned lease to future targets, and do not infer
that INPUT or a successful Claim authorizes launch or process stop. Launch
authorization and stop policy remain separate parent/product decisions. No
schema-5/session-journal adoption is implied. Existing schema/runtime behavior
and the current one-child path are unchanged.

## Required focused tests

Add tests under the private `server::multi_session` test module when this
contract is adopted. Use two launched fake targets and two fresh pinned
connections. Assert target identities and sink counters, not only reply
bytes.

1. **Independent target ownership and actual dispatch:** connect independently
   to targets A and B. Claim ownership through the public owner entry point
   for each target using each connection's own key and current owner revision;
   then send valid input on each target. Assert both claims succeed, each
   input reaches only its selected sink once, and one target's owner revision
   or input does not mutate the other. This is required because first-slice
   launch fixtures may seed owners internally and do not prove public Claim
   routing.
2. **Deduplication before target preconditions:** after a completed mutation,
   repeat exact bytes with a now-stale/unknown target and assert cached
   completion with no registry borrow or metadata sync. Reuse that key with
   changed bytes/target and assert global `OperationConflict` occurs before
   registry lookup. Also exercise an in-progress result if the fixture can
   hold one without adding production hooks.
3. **Scope and identity denials:** a live connection pinned to A submitting a
   new mutation for B gets encoded `Rejected(Unauthorized)` after exact
   lookup, without ledger admission or sink effect. Wrong-generation and
   wrong-lease keys are rejected by global preflight before lookup. Handoff
   to missing/expired recipient maps `UnknownContext`; recipient pinned to a
   different target or lacking INPUT maps `Unauthorized`; no owner transition
   or ledger admission occurs.
4. **Targetless requests:** valid OperationStatus, Receipt, and Detach work
   with no target borrow available and leave every target sink and metadata
   counter unchanged. Verify status's READ requirement and the existing
   asymmetric status/receipt identity error mapping.
5. **Describe pinning:** after connecting A and B, each Describe returns only
   that connection's target and info. A connection pinned to A never reports
   B even when B is the coordinator's newest or only live record. Missing,
   expired, or non-READ context fails before target lookup.
6. **Protocol and existing behavior:** malformed session payload retains the
   existing decode error mapping; Detach returns Detached only after a live
   context; human-owner Describe retains `UnsupportedOwner`. Keep the existing
   one-child `server::tests`, `p3_socket_detach`, and `p3_managed_launch`
   suites unchanged as regression evidence.

Tests should observe lookup/synchronization ordering with existing private
test structure or deterministic owner state. Do not add public test seams,
mutable target accessors, production callbacks, ignored tests, or a socket
route solely to observe ordering. If exact observation cannot be obtained
without such a seam, parent review must choose the narrowest test-only
approach before implementation.

## Adoption stages and verification

1. Parent reviews this contract against the then-current
   `GlobalAuthority`/`SessionHandler` source and accepts the dispatch order
   and metadata timing.
2. Implement only `MultiSessionOwner::handle_session` in the child module;
   keep the existing one-child runtime and every wire type unchanged.
3. Add the focused private unit tests above, especially Claim-then-input on
   both independently pinned targets.
4. Run focused multi-session tests, existing server owner tests,
   `p3_socket_detach`, `p3_managed_launch`, formatting, strict all-target
   Clippy, docs checks, and the repository verifier. Parent reviews and runs
   integrated checks before any runtime adoption.

This document records a proposed next source slice, not implemented behavior
or a product decision. No source, tests, runtime, shared docs, or commits are
included here.
