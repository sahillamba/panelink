# P5 drag selection and clipboard integration plan — 2026-10-25

Status: read-only integration design. No implementation, build, test, host UI,
server, or clipboard action was run for this plan. Phase 5 remains open. The
separate selection-model owner is implementing only
`src/terminal_selection.rs`, its `src/lib.rs` export, and
`tests/p5_terminal_selection.rs`; this plan does not edit or redefine that
owner's files.

## Decision summary

Add row-wrap metadata as volatile presentation metadata on `ScreenSnapshot` and
as a bounded, separately negotiated trailer on live-screen and history replies.
Do not change the `screen_checkpoint` encoding or saved history/checkpoint
formats. The existing terminal engine is authoritative: `vt100::Screen::row_wrapped`
reports visible row flags (including a cloned scrollback view), and each
`vt100::HistoryRow` already carries `wrapped`. `ScreenSnapshot` currently copies
cells and mode/cursor data but not those flags. Neither client nor server should
guess wrapping from a full row, cell content, or cursor position.

Use an optional handshake capability for the new reply trailer. Existing
clients do not request it and continue receiving the current exact schema-8
reply. New clients request it optionally; when the server does not select it,
the client must not offer wrapped-line copying. This preserves operation with
old peers while failing closed on the one metadata selection needs. The UI
request schema and persisted checkpoint schema remain unchanged.

The client owns drag selection and the system clipboard. A left-button press
inside pane content captures a bounded frozen selection source (cells and wrap
flags), drag motion updates its endpoint, and left-button release extracts
from that captured source and enqueues a nonempty copy. New output continues to
update the live snapshot. The compositor should paint the selection as a
presentation overlay from the frozen source for selected cells, while all
other cells come from the current live snapshot. This avoids mutating the live
snapshot, history snapshot, saved checkpoint, or compositor's committed
baseline. Ordinary output does not change the frozen text. Resize, buffer
switch, pane/layout geometry change, or view-epoch change cancels the gesture
and clears its overlay. Empty/collapsed selections do not call the clipboard
sink. Existing inner-app mouse-reporting behavior still needs an explicit
selection gesture policy before host acceptance; this design must not make
application mouse input unusable.

Use a dedicated bounded asynchronous local clipboard writer, injectable behind
a sink interface in tests. Its production implementation runs the absolute
`/usr/bin/pbcopy` path with argv and piped stdin (no shell, interpolation,
clipboard read, stdout/stderr capture, or content logging), enforces a selected
text byte cap and a wall-clock deadline, kills/reaps on timeout, and surfaces
only fixed redacted status labels. The writer lives with the attached client,
not the server or PTY owner. Its one-slot bounded queue prevents repeated drags
from accumulating text or threads; UI event handling only performs a bounded
nonblocking enqueue. Worker errors update local status and never terminate the
session. Closing/detaching the client cancels or drains only within a short
bounded deadline; it cannot defer clipboard work to a later attachment.

## Current data path and integration seam

1. `TerminalSession` parses child bytes into the vendored VT100 `Screen` and
   refreshes `ScreenSnapshot` in `src/terminal/mod.rs`. `Screen::row_wrapped`
   is authoritative for each visible row. `history_snapshot_from_screen`
   clones the screen, applies scrollback, and refreshes a separate snapshot,
   so it can carry the corresponding historical viewport flags without
   changing live state. `Screen::history_after` returns `HistoryRow { wrapped }`;
   persisted normalized row records already store that flag.
2. `src/multi_runtime.rs::dispatch_ui` builds a live checkpoint and calls
   `ui_protocol::prefix_screen_reply`. For a history request it calls
   `PtyEffects::history_snapshot`, encodes its checkpoint, then calls
   `prefix_history_reply`. Checkpoint bytes are intentionally independent from
   the new wrap trailer.
3. `src/ui_client/conn.rs::handshake` negotiates the new optional capability.
   `src/ui_client/net.rs::fetch_screen` and history result decoding validate
   replies and currently retain checkpoint bytes only. These are the points
   to carry decoded wrap metadata into `ScreenUpdate` and `ScrollResult`.
