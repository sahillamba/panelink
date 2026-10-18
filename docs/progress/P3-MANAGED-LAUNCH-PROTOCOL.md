# P3 managed-launch wire codec — 2026-10-18

Implemented the bounded schema-6 pure codec in
[`src/managed_launch_protocol.rs`](../../src/managed_launch_protocol.rs),
following the parent-approved
[`codec contract`](P3-MANAGED-LAUNCH-CODEC-TASK.md). The module provides typed
launch request, status query and result values; exact-size encoders/decoders;
and fixed redacted validation and writer errors. Launch requests and status
queries use the existing `OperationClass` vocabulary (`DurableMutation` and
`Query`). This is codec functionality only: it performs no filesystem,
process, socket, persistence or dispatch work and grants no launch authority.

The request view borrows raw path, argument and environment bytes from its
input, preserving non-UTF-8 bytes and environment order without copying caller
fields. Argument and environment access is lazy. Before allocating for
environment-name uniqueness checks, decode enforces the complete 65,536-byte
payload limit, count limits and minimum body size; the temporary `BTreeSet`
stores at most 4,096 borrowed names. Encoding validates the whole request
before its first writer call. The fixed query and result forms validate their
keys, reserved bytes, explicit outcome codes and target-presence contract.

## Automated evidence

The focused locked/offline test command was:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline managed_launch_protocol::tests -- --nocapture
```

It passed **11 tests, 0 failed**. Coverage includes raw non-UTF-8 fields and
order, empty arguments/values, all path and dimension limits, key and count
limits, exact 65,536-byte request and one-byte overflow, NUL at every byte
position, malformed and trailing data, every reserved field, duplicate and
reserved environment names, every allowed result outcome/target form,
redaction, invalid-input zero-write behavior and propagated writer failure.

Strict linting passed:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings
Finished `dev` profile; exit 0
```

The package-scoped format check passed with exit 0:

```text
cargo fmt --package panelink -- --check
```

`cargo fmt --all -- --check` still exits 1 on formatting differences in the
existing vendored `vendor/vt100` sources. The package-scoped check above passes;
vendored files were left untouched.

The documentation and whitespace checks passed:

```text
python3 scripts/check_docs.py docs/progress/P3-MANAGED-LAUNCH-PROTOCOL.md
Checked 1 Markdown file(s); 0 finding(s).
python3 scripts/check_docs.py
Checked 101 Markdown file(s); 0 finding(s).
git diff --check
exit 0
```

The parent-owned `src/lib.rs` exports the module. The parent still needs to
review the source and run its requested integration checks; this note does not
claim runtime launch acceptance or milestone completion.

Parent review found an outcome-presence mismatch: the initial implementation
and its roundtrip test accepted Accepted/Unknown without a target, contrary to
the task contract. Both now require the reserved target. The corrected test
also constructs invalid wire envelopes directly and verifies decoder rejection
and zero encoder writes for missing required targets. The focused parent rerun
passed all 11 tests. The initial passing roundtrips alone did not establish
contract compliance.
