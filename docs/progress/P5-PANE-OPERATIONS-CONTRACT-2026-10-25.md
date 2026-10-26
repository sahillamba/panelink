# P5 pane operations implementation contract

Date: 2026-10-25
Status: source-grounded design contract; parent wire refinement below supersedes the proposed wire shape. No runtime verification.

## Scope and current boundary

Deliver server-authoritative same-tab pane-location swap, absolute internal-divider setting, and zoom/unzoom over the existing four-leaf tree. Keep PTYs and target identities stable. Use the existing owner serialization, operation ledger, root-size limits, split geometry, and absolute resize path. Prototype bindings are `Ctrl-B m` (swap chooser), `Ctrl-B e` (divider adjust), and `Ctrl-B z` (zoom); these are implementation choices for this slice, not lasting user configuration decisions.

The code graph was queried first for architecture, layout, capability, wire, resize, and client symbols. It found the checked-in `PaneLayout`, `RootResize`, `LayoutReply`, owner, and saved-layout symbols, but not the new `swap` / `set_divider` methods named by the in-flight geometry assignment. Search results also lag the current UI protocol schema comments. I therefore checked the current checkout directly after graph discovery. At this read, `src/pane_layout.rs` still has no public swap/divider-set methods; the pure geometry owner is expected to add exactly `swap(source, dest, rev)` and `set_divider(target, rev, divider_index: u8, first_cells: u16)`. This note specifies their consumers and does not claim that worker code exists or passed.

## Parent wire decision — 2026-10-25

The first wire slice uses new `pane_operations_protocol.rs`, request/view
schema 13 and capability 64; the legacy UI request/reply codecs remain unchanged.
Requests are `ViewQuery`, `Swap(PaneSwap)`, `DividerSet(PaneDividerSet)` and
`ZoomSet(PaneZoomSet)`. Mutations return existing response-protocol status or
rejection bytes; callers query the authoritative view afterward. The separate
`PaneOperationReply` proposal below is superseded; status already includes its
outcome and does not need a second outcome field.

`PaneOperationsView` contains the underlying `LayoutReply`, optional exact
zoom target, and `confirmed_sizes: [Option<(u16, u16)>; MAX_PANES]`, present for
every occupied pane. Keep legacy `pending_confirmed` semantics relative to
underlying rectangles. Never reinterpret it as effective zoom geometry. New
clients derive desired dimensions from the effective zoom projection and
compare explicit confirmed sizes. A successfully zoomed PTY can therefore
still differ from its underlying pane rectangle. This explicit distinction
supersedes the provisional pending-size interpretation below.

Wire owner edits only handshake, the new codec module, its new integration
test and evidence note. Parent owns module export and runtime offer/dispatch.
Capability is vocabulary only until runtime support exists. Geometry and
startup fixture owners remain in separate file sets. All worker tests await
parent serial execution after release.

## Current source facts to preserve