4. `src/ui_client/state.rs::App::load_screen` and `apply_scroll_update` load
   the validated checkpoint into separate live/history `ScreenSnapshot`
   caches. Preserve row flags on the matching cache. `App::render` selects a
   live or history snapshot and supplies it to the compositor; `set_layout`,
   resize handling, alternate-buffer detection, and `view_epoch` already
   provide invalidation points.
5. `src/ui_client/host.rs::event_loop` decodes host mouse events, applies
   `App::handle_event`, polls the client worker, applies worker results, and
   renders/commits frames. It is the lifecycle owner for the clipboard writer.
   Today `App::handle_event` focuses on left-down, opens the menu on right-down,
   and routes wheel events; it ignores left drag/release. There is no user
   clipboard command path in the attached client.

## Wire shape and bounds

Current `ScreenReply` is a 64-byte header plus checkpoint; bytes 56..63 are
reserved zero. Current `HistoryReply` is the same header size, with offset at
52, mode flags at 54, checkpoint length at 56, and bytes 60..63 reserved zero.
Both replies require exact length and validate the embedded checkpoint. Keep
these layouts for clients that did not negotiate row-wrap metadata.

Add `CAPABILITY_SCREEN_ROW_WRAPS` as an optional session capability. The new
client includes it in `Hello.optional_capabilities`; the server selects it
only when supported. Do not make it required. Store the selected bit on each
`PaneConn` across reconnect and use the authenticated connection's selected
capabilities when dispatching replies. With the bit absent, send the legacy
reply bytes unchanged. The client treats missing negotiation as
`wrap_metadata_available = false` and disables auto-copy selection rather than
making line-break guesses.

For a negotiated `ScreenReply`, use bit 0 of the existing `u16` at offset 56
as `wrap_bitmap_present`; keep bits 1..15 and bytes 58..63 zero. For a
negotiated `HistoryReply`, use bit 2 of the existing mode flags at offset 54;
keep bits 3..15 and bytes 60..63 zero. Append one packed LSB-first bit per
checkpoint row after the declared checkpoint bytes, with 1 meaning that the
row soft-wraps into the following row. The bitmap length is derived from the
validated checkpoint row count: `ceil(rows / 8)`, at most 10 bytes because
terminal snapshots are bounded to 80 rows. Require unused high bits in the
last byte to be zero. Strictly reject a marker without the negotiated
capability, unexpected trailer bytes without a marker, an incorrect bitmap
length, nonzero reserved bits, or malformed checkpoint dimensions. Bump the
maximum ScreenReply and HistoryReply limits by at most 10 bytes and preserve
the existing checkpoint-specific limit. The capability defines this trailer's
version; a future incompatible form needs a new capability.

This design keeps every persisted byte and existing unnegotiated wire reply
stable. It also means a new client attached to an old server receives a valid
legacy reply but cannot claim wrapped-line selection support. Confirm the
handshake's persisted-lease behavior before implementation: leases cannot
silently gain a capability, so an existing saved attach context created before
this change may need to show selection unavailable until a new context is
explicitly created. Do not silently replace a caller context.

## Client selection and rendering contract

- Hit-test only confirmed pane content rectangles, excluding border/margin,
  menu/overlay, divider, and resize-pending areas. Capture the exact target,
  screen buffer, dimensions, pane rect, screen revision and `view_epoch` when
  drag begins. A drag stays in its origin pane; do not transfer selection to a
  different pane if focus/layout changes.
- Freeze one bounded source snapshot at press, including sanitized cell text,
  cell width/continuation topology and authoritative row-wrap flags. Feed the
  model the same terminal grid/cell-width semantics the renderer uses; do not
  convert host columns to Unicode scalar indices. The selection model remains
  pure and contains no process, filesystem, UI event, or clipboard behavior.
- Drag endpoint is clamped to the origin pane's content bounds. On release,
  extract once from the frozen source. A collapsed/cell-empty range produces
  no write. Do not send screen queries, input operations, paste events, or any
  request that could trigger child output as a consequence of selection.
