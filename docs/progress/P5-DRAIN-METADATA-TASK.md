# P5 canceled end-metadata finalization — 2026-10-21

Status: bounded task delivered and parent verified on the user's
explicit request for Luna on the next task. One GPT-6-Luna xhigh owner handles
the bounded writer/coordinator change. Parent owns atomic storage support,
fixed diagnostics/validator, independent acceptance, measurements and shared
documentation. This does not complete Phase5 or promise complete history.

## Start and ownership

Read ROADMAP.md and SESSION.md first, then AGENTS.md, README.md, SPEC.md,
PREBUILD.md, PANELINK-DISCOVERY.md, P5-FINAL-TAIL.md and P5-HISTORY-WRITER.md.
Preserve all prior dirty work. The graph is available/indexed; use graph-first
code discovery. Requested ms-rust remains absent. No dependencies, manifests,
vendor changes, storage-format change, commits, pushes or manual handoff.

Worker may edit ONLY src/history_writer.rs, src/history_capture.rs and
P5-DRAIN-METADATA.md (new result). Parent owns history_store.rs, diagnostics,
validator/tests, library exports/test modules, runtime if needed, raw reports,
canonical spec and shared docs. Parent ownership of ROADMAP is the worker's
append-only rule exception. Do not edit parent files.

## Approved cancellation policy

Only an explicit HistoryWriter::shutdown timeout enables best-effort
end-metadata finalization. Plain writer Drop and worker disconnect/panic keep
strict cancellation. The caller still waits at most min(wait,250ms); a detached
worker may subsequently perform this bounded metadata work while retaining
the singleton guard. This explicitly revises the earlier no-new-storage-stage
after-cancel contract. Never imply metadata was durable at Drop return.

For already accepted work whose actual status is Canceled, use only the
validated batch's immutable target, epoch and known Some(nonzero end_at).
Finalize in FIFO order across in-flight and queued work, at most the16 accepted
items. Do not append rows or save screens after cancellation. No new queue,
producer, retained row payload or owner-thread IO. Queued/non-final work and
unknown end time never invent an end. A currently executing end syscall that
returns Ok after timeout retains normal Committed status; do not also finalize
or label that work Canceled.

Canceled receipts remain Canceled even if metadata finalization succeeds;
coordinator durable row/screen cursors never advance from them. The actual
writer drain outcome stays TimedOut after late completion. Preserve all prior
failure bits. Skip this path after prior fail-closed state, panic or retry
exhaustion; these are not permission to rewrite invalid records. A finalizer
failure fails closed and prevents subsequent storage mutation. Work release,
terminal receipts and diagnostic publication precede storage-guard teardown.

Preserve16 accepted batches/32MiB including inflight, coordinator8 slots,
one regular/one final per target, final suffix policy, first timestamp,
nonreplayable input, stage ordering and all existing APIs. No total worker
wall-time guarantee: a filesystem syscall can remain blocked after caller
timeout, and maintenance/startup must remain excluded until settlement.

## Parent interfaces

Parent adds crate-private
HistoryStore::finalize_incomplete_end(target, epoch, end_at) -> io::Result<()>.
It verifies identity/arguments and loads validated history before any mutation:

- missing: create an empty gap=true ended segment;
- same live epoch: preserve rows/capture metadata, set conservative gap=true
  and first end atomically in one replacement;
- older LIVE epoch: reset incompatible rows to requested epoch, gap=true/end;
- newer epoch, corrupt/mismatched/unsafe/capture-disabled record: refuse;
- matching already-ended epoch: sync directory and preserve original first
  timestamp, rows and gap unchanged;
- mismatched already-ended epoch: refuse.

Neither screen checkpoint is touched. The gap expresses uncertified continuity
for a canceled capture, not proof that rows are absent. Retry after an uncertain
post-rename result must complete directory sync and preserve the first end.
The helper uses the existing storage codec/atomic replacement, with no format
or public recovery policy change.

Worker adds matching HistoryBackend::finalize_incomplete_end with a safe
default Unsupported error and no side effects; StoreBackend forwards to the
parent helper, while focused fake backends explicitly opt in. Unsupported is
nonretryable and must not be mislabeled a storage IO error. Other helper
errors retry at most3 total attempts/10ms fixed worker delay, recording actual
storage error and RetryExhausted only on the last unsuccessful attempt.
After final failure, record the fixed failure bit, fail closed and skip further
finalizers. No receipt may become Committed from a metadata-only result.

Parent adds fixed HistoryReason indexes19/20:
CanceledEndMetadataFinalized -> canceled_end_metadata_finalized;
CanceledEndMetadataFinalizeFailed -> canceled_end_metadata_finalize_failed.
The success bit is set only after helper Ok, including idempotent directory
sync; failure is set only for terminal helper failure/refusal, not an error
later recovered by retry. Both can be present across distinct work items.
No target IDs/paths/error text or counters enter diagnostics. Record before
guard release. Schema2 keeps its shape with explicit allowlist additions;
historical reports remain unchanged.

## Evidence

Worker tests: current and queued known-end items finalize after explicit
timeout, FIFO/bounded attempts, no payload calls after cancellation, same first
timestamp, unknown/nonfinal work skipped, plain Drop strict, failed-closed and
panic skip, helper failure/exhaustion, canceled receipts, late end-stage commit,
retained singleton through blocked finalization and late diagnostic visibility.
Keep all prior lifecycle/quota/suffix cases. No production test switch.

Run and paste ACTUAL summaries in P5-DRAIN-METADATA.md:

- cargo test --locked --offline --lib history_writer -- --test-threads=1
- cargo test --locked --offline --lib history_capture -- --test-threads=1
- cargo clippy --locked --offline --lib -- -D warnings
- rustfmt --edition 2021 OWNED Rust files only

Match Rust2021 style. Record built behavior, failure corrections, deviations,
tradeoffs, limits and ownership release. Notify production freeze so parent can
run sequential release workloads without competing Cargo work. Parent supplies
real-store timeout acceptance, storage helper/fault tests, strict validator
tests, two source-stable raw workload runs, complete permitted checks and final
canonical evidence. No ignored/failed/blocked result counts as a pass.
