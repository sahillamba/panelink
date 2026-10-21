# Bounded history failure/drain diagnostics — 2026-10-21

Status: delivered on the user's explicit request to put Luna on the next
slice; see [results](P5-HISTORY-DIAGNOSTICS.md). Parent owns contract, independent verification, CLI/example integration,
validator and shared docs. One GPT-6-Luna xhigh owner implements observability
inside the existing live history boundary. Prior uncommitted work is preserved.
Parent Phases3/5/6 stay open. No scheduling/loss-policy change is selected here.

## Ownership and prerequisites

FIRST read ROADMAP.md and SESSION.md, then AGENTS.md, README.md, docs/SPEC.md,
docs/PREBUILD.md, docs/PANELINK-DISCOVERY.md and P5-LIVE-HISTORY-BENCH.md.
Graph status returned Transport closed this turn; use source fallback without
repeated retries. The requested ms-rust skill is still absent; do not assert
its installation or complete guideline compliance.

Luna may edit ONLY:
- `src/history_diagnostics.rs` (new), `src/history_writer.rs`,
  `src/history_capture.rs`, `src/server_pty.rs`, `src/multi_runtime.rs`;
- `docs/progress/P5-HISTORY-DIAGNOSTICS.md` (new result note).

Parent owns `src/lib.rs`, `src/p3_cli.rs`, `examples/live_history_bench.rs`,
`scripts/validate_live_history_report.py`, `tests/test_live_history_report.py`,
new independent integration tests/raw reports and shared documentation. No
manifest/vendor/storage-format edits, commits or pushes. Parent ownership of
shared roadmap is the skill's append-only worker-state exception; workers do
not touch it. Match the crate's Rust2021 style.

## Invariants

This is observability only. Preserve FIFO, batch ownership and token handling,
stage-only retry(3 attempts/10ms), one producer, max16 accepted batches/32MiB,
at most8 capture targets, deferred screen bounds, first-end-time reservation,
submitted/durable receipt separation, existing sticky failure semantics and
one total250ms history drain. Do not add filesystem work/receipt waits to the
owner hot path. Do not change when ordinary Full is resampled or when final
payload is discarded. No queue-limit, timeout, retention, salvage or PTY-stop
policy changes and no test-only production switch.

Do not infer a failure from every admission rejection: ordinary Full can be
harmless coalescing with `history_failure=false`. New diagnostics describe
observed events, while the existing sticky flag remains the failure signal.
Expected ring/retention gaps are not automatically storage failures.

## Fixed public diagnostics API

Create `HistoryDiagnostics`: cheap Clone of an Arc holding only fixed-width
atomics (u32 reason bits and u8 writer-drain state are sufficient). No payloads,
paths, IDs, input, environment values, io::Error strings, unbounded logs/queues
or counters are needed. No diagnostic mutex or new dependency. Recording is
bounded atomic work. An independent handle stays valid before enable_history,
after pane retirement, after driver Drop and while a timed-out worker exits.
Bits are sticky; snapshots are nontransactional during concurrent activity.
Stable post-settlement snapshots must observe late worker error/cancel events.

`HistoryReason` is a Copy/Eq fixed enum with `as_str() -> &'static str`.
`HistoryDiagnosticSnapshot` is Copy/Debug and provides `reasons()` (bounded
iterator), `contains(HistoryReason)` and `writer_drain_outcome()`.
`HistoryDiagnostics::snapshot()` returns that value, and `record(reason)` is
crate-visible. Choose stable fixed discriminants <=31. Use exactly these reason
labels (enum variants may use their usual PascalCase equivalents):

- `admission_contended`, `admission_count_full`, `admission_bytes_full`;
- `final_payload_dropped_contended`, `final_payload_dropped_count`,
  `final_payload_dropped_bytes`, `final_payload_invalid`;
- `storage_error`, `retry_exhausted`, `receipt_failed`, `worker_failure`;
- `shutdown_timed_out`, `work_canceled`, `pending_receipts_at_deadline`,
  `final_metadata_unsubmitted`;
- `capture_setup_failed`, `capture_invalid`, `capture_registration_full`,
  `unknown_end_time`.

