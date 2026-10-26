# Phase 3 live scrollback integration review — 2026-10-23

## Scope and result

Independent read-only review of the uncommitted live-scrollback integration on
`codex/p3-scrollback-integration`, based on `9f3d8a0`. Reviewed the diffs in
`src/server_pty.rs`, `tests/p3_detach.rs`, and `tests/p3_agent_cli.rs` for bounded
allocation, UTF-8/control sanitization, history gaps and epochs, authority
preservation, and whether the new tests exercise the intended behavior.

**Scoped result: no actionable findings.** This review does not close Phase 3,
establish Apple Terminal compatibility, or replace the parent’s verification.
No tests were run while parent verification was in progress.

## Review notes

- `src/server_pty.rs:859-879` validates the requested byte and line limits,
  checks the existing failed-state condition, and returns the existing pane
  revision. Dispatch at `src/server_pty.rs:1027-1036` changes only the selected
  read source. The patch does not change control authentication, lease, target,
  or capability admission in the server; the same `Command::Read` path remains
  in force.
- `src/server_pty.rs:1380-1429` reserves output capacity from the validated
  byte budget, limits rows, avoids splitting UTF-8 scalars through
  `push_char`, and marks omitted content truncated. History cells go through
  `SnapshotCell::set_from`, whose existing `unsafe_for_host` policy replaces
  control, bidi, and line-separator characters before output.
- `src/server_pty.rs:1432-1474` handles an empty/current epoch directly and
  searches for the first retained cursor when the initial cursor reports a
  gap. The search interval is bounded by the `u64` ordinal; the helper then
  reports that older rows were evicted. The implementation uses borrowed
  history rows and does not materialize an unbounded intermediate collection.
- Unit coverage at `src/server_pty.rs:1632-1735` checks the newest physical-row
  tail, alternate-screen separation, wrapping, resize width, Unicode and
  sanitizer behavior, byte truncation, eviction, and RIS epoch reset.
  `tests/p3_detach.rs:540-574` checks the real PTY effect while alternate-screen
  mode is active. `tests/p3_agent_cli.rs:907-974` exercises the public CLI,
  checks source and truncation metadata, rejects visible-screen leakage, and
  checks one-row output.

## Limits

This is a source review only. The task prohibited running tests during parent
verification, so no runtime result is claimed. The vendored/selected `vt100`
history cursor behavior and retained-row bound are treated according to the
existing API contract and project history-cursor usage; they were not
revalidated by a new test run here. Broader Phase 3 owner/lifetime, host
acceptance, and managed-pane requirements remain open.
