# P5 workspace-tab live debug — 2026-10-25

## Scope and evidence

This source review is limited to the tab host, App state, key mapping, and the
real-PTY acceptance fixture. No Cargo, build, or test command was run for this
debug pass. The parent’s two existing acceptance logs remain the runtime
evidence: `/private/tmp/panelink-p5-tabs-acceptance-r1.log` and
`/private/tmp/panelink-p5-tabs-acceptance-r2.log`. Both fail at
`workspace_open_switches_registered_routes_through_nav_and_picker`, line 768,
while waiting for `Editors/Main`; the captured screen still shows the live
`Tools/Main` header and Tools shell. This is a real unresolved acceptance
failure, not a pass or a timeout to waive.

## Source findings

- `workspace_navigation` maps unmodified `[` to `PreviousProject`, and the App
  routes that action through the same typed route-request path used by `]` and
  tab navigation. `request_route` records one destination and freezes child
  effects; while frozen, ordinary input is consumed. Ctrl-X remains an
  immediate exit.
- Route preparation creates a fresh App, installs the checked destination,
  connects a fresh Net, and revalidates the registered route before publishing
  the attachment. Host adoption restores and drops the old guard, detaches the
  old worker nonblocking, resets output/compositor/decoder, opens and enters a
  fresh guard, then installs that App and worker.
- The fixture’s `wait_header("Tools", "Main")` requires the Tools/Main label
  to be inverse-highlighted and the picker to be closed. That is evidence of a
  rendered destination attachment, but it does not prove the destination
  shell has processed input and returned output.
- Source inspection did not establish a dropped `[` event, stale current-route
  identity, stale header frame, or route-switching freeze after successful
  adoption. A source-only review cannot identify which of those occurred in
  the two failed runs.

## Scoped fixture change

After picker selection renders Tools/Main, the acceptance scenario now sends
one unique `printf` to the Tools shell and waits for its exact output line
before issuing Ctrl-B `[`. The marker is not present in the echoed command,
the input is not resent, and there is no timing sleep. This distinguishes
destination shell readiness from header adoption and gives the next serialized
run a clearer failure boundary. It does not claim the shortcut now passes.

The host’s unnecessary `mut` on the fresh guard binding was removed as a
source-only lint cleanup. No App-state logic or route-controller behavior was
changed because this review found no source evidence warranting a speculative
rewrite or a new state regression.

## Parent follow-up

Run the existing focused fixture serially after the source owners release the
shared checkout. If the new Tools marker is absent, investigate destination
shell input/output readiness. If the marker appears but Ctrl-B `[` still leaves
Tools active, instrument the decoded key and typed route request at the host
boundary (without logging shell contents), then inspect the matching
request-ID result and installed route. Keep the acceptance failure open until
the expected Editors/Main frame and route-local shell identity both pass.

Scoped source checks may use Rust 2021 rustfmt and `git diff --check`; no check
result is claimed by this note.
