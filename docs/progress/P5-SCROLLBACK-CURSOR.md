# P5 read-only normal-history cursor — 2026-10-20

This bounded extraction slice revises the **selected third-party** VT100
component from local `0.16.2-panelink.1` to `.2`; the upstream remains VT100
0.16.2, MIT copyright 2016 Jesse Luehrs. No package, feature, native build
script, allocator or external dependency was added. Root `Cargo.toml` and
`Cargo.lock` pin the exact new local version. The upstream registry archive
SHA-256 was independently rechecked as
`054ff75fb8fa83e609e685106df4faeffdf3a735d3c74ebce97ec557d5d36fd9`.
The regenerated `vendor/vt100/panelink.patch` applied to a clean extraction of
that archive with every patched `src/` file and `Cargo.toml` comparing equal.
`PROVENANCE.json` now fingerprints the actual revised source and patch;
`scripts/check_component_sources.py` reports both components `match`.

`Grid::scroll_up` assigns one checked, increasing ordinal **only** when a
removed row enters normal scrollback. The row is moved, not cloned; every
retained record exposes the source width, wrap flag and borrowed styled cells.
`Screen::history_after(HistoryCursor { epoch, ordinal })` borrows only rows
after the caller's last accepted ordinal without altering the view offset,
including while the alternate grid is active. A stale epoch returns `Reset`,
evicted rows return `Gap`, future/zero-epoch cursors return `InvalidCursor`.
RIS advances the epoch even if no subsequent row is appended. Ordinal/epoch
overflow disables extraction without stopping screen parsing, and remains
disabled across resets. Zero history capacity yields no rows. Cursor and row
`Debug` redact all identifiers and text.

## Bounds and scoped source review

The existing 1,024 normal-row limit and zero alternate history are unchanged.
On tested Apple Silicon / Rust 1.99.1, `Row` is 40 bytes (previously 32)
and `Grid` is 104 bytes; at the existing 2,048-slot conservative deque
capacity, incremental row metadata reserves at most **16,384 bytes**, plus
16 bytes per grid for an ordinal/flag (two grids), and 8 bytes for `Screen`'s
epoch: at most **16,424 bytes** of static/container metadata beyond the
previous 63,088,640-byte model. The resulting conditional model is
**63,105,064 bytes** (60.18 MiB), still below 64 MiB. The base cell vectors,
retained-row count, snapshots, OSC and callback/input budgets are unchanged;
the borrowed delta allocates no second cell array and no per-pump history
copy. This is a scoped source/capacity model, not a measured RSS or a
cross-platform/toolchain promise. No persistent writer scratch is included.
The full source change, including tests, is reproduced in the patch and the
upstream MIT notice remains in `vendor/vt100/LICENSE`.

## Executed checks

- `cargo test --locked --offline --test p5_scrollback_cursor`: 6 passed,
  including exact LF/CSI S row order, alternate isolation, stale/future/reset
  cursors, a 1,024-row bounded suffix after eviction, partial-region exclusion,
  width/wrap/rendition after resize, and redacted Debug.
- `cargo test --offline --manifest-path vendor/vt100/Cargo.toml history_tests`:
  5 passed for checked ordinal/epoch exhaustion and Apple Silicon struct sizes.
  The first offline standalone attempt was blocked by missing dev dependencies;
  the required crates were downloaded by a subsequent run. The standalone
  generated `vendor/vt100/Cargo.lock` was removed afterwards; root lockfile
  remains authoritative.
- `cargo clippy --locked --offline --all-targets -- -D warnings`, root fmt,
  and fingerprint checker passed in focused checks. Final
  `python3 scripts/verify_non_ui.py` passed 8/8: root Rust library 500
  passed/4 ignored, binary 93 passed, cursor integration 6 passed,
  overall 15 ignored; decoder 39 passed, Python 31 passed, both component
  fingerprints matched, strict Clippy/fmt/build and Markdown checker
  152 files/0 findings. `env RUSTDOCFLAGS="-D warnings" cargo doc --locked
  --offline --no-deps --lib` passed, as did `git diff --check`.

## Next gate

This API is **not connected to Panelink history storage or CLI**. The existing
checkpoint writer still saves only visible screens. Before persisting rows,
define bounded normalized cell records and a durable cursor/gap/reset writer
that advances only after accepted storage, obeys the 8 MiB/age limits and
never executes or replays stored bytes. Intervening RIS resets beyond the
latest observable epoch, terminal output that enters and leaves scrollback
within one 16 KiB parser feed, and corrupted records must produce honest
capture gaps. Retention, deletion, search and recovery UI remain open.

This was the next gate at the time of the cursor-only slice. The subsequent
scoped writer and CLI are done at `f82d968`; current open V-HISTORY gates and
pause state are in [P5-SAVED-HISTORY.md](P5-SAVED-HISTORY.md) and
[ROADMAP.md](../../ROADMAP.md).
