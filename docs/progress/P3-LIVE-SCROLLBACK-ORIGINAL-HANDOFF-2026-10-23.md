# Phase 3 live scrollback read slice — original main-checkout handoff

Archived during consolidation on 2026-10-27. The original text below records
pending verification in the older main checkout. Current implementation and
later completed checks are in [the integrated result](P3-LIVE-SCROLLBACK-RESULT.md).
This historical pending state does not supersede those results.

Updated: 2026-10-23. This is a bounded live-read capability for the current
PTY parser. It does not read saved history files or expose a complete output
transcript.

## Behavior

`ReadSource::LiveScrollback` now reads borrowed normal-history rows from the
current `TerminalSession` through `history_screen()` and the vt100
`Screen::history_after` API. It does not change the emulator state or scrollback
viewport. The read remains available while the alternate screen is active
because the engine history API addresses the normal grid.

The result selects the newest retained physical rows up to `max_lines`, then
emits those rows in chronological order. Physical rows are separated by LF,
including rows marked wrapped by the terminal engine, with no trailing LF. The result
does not include visible screen rows and does not join wrapped rows. It
preserves cell spaces and combining text, skips wide-character continuation
cells, and applies the same `SnapshotCell` host-text sanitizer used by visible
screen snapshots. Raw escape sequences, control characters, OSC payloads,
bidi controls, and Unicode line separators are not returned.

The oldest retained cursor is found with a bounded binary search over the
current epoch's ordinal range. Each probe returns a borrowed exact-size
iterator; the search does not assume the configured ring capacity stayed the
same through resize. The output string is allocated only up to the caller's
byte limit, itself capped by `MAX_READ_TEXT_BYTES`. Cell sanitization uses one
fixed-size stack snapshot at a time. No history-row vector or full screen copy
is created.

`truncated` is true when current-epoch rows were evicted, `max_lines` omits
retained rows, or the byte budget omits text or rows. Byte truncation keeps
valid UTF-8 scalar boundaries; a separator is removed if the following row
cannot contribute any text. Empty history returns empty text with
`truncated=false`, including after RIS starts a new epoch. A zero-byte read of
retained rows returns empty text with `truncated=true`.

This reports only rows still retained by the live engine in its current
epoch. It makes no claim about evicted rows, earlier parser epochs, or a full
transcript. Saved history and previous-epoch data remain separate sources.

## Coverage and verification

Parser-helper coverage exercises newest-tail ordering, physical wrapped-row
separators, alternate-screen isolation, retained row width after resize,
Unicode combining and wide cells, bidi/line-separator/control sanitization,
UTF-8 byte boundaries, empty history, zero-byte reads, eviction, and RIS reset.
`tests/p3_detach.rs` updates the former unsupported-source assertion and reads
real PTY history while its alternate screen is active. The parent also added
an agent-CLI regression for the end-to-end command path.

The first focused run of
`cargo test --locked --offline --lib server_pty -- --test-threads=1` reported
5 passed and 1 failed: the eviction fixture expected `c,d`, while the engine's
two-row history ring correctly retained `b,c`. The fixture expectation was
corrected, then UTF-8 split-boundary and historical-width-after-resize cases
were added. The parent verifier is running after those source changes; final
results will be recorded there when available. No final focused rerun result
is claimed in this note yet.
