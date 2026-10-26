# P5 workspace-tab UI acceptance — 2026-10-25

Status: parent live private-PTY acceptance passed three consecutive runs after
the registry contention correction. This is scoped automated runtime evidence.

`tests/p5_workspace_tabs.rs` adds a macOS Apple Silicon outer-PTY scenario for
the documented public flow. It registers nondefault `Editors/Main`,
`Editors/Review`, and `Tools/Main` routes under a private `PANELINK_HOME`, then
starts the real `panelink workspace open PROJECT TAB` client in a controlling
PTY. The scenario observes the rendered terminal through the existing `vt100`
screen model and uses bounded command, attach, detach, and cleanup deadlines.

## Observable assertions in the test

- The initial routed client renders the selected project and tab in screen row
  zero, with every cell of the selected tab label inverse-highlighted, and
  creates route-local server/context state. Every route wait requires that
  inverse-selected label, active alternate screen and closed picker before
  shell input is sent.
- Ctrl-B `n` / `p` move between the two `Editors` tabs. The Review route gets
  its own server and context directory; returning to Main preserves the shell
  environment set earlier in that route.
- Ctrl-B `]` / `[` move from Editors to Tools and back. Tools cannot read the
  variable set in Editors, and returning to Editors cannot read the variable
  set in Tools. The separate route server locks and context directories remain
  present while switching and after Ctrl-X; the two live server generations
  are also compared before detach.
- Ctrl-B `w` displays both registered projects and the secondary tab in the
  picker; Right moves the visible selection to Tools/Main, Enter switches to
  that route, `[` returns to Editors/Main, and Escape closes a later picker
  without changing the active route. These are the supported picker keys in
  the final state handler.
- Ctrl-X restores the outer terminal settings and exits the client while all
  fixture-owned route servers and their context directories remain live.
- A raw `panelink attach --directory ... --context-dir ...` client and a plain
  no-selector `panelink` client retain the existing single-route presentation
  without the workspace header.
- Cleanup uses bounded, nonpanicking stop attempts for fixture-owned routes
  and the private default route; a process-identity-checked signal fallback is
  limited to each route's recorded server PID/start time.
- Shell markers must occupy a complete rendered screen line. Test commands
  split marker literals across adjacent shell strings, preventing an echoed
  command from satisfying its output wait.

The two distinct server roots also prove that the routes each have an initial
pane in an independent server namespace. They do not prove that two live
servers produce byte-for-byte equal full `TargetRef` values; generation is
part of that reference and is expected to differ between independent server
instances.

## Not asserted by this PTY scenario

- A key or paste arriving while route preparation is in flight is not replayed
  to the destination. The PTY does not expose a deterministic preparation
  barrier; sending input immediately after a route key would make the result
  depend on scheduler timing. This guarantee needs the host/controller
  deterministic worker seam described in the final tab-switch contract.
- Copy cancellation, server clipboard denial, preparation failure, stale
  completion, selection-save failure, and restore-before-worker-cleanup remain
  separate controller/host or manual acceptance cases.
- This private-PTY evidence is not Apple Terminal acceptance or a
  phase-completion claim.

The worker ran no Cargo commands while preparing this source. Parent runs
initially reproduced an immediate return failure after picker selection.
A temporary destination shell response probe hid that failure. Parent traced
the failed request to a registry read reporting missing registration during
selection-save lock contention. After bounded read-only Locked retry, the probe
was removed and the fixture passed three consecutive runs:
`/private/tmp/panelink-p5-tabs-contention-live-{1,2,3}.log`. Earlier failed runs
and the diagnostic probe pass remain separate evidence; see
[contention review](P5-TAB-REGISTRY-CONTENTION-2026-10-25.md).