- While dragging, render a temporary selection overlay from the frozen source
  for selected cells and a selected-cell visual style. Leave `ScreenSlot` live
  and history snapshots untouched; let normal poll updates and compositor
  baseline commits continue. On release, clear the highlight after enqueueing
  the copied text. If copy fails, keep only a fixed local status message.
- Cancel and clear on terminal resize (host or confirmed pane resize), target
  replacement/close, pane rectangle change, active normal/alternate buffer
  switch, scroll-history navigation or return-to-live `view_epoch` change,
  menu/modal ownership, detach, or loss of required row-wrap capability.
  Screen revisions caused by ordinary output do not invalidate the frozen
  selection source. A stale drag must never select cells from a different
  buffer or target.
- Right-button menu behavior remains independent. When an inner application
  has mouse reporting enabled, choose and test a deliberate selection gesture
  (for example a host-supported modifier drag) or leave drag events to the
  application. Do not infer a modifier from a gesture the host does not
  deliver. Keep the keyboard menu path. This is a host acceptance question,
  not something automated source tests can settle.

The current outer-host `ENTER_SEQUENCE` enables mouse mode 1000 plus SGR 1006.
Mode 1000 gives button press/release but does not request held-button motion,
so the current attach client cannot receive drag endpoints. The input parser
already has `MouseEventKind::Drag`; implementation must enable a reviewed
button-motion host mode (normally 1002 with 1006), handle `Drag(Left)` in the
client, and restore that exact mode in reverse order on every exit/unwind path.
This changes host behavior and needs focused sequence/restoration tests before
Apple Terminal acceptance. Verify that the target host reports motion as
expected; if it does not, selection stays unavailable rather than fabricating
endpoints. Do not enable unrestricted any-motion mode 1003.

## Clipboard execution boundary

Add a private attached-client service with a trait such as
`ClipboardSink::write(&self, text: &[u8]) -> Result<(), ClipboardError>` and a
bounded `ClipboardWriter::try_send(Vec<u8>)`. The production sink launches
`Command::new("/usr/bin/pbcopy")` directly with `stdin(Stdio::piped())`,
`stdout(Stdio::null())`, and `stderr(Stdio::null())`; it never uses `sh -c`,
puts text in argv/environment, or captures child diagnostics. Bound text before
enqueue, use a fixed queue capacity of one, and reject a full queue with a
redacted `clipboard busy` status. The worker owns all blocking child IO and
deadline/kill/reap handling. Use static error classes (`clipboard unavailable`,
`clipboard timed out`, `clipboard write failed`, `clipboard busy`) with no
path, OS error string, selection text, cell contents, or child stderr.

For bounded writes, the child must not block the UI or the socket/pane worker.
If stdin writing can block, supervise it with a bounded writer thread/channel
or nonblocking descriptor strategy; on deadline kill and reap the child before
joining/draining the writer. Do not leave a blocked detached thread behind.
Shutdown behavior and queue byte accounting must be explicit. The injected
test sink records bytes in test memory only; no test invokes `/usr/bin/pbcopy`
or changes the host clipboard. This path is an explicit user Copy action and
does not relax denial of child OSC 52/programmatic clipboard requests.

## Parent-owned implementation slices and exact interfaces

