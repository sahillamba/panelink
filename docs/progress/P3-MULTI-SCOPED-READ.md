# Multi-session scoped reads

This checkpoint adds synchronous scoped reconciliation and finite event-page
methods to the trusted `MultiSessionOwner` core. It does not add socket,
worker, CLI, runtime scheduling, a persistent watch loop, or child-stop
behavior. The pathname/runtime acceptance and explicit child-stop policy
remain unresolved; this checkpoint does not complete the five non-UI
milestones.

`MultiSessionOwner::reconcile` and `read_events` require a live connection
with both READ and EVENTS, select the exact full `TargetRef`, then verify the
existing grant's pinned target before validating a budget/cursor or observing
metadata. The implementation reuses extracted `SessionHandler` preflighted
bodies, preserving the single-target wrappers' authorization and effect
ordering. Each target retains its existing independent journal.

Reconciliation returns `ScopedReconcileState` with the selected target's
session-scoped cursor and bounded screen. Its complete schema-5 representation
is checked before return. The 128-byte prefix remains unchanged; the sink
receives `max_bytes - 128` text bytes, with limits 128..=65,536 bytes and
1..=1,024 lines. Event pages accept limits 1..=64, reject generation and
session cursor mismatches before journal reads, and preserve resync behavior
after event eviction. Both public result types redact their contents and
identifiers in Debug output.

The experimental multi-session connect path now advertises EVENTS when that
target has healthy notification metadata. Missing support omits an optional
capability and rejects a required capability before issuing a lease. A fresh
lease is required to gain newly available capability; resuming an older
grant does not widen it. Existing one-child capability negotiation remains
unchanged.

## Verification

All commands below were run offline with
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

- `cargo test --locked --offline --lib server::multi_session::tests::` — 17
  passed.
- `cargo test --locked --offline --lib server::tests::` — 33 passed.
- `cargo test --locked --offline --lib scoped_watch_protocol::tests::` — 9
  passed.
- `cargo test --locked --offline --test p3_multi_owner` — 2 passed,
  including real-child read, submit, and exact-retry coverage.
- `cargo test --locked --offline --test p3_socket_detach` — 2 passed.
- `cargo test --locked --offline --test p3_managed_launch` — 2 passed.
- `cargo fmt --check` — passed.
- `cargo clippy --locked --offline --all-targets -- -D warnings` — passed.
- `python3 scripts/check_docs.py` — 133 Markdown files checked, 0 findings.
- `git diff --check` — passed.

This evidence covers component behavior only. It is not runtime acceptance,
provider identity evidence, or milestone-completion evidence.
