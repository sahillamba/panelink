# Abrupt attach-client lifetime test review — 2026-10-23

Read-only review of the new `killed_attach_client_reattaches_to_same_live_child_without_replaying_input` case in `tests/p3_attach_four.rs`, against `P3-CLOSURE-AUDIT.md`. This is source review only; the parent rerun is still pending, so this note does not record a test pass.

The process boundaries are represented correctly. The fixture supervisor is the Rust `Child` retained for outer-PTY and cleanup duties. Its shell wrapper writes its own PID to the private marker and then uses `exec` for the real `panelink attach`, preserving that PID. `kill_attach_and_wait` signals only the marker PID, rejects the supervisor PID, observes the attach status, writes the release marker before waiting for the supervisor, and confirms the supervisor exits successfully. The test verifies the attach process is alive and the independent server is running before the signal; it then checks the server still runs and accepts control requests afterward. No Ctrl-X or `Detach` is sent to the first client before SIGKILL.

The scenario preserves the closure audit's boundaries. It describes the same target after the kill and after fresh attach, compares the managed shell PID before and after reattach, and checks that a unique output marker remains present. Initial and post-reattach output markers are split across separate `printf` arguments, so their complete strings do not occur in the echoed commands; the test checks each appears once. No input is sent between the SIGKILL and fresh attach. After reattach, the unique post-kill marker appears once and the earlier ready marker still appears once. Final cleanup explicitly stops the server, then waits for `ESRCH` on the captured managed-child PID.

One low-severity evidence improvement was identified: the test captures `attach_pid`, verifies that PID is alive, then ignores the PID returned by `kill_attach_and_wait` as `_killed_pid`. The helper re-reads the same private marker and its wrapper's `exec` preserves the attach PID, so source flow targets the intended process. An explicit equality assertion between these two values would make the killed-PID evidence direct and guard against future fixture changes.

The first killed client's terminal restoration is not asserted or claimed. `termios_restored()` is checked only for the second client after its orderly Ctrl-X exit. This test does not prove human Apple Terminal behavior, server-crash/orphan cleanup, or crash-proof recovery. The parent-run result must be recorded separately from this source review.

Parent follow-up: the test now asserts `killed_pid == attach_pid`, directly
connecting the verified live process to the PID passed to SIGKILL. Final
integrated execution is recorded in P3-OWNER-LIFETIME-RESULT.md.
