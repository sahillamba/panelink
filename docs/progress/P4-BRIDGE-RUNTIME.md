# P4 provider bridge runtime coordinator

Status: an opt-in owner coordinator now connects the grant-scoped v2 fresh
snapshot stream to `BridgeReceiver` and the asynchronous last-confirmed store
worker. The opt-in `serve --opencode-bridge` path now activates it for the
original exact managed occupant, using the pinned private launch configuration.
The ordinary launcher and subsequent shell splits do not enable the adapter.

`ProviderBridgeRuntime::start(path, target, project, listener)` takes the
parent-owned private listener and starts two bounded workers. The existing
binding writer opens and loads the caller-created per-target store on its own
thread. The IO worker generates a nonzero OS-random bridge instance, accepts
one source, issues one `Grant`, provisions the accepted stream with the typed
kind-5 Hello, then performs the fresh-reader handshake. It does not reconnect
or regrant after any source failure.

Only after the provisioned handshake does the IO worker send `Ready(instance)`
on its capacity-one startup channel. The owner combines that event with the
binding store's load result, restores any record stale at its saved revision
floor, requests explicit reconciliation, and then enables fresh snapshots.
Snapshots are challenge-bound, requested every 500 ms, and use one 300 ms IO
deadline. Each receipt carries its source sequence, owner monotonic receipt
time, Unix verification time, and the reconciliation epoch captured immediately
before that snapshot request. The capacity-one frame channel fails closed on
overflow. If the owner requests reconciliation while a snapshot is already in
flight, it consumes the old-epoch frame only to preserve contiguous sequence
accounting; that frame cannot meet the new barrier.

`poll(now)` does no disk or socket IO and consumes at most one frame. It checks
receipt age against the 3,000 ms lease, checks source sequence progression,
passes receipt and processing times to `BridgeReceiver`, submits only a pending
binding to the storage worker, and checks idle lease expiry. Lease deadlines
start at receipt, so a queued frame cannot extend identity validity or regress
the owner clock. Home, Child, and Unknown revocations schedule a new explicit
fresh-snapshot epoch; expiry, source failure, storage failure, and transport
loss do not revive automatically.

The runtime reconciles a save completion only when its private job token and
exact durable binding still match the receiver's current pending binding; the
core's confirmation callback then validates equality without performing IO. If
a newer frame replaces the pending value while an older save is in flight, the
older durable completion cannot promote the newer state. Once that completion
is consumed, the owner schedules the current pending value for its own save.
Uncertain storage failures latch and are never retried automatically.

The public snapshot contains only the configured target, bridge instance,
fixed lifecycle status, and last-confirmed revision. It exposes no provider
session ID, credential, raw frame, or current-at-crash claim. Storage and IO
failures latch `StorageFailed` or `Unavailable`. `request_reconciliation()` is
an owner operation for a fresh post-request snapshot after a nonterminal route
or state change. It advances a monotonic epoch. Disconnected or failed sources
are terminal for this runtime instance and are not restarted or regranted.

`finish(deadline)` requests cancellation and shares one wait budget capped at
250 ms across both workers. Plain Drop requests cancellation without waiting.
The IO worker may finish a running operation within its 300 ms deadline after
the owner gives up waiting; the store worker keeps its exclusive lock through
any running or uncertain write.

## Explicit recovery controls

The runtime exposes `request_validation(id)` and `request_navigation(...)` as
nonblocking owner calls. Both require an initialized, healthy source and writer;
IDs and navigation tickets are bounded and validated before copying. One
capacity-one command channel and one capacity-one reply channel allow only one
unconsumed control at a time. IDs are monotonic and checked for exhaustion.
Snapshot scheduling uses a separate atomic signal, so starting a reconciliation
cannot drain or discard a queued control command.

Validation runs on the IO worker with the same 300 ms operation deadline and
returns a typed exact-ID result. Only a `Verified` result arms navigation for
that same ID. Navigation is accepted once per runtime, and the worker checks
the cancellation flag immediately before asking the source to issue its
one-shot route action. A canceled accepted request returns `NotIssued` and
consumes the runtime's attempt. Any malformed/failed IO or missing reply
terminally marks the source unavailable; there is no reconnect or retry.
`ActionIssued` only proves that the provider callback issued an action. The
owner must request reconciliation after it and wait for a newly received,
durably saved selected-session binding before reporting that selection as
confirmed. `next_binding_revision()` and `confirmed_revision_for()` are
read-only owner helpers; the caller must poll source/storage state before using
the latter. Neither helper performs or reserves IO.

## Verification

Focused command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --lib provider_bridge_runtime -- --test-threads=1
```

Final focused runtime result after recovery controls: **7 tests passed, 0
failed, 0 ignored**. These cover provisioning through durable confirmation,
poll responsiveness during startup, an old in-flight snapshot rejected by a
new reconciliation epoch, automatic Home-to-new-session reconciliation, and a
newer pending binding saved after an older completion is rejected. The sixth
test exercises bounded exact-ID validation, one-inflight rejection, verified
same-ID navigation gating, canceled one-shot navigation, and the fact that
validation does not create a binding. A seventh test verifies that a failed
control operation returns its matching failure, latches the source unavailable,
and is not retried. Typed
receiver tests passed separately: **14 passed, 0 failed**; they include stale
queue rejection and delayed receipts that do not regress the owner clock or
extend the lease. Strict library Clippy (`cargo clippy --lib -- -D warnings`)
and Rust formatting pass. The broader `cargo clippy --lib --tests -- -D
warnings` is currently blocked by a `too_many_arguments` lint in the separately
owned `provider_recovery_prepared.rs` test worker; this runtime change did not
modify that file. Scoped `git diff --check` remains part of parent integration
verification.

A broader `provider_bridge` filter reached 44 passes but hit two pathname-bind
tests in `provider_bridge_launch` that fail under this host sandbox with
`EPERM`; those results are environment-blocked, not counted as passes.

The first test attempt used pathname Unix listeners and failed with host
`EPERM`; the tests were changed to use a connected Unix stream pair while
retaining the production listener path. Initial Clippy findings (a test-only
source variant and oversized worker argument list) were corrected before the
final passing lint. Pathname listener creation and `run_serve` activation are
owned by the parent integration slice.

This verifies the coordinator components on this host and in unit fixtures. It
does not prove provider executable attestation, hostile same-UID isolation,
production listener permissions, heartbeat scheduling by the server, or
conversation recovery. No dependency or commit was added.

## Parent integration review

The live path uses `start_managed`: its writer publishes one atomic joint
managed-launch/provider record. Snapshot requests run every500ms, complete
within300ms and expire after3000ms from host receipt. The initial source has
15s to connect; disconnect/failure is terminal. These values form the opt-in
pinned adapter policy, not a selection/durability ordering guarantee.

Processing time is sampled after dequeuing a receipt: a caller clock sampled
before worker publication cannot make a legitimate receipt appear to be in the
future. The initial durable-confirmation test now deliberately reuses that
earlier caller clock. Listener bind and nonblocking accept descriptor creation
use Panelink's cooperative exec gate, held only around creation.
