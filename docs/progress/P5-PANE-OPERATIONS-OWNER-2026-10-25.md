# P5 pane operations owner slice

Date: 2026-10-25
Status: owner implementation delivered for parent review; no Cargo, build, test,
runtime, or full-phase verification was run by this owner.

## Scope

This slice adds owner-side schema-13 query, swap, divider, zoom, and unzoom
handling in `src/server/multi_session.rs`, plus a pure revision-only
`PaneLayout::advance_revision` operation in `src/pane_layout.rs`. Tests were
added to the owner module and to `tests/p5_pane_operations.rs`. Runtime offer
and dispatch remain disabled.

## Public APIs

- `MultiSessionOwner::pane_operations_view(connection, now_secs)` requires a
  live connection with Read and pane-operations selected capabilities and
  returns an optional `PaneOperationsView`.
- `MultiSessionOwner::handle_pane_operation(connection, request, payload,
  now_secs, resize)` requires a live connection with Input and pane-operations
  selected capabilities, its own lease, a matching generation, and an exact
  schema-13 decode equal to the supplied typed request. It returns encoded
  response-protocol status or rejection bytes. ViewQuery is refused here.
- `PaneLayout::advance_revision(expected_revision)` advances the revision
  while preserving its full tree and geometry.

The owner checks retained exact-key payloads before target, revision, or zoom
preconditions. New admitted invalid requests are completed as Failed without
PTY effects. Operation effects use absolute sizes and do not change input
ownership, targets, process identity, or saved layout projection.

Zoom is an owner field separate from the underlying tree. The view reports
confirmed sizes for each occupied pane and keeps legacy
`pending_confirmed` relative to the underlying content rectangle. Zoom resizes
the selected PTY to root content size; hidden PTYs retain their confirmed size.
Root resize during zoom preserves the projection and underlying proportional
tree. Unzoom reconciles panes to the underlying tree. A spontaneous collapse
clears zoom when it removes the zoom target, and keeps zoom valid when a
different pane exits.

## Tests and scoped checks

Added owner fake-resize tests for zoom revision/view encoding, exact duplicate
deduplication, failed-size absolute retry, root resize during zoom, unzoom tree
preservation, stale-revision refusal without resize effects, hidden-pane size
retention, and swap/divider geometry with affected-pane resize calls. A
test-only live grant helper selects capability 64 in both the connection and
its grant; production connection capability offer remains unchanged. The new
integration test checks schema-13 view round-trip, underlying partition
preservation, and stale revision refusal. These tests are authored but have
not been run.

Scoped Rust 2021 formatting completed with:

```sh
rustfmt --edition 2021 src/server/multi_session.rs src/pane_layout.rs tests/p5_pane_operations.rs
```

`git diff --check -- src/server/multi_session.rs src/pane_layout.rs
tests/p5_pane_operations.rs` completed with no findings. No other files were
edited by this owner.

## Compatibility and verification boundaries

The handshake owner still does not offer capability 64, and
`src/multi_runtime.rs` still does not dispatch these requests. The new APIs
are not reachable through the live transport yet. Mutation responses carry
existing operation status/rejection; clients must query the authoritative
view afterward. Existing schema-9 replies remain unchanged.

The current shared tree has unrelated dirty work and the pre-existing geometry
and wire slices were reported as focused-pass only. Parent should run serial
Rust verification, including:

```sh
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::pane_zoom_commits_underlying_revision_and_exact_duplicate_has_no_effect
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::zoom_resize_failure_keeps_absolute_pending_size_for_new_key_retry
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::root_resize_during_zoom_keeps_projection_and_unzoom_restores_tree
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::stale_pane_operation_is_admitted_failed_without_resize_effects
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::zoom_resizes_only_selected_pane_and_reports_hidden_confirmed_size
cargo test --locked --offline --lib server::multi_session::runtime_helper_tests::swap_and_absolute_divider_commit_once_and_resize_affected_targets
cargo test --locked --offline --test p5_pane_operations
```

Then perform the parent's planned serial combined verification and review the
full diff. These focused owner changes do not complete the P5 pane operation,
transport, client, host, or phase acceptance gates.

## Parent review repair

Date: 2026-10-26. The parent reported two runtime-helper failures after R1:
the single-pane zoom retry fixture started at the requested effective size,
and unzoom called resize even though root resize had already brought the PTY
to its restored size. The single-pane zoom test now asserts zero resize calls;
the failed absolute zoom retry test now uses a two-pane layout so the target
must actually change size.

`apply_pane_size` and `apply_effective_size` now skip a resize when the
confirmed actual size already equals the requested size. Effective failed
targets are retained in `PaneSizeState.desired` as the absolute effective
target. View construction preserves legacy `pending_confirmed` semantics:
it reports a confirmed size only when it differs from the underlying pane
content size. Effective pending targets remain separately tracked in owner state,
including when the underlying rectangle happens to match. Hidden panes are not
marked resize-pending merely because zoom intentionally leaves their PTYs at
previous dimensions. Missing prior size state is
not replaced with the unconfirmed requested dimensions on a failed resize.
Zoom retry checks actual mismatch and the explicit matching pending target;
hidden panes retain their actual size without being marked pending solely for
their intentional zoom mismatch.

When the sole exited pane is also the zoom target, the no-collapse tombstone
path now clears zoom and advances/publishes the layout revision, avoiding a
dangling zoom target. Multi-pane sibling-exit behavior remains on the existing
collapse path. Added deterministic owner coverage for spontaneous zoom-target
exit with both multiple panes and a last-pane tombstone.

Scoped `rustfmt --edition 2021 src/server/multi_session.rs src/pane_layout.rs tests/p5_pane_operations.rs`
and `git diff --check` for the three owned Rust paths plus this evidence file
completed without findings. No Cargo, build, test, runtime, dependency,
transport, capability-offer, activation, clipboard, commit, or push command
was run. Parent serial verification remains required; the previously reported
full `runtime_helper_tests` result was 9 passing and 2 failing before these
repairs, recorded at `/private/tmp/panelink-p5-pane-owner-all-r1.log`. The
unrelated runtime `OperationClass` non-exhaustive match remains a parent-owned
repair.

## Parent follow-up — 2026-10-26

R2's broad helper run produced9 passes/3 failures, retained at
`/private/tmp/panelink-p5-pane-owner-all-r2.log`. Parent fixed the collapse
loop's last-pane early return so it reaches zoom cleanup, corrected new-key
fixture sequences after split admission, asserted the zoomed actual size in
legacy underlying-mismatch metadata, and used unequal pane dimensions in the
swap test. Equal-sized swaps correctly need no resize effect. Parent removed
an unused binding. Earlier R1 failures remain recorded separately.

All **12 runtime helper tests pass**, zero failed, in
`/private/tmp/panelink-p5-pane-owner-all-r3.log`. The full library run is pending
at this point; no complete-tree or live transport acceptance is claimed.
Capability64 is still not offered. Independent read-only owner/runtime review
`resume-20261026-pane-owner-review-r1` is active.

Parent full locked/offline serial library run now passes **871 tests, 0 failed,
4 ignored** in17.37s: `/private/tmp/panelink-p5-pane-lib-r1.log`. This covers
owner, runtime and pure components in the library; binary/client and actual
PTY integration still await the active file owners. Capability offer remains
disabled pending independent review and live fixture integration.
