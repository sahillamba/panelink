# P3 multi-session request dispatch

## Delivered behavior

`MultiSessionOwner::handle_session` decodes the existing session schema once and routes all seven `SessionRequest` variants without changing the wire types, one-child runtime, socket path, launch policy, or stop policy.

`Describe` requires a live Read-capable connection and selects the exact target copied from that lease's pinned grant. It does not use coordinator order or a request-supplied target. After selection, it calls the existing session-info handler and synchronizes that target once even when the handler returns an error. Global denial occurs before lookup and does not synchronize metadata.

`Claim`, `Release`, and `Handoff` use the existing immutable global owner-mutation preflight before any target lookup. Cached/rejected replies return immediately. A new request selects its full target, then delegates scope, recipient-grant validation, candidate ownership transition, admission, completion, and commit to `handle_owner_mutation_preflighted`. The selected target synchronizes once after the handler result, including denials. Stale targets retain the existing encoded `StaleTarget` response; a global cached result or changed-byte conflict takes precedence over target availability.

`OperationStatus` and `Receipt` use the existing global helpers without selecting or synchronizing a target. `Detach` requires a live context and returns `Detached`; it does not close an attachment itself, mutate grants, call a sink, or stop a child. Reconcile, event watch, runtime routing, launch authorization, and process-stop authority remain deferred.

## Verification evidence

The private multi-session owner test filter passed 13 tests. New routing coverage exercises public Claim followed by Submit independently on both fake targets; a successful same-target Handoff followed by recipient Submit, old-owner rejection at the current revision, recipient Release, and sibling isolation; Release isolation; cross-target, missing, expired, and no-Input Handoff denials without admission; wrong-generation/lease preflight denials; pinned Describe; mutation cache/conflict behavior with an empty target coordinator; targetless status, receipt, and Detach with unchanged shared sink counters; Read-capability and identity errors; malformed payload mapping; pre-lookup missing/expired/non-Read Describe denial; and metadata synchronization after selected human-owner Describe errors.

The separately owned public real-PTY integration target passes 2 tests: the original two-child assembly/lifecycle case and a Describe/Claim/Submit/duplicate-input case against two real children. The routing implementation itself uses deterministic private sink/journal tests and does not add a socket/runtime route.

This is library-core behavior only. It does not activate multi-session runtime routing or certify provider behavior, host acceptance, or completion of any broader milestone.
