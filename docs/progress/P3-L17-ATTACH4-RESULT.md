# L17-ATTACH4 result — four-pane attach host acceptance

Date: 2026-10-22

## Delivered

Added `tests/p3_attach_four.rs`, an Apple Silicon/macOS gated integration test
that runs the actual `panelink serve --initialize --allow-input` server, the
actual `panelink attach` client on a private 100x24 outer PTY, and
target-scoped `panelink control` commands. It drives Ctrl-B/right and chained
navigation/splits to four panes, obtains every full target from `control
layout`, verifies distinct marker isolation using a separate context per target,
checks the rendered markers against layout rectangles with `vt100`, and verifies
all divider cells and the multi-line T/cross junction. It also checks the
four-pane menu's `Pane limit reached (4)` reason and no layout change after the
fifth attempt; closes an interior lower-left pane with Ctrl-B/x/y, verifies its
sibling expands and all three remaining panes still accept isolated input; and
finally checks Ctrl-X detach, child survival, visual terminal-mode reset and
restored termios/descriptor flags.

If server startup reports `listener_unavailable`, the test prints `SKIPPED` and
returns. Such a return is a blocked environment check, not a pass. Apple
Terminal itself is outside this private-PTY test and remains unclaimed here.

## Commands and actual results

Every Cargo invocation used the required target directory:

```text
CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/luna/target-L17-ATTACH4
```

- `cargo fmt --check` — exit 0 after the final test edit; no output.
- `cargo clippy --locked --offline --all-targets -- -D warnings` — exit 0;
  output ended `Finished dev profile ... in 3.88s`.
- `cargo test --locked --offline --test p3_attach_four -- --test-threads=1`
  (required run 1) — 1 passed, 0 failed, 0 ignored; 7.05s.
- Same command (required run 2) — 1 passed, 0 failed, 0 ignored; 7.06s.
- Same command (required run 3) — 1 passed, 0 failed, 0 ignored; 7.06s.

Actual retry history: an initial `cargo fmt --check` reported formatting diffs
in the new test and concurrent edits to `src/server/multi_session.rs` and
`src/multi_runtime.rs`; the owned test was formatted, and the final formatter
check passed after the other edits settled. An initial compiled test run reached
the real scenario and failed at divider cell `(49,23)`, which the captured
screen showed as covered by the client's transient split-status overlay. The
fixture now waits four seconds for the documented three-second status TTL to
elapse before validating the full underlying divider partition; a subsequent
focused run passed before the three required final runs. Intermediate Cargo
retries also encountered in-flight errors in concurrent files: unresolved
`PtyEffects`, `PaneExit` module paths/type bounds, and `CLOSE_SCHEMA`, plus one
temporary `index` compile error in this test that was corrected within the
owned file. After concurrent edits settled, the required builds and checks
succeeded. No source outside the exclusive files was edited.

## Limits

- This proves the private outer-PTY acceptance scenario only. No Apple Terminal
  manual check was run or claimed.
- The host-gated integration binary reports zero runnable tests on non-macOS or
  non-aarch64 targets; the three passing runs above ran on the intended
  macOS/aarch64 host. Pathname-socket denial would be reported as SKIPPED and
  must not be treated as acceptance.
- No dependency, product source, commit, or push was added by this worker.

## Parent integration after the origin/main merge — 2026-10-22

The merged Wave2 client applies splits asynchronously in a worker and refuses
a second split while one is pending. The server layout can show the new pane
before the client receives the split reply, so the test's back-to-back
Ctrl-B splits were refused. The test now waits for the client's status to
leave "split pending"/"waiting for layout" before the next split. This is a
test-timing change; the product behaviour (refusal with a visible status) is
intended. Three consecutive runs: 1 passed, 0 failed each.