| Owner / slice | Files | Interface boundary |
| --- | --- | --- |
| Selection-model owner (already assigned) | `src/terminal_selection.rs`, `src/lib.rs`, `tests/p5_terminal_selection.rs` | Existing exact API: adapt rendered cells into `terminal_selection::Cell { text, wide_lead, continuation }`, rows into `Row { cells, wrap_after: SoftWrap::{Yes,No,Unknown} }`, create `ViewIdentity { target, view_id, epoch, rows, columns, buffer_id }`, call `begin(identity, rows, anchor)`, `Selection::extend(point)` on drag and `Selection::finish(current_identity)` on release. Respect `SelectionError::Empty`, `UnknownWrap`, `StaleView`, `TooLarge`, and `InvalidShape`; enforce `MAX_SELECTION_BYTES`. Model is pure and owns the frozen text/cell source. Do not edit these files here. |
| Parent: runtime snapshot metadata | `src/terminal/mod.rs` | Add preallocated wrap flags plus an availability marker to `ScreenSnapshot`; expose bounded `row_wrapped(row) -> Option<bool>`/`row_wraps()` access; copy and refresh flags in `copy_from`, `refresh`, `history_snapshot_from_screen` and checkpoint load/reset paths. For live rows use `screen.row_wrapped(row)`. A checkpoint-only load has unavailable wrap metadata until its negotiated reply bitmap is applied; never represent absence as authoritative `false`. |
| Parent: capability and reply codec | `src/handshake_protocol.rs`, `src/server.rs`, `src/ui_protocol.rs` | Define optional capability bit; negotiate without changing required-capability behavior; extend `ScreenReplyView`/`HistoryReplyView` with borrowed wrap-bitmap access and `wrap_metadata_available`; extend `prefix_screen_reply`/`prefix_history_reply` to accept an optional bounded bitmap and strictly validate extension/trailer shape. Preserve exact legacy bytes when `None`. |
| Parent: server reply production | `src/multi_runtime.rs` (plus `src/server_pty.rs` only if a wrapper accessor is needed) | Capture wrap bits from the exact `ScreenSnapshot` used to encode each checkpoint; pass them only when `connection.selected_capabilities()` includes the feature bit. For history, use flags from the exact history snapshot returned for that offset. |
| Parent: client transport/cache | `src/ui_client/conn.rs`, `src/ui_client/net.rs`, `src/ui_client/state.rs` | Request optional bit in fresh handshake, persist selected bit on `PaneConn` and reconnect, decode/map bitmap into `ScreenUpdate` and `ScrollResult`, and apply it only to the matching live/history snapshot and `view_epoch`. Preserve target/revision checks. |
| Parent: events/composition | `src/ui_client/keys.rs`, `src/ui_client/state.rs`, `src/ui_client/host.rs`, `src/terminal/compose.rs` | Change host mouse reporting from mode 1000 to button-motion 1002 (retaining SGR 1006) with paired reverse restoration; classify `MouseEventKind::Drag(Left)` separately from press/release and handle it in `App`. Add drag lifecycle and a frozen bounded source per active selection. Add a borrowed per-pane selection overlay to composition so selected cells paint from the frozen source while other cells use the current snapshot. Never mutate live/history snapshots or commit an overlay as the underlying PTY state. Clear through existing layout/resize/buffer/history epoch transitions. |
| Parent: clipboard service | Prefer new `src/ui_client/clipboard.rs` and its private module declaration; `src/ui_client/host.rs` and `src/ui_client/worker.rs` only for lifecycle/enqueue/result/status integration | One-slot bounded async writer, injected sink in unit tests, production absolute-path pbcopy sink, hard payload/deadline bounds, fixed redacted errors, bounded close. Keep the writer on the attached client side. |
| Parent: focused tests | `src/ui_protocol.rs` unit tests, `src/terminal/mod.rs` unit tests, `src/ui_client/state_tests.rs`, `src/terminal/compose.rs` tests, `src/ui_client/clipboard.rs` tests, and new attached-client tests only if private helpers are insufficient | Exact cases below. Do not add new modules into the selection owner's test/file set. |

The `src/terminal_selection.rs` owner and the parent-owned files above are
disjoint. Parent must review the model API and its tests before integration;
do not guess or shadow the API while that work is in progress.

## Focused test matrix and commands for later implementation

No command below was run for this plan. Run focused cases only after the parent
assigns implementation and the matching slice is ready.

1. **VT100/snapshot metadata:** normal visible rows distinguish wrapped from
   hard line breaks, including exact last-column autowrap and wide-cell edge;
   alternate-screen rows report their own buffer's flags; scrollback-cloned
   viewport flags match the exact rows; `copy_from` carries flags; checkpoint
   encoding/decoding remains byte-for-byte unchanged and does not serialize
   wrap flags. Suggested: `cargo test --locked terminal::` and focused
   checkpoint golden tests.
