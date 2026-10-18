# Multi-owner public input routing: real-PTY evidence

This adds a second real-private-PTY case to `tests/p3_multi_owner.rs` while
preserving the launch/read isolation case. Both children are launched through
the actual preparation and spawn path. Each emits a unique READY/PID sentinel,
then reads two submitted tokens and emits a structured ACK containing its PID
and monotonically increasing count.

The test connects separate Read|Input leases pinned to the two targets, uses
encoded public `Describe` and `Claim` session requests to acquire each
target's current input-owner revision, and submits through encoded control
requests. Each child's ACK appears only on its own target and contains that
child's original READY PID. It retries each first Submit with identical bytes
and key, compares the cached response, then verifies the ACK count remains one.
Each child subsequently receives a distinct second token with a fresh key and
current described owner revision; the final count is two with no third ACK and
no duplicate first ACK. Dropping the owner reaps both PIDs, checked using
signal zero and `ESRCH`; the storage lock remains held until afterward.

## Verification

On Apple Silicon macOS, the focused real-PTY target passed:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_multi_owner -- --nocapture
2 passed; 0 failed; 0 ignored
```

Strict test-target Clippy passed with
`cargo clippy --locked --offline --test p3_multi_owner -- -D warnings`.
Repository `cargo fmt --check` passed. `python3 scripts/check_docs.py` checked
129 Markdown files with zero findings. The first draft compile had an import
path error for `SessionReply`; the test now imports it from `panelink::server`
and the focused test and all checks above pass.

This verifies public in-process request routing and child input effects. It
does not establish socket/runtime adoption, launch authorization policy, or
cross-platform behavior.
