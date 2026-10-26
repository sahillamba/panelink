# P5 pane operations client transport — 2026-10-26

Status: bounded client transport source is implemented. No Cargo, build, or
test command was run for this slice. Host key handling, rendering, module
export, and result application remain parent integration work.

## Client API

`src/ui_client/pane_operations.rs` defines the redacted `PaneAction` variants
`Swap`, `DividerSet`, and `ZoomSet`. The action carries complete captured
targets and the observed layout revision. `captured_target()` chooses the
connection target; `request(key)` converts the action to the schema-13 typed
request without allocating a key. `PaneOperationUpdate` carries the validated
`PaneOperationsView` and terminal `Outcome` for `Applied` or `Partial` results.

`WorkerHandle::pane_operation(id, action)` is an inherent, bounded asynchronous
submission. It returns queue admission immediately. Its result is
`ClientResult::PaneOperation { id, action, result }`. The worker reserves the
maximum 504-byte view plus the control-result reserve and action plan size.
Poll results also include 504 bytes for an optional pane-operations view.

`Net::pane_operation` verifies that the captured target connection selected
capability 64, reserves exactly one existing `PaneConn` operation key, sends
the exact captured payload, validates the matching response key/status, and
queries the authoritative view after `Applied` or `Partial`. Only those two
outcomes carry a view update. Rejection, failure, unknown outcome, in-progress
status, malformed reply, and query failure return fixed labels. An in-progress
or malformed response leaves the context key unresolved for the existing
status-settlement path; this transport does not issue a replacement key.

## Polling and compatibility

Fresh attachments and resumed connections record whether capability 64 was
selected; reconnect refreshes that selection. A selected connection uses the
schema-13 view query and treats a malformed or out-of-scope view as an error.
The live connection authorizes the current view for its exact server
generation and workspace; its pinned occupant may already have exited and be
absent from the current layout. Every target in the returned view must match
that connection's generation and workspace. Once accepted,
`refresh_layout` passes the layout to `sync_panes`, which removes the exited
target's stale connection while retaining or attaching the current occupants.
A connection without capability 64 continues to use the unchanged legacy
layout query. Validated operations views are retained alongside the underlying
`LayoutReply`; all layout targets remain in the pane registry and the existing
screen polling continues for every connected pane, including hidden panes.
Changes to the view, including zoom-only revisions, are delivered through
`NetUpdate::pane_operations`.

## Parent integration required

- Export `ui_client::pane_operations` from `src/ui_client.rs`.
- Add `ClientResult::PaneOperation` handling in the host match. Apply the
  returned view only on `Ok(update)` and surface the `Outcome` so partial
  completion can be reported. Keep action IDs matched to the pending UI
  request; do not synthesize a new operation on an unknown result.
- Keep rendering projection separate from the full `PaneOperationsView.layout`
  target registry. Host key bindings and display projection are not part of
  this transport slice.
- Server production capability offer remains controlled by the parent and
  should be enabled only after the owner/runtime integration gate is cleared.

Source inspection confirms the protocol codec's 504-byte view bound and the
current response/status shape. A fixed-label client scope check now validates
generation and workspace for every returned target without requiring the
querying occupant to remain present. Two focused tests were authored: one
round-trips a survivor-only schema-13 view, accepts it for the exited pinned
target, and applies the same stale-target pruning helper used by `sync_panes`;
the other refuses mismatched workspace and generation. These tests are pending
the parent's serial Cargo run. They exercise codec, scope validation and
connection-list pruning, but do not inject a live `PaneConn` request through
`refresh_layout`; that network-path lifecycle remains unverified. Scoped Rust
2021 formatting and `git diff --check` passed. These are source-only checks,
not runtime verification.

## Parent execution and full refresh-path regression — 2026-10-26

The first integrated client run passed121/failed1 because the new view fixture
used outer24x80 as its confirmed PTY size while pending_confirmed was absent.
Parent corrected it to content22x78. Failure is retained at
`/private/tmp/panelink-p5-pane-client-refresh-r1.log`.

Parent added an actual socket-pair regression using PaneConn bootstrap with
private context files. It invokes Net::refresh_layout through the exited
first connection, receives a validated survivor-only view, checks that the
stale PaneConn is removed, and refreshes successfully through the survivor's
socket. Wrong-generation/workspace rejection tests remain. This extends the
owner's pure helper coverage; it does not require a real PTY or child exit.

The complete client filter now passes **123 tests, 0 failed** at
`/private/tmp/panelink-p5-pane-client-refresh-r2.log`. Outer-PTY controls and
full current-tree verification still await the active acceptance owner.
