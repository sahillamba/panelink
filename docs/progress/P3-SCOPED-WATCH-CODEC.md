# Scoped watch codec — 2026-10-18

This milestone adds a pure schema-5 codec for explicit per-session watch
cursors. It leaves the existing schema-4 codec, schema-3 Event payload, event
journal, runtime dispatch, and schema negotiation unchanged. The codec alone
does not enable multi-session watch behavior.

## Wire contract

All integers are network byte order. The cursor is
`{ generation: u64, session: u64, sequence: u64 }`; generation and session
must be nonzero and sequence zero is valid. A cursor is accepted only when its
generation and session both match the full target (or reconciliation
`info.target`). Decoding a WatchEnd also requires the caller's requested target
so a response from another session cannot be accepted implicitly.

| Payload | Size | Schema-5 cursor session | Other reserved bytes |
| --- | ---: | --- | --- |
| Reconcile request | 56 bytes | None; full target scopes it | Existing schema-4 fields remain validated |
| Watch request | 72 bytes | 64..72 | Existing schema-4 fields remain validated |
| Reconciliation prefix | 128 bytes, then bounded text | 108..116 | 101..104 and 116..128 remain zero |
| WatchEnd | 32 bytes | 24..32 | Existing schema-4 fields remain validated |

The request/response shapes retain their schema-4 lengths and validation
limits. Reconciliation remains capped at 65,536 bytes including its prefix;
the decoded text borrows from the input. To reuse the existing schema-4
validation without changing it, schema-5 decode makes a temporary copy no
larger than the already-validated complete payload bound, changes the schema
field to 4, and clears only the newly assigned session field before delegation.
The schema-5 session is validated against the decoded full target separately.
Encoding follows the corresponding schema-4 validation and then writes schema
5 and the explicit session field.

Schema-4 decoders reject schema 5 and schema-5 decoders reject schema 4. This
module does not negotiate versions, reinterpret schema-3 events, or enable any
legacy/runtime behavior. Its public request, reconciliation, cursor, end, and
error Debug implementations redact target IDs, cursor values, and snapshot
text.

## Automated evidence

Focused tests cover request/reconciliation/WatchEnd round trips, sequence zero,
maximum integer values, target generation/session mismatch, zero sessions,
all WatchEnd statuses and event-count invariants, cross-schema rejection,
reserved bytes and unknown kinds, truncation/trailing input, the exact and
over-limit reconciliation sizes, text/session-info validation, and redaction.
The reconciliation test also verifies decoded text borrows the original input.

Commands run on this checkout:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline scoped_watch_protocol -- --nocapture
9 passed; 0 failed.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings
Finished successfully with warnings denied.

cargo fmt --check
Finished successfully.
```

The first focused Cargo attempt was delayed by an unrelated in-progress syntax
error in `src/provider_binding.rs`; after its owner corrected that source, the
actual repository Cargo command compiled and all focused tests passed. No
temporary harness is part of the evidence. No dependencies or runtime paths
were added. Host verification is the current macOS/aarch64 checkout; this
codec-only result does not claim runtime session isolation or cross-platform
compatibility.
