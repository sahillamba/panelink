# Combined-tree baseline audit — 2026-10-23

## Scope and current Git evidence

Reviewed the current roadmap and session handoff, `AGENTS.md`, and
`STASH-MERGE-RESULT.md`. No Cargo command was run. The worktree reports
`## HEAD (no branch)` and no changed paths. `HEAD`, `main`, `origin/main`, and
`origin/HEAD` all point to `bd84625` (`main merge`); its parent is `70f589c`
(`main merge`), whose parent is `e2725af`.

The top sections of `SESSION.md` and the roadmap's current-status block still
describe the pre-commit checkpoint. In particular, `SESSION.md` says the
five paths remain unmerged because `git add` could not create `.git/index.lock`,
and says no commit was made; `ROADMAP.md` says index resolution is the next
action and frames the current verification as an uncommitted checkpoint. Those
claims are stale after `70f589c` and `bd84625`: the current index/worktree is
clean and the merge result is committed at `bd84625`. Update only the current
status/handoff statements to point at `bd84625` and explain the two integration
commits as the recorded Git resolution. Keep historical pre-commit command
failures in the evidence record, clearly marked as historical. Do not change
the 095bf45 upstream-only test result or the stash-only result into combined
tree evidence.

## Combined-tree integration failures

The recorded full sweep was
`cargo test --locked --offline --no-fail-fast -- --test-threads=1`. It ran 49
integration targets and reported 196 passed, 27 failed, and 11 ignored; 40
targets had no failures. The exact nine failing integration targets and the
recorded causes are:

| Target | Failed tests | Cause supported by the run record |
| --- | ---: | --- |
| `p3_agent_cli` | 2 | One startup response explicitly returned `listener_permission_denied`; the second assertion failed during the same target run. The listener denial is evidenced; the second assertion's independent cause is not established. |
| `p3_attach_four` | 1 | Startup response explicitly returned `listener_permission_denied`. |
| `p3_four_panes` | 1 | Test unwrapped a missing value at `tests/p3_four_panes.rs:240`; the log does not identify why that value was absent. Cause unknown from this evidence. |
| `p3a_attach_host` | 5 | Four tests failed after startup returned `listener_permission_denied`. The full-stderr-pipe assertion timed out waiting for `error: server unavailable`; its cause is not established by that assertion. |
| `p3a_default_open` | 6 | All six attach tests exited before raw mode with “could not start the panelink server (see server.log)”. The target output does not include the server log cause; startup failure is established, root cause unknown from this output. |
| `p3a_serve_pathname` | 1 | Startup response explicitly returned `listener_permission_denied`. |
| `p3b_close_runtime` | 5 | All five attach tests exited before raw mode with the server-startup message. The target output does not include the server log cause; root cause unknown from this output. |
| `p5_recovery_chooser` | 1 | Its private-PTY helper reported that the chooser did not display its prompt and instead received the server-startup message. The helper output does not establish why the server failed. |
| `p5_recovery_relaunch` | 5 | Each affected test explicitly reported `BLOCKED: pathname socket bind denied by environment`. This is an environment permission failure, not evidence that relaunch behavior passed. |

There is corroborating evidence for pathname-bind restrictions: the same sweep
ignored pathname-socket tests with explicit EPERM notes, the binary suite had
118 passes and one pathname-bind failure, and a direct loopback bind denied
the Python probe's listener. This supports the environment restriction as a
cause for the explicit listener-denial cases. It does not identify the cause
of failures whose output only reports a missing value, an unmet stderr
assertion, or generic server startup failure. The socketpair/private-PTY
four-pane tests passing establish the scoped path they cover; they do not
clear the nine failures or establish pathname-listener acceptance.

## Commands and observed summaries

Commands actually run for this audit:

- `git status --short --branch` → `## HEAD (no branch)`; no worktree changes.
- `git log --oneline --decorate -8` → `bd84625 (HEAD, origin/main, origin/HEAD, main) main merge`; `70f589c main merge`; then `e2725af`.
- `git show -s --format='%H%n%P%n%s' bd84625` and the same for `70f589c` → confirmed the two commit parent links above.
- `rg -n 'test result:|FAILED|Error:|Permission denied|listener_permission_denied|EPERM|panicked|failures:' /private/tmp/panelink-stash-merge-all-tests.log` → located per-target failures and explicit environment-denial messages in the retained full-sweep log.
- `sed` excerpts from `/private/tmp/panelink-stash-merge-all-tests.log` → confirmed the failure detail summarized in the table.

No new test result is claimed here. The referenced temporary full-sweep log is
the evidence source for the target-level failure details; this audit records
its exact distinctions between explicit permission failures and unknown
startup/assertion causes.
