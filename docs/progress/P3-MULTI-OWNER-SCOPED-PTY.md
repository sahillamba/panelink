# Multi-owner scoped reads: real-PTY evidence

The existing two-child read-isolation test now also exercises the scoped
reconcile and event APIs against two live private-PTY children. The original
Read-only leases remain Read-only: reconciliation with them returns
Unauthorized. The test then creates separate fresh Read|Events leases for the
same targets, without changing the old grants.

Each owner's reconciliation result is checked against its exact full target,
generation, and session cursor. Its text contains only that child's READY/PID
sentinel, not the sibling marker. The returned typed state is encoded and
decoded with the actual schema-5 codec; the encoded length equals the existing
128-byte prefix plus text length and stays within the requested 3,000-byte
budget. Event reads from each current cursor return the same session-scoped
cursor; reads from each initial cursor contain only events for that target.
Using target A's cursor for target B returns `TargetStale`, and using A's
Events-capable lease to reconcile B returns Unauthorized. Both PIDs are
confirmed present before owner drop and reaped afterward; the server storage
lock remains held until after owner drop.

## Verification

The focused test target passed on Apple Silicon macOS:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_multi_owner -- --nocapture
2 passed; 0 failed; 0 ignored
```

Strict test-target Clippy passed with
`cargo clippy --locked --offline --test p3_multi_owner -- -D warnings`.
`rustfmt --edition 2021 --check tests/p3_multi_owner.rs` and repository-wide
`cargo fmt --check` passed. `python3 scripts/check_docs.py` checked 132 Markdown
files and found zero issues.

This evidence covers the synchronous in-process scoped APIs and private PTYs,
not socket/runtime adoption or scheduled watch delivery.
