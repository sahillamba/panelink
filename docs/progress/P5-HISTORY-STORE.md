# P5 history checkpoint-store groundwork — 2026-10-20

`src/history_store.rs` adds a private, bounded store for the existing
sanitized screen-checkpoint codec. A caller supplies an existing absolute
0700 directory; the store holds its directory descriptor, checks owner and
mode, opens relative entries with `NOFOLLOW`, and requires regular 0600 files
with one hard link. The five-field immutable target and normal/alternate
buffer select separate checkpoint slots. A complete record is decoded and its
target/buffer verified before a temporary 0600 file is written, synced, and
renamed; the directory is synced before success. A previous malformed or
non-private destination fails closed. A later checkpoint must advance the
`(output_sequence, screen_revision)` pair. A read is length-bounded and
validated before it is returned; it cannot execute terminal escape sequences.

Executed: `cargo test --locked --offline --test p5_history_store` — 3 passed,
0 failed/ignored. It proves reopen, replacement ordering, distinct buffer and
target slots, malformed and partial records, unsafe permissions and symlink
rejection. `cargo clippy --all-targets --locked --offline -- -D warnings` and
`cargo fmt --check` pass. The first test run failed because the test fixture
used an invalid 4x8 viewport; corrected to the supported 8x20 viewport.
The first integrated verifier run was 7/8: the unrelated storage-lock fixture
failed once with `WouldBlock`. Its exact test passed in isolation and a full
verifier rerun passed 8/8 (500 library tests passed/4 ignored, 93 binary tests
passed, 15 ignored in the whole Rust suite, decoder 39, Python 31, Markdown
150 files/0 findings). Neither failed run is counted as a pass.

This is a library component, **not live history**. No server calls it yet; no
directory is created automatically. Single-writer coordination and a recovery
protocol for an interrupted rename/directory sync still need integration.
The store writes one checkpoint at a time and does not implement scrollback,
five-second cadence, detach/stop capture, per-session clear, 30-day expiry,
an 8 MiB aggregate quota, searching, or read-only reboot UI. The vendored
VT100 epoch/reset extraction and post-patch memory/provenance review in
[P5-SCROLLBACK-SOURCE-REVIEW.md](P5-SCROLLBACK-SOURCE-REVIEW.md) still gate
normal history. Running PTYs and provider conversations are untouched.

Update 2026-10-20: the two-pane server now uses this store for live visible
screen checkpoints. The statements above describe the original standalone
slice; see [P5-LIVE-CAPTURE.md](P5-LIVE-CAPTURE.md) for the integrated scope,
verification and remaining crash/retention/recovery limitations.
The integrated writer also checks that the named directory remains the same
private inode before a write and accepts an identical already-installed
checkpoint after an uncertain rename/directory sync. The history-store suite
now passes 4/4 (including a renamed-directory regression).
