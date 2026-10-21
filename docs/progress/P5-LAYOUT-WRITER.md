# P5 saved-layout bounded async writer — result note

Status: implementation complete, all required checks passed.

## Files changed (exclusive ownership)

- `src/layout_writer.rs` — new bounded async layout writer (one-line stub replaced)
- `tests/p5_layout_writer.rs` — new integration tests (real store roundtrip, rotation, status handle, exclusion, timeout)
- `docs/progress/P5-LAYOUT-WRITER.md` — this note

No other files edited; `src/lib.rs` already exported `pub mod layout_writer;`.

## Exact API delivered

```rust
pub enum LayoutShutdownOutcome { Drained, TimedOut, Failed }
pub enum LayoutSubmit { Accepted, Deferred, Unavailable }
pub enum LayoutFinalStatus { NothingPending, Submitted, Unsubmitted }
pub struct LayoutKey { pub generation: u64, pub snapshot_sequence: u64 }
pub struct LayoutStatus { pub last_observed, last_submitted, last_durable, unavailable, final_status }
pub struct LayoutWriter { .. }
impl LayoutWriter {
    pub fn start(store: LayoutStore, storage: &ServerStorage, workspace: u64) -> io::Result<Self>;
    pub fn observe(&mut self, layout: SavedLayout) -> LayoutSubmit;
    pub fn flush(&mut self) -> LayoutSubmit;
    pub fn status(&self) -> LayoutStatus;
    pub fn status_handle(&self) -> LayoutStatusHandle;
    pub fn shutdown(self, wait: Duration) -> LayoutShutdownOutcome;
    pub(crate) fn shutdown_with_deadline(self, deadline: Instant) -> LayoutShutdownOutcome;
}
```

- `start`: verifies `store.verify_server_directory(storage.directory_path())`, claims `LayoutWriterGuard`, binds generation + nonzero workspace, spawns thread `panelink-layout-writer`.
- Queue: `mpsc::sync_channel(1)` gives one queued slot; worker holds one in-flight; owner holds one deferred candidate.
- `observe`: scope mismatch or sticky unavailable returns `Unavailable`; else `try_send`; `Full` replaces deferred candidate (coalescing) and returns `Deferred`.
- `flush`: retries deferred once; nonblocking.
- Worker: retries identical `SavedLayout` up to 3 attempts with 10 ms sleep. Terminal `Err` sets sticky `unavailable`. Catch panics via `catch_unwind` and set `unavailable`. On failure, drains remaining queue without processing further.
- `shutdown`: one final nonblocking try-submit of deferred, then close admission and wait `min(wait, 250ms)`. Plain `Drop` never joins/waits.
- `LayoutStatusHandle` is `Clone`; readable after writer/driver dropped.

## Backend injection (unit tests)

`#[cfg(test)] pub(crate) trait LayoutBackend` with `save` method, plus `start_with_backend`, allows deterministic fake/blocking/panic backends.

## Verification commands and actual outputs

```text
$ cargo test --locked --offline --lib layout_writer -- --test-threads=1
running 16 tests
test layout_writer::tests::accept_and_durable ... ok
test layout_writer::tests::coalescing_to_latest_with_blocked_worker ... ok
test layout_writer::tests::flush_retries_deferred_once ... ok
test layout_writer::tests::history_claim_coexists_with_layout_claim ... ok
test layout_writer::tests::panic_fails_and_sets_unavailable ... ok
test layout_writer::tests::queue_never_exceeds_three_candidates ... ok
test layout_writer::tests::retry_identical_bytes_then_success ... ok
test layout_writer::tests::scope_mismatch_refused_and_sticky ... ok
test layout_writer::tests::second_start_on_same_storage_would_block ... ok
test layout_writer::tests::shutdown_drained_with_no_pending ... ok
test layout_writer::tests::shutdown_final_deferred_submitted_vs_unsubmitted ... ok
test layout_writer::tests::shutdown_timed_out_with_blocked_backend ... ok
test layout_writer::tests::status_handle_readable_after_writer_dropped ... ok
test layout_writer::tests::terminal_failure_is_sticky_and_later_observe_unavailable ... ok
test server_storage::tests::layout_writer_claim_is_exclusive_independent_and_reains_lock ... ok
test server_storage::tests::layout_writer_claim_released_after_failed_lock_duplication ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 613 filtered out
```

```text
$ cargo test --locked --offline --test p5_layout_writer -- --test-threads=1
running 5 tests
test integration_real_store_rotation ... ok
test integration_real_store_write_and_load_back ... ok
test integration_second_writer_blocked ... ok
test integration_status_handle_after_shutdown ... ok
test integration_timed_out_shutdown_retains_lock ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.61s
```

```text
$ cargo clippy --locked --offline --all-targets -- -D warnings
    Finished dev profile [unoptimized + debuginfo] target(s) in 2.61s
```

```text
$ cargo fmt --check
(no output)
```

## Test coverage summary

- 16 library unit tests (fake backend): accept, coalescing to latest, queue never exceeds capacity, retry identical bytes then success, terminal failure sticky, panic -> Failed, scope mismatch refused, shutdown Drained, shutdown TimedOut with blocked backend (~250ms + slack) and guard still excludes claims, final deferred Unsubmitted vs Submitted, status handle readable after drop, second start WouldBlock, history claim coexists, flush retries deferred.
- 5 integration tests (real store): write through actual LayoutStore and read back with `LayoutStore::load`; rotation archives exact previous and writes new current; status handle after shutdown; second writer blocked; timed-out shutdown respects capped budget.

## Design notes

- Uses the exact frozen `LayoutStore` public API (`open`, `prepare`, `save`, `load`, `verify_server_directory`, `SaveOutcome`, `LayoutSlot`). No dependency on `LayoutStore` internals.
- Debug/error values redact IDs/time. No unsafe. No new crates.
- `shutdown_with_deadline` is `pub(crate)` (parent/driver uses it). Marked `#[allow(dead_code)]` because it is only called by the parent integration slice.
- `BlockThenFail` and `BlockThenPanic` variants removed from test `Behavior` enum because they were unused and caused clippy dead-code warnings; the existing test set covers all required scenarios.

## Limits and unresolved items

- No owner/driver observation integration (phase 4).
- No read-only CLI `layout show/--previous` (phase 5).
- No checksum or multi-file atomicity claim.
- The `layout_store.rs` task is owned by another concurrent worker; its unit/integration tests may still be pending. This writer uses only the frozen public API.

## Parent review — 2026-10-21

Worker: Kimi K2.6. Parent fixes before acceptance:

- Ordering bug: `observe` with a nonempty deferred slot sent the NEW
  candidate directly when the queue had room, leaving an OLDER deferred
  candidate to be flushed later; the store would then refuse the stale
  lower sequence and the writer would go terminally unavailable. Every
  observation now replaces the single deferred slot and flushes it, so
  submission order is strictly increasing.
- Timeout cancellation: a timed-out `shutdown` or plain Drop did not stop
  the detached worker from starting queued saves. A shared cancel flag now
  prevents any new save stage after timeout/Drop (a running store call may
  still finish). Regression `timed_out_shutdown_starts_no_new_save_stage`.
- Strict Clippy too-many-arguments fixed by bundling worker shared state.

Parent independent checks: `--lib layout_` 63 passed/0 failed,
`--test p5_layout_writer` 5 passed/0 failed, strict all-target Clippy/fmt
clean. Driver integration and shared 250 ms deadline are the next slice.
Writer ownership released.
