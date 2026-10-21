# P5 bounded saved history — 2026-10-20

Recovery follow-up (2026-10-21): deterministic software-injected pre/post-rename
sync faults and successful first-end-time retry/expiry are now covered in
[P5-RECOVERY-FAULTS.md](P5-RECOVERY-FAULTS.md), including the real PTY capture
caller. Unchanged screen retries preserve their original capture metadata.
[P5-RESTART-ACCEPTANCE.md](P5-RESTART-ACCEPTANCE.md) adds a real private-PTY
scenario read by fresh history CLI processes: retained rows and latest
checkpoints survive, overwritten unrecorded A is unavailable, and reads cause
no storage changes or workspace launch. These supersede the corresponding
unproved-test notes below. The subsequent
[temporary-file cleanup](P5-TEMP-CLEANUP.md) discards only recognized
uncommitted files under writer exclusion, via startup and a stopped-workspace
CLI. The [synthetic throughput baseline](P5-HISTORY-THROUGHPUT.md) records
actual release measurements, including owner-thread storage-call stalls.
The [bounded writer component](P5-HISTORY-WRITER.md) now provides owned
batches, durable receipts, capacity limits and retained-lock shutdown. The
[live integration](P5-LIVE-WRITER-RESULT.md) adopts it for initial and split-created
sessions, with separate submitted/durable state and bounded terminal metadata
that survives pane retirement.
The subsequent [live workload observations](P5-LIVE-HISTORY-BENCH.md) cover
two bounded actual-store release runs and preserve burst failure flags and
preliminary missing end metadata. Full machine reboot/layout recovery,
committed-record repair and representative workload acceptance remain open. Current integrated results and sandbox
blockers are in [SESSION.md](SESSION.md).

Committed-file follow-up: [P5-COMMITTED-CORRUPTION.md](P5-COMMITTED-CORRUPTION.md)
defines whole-file refusal for malformed or truncated committed records, with
byte-preserving reopen/CLI acceptance. No prefix salvage or automatic repair is
implemented; structure-valid altered text remains undetectable without a
checksum. Temporary cleanup is separate and never promotes an orphan.

The two-pane server now consumes the VT100 `.2` normal-history cursor after
each bounded PTY pump. It sanitizes borrowed row cells with the checkpoint
cell policy and submits versioned, complete-target batches to the worker.
Pending ordinary batches coalesce screens and resample rows on later pumps. Only
rows that enter normal scrollback are retained; alternate-screen contents are
the latest separate checkpoints, not fabricated transcript rows. A row record
has immutable target, engine epoch, ordinal, capture time, original width,
wrap and validated cells. The current segment is at most **1,024 rows and
8 MiB** per managed session. Eviction, skipped ordinals and RIS mark a gap;
rows are acknowledged in memory only after the synced atomic replacement.
Malformed, partial, symlinked or unsafe records fail closed for history while
the server-owned PTY continues. No terminal input, raw output, OSC clipboard
request or provider conversation is written to these segments.

Even sessions with no scrolling get a metadata segment. Confirmed child exit
records the start of a 30-day age window. Pruning at server startup removes
only ended targets older than 30 days; a clock rollback does not accelerate
expiry. `panelink history list` gives full immutable targets, `history show
TARGET` renders validated normal rows and the latest separate screen
checkpoints as text, and `history search TARGET TEXT` searches both with
source labels. `history clear TARGET` requires the workspace server to be
stopped and removes only Panelink's saved rows/checkpoints for that target.
The read-only commands never spawn a child or replay saved terminal effects.

Review follow-up: explicit close now parses bounded available output during
the existing HUP/KILL cleanup window and takes a final sanitized capture before
closing the PTY. If cleanup output was read or EOF was not confirmed, the
segment is marked with a gap rather than claiming complete history. The current async coordinator retains final metadata after effect retirement;
unresolved storage failure remains visible and can prevent automatic expiry.
The row writer compares retained ordinals by an ordered merge (including gaps
and an already evicted prefix), and accepts a post-rename retry when retained cells/identity match even
if the new observation timestamp differs. `history clear` holds the validated
server-storage lock throughout deletion, excluding direct `serve` startup.
Focused follow-up tests cover cleanup output or an explicit incomplete result,
full-segment duplicate retry, a timestamp-only retry with conflicting-content
rejection, maintenance lock exclusion, and live-server clear denial. The
independent review's original required issues were addressed within this
scoped slice. Subsequent deterministic post-rename sync and end-time retry/expiry checks
are recorded in [P5-RECOVERY-FAULTS.md](P5-RECOVERY-FAULTS.md).
The final full verifier reran 8/8 after these changes: history store 10/10,
screen checkpoint 3/3 and close/history runtime 5/5; 15 Rust tests remain
ignored. Strict rustdoc and diff checks are recorded separately.

## Original slice verification and remaining limits

Focused integration checks demonstrate persisted normal rows surviving pane
close and server shutdown; a fresh `history show`/`search` reads them after
the server has exited, and `clear` removes the exact target. Store checks
exercise 1,024-row/8 MiB truncation, ended/live retention distinction,
identical lost-ack retry, corruption/symlink denial and isolation. The
initial parallel outer-PTY run failed three fixture startup waits before
history assertions; a serial rerun passed 5/5. A later verifier run failed
the unrelated non-TTY preflight test once, and its first isolated retry timed
out; the next isolated run passed and the full rerun passed 8/8. Root Rust
library 500 passed/4 ignored, binary 93 passed, saved-row codec 1/1,
history store 8/8, cursor 6/6 and close/history runtime 5/5; 15 total
ignored. Decoder 39, Python 31, source fingerprints, strict Clippy/fmt/build
and Markdown 153 files/0 findings passed. Strict rustdoc and `git diff
--check` passed after the final source edit. Failed runs are not counted as
passes.

Recovery scope is intentionally narrower than full workspace restoration:
there is no saved layout/setting replay, process resurrection or OpenCode
conversation recovery. A rapid alternate-buffer transition between PTY reads
may be missed; terminal content overwritten before it entered scrollback or a
checkpoint is unavailable. Gaps indicate discontinuity, not complete
transcripts. Full V-HISTORY reboot/layout acceptance and earlier Phase 2/3
host gates remain separate.

One bounded terminal pump can hold more rows than the retained engine ring;
the writer reports a gap and keeps only its surviving suffix. On rapid
natural exit, teardown also tries at most 64 immediately available PTY reads
before retaining the final snapshot, and flags a gap if EOF was not observed.
Data that never entered the ring cannot be reconstructed. Interrupted
dot-prefixed temporary files are not treated as committed rows; the current
policy discards only recognized safe orphan files as linked above, without
repairing committed records. The 8 MiB limit applies to the normal-history segment, not the two
separate screen checkpoint files. The current worker syncs the whole bounded
segment; the earlier synthetic storage timings do not establish representative
high-output workload or UI latency acceptance.