- `src/pane_layout.rs`: `PaneLayout` owns a fixed seven-node tree for at most four leaves. `TreeNode::Split` already stores `first_cells: u16` and `total_cells: u16`; `state_from_tree` applies their integer ratio when recomputing geometry. Root resize therefore already preserves the split proportions. `LayoutSnapshot` is a spatially ordered fixed array of all occupied `PaneGeometry` entries plus spatially traversed dividers. Divider index is an index in that current snapshot traversal, not a stable split ID. The geometry worker's divider setter resolves the selected split as an ancestor of `target`; do not add a split-ID wire field.
- `src/protocol.rs`: `OperationKey { generation, lease, sequence }`; `Ledger` compares the retained request payload byte-for-byte and records the compact `Outcome` (`Accepted`, `Applied`, `Failed`, `Partial`, `Unknown`). Outcomes are volatile and retained until receipt. `known_reply` is the owner path used before revalidating stale preconditions. Pane operations must check duplicate key and exact payload before revision, target, or divider-index staleness. Same-key/different-payload is conflict; a new key is required for a new absolute desired position. The ledger does not retain arbitrary typed reply bodies, so after a duplicate/status reply the client queries the authoritative layout view.
- `src/handshake_protocol.rs`: schema 1 uses fixed 28-byte Hello and 40-byte Welcome; known capability bits are `READ=1`, `INPUT=2`, `EVENTS=4`, `WORKSPACE_EVENTS=8`, `PANE_METADATA=16`, and `SCREEN_ROW_WRAPS=32`. A pane-operations capability may use the next bit, `64`, only after adding it to `KNOWN_CAPABILITIES` and the server/client offer/selection paths. Unknown optional bits are tolerated as unselected; an unknown selected bit is invalid to existing clients. Do not select bit 64 for peers that did not offer it. No new `MessageKind` is needed: `src/protocol_wire.rs` already frames opaque `Request`, `Response`, `Snapshot`, and negotiated `Event` payloads.
- `src/ui_protocol.rs`: current requests use strict UI schema 8; known request kinds are LayoutQuery 1, ScreenQuery 2, KeyInput 3, RootResize 4, HistoryQuery 6, and MouseWheelInput 7. Current `LayoutReply` is fixed 424 bytes under reply schema 9, with all four pane slots, complete `TargetRef`s, underlying rectangles, divider rectangles, and per-pane pending-resize information. Preserve these codecs byte-for-byte for clients without the new capability. Add a distinct negotiated pane-operation request/reply schema or a rigorously additive version; do not reinterpret existing kinds or append bytes to the old fixed record.
- `src/server/multi_session.rs`: `handle_root_resize` already authenticates a live connection, requires `CAPABILITY_INPUT`, requires the operation key's lease to equal the caller's lease and generation to match, and calls `known_reply` before resize/revision validation. It does **not** require input ownership or a pinned target and is not restricted to a designated root-resize client. Preserve that root-resize authorization policy; do not invent a viewport-owner restriction. `resize_layout` commits geometry, records the layout observation, and applies absolute content sizes to each pane; failed effects produce `ResizePending` with a committed snapshot. Same-size requests retry pending panes only. Pane mutation can use the same live-connection/input-capability/own-lease baseline plus exact target workspace scope; swap/divider/zoom must not silently claim or change input ownership.
- `src/multi_runtime.rs`: one serialized owner worker dispatches decoded UI requests and encodes the resulting snapshot/pending flags. Route new requests there only after negotiated capability validation and strict codec validation, through that same worker.
- `src/ui_client/state.rs`: `App` stores a `LayoutReply`, derives a local `PaneLayout`, and keeps focus, per-target screen slots, history, and input queues by complete target. `set_layout` ignores older same-generation revisions, cancels stale selections, preserves target slots when present, and updates focus only when the focused target disappears. `src/ui_client/net.rs` keeps pane connections keyed by target. `src/terminal/compose.rs` composes every pane in the layout.
- `src/saved_layout.rs` and `src/layout_store.rs`: the current observation record is 280 bytes and the read-only legacy record is 160 bytes. It captures root geometry, target refs, pane rectangles and divider rectangles; it has no explicit ratio tree or zoom mode. Keep both codecs unchanged in this slice. The owner must publish/save the underlying non-zoom tree, never a display projection. Runtime ratios and zoom are deliberately not reboot/restore state; saved geometry is an observation and does not promise ratio or zoom recovery after root-size changes. Keep the physical reboot/restore acceptance gate open.

## Typed API and authoritative state

Use the geometry worker APIs as the pure transition boundary:

```rust
PaneLayout::swap(source: TargetRef, dest: TargetRef, expected_revision: u64)
    -> Result<LayoutSnapshot, LayoutError>
PaneLayout::set_divider(
    target: TargetRef,
    expected_revision: u64,
    divider_index: u8,
    first_cells: u16,
) -> Result<LayoutSnapshot, LayoutError>
```

