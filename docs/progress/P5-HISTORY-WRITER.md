# ADR: bounded background history writer

**Status:** Accepted component; live adoption is tracked in
[P5-LIVE-WRITER-RESULT.md](P5-LIVE-WRITER-RESULT.md).
**Date:** 2026-10-21
**Deciders:** parent orchestrator within the existing prototype scope.

## Context

The [synthetic baseline](P5-HISTORY-THROUGHPUT.md) observed median synchronous
append calls of 22–49 ms. At the component decision, `PtyEffects` called this
storage on the sole owner thread, including before child shutdown. Pane retirement immediately
drops its effects. Moving writes requires owned data and explicit durable
receipts; it cannot depend on a retired pane polling again. A filesystem sync
cannot be canceled by timing out a thread join. Offline maintenance and a new
server generation must remain excluded while an old writer can still write.

## Decision and component contract

Implement one independently written `history_writer` component, using only
the existing dependency graph. The original component slice excluded live capture wiring. The subsequent
live integration establishes the adoption rules below; its evidence is
recorded separately.

- `HistoryBatch::new(target, epoch, token, gap, rows, screens, ended_at)`
  validates sanitized records, full target/epoch identity, increasing row
  ordinals, at most 1,024 rows and one checkpoint per normal/alternate buffer.
  Tokens/epochs/end times (when present) are nonzero. No raw PTY/input data.
  Convert retained payloads to boxed slices so excess Vec capacity is not
  hidden from accounting. Debug/error output must redact contents.
- `HistoryWriter::start(store, &ServerStorage)` verifies that the held store
  is the canonical `history` child of that server directory, then duplicates
  the verified singleton lock with CLOEXEC before starting its one worker.
  A per-server writer claim rejects a second worker and remains held until
  the first worker exits, including after a shutdown timeout. A failed worker
  start releases its claim. The caller must cease other direct mutations of
  that history store.
  The worker owns the store and duplicate until all possible writes cease.
- `try_submit(batch)` never waits on filesystem work or queue space. It either
  returns a receipt or returns ownership of the whole rejected batch with a
  fixed reason. Admission limits include the currently executing batch:
  16 batches and 32 MiB encoded payload, with each batch bounded by
  `1024 * MAX_ROW_BYTES + 2 * MAX_SCREEN_CHECKPOINT_BYTES`. Additional bounded
  metadata and the store's decode/rewrite scratch are outside this payload
  quota; it is not a total RSS claim. There is only one producer handle.
- Execute batches FIFO. Within a batch: establish/reset live history and gap,
  append rows, save checkpoints in supplied order, then mark ended. These are
  individually durable operations, **not an atomic multi-file transaction**.
  A receipt commits only after every stage's directory-sync acknowledgment.
  It carries immutable target/epoch/token identity; admission is not a commit.
  Accepted tokens must strictly increase across the writer; rejected work
  does not advance that counter. Reusing a token is rejected before effects.
- Retry only the failed stage, with the exact same bytes/time, up to three
  attempts total and a 10 ms worker-only delay. Never restart a whole batch
  after an end rename. Any storage error sets a sticky indicator even if its
  retry succeeds. Exhaustion fails the writer closed; later batches fail
  without applying effects. No automatic repair or fabricated gap/commit.
- `Receipt::poll` is nonblocking and stable on repeated calls. Worker panic
  or channel disconnect cannot leave a receipt claiming success or pending
  forever. Completed receipts may outlive the writer and pane. Failed or
  canceled accepted work may have earlier durable stages or an uncertain
  post-rename stage on disk; these results do not mean rollback/no effects.
- Explicit `shutdown(wait)` rejects new submissions and drains for at most
  min(wait, 250 ms). A timeout reports incomplete work and requests cancellation
  between storage operations. It cannot cancel a storage syscall in progress;
  the detached worker retains the singleton lock until it returns. Remaining
  row/screen stages and queued payloads are canceled, never acknowledged as
  committed. The later [timeout-only finalization slice](P5-DRAIN-METADATA.md)
  permits bounded best-effort gap/end metadata writes for accepted known-end
  canceled items while retaining the guard. Receipts remain canceled and the
  drain outcome remains timed out. An already executing final stage may return
  a successful durable receipt after timeout; timeout itself does not acknowledge
  or revoke that operation. Plain Drop requests strict cancellation and detaches
  without joining or starting finalization. No end durability at caller return
  or total filesystem wall-time bound is promised.

The establishment stage loads metadata. Missing metadata calls `begin_history`.
Existing live metadata with the same epoch and no gap calls `begin_history`
again (including its retry sync); a newer batch epoch or explicit gap calls
`mark_history`. Ended metadata or an older batch epoch fails closed. Thus an
empty initial batch still establishes retention identity, and an empty reset
still records its gap and epoch. Only the failed stage retries.

## Options considered

| Option | Complexity | Cost and consequence |
| --- | --- | --- |
| Keep synchronous capture | Low | Current measured storage wait remains on owner/close paths |
| One bounded writer | Medium | Preserves serialization; requires receipts, pressure policy and retained lock |
| Worker per pane or unbounded queue | Higher | More concurrent disk work or unbounded memory; harder retirement/lock ordering |

Select the single bounded writer. A finite retry budget sacrifices automatic
recovery from prolonged failure in favor of an explicit failed state. This
component returns rejected ownership rather than silently dropping or
coalescing it. It does not itself know the emulator cursor or lifecycle.

