# Phase 3 server-death acceptance review — 2026-10-23

Independent read-only review of the worker diff in
`tests/p3a_default_open.rs` against `P3-SERVER-DEATH-TASK.md`. The test now
records a cooperative shell PID and output marker, verifies the stale socket
after server SIGKILL, checks new generation and target identity, rejects the
old control context, verifies the external append marker stayed unchanged,
reads fresh live output, and explicitly stops the replacement workspace.
The trap and bounded wait establish only the cooperative shell case. The test
makes no orphan-free or arbitrary-descendant termination claim, and it does
not probe uncertain in-flight input.

## Finding resolution

The initial review found that the acceptance SIGKILL and fixture `Drop`
cleanup trusted a PID without validating its recorded process identity. The
parent addressed this with `Fixture::verified_server_pid`, which parses the
PID and start time, checks the process owner and non-zombie state via
`proc_pidinfo`, and requires an exact start-time match before either signal
site. This closes the review finding and follows the same PID-plus-start-time
boundary as production `default_open::live_server_pid`.

## Other contract checks

- The old context is used through the existing-control path and the test
  requires a failed response; the source routes this to
  `ContextFile::open_existing`, while `run_init` refuses an already-existing
  path. The test now snapshots the old context bytes and verifies they remain
  unchanged after rejection. The replacement also gets a separate fresh
  context.
- The old and new output markers are assembled across separate `printf`
  calls, so command echo cannot satisfy them. The new marker is confirmed by
  a live control read exactly once; the private side-effect file is compared
  byte-for-byte before and after restart.
- The new test checks the original cooperative shell exits after PTY master
  closure but signals no child or descendant. This remains scoped evidence,
  consistent with the task's explicit orphan caveat.
- `process_exited` treats a zombie as exited and only treats `ESRCH` as an
  absent PID when `proc_pidinfo` cannot report it. It does not retain a start
  identity for the PID during observation, so rapid PID reuse could cause a
  bounded false timeout; it does not signal that reused process.

The worker's initial targeted run failed in server startup before exercising
the new assertions. Parent reported an escalated rerun passed 1/1 in 3.81s;
the parent reviewed rerun then passed1/1 in3.73s. Final combined verification
is recorded separately in P3-SERVER-DEATH-RESULT.md. This
reviewer ran no tests. No code was changed during this review.
