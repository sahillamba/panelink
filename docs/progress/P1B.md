# P1B: Protocol admission ledger

Status: implemented and verified with the available Rust toolchain.

## Implemented

- Added the dependency-free, in-memory operation ledger in `src/protocol.rs`, exported by `src/lib.rs`.
- Implemented generation and lease checks, a fixed 1,800-second TTL, renewal, duplicate payload comparison, strictly increasing admission sequences, completion records, cumulative receipts, and conservative handling of old or retired keys.
- Kept payload bytes private and volatile. Custom `Ledger` debug output contains only the generation and lease count. No operation execution, persistence, serialization, or I/O is included.
- Bounded each stored payload to 65,536 bytes and each lease to 1,024 unreceipted operations. Oversized payloads, capacity errors, and incomplete receipts do not mutate admission/receipt state.
- Added reconnect classification: queries may be reissued, durable mutations require the identical key and payload, input/signals/clipboard are never resent, and absolute resize requires reconciliation.
- Kept read-only operation status distinct from admission: `OperationStatus::NotAdmitted` is observational and never authorizes dispatch; only `Admission::Execute` from `admit` reserves a new key. Mutable ledger access must be synchronized by its server caller.
- Added unit scenarios for duplicate/conflicting payloads, payload and entry limits, pending receipt rejection and state preservation, observational status, completion and retirement, exact-boundary expiry, post-expiry completion, expired-ID reopening, wrong generation, out-of-order sequences, retry classes, redacted debug output, and zero IDs.

## Verification evidence

The initial environment probe found Cargo and rustc unavailable. The toolchain became available during this task, so the final source was checked again after formatting.

```text
$ cargo test --lib protocol
running 8 tests
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out
exit status: 0

$ cargo fmt --check
exit status: 0

$ rustfmt --check --edition 2021 src/protocol.rs
exit status: 0

$ rustc --version
rustc 1.99.1 (58cf8f9c3 2026-10-13)
exit status: 0
```

The targeted protocol tests compiled and passed. `cargo fmt --check` and the module-scoped rustfmt check passed. No runtime behavior outside these unit scenarios is claimed as verified. The requested `ms-rust` skill is not installed per the repository instructions, so no skill revision or compliance claim is recorded.

## Limits and handoff

The ledger is process-local memory for a single server generation. It does not survive restart, provide exactly-once execution across crashes, synchronize concurrent callers, create server-issued lease IDs, or execute/replay operations. A server must synchronize mutable ledger access, serialize operations, and reserve before dispatching side effects; a status observation never authorizes execution. Callers must supply nondecreasing logical timestamps. OpenCode identity and terminal/PTY behavior remain outside P1B.
