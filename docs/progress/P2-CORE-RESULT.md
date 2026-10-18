# Phase 2 bounded terminal core result

Status: bounded same-process PTY/core experiment implemented and locally verified on the repository's tested Apple Silicon macOS host. This is not Phase 2 or M1 completion, a terminal-compatibility claim, or a security audit. The core owns one child, PTY, VT100 parser and sanitized visible snapshot. Parent integration review is complete for this core scope. Host input decoding/rendering, terminal restoration, server lifetime, persistence, reconnect, and OpenCode identity are outside this implementation.

## Implemented scope

The implementation is in [`src/terminal/mod.rs`](../../src/terminal/mod.rs), with focused unit tests in [`src/terminal/unit_tests.rs`](../../src/terminal/unit_tests.rs) and real-PTY coverage in [`tests/p2_pty.rs`](../../tests/p2_pty.rs). The dependency selection and vendored engine corrections are recorded separately in [P2-REPLACEMENT.md](P2-REPLACEMENT.md); the local engine preserves its upstream MIT notice and provenance.

The public API accepts only checked sizes (5–80 rows by 20–240 columns), a resolved launch, explicit arguments/environment, typed keys, explicit paste, and coalesced resize requests. Spawn validates paths, arguments and environment including exact `TERM=panelink-probe`; it uses owned CLOEXEC PTY descriptors, a serialized spawn path, `env_clear`, the requested working directory, and a narrowly scoped `pre_exec` hook for session/controlling-terminal setup. Child stdio is attached to the PTY. Public error/debug output does not format launch values, input, paste or screen text.

One pump reads at most 16 KiB, processes that chunk, checks child status, performs at most one bounded input write and applies the newest pending resize only after the PTY resize succeeds. The fixed 64 KiB queue admits paste atomically including bracket framing, retains exact partial-write suffixes, and discards/disables input after hard write errors, EOF, child exit or cleanup. Query callbacks admit only the exact four CSI query forms; their fixed 64-event/256-byte slots and one-second reply limit are bounded. Clipboard reads, writes and clears are discarded with no reply to child input and no host effect (updated after the operator shell-input regression on2026-10-18). Replies append behind already queued input, preserving each admitted paste's contiguous bytes.

The engine retains 1,024 normal history rows and no alternate history. The exposed snapshot is a single borrowed object containing visible cells and selected mode/cursor/style data; it uses fixed 32-byte per-cell text storage and replaces controls, bidi formatting controls, and Unicode line/paragraph separators with U+FFFD. The implementation does not expose the engine's formatted byte stream as host output. `shutdown` and `Drop` share a cached HUP/KILL/reap attempt with at most one second in each polling window; cleanup never replays input or performs a blocking wait. Explicit shutdown closes the controller even when cleanup returns an error, rejects later pump/resize calls with `SessionClosed`, disables input and preserves the last snapshot. Natural child exit remains distinct: pump continues draining final PTY output until EOF. Engine/snapshot/input allocations occur before child spawn.

## Allocation model and limits

The reviewed upstream `vt100::Cell` is 32 bytes and contains fixed storage for up to nine Unicode scalars. The vendored engine limits additional combining scalars to eight, clamps CSI cell/line insertion and downward scrolling to affected grid bounds, drops ignored CSI before dispatch, disables VTE default features, and uses VTE's fixed 1,024-byte OSC storage. A measured width-239 → 240 resize followed by CSI 240 `@` is included in the regression test; retained per-row capacity is budgeted at 1,024 cells to cover the observed 956-capacity growth. Huge CSI line/scroll counts are tested as well.

The parent-reviewed conservative budget is:

| Component | Budget (bytes) |
| --- | ---: |
| Normal history backing: 1,024 × 1,024 × 32 | 33,554,432 |
| Active normal and alternate rows: 160 × 1,024 × 32 | 5,242,880 |
| One transient row: 1,024 × 32 | 32,768 |
| Scrollback `VecDeque<Row>` slots (metadata only) | 65,536 |
| Active `Vec<Row>` slots | 10,240 |
| Fixed parser/other state and allocator margin | 2,097,152 |
| Two overlapping transient snapshot allowances: 2 × 10 MiB | 20,971,520 |
| Callback/event allowance (conservative 64 × 16 KiB) | 1,048,576 |
| Bounded input buffer | 65,536 |
| **Total** | **63,088,640 (60.17 MiB)** |

