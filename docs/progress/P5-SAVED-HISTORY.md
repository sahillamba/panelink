# P5 bounded saved history — 2026-10-20

The two-pane server now consumes the VT100 `.2` normal-history cursor after
each bounded PTY pump. It sanitizes borrowed row cells with the checkpoint
cell policy and writes one versioned, complete-target segment per pump. Only
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
segment is marked with a gap rather than claiming complete history. Failed
end-time recording remains retryable while the effect handle is retained;
unresolved storage failure remains visible and can prevent automatic expiry.
The row writer compares retained ordinals by ordered offset (not repeated
linear scans), and accepts a post-rename retry when cells/identity match even
if the new observation timestamp differs. `history clear` holds the validated
server-storage lock throughout deletion, excluding direct `serve` startup.
Focused follow-up tests cover cleanup output or an explicit incomplete result,
full-segment duplicate retry, a timestamp-only retry with conflicting-content
rejection, maintenance lock exclusion, and live-server clear denial. The
independent review's original required issues were addressed within this
scoped slice. A deterministic injected failure *after* directory rename and
successful later expiry after an actual failed end-time sync have not been
demonstrated; those remain separate recovery evidence, not counted passes.
The final full verifier reran 8/8 after these changes: history store 10/10,
screen checkpoint 3/3 and close/history runtime 5/5; 15 Rust tests remain
ignored. Strict rustdoc and diff checks are recorded separately.

## Verified scope and remaining limits

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
dot-prefixed temporary files are not treated as committed rows; they require
an explicit safe repair/cleanup policy before the full reboot acceptance is
closed. The 8 MiB limit applies to the normal-history segment, not the two
separate screen checkpoint files. Each owner-thread batch syncs the whole
bounded segment; representative high-output throughput and disk latency have
not been measured.