2. **Wire negotiation and codec:** legacy prefix/decode golden tests are
   unchanged byte-for-byte; optional-capability peers negotiate and
   unnegotiated peers receive the exact old payload; metadata bitmap length is
   1, 10 and malformed lengths; rows 1, 8, 9, 80; unused tail bits; absent
   marker with trailer; marker without trailer; unexpected flags/reserved
   bytes; invalid checkpoint dimensions; payload maximum. Suggested:
   `cargo test --locked ui_protocol::` and handshake capability tests.
3. **Server source fidelity:** live normal and alternate snapshots encode the
   corresponding parser row flags; history replies encode the `history_snapshot`
   row flags at each tested offset; a live-output update during history view
   does not substitute live wraps into the history reply. Suggested focused
   `multi_runtime` screen/history dispatch tests and `terminal::` tests.
4. **Selection model integration:** use the separate owner's exact model API
   (`begin` / `extend` / `finish` with `ViewIdentity`, `Row`, `Cell`, and
   explicit `SoftWrap` values);
   test forward/reverse drag, one cell, empty/collapsed, multiline hard break
   versus soft wrap, wide scalar continuation cells, combining marks, tabs and
   sanitized blanks, boundary clamping, same-pane only, and immutable source
   while live output changes. Suggested:
   `cargo test --locked --test p5_terminal_selection` plus client state tests.
5. **Client lifecycle/compositor:** host decoder yields `Drag(Left)` under 1002;
   drag only in content, not chrome/menu/divider;
   press/extend/release exact event behavior; output updates live cells while
   frozen selected text/highlight stays stable; buffer/layout/target/resize/
   history-epoch changes clear it; async history completion with stale
   `view_epoch` is ignored; selected overlay redraw is incremental and after
   clearing the compositor matches a fresh render of current live snapshots.
   Suggested `cargo test --locked ui_client::keys` and
   `cargo test --locked ui_client::state_tests`, plus focused `terminal::compose`
   and host enter/restore sequence tests. Assert that 1002
   and 1006 are both disabled during restoration after normal exit and
   error/unwind cleanup.
6. **Clipboard without host mutation:** injected sink observes exactly the
   selected UTF-8 bytes; empty selection causes zero writes; queue-full,
   timeout, spawn failure, broken/slow stdin and shutdown are bounded and
   produce static redacted status; UI event loop remains responsive; no
   detached writer thread remains after timeout. Tests must never launch the
   real host clipboard command. Suggested new clipboard module tests and
   attached-client private IPC tests with the sink injected.
7. **No child output side effects:** release/selection makes no `ScreenQuery`,
   key, paste, wheel, or clipboard protocol request to the server; OSC 52 from
   child output remains denied with no host clipboard mutation and no reply
   bytes to the child. Suggested client effect-spy test and preserve the
   existing OSC 52 denial test.

After implementation, use the repository's established commands and gates,
including `cargo fmt --check`, focused Rust tests, then the full required
phase verifier before phase status/commit decisions. Apple Terminal selection,
exact user clipboard sentinel, failure reporting, and mouse-reporting
pass-through require a separate human host acceptance. This plan is not that
acceptance and does not close Phase 5.

## Product and evidence limits

The workspace/tab hierarchy choice remains pending. This work is limited to
selection and copy-on-release in the current attach client; it does not add
workspace tabs, new-shell/provider menu actions, paste, keyboard copy mode, or
an application-mouse gesture policy by assumption. The confirmed copy-on-drag
requirement and current clipboard authority contract are preserved. Clipboard
policy questions for corporate use remain unresolved. Automated model/codec/
client tests cannot prove Apple Terminal delivers drag events or that a real
user clipboard changes correctly. Phase 3's separate four-pane Apple Terminal
gate and Phase 5's hierarchy, reboot/layout, and representative-workload gates
remain open.
