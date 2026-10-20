# P5 live screen-checkpoint capture — 2026-10-20

The two-pane `serve` runtime now opens an existing-or-new private `history/`
directory beneath its private server directory. Each server-owned `PtyEffects`
gets the exact immutable target and a shared, owner-thread-only checkpoint
writer. The server saves bounded, sanitized visible screen checkpoints for
normal and alternate buffers separately. Changed screens are coalesced to an
attempt about every five seconds per pane; the first snapshot, buffer change,
observed child exit/PTY EOF, explicit pane stop, authenticated logical detach,
and orderly server shutdown request immediate capture. The output sequence is
the cumulative PTY read-byte count, **not** a terminal command count or a
durable replay offset. No input, OSC, raw PTY stream or provider conversation
is persisted. The store validates its named private directory and file modes
before writes, syncs the temporary file and directory around atomic rename,
and accepts an identical committed record on retry after an uncertain sync.
Corrupt or replaced storage fails closed for history while PTYs keep running.

The initial `serve` JSON includes `history_capture` = `enabled` or
`unavailable`. The server emits a fixed error line once on the first detected
capture write failure, including a failure in a subsequently retired pane.
The startup availability value is only the initial observation; no attached
client UI indicator is implemented. A disk read-only recovery viewer is also
not implemented; the store library can reopen validated records read-only
after server shutdown.

## Executed checks and combined close review

- `python3 scripts/verify_non_ui.py`: 8/8 passed. Rust library 500 passed/4
  ignored, binary 93 passed; full suite 15 ignored. Decoder 39 passed, Python
  31 passed, source fingerprints match, fmt/strict Clippy/build pass and
  Markdown checker 150 files/0 findings.
- `cargo test --locked --offline --test p3b_close_runtime`: 4 passed. In private
  outer-PTY workspaces, covers keyboard close, pane listing/close/re-split,
  natural child exit, saved checkpoints for closed/last panes after server
  teardown, live screen contents, separate alternate checkpoint, and a corrupt
  destination causing a visible error while a shell continues accepting keys.
- `cargo test --locked --offline --test p5_history_store`: 4 passed, covering
  reopen, ordering/identical retry, malformed entries, symlinks and a renamed
  history directory. `p3a_default_open`: 6 passed/1 ignored. Strict rustdoc
  (`env RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib`) and `git diff
  --check` passed. A first separate rustdoc command was malformed and failed
  in the shell; only the corrected invocation is counted as the rustdoc check.

## Remaining contract gaps

One PTY read can contain multiple screen/buffer transitions: this slice sees
only the final sanitized snapshot after that read. A rapidly discarded
alternate screen can therefore be missed; neither a five-second cadence nor
the separate normal/alternate files prove every discarded alternate screen was
saved. Sync and encode run on the owner thread and can delay requests. Disk
capture has no aggregate 8 MiB quota, normal scrollback extraction, 30-day
retention, per-session clear, search, saved-layout restoration or recovery UI.
The VT100 epoch/reset and provenance review in
[P5-SCROLLBACK-SOURCE-REVIEW.md](P5-SCROLLBACK-SOURCE-REVIEW.md) remains the
next history gate. These limitations keep V-HISTORY/M4 open.
