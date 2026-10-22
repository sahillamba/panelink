# L18 child exit-status implementation result

Status: implementation complete; Phase 3 remains open.

## Changes

- Added `PaneExit::{Code(i32), Signal(i32)}` and carried it from the retained,
  owner-polled PTY `ExitStatus` into pane status. The status is absent until
  `TerminalSession` has collected the child result; no status is inferred.
- Encoded exit status in bytes 50–55 of each existing 56-byte pane entry:
  kind 0/1/2, reserved zero, then little-endian signed i32. Kind 0 with a
  nonzero value, unknown kinds, nonzero reserved bytes, truncation, and old
  schema-9 pane replies are rejected. `PANE_REPLY_BYTES` is unchanged.
- Bumped the pane reply schema field from 9 to 10. The pane query remains
  schema 9; older reply decoders reject schema 10 and newer decoders reject
  schema 9.
- `panelink pane list --json` now includes `exit_status` as `{"code":N}`,
  `{"signal":N}`, or `null`. `wait --exit` reports a confirmed status as
  `exited (code N)` / `exited (signal N)`; `null` and
  `exited (status unavailable)` are used only when the original pane has
  already disappeared during collapse.
- Updated the split-pane exit integration scenario to run `exit 3` and accept
  only code 3 when observed before collapse or the documented disappeared-pane
  fallback. Its test output records which path occurred.

## Verification evidence

All Cargo commands below used:

```text
CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/luna/target-L18-EXIT-STATUS
```

- `cargo fmt --check`: passed before the concurrent syntax error appeared in
  `src/default_open.rs`. A final rerun was blocked by the same out-of-scope
  parse error (`unexpected closing delimiter`, moving between lines 2048 and
  2073 as concurrent edits continued).
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed before
  the concurrent syntax error appeared. Final rerun was blocked by the parse
  error in `src/default_open.rs`.
- `cargo test --locked --offline --lib --bins`: final run exit 0;
  library 643 passed, 0 failed, 4 ignored; binary 102 passed, 0 failed,
  0 ignored. The initial run caught one pane-query test failing after the
  reply-schema bump; query schema was kept at 9 and the test then passed. A
  later rerun was blocked by the concurrent `src/default_open.rs` parse error.
- First
  `cargo test --locked --offline --test p3_agent_cli --test p3_four_panes --test p3a_runtime --test p3b_close -- --test-threads=1`:
  exit 0. p3_agent_cli 2/2, p3_four_panes 1/1, p3a_runtime 1/1,
  p3b_close 2/2 (total 6 passed).
- Second exact integration command: blocked at compile time by concurrent,
  out-of-scope edits in `src/default_open.rs`, with `unexpected closing
  delimiter` reported at lines 2048, 2062, 2074, and 2073 across repeated
  attempts while the file was changing. The L18 worker did not modify
  `src/default_open.rs`.
- `git diff --check` for the six owned source/test paths: exit 0, no output.

## Test-path observation and limits

The real-server integration exercised the documented pane-collapse fallback.
An explicit serial rerun of
`target-L18-EXIT-STATUS/debug/deps/p3_agent_cli-430a2f0f18de4e57 --exact
agent_cli_list_read_claim_send_wait_events_close_and_stale_indices --nocapture
--test-threads=1` reported `L18 exit-status acceptance exercised documented
pane-collapse fallback` and passed 1/1. This shows that current owner polling
and sibling collapse can happen before the wait client retrieves pane status;
the test accepts code 3 if collected first and otherwise verifies the null
fallback for the disappeared original target.

The status is only exposed while the exited pane remains present in the current
layout. Once a sibling collapse removes that pane, there is no retained
workspace status query for its tombstone, so the CLI reports the documented
fallback. No new dependency was added. Documentation follow-up is needed in
`docs/progress/P3-AGENT-CLI.md` to describe exit-status JSON and the schema-10
reply (owned by the parent per the task).