The runtime snapshot is one fixed-storage vector with at most 19,200 cells; its measured element size is at most 64 bytes, or 1,228,800 bytes. The runtime's fixed reply slots use about 16.5 KiB, below the budget's 1 MiB callback/event allowance. A `VecDeque<Row>` with capacity 2,048 reserves metadata slots only; unused slots do not contain constructed row cell vectors. The total includes two 10 MiB transient snapshot allowances and a 2 MiB fixed-state/margin allowance, and remains below 64 MiB.

This is a toolchain-specific allocation-capacity model, not an RSS measurement or a portable guarantee about allocator overhead. It relies on the inspected Rust 1.99.1 `RawVec`/`VecDeque` growth behavior and the current patched engine. Recheck it after toolchain, engine, dimension, history or snapshot changes. VTE truncates oversized OSC input and may dispatch a truncated prefix at termination: fixed storage is established, but oversized or malformed OSC is not guaranteed to be wholly rejected before a callback. No claim is made that every upstream parser state is semantically validated.

## Parent integration verification — 2026-10-17

The original implementation was reviewed by the parent. The follow-up Luna agent saved lifecycle corrections but hit a usage limit before finishing tests/reporting; the parent inspected the saved edits, finished the tests and independently verified the final tree. No live agent or temporary probe source is needed to use this implementation.

Corrections: explicit controller closure and closed-session guards; discard replies when input is disabled; treat a zero-byte write of nonempty input as `WriteZero` and discard its suffix; allocate retained state before spawning. The HUP test now waits for a marker emitted after trap installation and asserts SIGKILL plus the one-second grace interval. The earlier unhandshaked test could pass before the trap was installed; its short runtime was not escalation evidence. Paste tests now compare exact child-observed hex bytes with and without framing, after atomic oversized rejection. The clipboard integration test is named for the read behavior it actually tests; writes/clears are tested through callbacks. Added fixed-OSC/combining, controller-release and final-output-drain regressions. Exit/input testing waits for observed status rather than assuming a child has exited after a fixed sleep.

Commands ran in `/Users/sahillamba/panelink` with `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`, on macOS 26.5.2 arm64 / Rust-Cargo 1.99.1:

| Actual command | Actual result |
| --- | --- |
| `cargo test --locked --offline` | Exit 0: 30 library, 1 binary, 11 real-PTY integration tests passed; 0 doctests. |
| `cargo fmt --check` | Exit 0, no output. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0; Cargo reported 0.25s. |
| `cargo run --locked --offline -- --help` / `--version` | Exit 0 / 0. |
| `cargo run --locked --offline -- doctor` | Expected exit 1; runtime unsupported. |
| `cargo run --locked --offline -- --unknown` | Expected exit 2. |

Actual final suite summaries:

```text
running 30 tests
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 11 tests
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.03s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The CLI still exposes help/version/doctor only. Test children run through the library API; no `pty-probe` command exists yet. No build-time benchmark is inferred from these check durations.

## Remaining gates

The tests use harmless local shell/utility children and a synthetic TERM. They do not verify full-screen application behavior against the compiled `panelink-probe` terminfo, interactive vi, host keyboard decoding, paste behavior in a real host terminal, rendering/restoration, Apple Terminal compatibility, cross-version/macOS support, detached-descendant cleanup, process-server lifetime, persistence/reconnect or OpenCode identity. No host renderer or host clipboard path exists in this scope. The requested `ms-rust` skill was unavailable, so no guideline revision or compliance claim is made. The remaining Phase 2 host gates are still required; see [host input review](P2-HOST-INPUT.md).

## Subsequent resume observation

The next full-suite recheck exposed an intermittent failure in the final-output test: its child emitted a status query and exited without reading the reply. An OS write could fail with InputOutputError before `try_wait` observed exit, correctly invoking the required hard-error cleanup path. That fixture conflated normal EOF draining with a legitimate write-failure shutdown. Parent removed the query and renamed the test `final_output_larger_than_one_chunk_is_drained_after_exit`; the targeted corrected test passed (1 passed, 10 filtered). Exact query replies remain covered by fixtures that read them; injected hard writes retain their own failure test. A passing final-output test does not claim that final screen draining survives a hard write failure. The roadmap tracks full integration verification after the current renderer task.

Final subsequent integration: corrected normal-drain fixture and all prior core cases pass in the expanded application suite49 (37 library including7 renderer,1 binary,11 PTY). A separate39-test dependency decoder suite passes. Formatting, Clippy and expected diagnostic CLI exits pass. See latest INTEGRATION.md and P2-INPUT-REVIEW.md for the component checkpoint. This supersedes the pending-verification note above.
