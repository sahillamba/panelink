# P5 saved-layout bounded async writer — bounded task — 2026-10-21

Status: assigned (stage 3 of [P5-SAVED-LAYOUT.md](P5-SAVED-LAYOUT.md)).
Read AGENTS.md, P5-SAVED-LAYOUT.md ("Future store, writer and durability
contract"), P5-LAYOUT-STORE.md (store result) and src/history_writer.rs
(`HistoryWriter::start/spawn/try_submit/shutdown`, `worker_entry`, Drop) for
house style. Write an independent, much smaller component; do not refactor
history_writer.

## Exclusive ownership

ONLY new `src/layout_writer.rs`, new `tests/p5_layout_writer.rs` (if public
API tests are useful) and new `docs/progress/P5-LAYOUT-WRITER.md`. Parent has
exported `pub mod layout_writer;` (macOS aarch64 cfg) and added in
src/server_storage.rs (parent-owned, do not edit):

```rust
pub(crate) struct LayoutWriterGuard;                       // holds dup singleton lock
pub(crate) fn claim_layout_writer(&self) -> io::Result<LayoutWriterGuard>; // exclusive
pub(crate) fn directory_path(&self) -> &Path;
pub const fn generation(&self) -> u64;
```

and in layout_store (`LayoutStore::verify_server_directory(&self, server_dir)`,
`save(bound_generation, bound_workspace, &SavedLayout) -> io::Result<SaveOutcome>`).
No other file edits, no dependencies, commits or pushes.

## Exact API

```rust
pub enum LayoutShutdownOutcome { Drained, TimedOut, Failed }        // Copy/Eq/Debug
pub enum LayoutSubmit { Accepted, Deferred, Unavailable }           // Copy/Eq/Debug
pub enum LayoutFinalStatus { NothingPending, Submitted, Unsubmitted } // Copy/Eq/Debug
#[derive(Clone, Copy, Eq, PartialEq)] // redacted Debug
pub struct LayoutKey { pub generation: u64, pub snapshot_sequence: u64 }
pub struct LayoutStatus {            // Copy, redacted Debug
    pub last_observed: Option<LayoutKey>,
    pub last_submitted: Option<LayoutKey>,
    pub last_durable: Option<LayoutKey>,
    pub unavailable: bool,           // sticky: terminal failure/refusal/setup
    pub final_status: LayoutFinalStatus,
}
pub struct LayoutWriter { .. }       // redacted Debug
impl LayoutWriter {
    pub fn start(store: LayoutStore, storage: &ServerStorage, workspace: u64) -> io::Result<Self>;
    pub fn observe(&mut self, layout: SavedLayout) -> LayoutSubmit;  // never blocks on disk
    pub fn flush(&mut self) -> LayoutSubmit;   // retry deferred, nonblocking (owner tick)
    pub fn status(&self) -> LayoutStatus;
    pub fn status_handle(&self) -> LayoutStatusHandle; // Clone; status() after writer/driver dropped
    pub fn shutdown(self, wait: Duration) -> LayoutShutdownOutcome; // wait capped at 250 ms
}
```

`start`: verify `store.verify_server_directory(storage.directory_path())`,
claim the layout guard, bind `generation = storage.generation()` and
`workspace` (nonzero) and spawn one thread `panelink-layout-writer`. The
guard moves into the worker and is dropped only when the worker exits
(including after a timed-out shutdown or Drop) — exclusion never released
early.

Queue: `mpsc::sync_channel(1)` gives one queued slot; the worker holds one
in-flight; the writer holds at most one owner-deferred latest candidate (3 x
160 encoded bytes max; store `[u8;160]` / SavedLayout values, no Vec growth).
`observe` checks the candidate matches bound generation/workspace (else
Unavailable, sticky, no submit), records last_observed, then try_send; if
Full, replace the deferred candidate (coalescing — intermediate snapshots may
be lost; that is not a durable save) and return Deferred. `flush` tries the
deferred candidate once. Never wait for queue space or disk.

Worker: for each candidate call `store.save` up to 3 attempts with 10 ms
sleep between, using the IDENTICAL SavedLayout each time (never regenerate
sequence/time). Ok -> last_durable = key. Terminal Err -> sticky unavailable,
stop processing further candidates (fail closed; drain remaining queue as
failed), keep PTYs unaffected. Catch panics (catch_unwind) as Failed. Do not
log content/IDs/paths.

`shutdown(wait)`: make ONE final nonblocking try-submit of the deferred
candidate (Submitted, or Unsubmitted if still full/stopped; record
final_status and keep last_observed), then close admission (drop sender) and
wait on a done channel for min(wait, 250ms). On timeout return TimedOut; a
running store call may finish later but the result stays TimedOut and the
worker still retains its guard until it exits. Plain Drop: cancel, never
join/wait. Submitted-after-shutdown is impossible. Expose
`pub(crate) fn shutdown_with_deadline(self, deadline: Instant)` too so the
driver can share one 250 ms budget with history (the parent integrates).

For tests add `#[cfg(test)] pub(crate) trait`-style backend injection (like
`HistoryBackend`) so unit tests can block/fail saves deterministically.

## Tests (actual runs required)

Units with a fake/blocking backend: accept, coalescing to latest with a
blocked worker (exactly last observed reaches disk after unblock), queue never
exceeds 3 candidates, retry identical bytes then success, terminal failure is
sticky and later observe returns Unavailable, panic -> Failed, scope mismatch
refused, shutdown Drained, TimedOut with a blocked backend returns within
~250ms + slack and the guard still excludes `claim_layout_writer` and
`ServerStorage::open` until unblocked, final deferred Unsubmitted vs
Submitted, status handle readable after writer dropped, second
`LayoutWriter::start` on same storage -> WouldBlock, history claim coexists.
Real-store integration: write through actual LayoutStore into a temp server
directory (use `ServerStorage::initialize` + `LayoutStore::prepare`) and read
back with `LayoutStore::load`.

Run and paste ACTUAL output into P5-LAYOUT-WRITER.md:

```text
cargo test --locked --offline --lib layout_writer -- --test-threads=1
cargo test --locked --offline --test p5_layout_writer -- --test-threads=1   (if created)
cargo clippy --locked --offline --all-targets -- -D warnings
cargo fmt --check
```
