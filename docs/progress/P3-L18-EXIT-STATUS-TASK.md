# L18 task: child exit status in pane status and agent CLI

Worker: GPT-6-Luna (xhigh), launched by the parent 2026-10-22 via
`~/.opencode/bin/opencode run -m tesco-litellm/gpt-6-luna --variant xhigh`.

## Goal

`panelink pane list` and `panelink wait N --exit` report
`exited (status unavailable)` / `"exit_status":null` because the pane reply
has no exit status. `TerminalSession::exit_status()` exists
(src/terminal/mod.rs ~1092). Carry a confirmed exit status (exit code or
terminating signal) to the agent CLI.

## Exclusive files

- src/multi_runtime.rs (PaneEntry, pane reply codec `encode_pane_reply` /
  `decode_pane_reply`, the code that builds PaneEntry values, and its tests)
- src/p3_cli.rs (only places constructing/printing PaneEntry and the
  `request_panes` path)
- src/server/multi_session.rs (only to read the exit status from the
  session's effects when building pane status; minimal accessor)
- src/server_pty.rs (only a minimal read-only accessor if needed)
- src/agent_cli.rs (only `push_pane_json`, `finish_wait` exit output and the
  exit wait path)
- tests/p3_agent_cli.rs
- docs/progress/P3-L18-EXIT-STATUS-RESULT.md (create)

Do not edit other files. Another worker edits src/default_open.rs
concurrently; do not touch it.

## Contract

- Add `exit: Option<PaneExit>` to `PaneEntry`, where `PaneExit` is
  `Code(i32)` or `Signal(i32)`. `None` means running or not yet confirmed.
  Never infer a status that the PTY layer has not confirmed.
- Wire: use the six reserved bytes at the end of each 56-byte entry
  (offsets 50..56): 1 byte kind (0 none, 1 code, 2 signal), 1 reserved zero,
  4 bytes little-endian i32 value. Kind 0 requires value 0. Unknown kind or a
  nonzero reserved byte is rejected. Bump the pane reply schema/version field
  if the reply has one so old decoders fail closed instead of silently
  misreading; document the bump. Keep `PANE_REPLY_BYTES` unchanged.
- Codec unit tests: round-trip none/code/signal, negative/large values,
  rejection of bad kind and nonzero reserved bytes, truncated prefixes.
- agent CLI JSON: `"exit_status":{"code":N}` or `{"signal":N}` or `null`;
  `wait --exit` message `exited (code N)` / `exited (signal N)`, falling back
  to `exited (status unavailable)` only when the pane disappeared before the
  status was observed (existing collapse path). Update docs text in
  P3-AGENT-CLI.md is the parent's job; describe needed changes in your result.
- Extend tests/p3_agent_cli.rs: a pane running `exit 3` in a split pane where
  the other pane survives; because exited panes collapse, assert whichever
  observable result is truthful (status code 3 if observed, otherwise the
  documented fallback) and record which path the test exercised. Prefer
  making the wait observe the status before collapse if the server exposes it.

## Verify (record actual outputs)

- `cargo fmt --check`
- `cargo clippy --locked --offline --all-targets -- -D warnings`
- `cargo test --locked --offline --lib --bins`
- `cargo test --locked --offline --test p3_agent_cli --test p3_four_panes --test p3a_runtime --test p3b_close -- --test-threads=1` (twice)

Do not commit, do not add dependencies. Write actual results to the result file.
