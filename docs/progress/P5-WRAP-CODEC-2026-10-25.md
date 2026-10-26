# P5 optional row-wrap codec slice — 2026-10-25

Status: bounded codec implementation and focused automated checks complete.
This is a protocol component only; Phase 5 remains open. No handshake,
server/client integration, activation, reboot, user configuration, dependency,
vendor, commit, or push work was performed in this slice.

## Wire and API behavior

- The existing `prefix_screen_reply` and `prefix_history_reply` remain
  unchanged. The new `prefix_*_reply_with_wraps` functions accept an optional
  `Option<&[bool]>`; `None` delegates to the legacy encoder and preserves its
  exact bytes.
- With row-wrap flags present, ScreenReply sets marker bit 0 at offset 56;
  HistoryReply sets marker bit 2 in mode flags at offset 54. One LSB-first bit
  per checkpoint row follows the checkpoint. Unused high bits must be zero.
- `decode_*_reply_with_wraps(bytes, wraps_negotiated)` accepts legacy payloads
  with either negotiation value and accepts marked payloads only when the
  capability was negotiated. Existing decoders pass `false`, so extended
  payloads remain rejected through legacy APIs.
- Reply views expose `wrap_metadata_available()` and borrowed
  `row_wrapped(row) -> Option<bool>`. Absence returns `None`; out-of-range
  rows also return `None`.
- Bitmap length is derived from the fully validated checkpoint row count and
  caller flag count must match exactly. Reserved marker bits, malformed
  checkpoint data, truncated/trailing trailers, and nonzero bitmap padding are
  rejected. Screen/history reply byte limits add at most ten bytes; the
  checkpoint-specific bound and encoding are unchanged.

The existing checkpoint codec rejects viewports shorter than five rows.
Therefore row-count 1 was checked at the bitmap-length helper boundary; full
reply round trips cover the minimum supported five rows and 8, 9, and 80 rows.
This keeps checkpoint geometry policy unchanged.

## Verification

Environment: `CARGO_HOME=/private/tmp/panelink-p4-cargo-home`,
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.

- `cargo test --locked --lib ui_protocol -- --test-threads=1` — 21 passed,
  0 failed, 0 ignored; 815 unrelated library tests filtered out.
- `cargo clippy --locked --lib -- -D warnings` — passed.
- `cargo fmt -- src/ui_protocol.rs` — passed.
- `git diff --check` — passed.

Tests cover exact legacy byte equivalence, negotiated trailer round trips,
checkpoint-derived row-count mismatch, 1/8/9/80 bitmap lengths, 5/8/9/80 row
round trips, reserved bits, negotiation refusal, tail padding, truncation,
trailing bytes, deterministic malformed payloads, and exact maximum-size
80-by-240 replies.