`set_divider` interprets `divider_index` in the current spatial snapshot traversal, resolves that split only when it is an ancestor of the exact `target`, keeps the existing split axis/child order, and validates the complete resulting tree and every leaf's existing minimum before committing one revision. It is an absolute first-span request, not a delta; preserve the current `first_cells / total_cells` proportion on later root resize. `swap` exchanges the two leaf identities in their existing tree locations, preserving each `TargetRef`, process/session, occupant revision, and input-owner revision. Both reject stale revision/targets and invalid geometry without mutation.

The pane-operation wire request types are exact wrappers around those operations:

```rust
PaneSwap { key: OperationKey, expected_layout_revision: u64,
           source: TargetRef, destination: TargetRef }
PaneDividerSet { key: OperationKey, expected_layout_revision: u64,
                 target: TargetRef, divider_index: u8, first_cells: u16 }
PaneZoomSet { key: OperationKey, expected_layout_revision: u64,
              target: TargetRef, zoomed: bool }
PaneOperationsView { layout: LayoutReply, zoomed_target: Option<TargetRef> }
PaneOperationReply { key: OperationKey, status: OperationStatus,
                     outcome: Option<Outcome>, view: Option<PaneOperationsView> }
```

These mutations are absolute and retryable by identical key/payload. `PaneZoomSet { zoomed: false }` names the current zoom target; reject a mismatch as stale. A zoom target must be a current occupied leaf. `PaneOperationsView.layout` always holds the unchanged full underlying partition and all target identities. The client derives one display projection for `zoomed_target` using the full root rectangle; it must not overwrite or send that projected rectangle as the underlying layout. The zoomed PTY's desired size is `content_rect(root_rect(rows, columns))`; hidden PTYs remain alive, continue polling/output capture, and retain their last accepted dimensions. In this first schema, `pending_confirmed` is interpreted against each pane's effective zoom/unzoom geometry; return the actual confirmed size when pending. A future schema can normalize this into explicit actual/desired PTY-size fields without altering old replies.

Zoom and unzoom each advance the same layout revision. Unzoom restores the exact underlying tree and retries every pane whose actual size differs from its current content rectangle. Root resize remains allowed during zoom: resize the underlying tree proportionally, resize the zoom target to the new full-root content dimensions, leave hidden siblings at their last accepted sizes, and publish the new revision plus zoom target. A failed resize leaves the committed view in place and reports `Partial` plus per-pane pending/confirmed state; retry by querying then reconciling absolute desired sizes, never by replaying a relative divider action.

For the bounded first slice, reject split, close, swap, and divider-set requests while zoom is active with a stable `zoom_active` result. The user can unzoom, then make the topology change. This keeps the underlying tree stable while the display is projected. If a child exits independently while zoomed, the existing owner collapse path must clear zoom if it removes the zoom target and publish the resulting ordinary layout in that same serialized owner transition; it must not leave a dangling target. Explicit stop semantics remain unchanged.

## Protocol, admission, and client behavior

Use an additive pane-operations capability (proposed bit 64) and a new exact UI payload version for the three mutations plus the wrapped `PaneOperationsView`. Continue using existing opaque envelope kinds. Require that capability before decoding/dispatching new kinds; an older/nonselecting client continues to receive schema-9 `LayoutReply` and existing root-resize behavior. Validate exact byte length, schema/kind, zero reserved bytes, nonzero key components/revision, generation agreement across operation key and targets, valid complete targets, distinct swap targets, boolean zoom flag, and fixed-width divider index/value before owner admission. Then validate workspace scope, exact current target refs, current layout revision, spatial divider ancestor/limits, and geometry.

For each request, owner order is: authenticate live connection and selected capability; verify key belongs to caller lease/generation; find retained key and compare exact payload; for a new key validate current refs/revision/workspace/geometry and conflicts; admit once; commit the pure transition once; perform absolute PTY resize effects; record `Applied`, `Failed`, or `Partial`; return current authoritative view and pending size observations. A same-key exact duplicate returns the ledger status without another geometry or PTY effect, even when the original revision is now stale. A lost response is resolved by operation status using the original key, followed by a layout-view query. No input bytes, screen content, PTY identity, or process lifecycle operation is part of these requests.

