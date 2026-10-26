# P5 tab registry read contention — 2026-10-25

Status: bounded read retry implemented; Rust tests and the immediate-switch
fixture have not been run for this change. Parent owns serial validation.

Parent update: the full binary target now passes **190/0**, including the
held-lock and retry-policy regressions, and the immediate live fixture without
the shell-response probe passes **three consecutive runs**. Logs:
`/private/tmp/panelink-p5-tabs-binary-final-r3.log` and
`/private/tmp/panelink-p5-tabs-contention-live-{1,2,3}.log`. Parent corrected
the lifecycle callback's bool-to-Result conversion and two test generic types
after the first compiler runs, and removed an unused-parameter warning.
Full current-tree and supporting validation is running; no host acceptance
or phase completion is inferred from these focused results.

## Diagnosis

The parent reported a prior full Rust run of 1,342 passed, 0 failed, and 15
ignored, while the live immediate `Tools → Editors` fixture intermittently
failed after picker selection. A shell-response probe made the race disappear,
so it is not treated as a fix or proof. The earlier focused fixture runs
`/private/tmp/panelink-p5-tabs-acceptance-r1.log` and
`/private/tmp/panelink-p5-tabs-acceptance-r2.log` both failed in
`workspace_open_switches_registered_routes_through_nav_and_picker` while
waiting for Editors/Main. The parent trace
`/private/tmp/panelink-tabs-trace-20261025.txt:26` recorded
`FAILED request=10 label=workspace is no longer registered`.

Source inspection found that `workspace_catalog` mapped every store error to
`()`. During asynchronous selection persistence, the writer can hold the
registry lifetime lock through file and directory sync. A concurrent
read-only `WorkspaceIndexStore::open` therefore returns `StoreError::Locked`,
which was reported as an absent route. The index record and route entries
were still present. This is source inference consistent with the trace; no
post-change runtime result is claimed.

## Change

`src/default_open.rs` adds a read-only registry retry with a two-second
deadline and eight-millisecond backoff. It retries only `StoreError::Locked`.
Corrupt, unsafe, I/O, uninitialized, missing-route, and other store errors
return immediately and remain fail-closed. Exhausted contention has a fixed
`workspace registry is busy; try again` status; other registry failures no
longer claim that the route was removed.

The retry is used for initial route resolution, route registration recheck
under the lifecycle lock, final post-connect route validation, and
asynchronous catalog refresh. The lifecycle lock is acquired before the
registration read. Every read helper drops the store before returning, and
the route worker releases its store before connect. Route-worker reads check
cancellation between attempts; controller drop remains nonjoining. No
registry mutation, launch, or input retry was added.

The existing `workspace_catalog`, `resolve_workspace_route`, and
`ensure_workspace_route` crate-local entry points retain their signatures as
wrappers. `ensure_server_checked` is private; its callback now returns a
typed `Result` so contention is not collapsed into “not registered.” Direct
call-site inspection found all current callers within `src/default_open.rs`;
they were updated. Compilation remains parent work.

## Added regression coverage

New `#[cfg(test)]` cases in `src/default_open.rs`:

- deterministic retry-policy checks that Locked retries, the deadline bounds
  waiting, cancellation exits before another attempt, and corruption or an
  absent record does not retry;
- a real `WorkspaceIndexStore` lock held by one thread while another performs
  catalog resolution, then released through a channel; the test checks the
  route resolves and registry revision remains unchanged.

These tests are not run here. Parent should run them, the client route tests,
the immediate-switch fixture without the shell-response probe, then its
serial full/supporting checks. The actual fixture result remains open until
that run.

## Local checks

`rustfmt --edition 2021 src/default_open.rs src/ui_client/routes.rs` and
`git diff --check` passed. No Cargo, build, or test command was run. No
dependency, config, activation, clipboard, commit, or push operation was
performed.
