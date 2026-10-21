# Live history writer integration task — 2026-10-21

Status: implementation and focused checks delivered; current integrated
evidence is in [P5-LIVE-WRITER-RESULT.md](P5-LIVE-WRITER-RESULT.md). Parent owns
design/review/shared documentation; the explicitly requested Luna xhigh owner
implemented the runtime boundary. This
adopts the [tested component](P5-HISTORY-WRITER.md), not a new dependency.
Preserve all prior uncommitted work. Phase 3/5 remain open.

## Required behavior

All history store mutation/reads needed by capture must run on the existing
writer after startup preparation. `MultiServerDriver` and split-created
`PtyEffects` use the asynchronous path. Existing explicitly synchronous
component APIs may remain for compatibility, but must not be used by that live
path. Startup history cleanup/pruning remains before capture; journal/storage
identity checks are separate from history scheduling.

Submission is not durability. Keep per-target submitted and durable history
cursors plus screen metadata distinct. Only matching successful receipts advance
durable state, including after epoch reset; stale receipts cannot mark newer
observations committed. Exact accepted bytes/time survive writer stage retries.
While pending, resample rows from the bounded emulator ring later. Ring overflow
or RIS records a gap and retained suffix; no stored effects are replayed.

Keep normal and alternate checkpoints distinct. Cadence remains best effort,
five seconds for ordinary changed screens, plus buffer changes, detach and
confirmed exit/orderly shutdown. Intermediate frames may coalesce. Preserve
the latest observed checkpoint of each buffer in bounded volatile storage
while the worker is busy; never call an old receipt a newer saved screen.

Reserve bounded first-end-time/epoch/gap metadata independently of ordinary
payload admission. Pane retirement immediately drops its `PtyEffects`; final
work must survive without another poll of that handle. Queue final data behind
already accepted data for that target. If pressure drops final rows/screens,
retain an explicit gap before the end marker and set the sticky diagnostic.
Detach does not end a session. A failed writer never implies rollback or
complete history, and must not stop/restart the PTY.

## Bounds and ownership

Use at most eight registered capture targets (two live panes plus a bounded
retired backlog). Registration exhaustion disables capture for the new target
with a visible failure; it cannot reject an otherwise valid PTY launch/split.
Do not reuse a slot until its terminal result has settled, and bind handles
to full immutable targets to prevent stale-slot aliasing.

The preferred queue partition permits at most one ordinary accepted batch per
target and one final batch, preserving eight of the component's sixteen count
slots for terminal work. The 32 MiB actor payload quota includes in-flight
data. A full final payload may fall back to metadata only (gap/end), after
retaining its first end time. Transient admission contention retains bounded
metadata for retry. Any alternative must prove the same terminal reservation.

At most two deferred screen records per registered target, each bounded by
`MAX_SCREEN_CHECKPOINT_BYTES`; no additional unbounded row queue. Extra bounded
metadata and storage scratch remain distinct from payload/RSS claims. Shared
coordinator locking must never cover filesystem work or waiting for receipts.
Keep data/error Debug output redacted and introduce no production test switches.

## Lifecycle and shutdown

Owner poll, input and control replies proceed with a deliberately blocked
history sink. Child stop/reap retains its existing bounded PTY semantics and
must not wait for history acknowledgments. Driver teardown stops/captures all
children before closing the writer; automatic later effect Drop cannot submit
new work after writer shutdown.

Use one total history drain budget of 250 ms, including any coordinator flush,
not two consecutive full budgets. Timeout is incomplete capture and sets the
sticky failure signal; the worker retains its singleton lock and exclusive
claim through storage teardown as already tested. An executing final stage can
still acknowledge after timeout if durable; queued/still-unsubmitted work is
not counted committed. Surface a failure raised during driver Drop in the CLI
as well as ordinary tick failures. Do not claim machine-reboot acceptance.

## File ownership

Luna owns `src/history_writer.rs`, `src/server_pty.rs`, `src/multi_runtime.rs`,
optional new `src/history_capture.rs`, `tests/p5_live_history.rs`, and
`P5-LIVE-WRITER-RESULT.md`. Parent owns exports, CLI shutdown diagnostic,
independent tests and canonical/roadmap/session documentation. No overlap,
dependency/vendor edits, worker commits or pushes.

## Original verification plan (actual results linked above)

- Deterministic blocked writer with real PTY output/input and socketpair control
  progress; no pathname socket or manual Terminal dependency.
- Delayed receipt/epoch reset, separate normal/alternate saved checkpoints,
  ordinary queue pressure, retained suffix gaps and final close/retirement.
- Full payload pressure must not starve end metadata; registration cap and
  detached worker lock/claim retention; one bounded shutdown drain.
- Sticky failure while a shell continues and shutdown diagnostic propagation.
- Re-run existing synchronous capture/component cases without weakening them.

Commands: `cargo test --locked --offline --lib history_writer`,
`cargo test --locked --offline --lib server_pty`,
`cargo test --locked --offline --test p5_live_history`,
`cargo clippy --locked --offline --lib -- -D warnings`, `cargo fmt --check`,
then parent `python3 scripts/verify_non_ui.py` and all integration targets as
needed if the known binary EPERM prevents Cargo from reaching them.

Parent baseline this turn: existing writer integration 6, store integration 10
and fresh recovery CLI 1 all pass (17 total). The preceding full verifier was
7/8 with library525/4ignored and binary93/1failed; all26 integration targets
73passed/17failed/11ignored. These are previous checks, not current acceptance.