On the client, retain `LayoutReply` as the full target registry even during zoom. Keep per-target network connections and screen/checkpoint polling for hidden panes; filter only the renderer's display projection. Do not remove hidden targets from focus/chooser models, or clear their screen slots merely because they are not drawn. When unzooming, the already-retained targets become visible after the authoritative view arrives. The divider chooser captures `TargetRef` and layout revision when opened; an unavailable pane or a revision change cancels/reports stale selection rather than retargeting. In chooser/divider modes, consume their key events locally; never forward them as PTY input. For swap, chooser source/destination are exact captured refs. `Ctrl-B m`, `Ctrl-B e`, and `Ctrl-B z` remain prototype bindings.

Workspace layout event journal currently observes revision/terminal-empty transitions, not geometry or zoom payloads. Each operation increments revision, emits the existing observation event, and requires clients to query `PaneOperationsView` to reconcile; an event is never a mutation replay. A zoom-only revision must be journaled even when the ordinary tree geometry is unchanged.

## Follow-up ownership and required contract tests

Run these as short sequential owners after the geometry API is released; each worker edits only its listed files and leaves commits/checks to the parent.

1. **Typed wire owner:** `src/handshake_protocol.rs`, `src/ui_protocol.rs`, and a new `tests/p5_pane_operations_wire.rs`. Add capability bit 64 negotiation and the three request structs, `PaneOperationsView`, and `PaneOperationReply`; keep old schemas/codecs intact. Add exact golden round-trips and malformed length/schema/kind/reserved/key/target/generation/duplicate-target/divider-bound/zoom-flag refusal cases. Assert the old 40-byte root-resize request and 424-byte schema-9 layout reply remain unchanged and that an unnegotiated connection cannot dispatch the new payload.
2. **Owner and transport owner:** `src/server/multi_session.rs` and `src/multi_runtime.rs`, plus a new `tests/p5_pane_operations.rs`. Add owner entry points `handle_pane_swap(connection: &Connection, request: PaneSwap, payload: &[u8], now_secs: u64, resize: Resize) -> Result<PaneOperationReply, ServerError>`, and corresponding `handle_pane_divider_set` / `handle_pane_zoom_set` methods. Each uses the existing `EffectSink` resize closure pattern and returns the key/status/outcome with an authoritative `PaneOperationsView` on committed or partial outcomes. Add dispatch through the existing serialized worker. Tests cover duplicate-before-stale, same-key payload conflict, wrong lease/capability/generation, exact-target and cross-tab refusal, stale revision, zoom-active block policy, PTY resize failure/Partial and absolute retry, root resize during zoom, hidden PTY identity/output continuity, zoom-target exit cleanup, and no effect on rejection. Preserve root resize's existing no-input-owner/no-pinned-target policy.
3. **Client owner:** `src/ui_client/keys.rs`, `src/ui_client/state.rs`, `src/ui_client/net.rs`, `src/terminal/compose.rs`, and a new `tests/p5_pane_operations_client.rs` (or the smallest already-owned client test module selected before work). Add API calls that capture exact targets/revision, consume prototype mode keys, render zoom from `PaneOperationsView` without mutating its underlying `LayoutReply`, and retain/query all hidden target connections/screens. Tests cover stale chooser cancellation, no PTY key leakage, all target refs retained in zoom, full-root projection, hidden PTY screen updates, unzoom restoration, and root viewport resize while zoomed.

No persistence owner is assigned: `src/saved_layout.rs` and `src/layout_store.rs` remain unchanged. A separate restore/reboot decision and versioned codec task is required before claiming ratio/zoom restore. Required later evidence remains distinct: pure geometry and codec tests, owner fake-resize tests, multi-PTY continuity integration, client interaction tests, existing host acceptance, and physical reboot/restore acceptance. This contract has run no Cargo, build, or tests and makes no runtime claim.
