# Phase 5 composite recovery acceptance — 2026-10-25

## Scope

Added one isolated fresh-process acceptance case at
`tests/p5_composite_recovery.rs`. It composes a committed two-pane saved
layout with real terminal history for its first historical target. The second
pane intentionally has no history, so the valid layout and per-pane history
availability are observed together.

The history fixture uses a private PTY. It records normal scrollback, displays
alternate-screen A and overwrites it with B before capture, saves B as the
alternate checkpoint, then returns to normal and saves the final normal
checkpoint. It submits one input canary whose test-only side effect is recorded
once before restart, and emits a private OSC 52 canary in PTY output.

After stopping the PTY and dropping the history handle, fresh CLI processes
inspect `recovery view` and `history show`. The test compares the private
storage tree and file bytes around each read, checks that inspection did not
start the configured shell, and confirms that the missing second-pane history
does not invalidate the saved layout. It then explicitly runs
`recovery relaunch`, checks two fresh shells in the fixture cwd/home/environment,
checks saved spatial order and new live target IDs, and verifies applied
old-to-new links. Historical history remains readable under its old ID after
relaunch. The fresh shell has a bounded stdin probe for the historical input
canary; no replay marker appears. The original input side-effect marker remains
at exactly one execution, and the read-only CLI output contains no ESC, OSC 52
introducer, or canary payload.

The subprocess runner uses private process groups and bounded waits, including
pipe collection. Fixture teardown requests stop for its detached test server
before removing the private root.

## Verification

Command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --test p5_composite_recovery -- --test-threads=1
```

Result: **1 passed, 0 failed, 0 ignored** in **2.41s**. The test binary was
written under `/private/tmp/panelink-p4-bridge-store-target`; the active
workspace binary was not overwritten. `rustfmt --edition 2021 --check
tests/p5_composite_recovery.rs` passed.

Repository-wide `cargo fmt --check` reports one formatting difference in the
concurrently edited parent-owned `src/terminal_selection.rs:240`. That file was
left untouched; formatting the new test file directly passes.

## Evidence boundary

This is orderly process/store restart evidence using a private PTY and fresh
CLI/server processes. It is not a physical Mac reboot, abrupt server-death,
filesystem power-loss, Apple Terminal interaction, or OS clipboard test. It
does not establish atomicity across the independent layout and history files,
recovery of overwritten alternate A, resumption of prior processes, or provider
conversation recovery. No production source, dependency, vendor file, global
configuration, user Terminal, or live server was changed by this task. Other
concurrent workspace edits were preserved.
