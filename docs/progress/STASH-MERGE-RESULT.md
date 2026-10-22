# Main/stash conflict resolution — 2026-10-22

Scope: combine main at `e2725af` with the reapplied local layout-hardening and
async-launch stash. No new phase, commit or push is part of this task.

## Resolution

- Kept upstream four-pane limits, split-tree geometry, 280-byte current saved
  layout format and legacy160-byte migration, agent CLI, four-pane recovery
  relaunch/attach and pane exit-status reporting.
- Kept the stashed serialized launch worker, owner-side pending admission,
  duplicate/uncertainty behavior, guarded journal ownership and shutdown
  cancellation, plus abandoned layout-shutdown cancellation.
- Resolved `multi_runtime.rs` by retaining both `PaneExit` and the launch-test
  module. The owner keeps MAX_PANES-sized arrays together with async state.
  Natural exit facts are retained while the split preview is reserved;
  settlement reconciles every eligible exited pane with one layout revision
  per collapse. Root resize/close remain busy until that preview settles.
- Reconciled ROADMAP/SPEC/SESSION without replacing either history. Prior
  Phase2/Phase6 scoped acceptance and its limits remain recorded; neither
  upstream993 tests nor the stash641-library/73-integration result is treated
  as evidence for this combined tree. Updated the README's saved-record size.

Two Luna owners resolved the core and documentation with exclusive files.
A third added socketpair/real-PTY acceptance for the combined four-pane launch
path; parent resolved the runtime, reviewed the integrations and ran checks.

## Verification

Final serial library suite: **675 passed,0 failed,4 ignored**. The combined
tree passes all-target compilation, strict Clippy, formatting and component
fingerprint checks and build. Decoder tests pass39/39. Python ran77 tests:
76 pass and1 fails because the OpenCode probe cannot reserve its loopback
listener in this environment. Focused owner split tests pass22/22, including async2→3→4
growth and multiple natural exits during a pending split. An initial test
fixture used two-pane dimensions for a nested split and was corrected to use
the actual selected rectangle. Another initial runtime test addressed a new
pane over a lease scoped to the original pane; the server correctly refused
it. The corrected test uses its pinned target and passes; authorization was
not weakened. A history-receipt test timed out in a concurrent verification
run; its isolated rerun and the subsequent final serial library run pass
without changing history code. Failed intermediate logs are retained.

The full `cargo test --locked --offline --no-fail-fast -- --test-threads=1`
sweep ran all49 integration targets:196 passed,27 failed,11 ignored;
40 integration targets have no failures. Binary tests:118 passed,1 failed
at pathname-socket bind EPERM. The sweep also includes the earlier now-fixed
runtime fixture failure; the final library run above supersedes that result.
It is not an all-passing full-suite result.

The9 failing integration targets are `p3_agent_cli`, `p3_attach_four`,
`p3_four_panes`, `p3a_attach_host`, `p3a_default_open`, `p3a_serve_pathname`,
`p3b_close_runtime`, `p5_recovery_chooser`, and `p5_recovery_relaunch`.
Several report explicit `listener_permission_denied` or socket-bind denial;
others fail when server startup does not produce the expected target, or at
the host/chooser prompt assertion. Those latter assertions do not establish
their root cause by themselves. None is relabeled a pass. The private-PTY
socketpair tests establish four-pane async launch behavior without a pathname
listener; the broader CLI/host acceptance remains unproved in this environment.

The full sweep's raw total was988 passed,29 failed,15 ignored before replacing
the stale library result with its final rerun. These counts are preserved as
an execution record, not recombined into a claimed successful full run.
Logs are temporary `/private/tmp/panelink-stash-merge-{all-tests,lib-serial,clippy}.log`.

## Git index boundary

The five conflicted worktree files contain no merge markers. Attempting
`git add ROADMAP.md docs/SPEC.md docs/progress/SESSION.md src/multi_runtime.rs src/server/multi_session.rs`
failed with `Unable to create .git/index.lock: Operation not permitted`.
The index therefore still reports those five paths unmerged; no staging,
commit, stash deletion or push was performed by this task. Content resolution
and index resolution are distinct. The command above marks the five paths
resolved from a context with writable Git metadata; the other reapplied
changes and untracked implementation files are preserved.
