# Phase 5 screen checkpoint codec

This milestone adds a bounded codec for one normalized visible screen. It does
not add history persistence, retention, cadence, recovery, or emulator restore.

`screen_checkpoint::encode` validates a `ScreenSnapshot` and metadata before
building a record. `decode` validates the complete bounded record and returns a
borrowed, read-only view. Version one uses a 96-byte network-endian header and
48-byte row-major cells, with a maximum size of 921,696 bytes. It retains normal
and alternate buffer identity, cursor metadata, safe UTF-8 cell text, colors,
rendition, and explicit wide-cell topology. It rejects malformed lengths,
padding, reserved bytes, flags, colors, unsafe scalars, and inconsistent wide
cells. Debug output and errors redact record contents.

The Unix integration tests use an explicit `/bin/sh` launch in a mode-0700
private temporary directory and a real `TerminalSession` PTY. A child waits at
input barriers while tests capture normal and alternate snapshots containing
styled Unicode and a synthetic OSC 52 canary; a separate test captures an
80-by-240 viewport. The encoded records retain normalized cells and buffer tags,
not ESC/control-stream bytes or the canary. These tests verify the captured
screens only; they do not claim overwritten frames survive.

Checks on 2026-10-18:

- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline screen_checkpoint` — 8 codec unit tests passed.
- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p5_screen_checkpoint` — 2 real-PTY tests passed.
- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings` — passed.
- `cargo fmt --check`, `python3 scripts/check_docs.py` (111 Markdown files, 0 findings), and `git diff --check` — passed.

The codec does not write or read storage and makes no atomicity, durability,
secret-free screen-content, or automatic recovery claim. Decoded data has no
API that writes to a PTY or restores emulator state. Integration into a durable
store remains a separate review.
