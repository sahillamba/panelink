# Borrowed session handler — Stage 1

This change implements only the parent-adopted Stage 1 from
[`P3-MULTI-SESSION-OWNER-TASK.md`](P3-MULTI-SESSION-OWNER-TASK.md). The
one-session `StateOwner<E>` facade remains the runtime entry point. Its private
state is now one `GlobalAuthority` plus one existing `TargetState<E>`.

`GlobalAuthority` owns generation, `allow_input`, the single operation ledger,
and the reusable lease-grant table. It also owns connection creation and
renewal, startup completion, capability calculation, live-connection checks,
and active input-grant checks. Capability calculation receives an immutable
target state and reads its existing notification flag without polling the
effect sink.

`SessionHandler<'_, E>` borrows the global authority and target state for one
call. It owns the existing control/session handlers, read/watch/reconcile and
poll logic, and their target helpers. `StateOwner` keeps its public method
signatures and const accessors and forwards handler calls through short-lived
borrows. Its effect accessors remain direct. Exact duplicate lookup still
uses the one global ledger before stale-target checks; input and owner
mutation admission order remains unchanged. Poll still expires global leases
and grants before target input ownership and sink progress, then synchronizes
metadata.

This is an ownership/layout seam only. It adds no second session, routing,
grant scope, schema change, stop behavior, capability, or dispatch. All new
types remain private. Constructor validation, initialization order, the
existing grant-map allocation position, and redacted `Debug` output remain in
place without an `E: Debug` bound. Existing tests were updated only for the
private `global` and `session` field paths; no assertions or tests were added.

The first graph query reported that the checkout was not indexed. Repository
indexing then succeeded in fast mode (2,824 nodes, 15,256 edges), and the
follow-up `StateOwner` search returned its implementation methods in
`src/server.rs`. Source reads and tests remain authoritative for this change.

## Verification record

Using `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

- `cargo test --locked --offline --lib server::tests::`: 29 passed, 0 failed,
  0 ignored.
- `cargo test --locked --offline --test p3_socket_detach`: 2 passed, 0 failed,
  0 ignored.
- `cargo test --locked --offline --test p3_managed_launch`: 2 passed, 0 failed,
  0 ignored.
- `cargo fmt --check`: passed.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- `python3 scripts/check_docs.py docs/progress/P3-BORROWED-HANDLER.md`: 1 file,
  0 findings.
- `git diff --check -- src/server.rs`: passed.

The integrated verifier and broader review remain with the parent. This
private borrowing seam does not implement or certify multi-session behavior.