## Runtime adoption rules

Keep submitted and durable cursor/checkpoint state separate; only a matching
receipt advances durable state. While work is pending, coalesce observations
by resampling the engine later. Ring overflow/reset requires an explicit gap
and a retained suffix. Preserve normal/alternate checkpoint distinction and
stable metadata on retry. Reserve bounded terminal/gap metadata outside the
ordinary payload admission path: queue pressure or immediate pane retirement
must not silently discard the first end time or continuity loss. A known lost
final payload needs a gap before its end marker. Detach is not an end.

Owner poll, input, stop and child reap must run without waiting for history
receipts. Drain only at the server lifecycle boundary with the bounded wait;
surface failure/timeouts through the existing sticky diagnostic. Prove a
stalled sink cannot delay PTY/control progress, end signaling survives pane
retirement, and shutdown retains the server lock when a writer is stalled.
The subsequent integration uses these rules, with eight bounded target slots
and separate submitted/durable state. See the
[live result](P5-LIVE-WRITER-RESULT.md) for current verification; the original
component results below remain historical evidence.

## Verification and action items

- [x] Component implementation and deterministic blocked-backend tests:
  admission count/bytes including in-flight work; owned rejection; durable
  receipt delay; FIFO; exact stage retry; fail-closed behavior; cancellation;
  panic/disconnect; bounded shutdown and retained lock.
- [x] Actual HistoryStore roundtrip/end retry with sync-fault injection.
- [x] Parent source review, targeted tests and repository checks; record actual
  results below without promoting environment-blocked acceptance.
- [x] Scoped runtime capture adoption and blocked-writer PTY/control acceptance;
  see [P5-LIVE-WRITER-RESULT.md](P5-LIVE-WRITER-RESULT.md). Host/pathname and
  full reboot/layout gates remain separate.

Planned commands: `cargo test --locked --offline --lib history_writer`,
`cargo test --locked --offline --test p5_history_writer`,
`cargo test --locked --offline --lib server_storage`, `cargo fmt --check`,
and `python3 scripts/verify_non_ui.py`. This list is a plan, not test evidence.

## Actual component results

The Luna xhigh implementation owner and read-only reviewer released their
files after parent source review. The parent supplied the guarded-lock/domain
binding and six independent actual-store integration cases, and independently
ran the full verifier and all integration targets. There is no dependency or
vendored component change. Prior uncommitted work is retained. No commit or
push was made because the parent phases/required acceptance remain open.

| Check | Actual result |
| --- | --- |
| Writer units | 14 passed, including deterministic blocked count/byte admission, stage retry, FIFO/reset, failure/panic, receiver teardown, cancellation and retained lock/claim. Included in parent full run. |
| `cargo test --locked --offline --test p5_history_writer` | 6 passed: actual-store reopen with rows/both buffers/end, FIFO epoch reset, unsafe moved-store failure, domain mismatch, token reuse and exclusive writer claim. |
| Store units | 7 passed; includes 3 new retry/gap regressions below. |
| Storage-lock units | 13 passed before the final claim change; final root run also passes with claim rollback assertions. Two units are new in this slice. |
| Root library | 525 passed, 0 failed, 4 ignored. |
| Root binary | 93 passed, 1 failed at the existing pathname socket bind EPERM. |
| Integrated verifier | 7/8, exit1; decoder39, Python31, fingerprints, fmt, strict all-target Clippy, build and docs158/0 pass. |
| All 26 integration targets separately, serial within each | 73 passed, 17 failed, 11 ignored. Same four runtime targets/16 startup failures and prior host diagnostic assertion. |
| Strict library rustdoc / doctest command | Passed; 0 doctests executed. |

There are **25 new passing cases** in this slice: 14 writer units, 6 writer
integration cases, 3 store regressions and 2 retained-lock units. The initial
component compile/type/lint errors and a parent unsupported 2-row snapshot
fixture were corrected before final verification. Review caught and corrected
quota refund before payload destruction, pending receipts on panicked channel
teardown, workspace binding, receipt token reuse and multiple simultaneous
writer claims. The read-only reviewer found no remaining blocker in the
reviewed merge/guard lifecycle. Ignored/failed checks are not acceptance, and
no runtime responsiveness or host/provider/reboot result is claimed.

## Store retry corrections found during component review

The wider writer batch envelope exposed two existing `append_rows` bugs.
After a batch evicts its own prefix under the 8 MiB cap, a lost directory-sync
acknowledgment could not retry because the first incoming ordinal was no
longer saved. A valid sparse segment (for example 1,2,5,6 after a known gap)
also failed retry because ordinal offsets did not match vector indices.
Two deterministic regressions failed with `InvalidData` before reaching the
second injected sync fault. Both pass after the ordered-merge correction.

The store now validates all incoming rows before matching, checks each retained
overlap, rejects interior backfill and conflicting retained cells, and permits
an evicted prefix only alongside retained overlap or a new tail. Entirely
stale batches with no retained match are rejected. Evicted cells cannot be
compared or recovered; successful retry acknowledges only the bounded
retention result and retains its gap. A third regression covers a new gap on
a duplicate-only batch: the gap now persists before acknowledgment, including
post-rename sync retries. The original 10 store integration tests still pass.

The source change adds bounded validation/merge work to the append path. The
earlier release timings remain observations of the preceding source, not new
measurements or a performance claim for this revision.
