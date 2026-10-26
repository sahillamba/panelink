# P5 opt-in workspace-tab UI integration — 2026-10-25

Status: source-verified bounded UI integration. Phase 3 and Phase 5 remain
open. This note records implementation scope and evidence; it does not declare
the tab feature or either phase complete.

## User-visible scope

The opt-in `panelink workspace open` client displays a persistent project/tab
strip on row zero. Clicking the project control opens its chooser; clicking a
tab requests that route. Ctrl-B then `w` opens the chooser, `n`/`p` navigate
tabs, and `[`/`]` navigate projects. The
catalog is an immutable snapshot for navigation; refresh retains the displayed
active snapshot if its route has been removed. Projects are named groups, with
no project-root discovery. The UI navigates registered entries but does not
create, remove, or stop projects or tabs.

Plain `panelink` and raw attach retain the legacy headerless client. The tab UI
is available only through the opt-in workspace-open route.

## Route and input behavior

- At most one route preparation is pending. During preparation, the current
  route remains displayed and child input is frozen.
- Switching cancels local selection/copy work. Keys that were not submitted
  through the old route are discarded with the existing notice; no key or
  uncertain operation is retargeted to the destination.
- If preparation fails, the old attachment remains the actual route.
- Successful installation uses a fresh App and network attachment for the
  destination. Ctrl-X restores the host terminal first, then makes a
  nonjoining best-effort detach of the old worker.
- Persisting the selected route is asynchronous and begins after installation.
  It may finish after detach; there is no rollback or completion-order
  guarantee.

## Evidence

The parent reports that a fresh focused client run passed **98/0** at
`/private/tmp/panelink-p5-tabs-client-r4.log`. It includes the injected
stalled-save and nonjoining-drop cases. These are controller/backend tests and
do not prove real filesystem `fsync` timing. The tab foundations also passed
**24 compositor tests** and **8 workspace-tab model tests**.

The first two live outer-PTY fixture attempts failed after returning from the
picker (`-r1` and `-r2`). An agent probe then added one Tools-shell response
before returning from the picker. With that probe in place, the live fixture
passed **1/0** at `/private/tmp/panelink-p5-tabs-acceptance-r3.log`. This is a
probe-assisted fixture pass only. It does not establish that the original
fixture race was fixed or that all input-readiness cases are proven. Immediate
post-header shortcut responsiveness was subsequently investigated by the
parent. A failed request reported missing registration while an asynchronous
selection save held the registry lock. Catalog reads discarded the typed
Locked error. The read-only retry now waits only for Locked, off the host
loop, with cancellation and a two-second deadline. Unsafe, corrupt and missing
data still fail closed. The shell-response probe was removed; the immediate
fixture then passed three consecutive runs. See
[registry contention evidence](P5-TAB-REGISTRY-CONTENTION-2026-10-25.md).

The complete post-fix binary target passes **190/0**, including deterministic
retry policy and a real held-registry-lock regression. Logs are
`/private/tmp/panelink-p5-tabs-binary-final-r3.log` and
`/private/tmp/panelink-p5-tabs-contention-live-{1,2,3}.log`.

The full Rust **1304/0** baseline predates the tab UI integration. The final
tab checkpoint results follow below; that older baseline is not current-tree evidence. No
claim is made here about real Apple Terminal behavior, real clipboard access,
reboot restoration, or representative workload performance.

The first full tab run timed out in a history receipt unit test; an unchanged
isolated recheck and full rerun passed **1342/0/15 ignored** before the registry
retry correction. The final post-correction full run passes **1344/0/15 ignored**,
71 result groups, `/private/tmp/panelink-p5-tabs-full-final.log`. Decoder39,
component fingerprints, root fmt, strict all-target Clippy, isolated build,
Python232run with one opt-in skip, and Markdown359/0 supporting checks also
pass. Earlier
failures remain recorded rather than reclassified as passes.

## Remaining acceptance and phase status

Manual Apple Terminal acceptance, real clipboard acceptance, physical reboot
and workload acceptance, and keyboard copy mode remain open. Phase 3 and Phase
5 are not complete. No phase-completion commit, push, or feature activation is
authorized by this evidence update.
