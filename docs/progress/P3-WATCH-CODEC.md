# P3 live watch codec — 2026-10-18

W1 delivered the pure schema-4 request and response codec in
[`src/watch_protocol.rs`](../../src/watch_protocol.rs). The module defines
bounded reconciliation and finite-watch payloads; it does not perform I/O,
read terminal state, deliver events, or enable a runtime capability. Existing
schema-3 Event payloads and requestID-zero framing are unchanged.

The public API is `WatchRequest::{Reconcile, Watch}` with
`encode_watch_request` / `decode_watch_request`, borrowed
`Reconciliation<'a>` with `encode_reconciliation` / `decode_reconciliation`,
and `WatchStatus` / `WatchEnd` with `encode_watch_end` / `decode_watch_end`.
`WatchError` has fixed data-free diagnostics. Debug output redacts targets,
cursors, revisions, event counts, and reconciliation text.

The codec enforces the 56-byte Reconcile request and 72-byte Watch request,
including zero-filled reserved fields and big-endian integers. Reconciliation
uses a 128-byte prefix followed by exactly the declared UTF-8 display text; the
embedded 72-byte `SessionInfo` is validated by its existing codec. Complete
responses are bounded at 65,536 bytes and 1,024 lines. Only LF is permitted
among control characters; the codec rejects invalid UTF-8, trailing bytes,
zero screen revisions, and cursor generations that differ from the target.
The 32-byte WatchEnd requires 1–64 events and a nonzero sequence for Changed;
TimedOut and ResyncRequired carry no events. Sequence zero remains valid for
initial/empty cursors.

Focused verification used a temporary dependency-free Cargo harness that
imports the actual project modules by path, because the parent-owned
`src/lib.rs` export was pending during this worker run:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --manifest-path /private/tmp/panelink-watch-codec-harness/Cargo.toml watch_protocol::tests -- --nocapture
11 passed; 0 failed; 86 filtered out
```

The harness printed one existing `event_journal::exhaust_sequence_for_test`
dead-code warning because unrelated journal tests were filtered; no
watch-protocol warnings were emitted. Golden-layout and round-trip checks cover
all three payloads. Negative and boundary checks cover lengths, schema/kind,
reserved bytes, identifiers, generation agreement, request budgets, event and
wait limits, UTF-8, forbidden controls, line count, complete-payload size, and
Debug redaction. `rustfmt --edition 2021 --check src/watch_protocol.rs` exited
0. Root-crate integration and parent-owned cross-component checks are separate
remain separate from this W1 evidence. After the parent added the module
export, an initial root focused command
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib watch_protocol::tests -- --nocapture`
did not compile because the then-current W3-owned `server_io.rs` had a duplicate
`Clone, Copy` derive on `RequestSchema` and unresolved `invalid_data` calls.
After W3 corrected those errors, the same root command exited 0 with 11 passed,
0 failed, and 221 filtered out. Its only warning was the unrelated
`event_journal::exhaust_sequence_for_test` helper unused by this filtered run.
No compiler error or warning referenced `watch_protocol.rs`.

No dependency, runtime behavior, capability advertisement, phase-completion
claim, commit, or push was added by W1. Ownership of the two assigned paths is
released to the parent for review and integration.
