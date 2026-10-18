# Prototype target-pinned grants — parent contract, 2026-10-18

Adopt a conservative private-library prerequisite for milestone 3. The runtime
still owns only one target; its successful behavior and public API remain
unchanged. This does not choose the permanent cross-pane permission model,
expand existing authority, add an endpoint, or imply input grants launch/stop.

Exclusive files: src/server.rs and P3-PINNED-GRANTS.md. Parent owns shared docs.
Build on the released GlobalAuthority/SessionHandler split. No dependencies,
module exports, runtime/CLI changes or commits.

Add an immutable full TargetRef to each LeaseGrant. GlobalAuthority::connect
already receives a TargetState: new leases bind exactly to session.target.
Resume requires that exact five-field target in addition to existing generation,
expiry and capability checks, before renewing a lease; mismatch returns the fixed
UnknownContext error and changes nothing. Renewal retains the original scope.
There is no public grant-broadening method or target set in this prototype.

Add a private global target-scope check. New control and owner mutations retain
current ordering: authenticate/live context, key-lease and capability checks,
global existing_admission exact bytes, then current full-target staleness check,
then immutable grant scope before any sink validation, owner candidate mutation,
new ledger admission or effect. Cached exact results and changed-byte conflicts
must retain their global precedence. A stale target still returns StaleTarget as
before; a valid selected session outside the pinned scope returns Unauthorized.
No denied new operation is reserved and no unauthorized target data is returned.

Describe has no explicit target, so check scope against selected session.target
before exposing its info. Reconcile/read_events check scope after current live/
capability/full-target checks and before metadata/sink access. Global status,
receipt and detach remain lease-scoped and independent of selected target,
without process effects. Do not change their semantics.

Input handoff requires the recipient's existing active input grant to be pinned
to the same exact target as the owner mutation; add that target to the private
active_input_grant signature. A live recipient for another target is Unauthorized
before admission/owner change. A fresh explicit lease is required for a different
target; old lease resume must never acquire a later target implicitly.

Tests in the private module may construct two TargetState values sharing ONE
GlobalAuthority, using fake sinks. Add meaningful scope tests: distinct targets
share global lease IDs/ledger, wrong-target resume does not renew or mutate grant
scope, Describe/read/watch/input/owner operations cannot access another target,
denied new keys remain NotAdmitted and sink counters unchanged, cross-target
handoff cannot transfer ownership, changed-byte same-key conflicts remain global
before target/scope checks, exact old results stay cached. Test each of the five
target fields, not just session. Preserve all existing semantic assertions.
No multi-session public type is introduced in this step.

Verify exact server::tests::, p3_socket_detach and p3_managed_launch targets,
strict all-target Clippy, package fmt, docs/diff with locked/offline writable
Cargo cache. Parent reviews and runs the integrated verifier. Record actual
results and prototype limitations.