`HistoryDrainOutcome` is Copy/Eq with `NotRequested`, `Drained`, `Failed`,
`TimedOut` and `as_str()` -> `not_requested`, `drained`, `failed`, `timed_out`.
It describes the ACTUAL writer shutdown result, not whole-driver teardown or
an aggregate success claim. Publish it from `HistoryWriter::shutdown` before
returning; never change a timed-out request to drained when late work finishes.
An unsubmitted final intent can coexist with a drained writer queue; its
separate reason must remain visible. Plain Drop without shutdown leaves
NotRequested, but records actual canceled work if any.

`HistoryWriter::start` keeps its existing signature and uses a fresh diagnostic
handle. Add `start_with_diagnostics(store, storage, diagnostics)` and
`diagnostics()` returning a shared clone. The driver creates its handle in
`new` and always passes that SAME handle to start; expose
`MultiServerDriver::history_diagnostics()` returning a clone, so a caller can
retain it before enable_history. Startup failure records capture_setup_failed.
Coordinator derives the writer's handle; capture-local failures use the same
handle. A failed writer start cannot replace/strand an earlier observer.

## Exact event attribution

Preserve `SubmitReason::Full` compatibility. Add public `CapacityReason` with
`Contended`, `Count`, `Bytes` and `SubmitError::capacity_reason() -> Option<_>`.
Return it ONLY for Full: actual try-lock contention, count guard, byte guard,
or channel-full count guard. Distinguish at the check, not from later queue
snapshots. Admission should record corresponding observation bits. A rejected
batch still returns unchanged ownership; its token is not consumed.

Final-payload loss records the PAIRED reason at that actual rejection, e.g.
final_payload_dropped_bytes, not a correlation with some earlier global Full.
Invalid batch construction records final_payload_invalid. These flags do not
assert that all metadata was lost. Generic local identity/encoding/order errors
record capture_invalid; target-slot exhaustion records capture_registration_full.
Unknown confirmed end time records unknown_end_time, keeping the original
None/gap/failure behavior. Do not count a repeated finish as a fresh end time.

Record storage_error at an actual failed storage stage (including recovered
errors), retry_exhausted at the last unsuccessful attempt, worker_failure on
panic/disconnect paths, receipt_failed when coordinator sees Failed/Canceled
or mismatched identities, and work_canceled when ACCEPTED work actually ends
Canceled (not merely a cancellation request with no work). Last executing stage
may still commit after timeout and must retain that receipt status.

Record shutdown_timed_out at actual writer recv timeout. At coordinator drain
exit, record pending_receipts_at_deadline ONLY when accepted receipts remain
Pending and the budget was reached; record final_metadata_unsubmitted ONLY
when a reserved end intent is still unsubmitted at that boundary. A failed
settled receipt alone is not pending. Do not call all nonempty target slots
unsubmitted metadata. Release coordinator guards between waits and before
writer shutdown exactly as today. Record before lock/claim teardown so a
post-settlement snapshot includes worker events.

Every driver sticky failure path should have an honest reason (generic
capture_invalid is acceptable when a narrower fixed cause is unavailable).
Do not add runtime-format strings to explain it. Keep existing redacted Debug
and receipt/lock behavior intact.

## Verification and handoff

Add deterministic tests in owned modules for count/bytes/try-lock attribution,
ordinary Full with no sticky failure vs paired final loss, storage retry vs
exhaustion, canceled work vs late final commit, pending/unsubmitted drain
reasons, shared handle before enable/after Drop and target retirement. Reuse
existing fake backends/gates; do not manufacture real disk errors or increase
timeouts/ignore tests. Preserve all old tests and signatures except additive
API. Parent supplies actual-store public-API checks and benchmark integration.

Run `cargo test --locked --offline --lib history_writer -- --test-threads=1`,
`cargo test --locked --offline --lib history_capture -- --test-threads=1`,
`cargo test --locked --offline --lib history_diagnostics`, targeted runtime
cases as needed, and `cargo clippy --locked --offline --lib -- -D warnings`.
Format ONLY owned Rust files with edition2021. Parent runs all-target checks
and actual release workloads once source is stable; don't duplicate broad
suites. Report actual commands/results, failures/fixes, deviations and limits;
release files after writing the result note. No phase-completion claim from
blocked socket/host cases. No ms-rust/dependency/security audit stamps.
