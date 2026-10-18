# Live watch delivery evidence — 2026-10-18

Status: W3 implementation and focused checks complete; parent integration and
phase review remain separate. This records transport/worker/driver delivery,
not host acceptance or completion of the broader P3 phase.

## Delivery boundary

`AuthenticatedStream` continues to deny Event and Snapshot frames by default.
The worker enables only fixed 72-byte schema-3 Event frames after validating
the Welcome and its selected Events capability. Event request IDs remain zero;
Snapshot remains unsupported. Incoming client Event frames are rejected as an
invalid request role. Reconciliation and watch schema-4 payloads are decoded
before owner submission. Workers require negotiated Read and Events for either
request, and the owner methods recheck live connection, capability, and exact
target authority.

Reconciliation responses are checked against the requested full target,
complete encoded byte cap, line cap, and W1 decoder invariants before writing.
Watch replies validate the complete batch before the first byte: count and
limit, event identities and consecutive cursors, exact terminal cursor and
status, current generation, and the empty-event cursor rule for timeout/resync.
An expired-cursor resync hint cannot move behind the requested cursor. Events
use request ID zero; the terminal `WatchEnd` response preserves the request ID.

One absolute 250 ms deadline covers validation, encoding, every Event frame,
and the terminal response. At most 64 Events are emitted; including the 24-byte
frame headers and 56-byte terminal frame, the wire batch bound is 6,200 bytes.
That transport bound is distinct from the journal's 6,144 bytes of retained
logical Event records. The existing per-connection worker remains the sole
writer. No extra worker, retry, or unbounded output queue was added.

`ServerDriver` retains positive waits as owned pending requests in a Vec with
capacity eight. A validated request bounds each `Instant` wait to at most one
second. Each owner tick re-reads the journal through the live `StateOwner` API,
so grants and exact target authority are checked again before a result is
formed. The owner loop checks at most eight queued requests and eight retained
watches per tick; shutdown settles retained waits with a fixed error. The
read-only `pending_watches()` observation supports deterministic integration
coordination and does not affect authority or scheduling.

## Verification

Commands run with `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

- `cargo test --locked --offline --lib watch`: 17 passed, 0 failed. This
  includes the watch codec tests, retained finite wait, eight-watch cap and
  shutdown, ordered Event/terminal delivery, malformed-batch rejection before
  bytes, client Event role rejection, and a slow-reader shared-deadline test.
- `cargo test --locked --offline --lib transport::tests`: 19 passed, 0 failed,
  1 ignored. The ignored pathname-bind test reports sandbox `EPERM`; it needs a
  native Apple Silicon macOS run.
- `cargo test --locked --offline --lib server::tests::`: 29 passed, 0 failed,
  including existing server behavior and W2's authority, journal, reconciliation,
  and failure cases.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- `rustfmt --edition 2021 --check src/server_io.rs src/server_runtime.rs src/transport.rs`:
  passed.

Integration compile review surfaced a duplicate derive, missing fixed
`invalid_data` helper, and a missing test-only `Read` import; these were fixed.
Strict Clippy then identified the watch reply's argument count and the slow
reader test's `loop`/`let-else` form; the reply context and `while let` form
resolved those findings. The final strict run above passed.

## Remaining evidence

The Unix socketpair transport and owner-driver behavior are covered here.
Pathname binding was denied by the current sandbox, so its ignored native-host
test and broader Apple Silicon host compatibility remain unverified. Parent
owns the real PTY integration tests, CLI, exports, and phase-level review; no
claim about those checks or overall P3 completion is made here.
