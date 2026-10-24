# P4 asynchronous provider-binding writer

Status: a standalone owner-side async adapter now wraps
`ProviderBindingStore`. The module is exported for runtime integration; it
does not activate a provider bridge or connect the owner loop by itself.

`ProviderBindingWorker::start` starts one dedicated thread. That thread opens
the configured store, takes its lifetime lock, and loads the committed
last-confirmed record before reporting readiness. The owner can wait outside
the hot loop for at most 250 ms, or poll initialization later. A failed open
or load is fail-closed. Only after readiness can the owner use `submit`, which
checks the complete configured target, project identity, and provider version
and performs a bounded `try_send` to a capacity-one channel.

One request may be outstanding at a time. `BindingJobToken` binds the full
target (including server generation and occupant revision), bridge instance,
provider revision, and a nonzero worker issuance sequence. The owner must
compare the completion token with its still-pending observation before
adopting it. The completion's fields are private and are created only after
`ProviderBindingStore::save` returns. A successful completion carries the
exact submitted `Binding` for owner adoption without another disk operation;
failed saves carry no binding and latch the writer unavailable. The owner may
then call the core's durable acknowledgment path only for the matching
successful completion. A save error is never automatically retried because
the error may follow an atomic rename and leave commit state uncertain.
The owner passes `&Binding`; the worker makes its immutable copy through the
existing bounded codec, so Busy does not consume the owner's pending value.

`poll`, `submit`, and plain `Drop` do not wait for storage. `finish(deadline)`
cancels queued work and waits only until the deadline capped at 250 ms. A
running filesystem call may outlive that wait; the detached worker retains the
store and its exclusive lock until it exits. The API does not kill the thread
or claim that a timed-out write was canceled.

The private backend seam supports deterministic blocking and failure tests; it
does not add production fault controls. A real store plus blocked test backend
proves that owner submission and polling return while a save is stalled, a
second submit is rejected, and a timed-out finish leaves the store lock held
until the running call exits.

## Verification

Focused command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --lib provider_binding_worker -- --test-threads=1
```

Final focused command passed: **6 tests, 0 failures, 0 ignored**. It covers
successful worker-created durability proof, busy bound, complete scope
rejection, startup failure, failed save refusing success, nonblocking drop,
and timed shutdown retaining the lock across blocked storage work. Strict
library Clippy passed with `-D warnings`. `rustfmt --edition 2021
src/provider_binding_worker.rs` and scoped `git diff --check` passed.

Earlier focused attempts encountered an inline test mutability error and
concurrent `provider_bridge_v2.rs` compile errors. Both were corrected before
the final passing run; those earlier attempts are not counted as passes.

No dependencies or commits were added. The requested Rust `ms-rust` skill is
still absent per repository guidance; no compliance claim is made. Runtime
activation, caller-side stale/canceled completion handling, provider grant
delivery, and full Phase 4 recovery remain open.
