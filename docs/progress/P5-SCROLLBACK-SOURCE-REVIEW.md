# P5 normal scrollback extraction source review — 2026-10-18

Status: source review and proposed extraction contract only. No terminal,
storage, protocol, or dependency code changed. This does not complete history,
checkpoint, restart, or recovery work.

## Finding

The current `TerminalSession` snapshot cannot provide the required normal
scrollback stream. `TerminalSession::spawn` configures the vendored parser with
1,024 history rows, but each `pump` parses at most one 16-KiB PTY read and then
refreshes one sanitized snapshot from the currently visible active buffer.
That snapshot contains at most the configured viewport cells. It has no row
IDs, no history cursor, and no durable writer. The snapshot refresh does not
copy history on each pump, which should remain true after extraction is added.

The vendored VT100 callbacks do not report row movement. Their trait handles
unhandled input and extra terminal requests; normal linefeed, auto-wrap, CSI
scroll, and screen mutations are dispatched directly to `Screen` and `Grid`.
`CoreCallbacks` currently uses those callbacks for bounded query replies and
clipboard denial. A CSI callback or a diff of consecutive visible snapshots
cannot distinguish a row entering scrollback from cells being overwritten,
erased, inserted, or redrawn.

The precise row hook is the normal-grid enqueue in `Grid::scroll_up`: after a
row is removed from the drawing grid, it is pushed into history only when
history capacity is nonzero and the scroll region is the full screen. This is
the only `scrollback.push_back` site in the vendored engine. Row insertion alone
is not a complete cursor contract, however: RIS replaces the screen and drops
its history without appending a row. Extraction also needs an explicit reset
notice/epoch transition at the RIS dispatch around `Screen::ris`, even when no
row follows. A read-only bounded delta plus reset notice can avoid a second
parser and a full-history export on every pump. Exact field placement remains
for parent design review.

## Current source behavior

| Area | Checked-in behavior and implication |
| --- | --- |
| App snapshot | The parser is constructed with `HISTORY_ROWS = 1,024`; `pump` parses one bounded read and calls `snapshot.refresh`. `ScreenSnapshot` owns visible cells and mode/cursor fields only. |
| Active buffers | The engine owns a normal `Grid` with the configured scrollback and an alternate `Grid` with capacity zero. `Screen::grid()` selects the active one. Alternate output therefore has no scrollback rows to export. |
| Public view APIs | `Screen::cell`, `row_wrapped`, `rows`, and `contents` address `visible_rows()` for the active grid. `set_scrollback` changes that grid's view offset; `scrollback()` returns the view offset, not the stored row count. These APIs do not provide a read-only iterator over all normal history. Text-only rows also omit rendition data. |
| Exact insertion | `Grid::scroll_up` removes rows from the drawing grid. It appends each removed row only if `scrollback_len > 0` and `scroll_region_active()` is false; a full history deque drops its oldest row with `pop_front`. This covers full-screen linefeed/auto-wrap scrolling and full-screen CSI `S`. |
| Not insertion | Partial-region scrolling is excluded. `scroll_down`, CSI insert/delete lines, cell edits, erase, and ordinary redraws do not append a row. They must not be inferred as history records. |
| Resize and reset | Resize changes/truncates drawing rows and clears their wrap flags when columns change; it does not transfer removed drawing rows into scrollback. Existing history rows are not reflowed by `Grid::set_size`. RIS replaces the screen and drops the old history. These operations need explicit gap/generation behavior; they are not row-insertion events. |
| Row representation | A row owns fixed-width `Cell` values and a `wrapped` bit. Cells expose text, wide-cell topology, foreground/background, and bold/dim/italic/underline/inverse. The current public screen accessors expose cells/wrap only for the active visible window, not a retained history row directly. |

Primary source references: [snapshot construction and refresh](../../src/terminal/mod.rs#L34), [pump parsing and refresh](../../src/terminal/mod.rs#L960), [snapshot cell sanitization](../../src/terminal/mod.rs#L316), [vendored callback contract](../../vendor/vt100/src/callbacks.rs#L1), [normal and alternate grid construction](../../vendor/vt100/src/screen.rs#L67), [visible-row and scrollback-view APIs](../../vendor/vt100/src/grid.rs#L126), [history insertion and eviction](../../vendor/vt100/src/grid.rs#L562), [row cell and wrap representation](../../vendor/vt100/src/row.rs#L3), [resize behavior](../../vendor/vt100/src/grid.rs#L66), [alternate-buffer transitions](../../vendor/vt100/src/screen.rs#L651), [RIS dispatch](../../vendor/vt100/src/perform.rs#L72), and [RIS reset](../../vendor/vt100/src/screen.rs#L997).

## Proposed extraction contract

1. Keep row extraction at the grid insertion point. Assign a checked,
   monotonically increasing ordinal to every row appended to the normal
   scrollback deque. Use a wrapper such as `ScrollbackRecord { ordinal, row }`
   so the row is moved, not cloned. The cursor is `{engine_epoch, ordinal}`
   under the immutable target-generation scope. Durable record identity is
   scoped by target generation and includes the engine epoch, which changes at
   RIS. A row fingerprint, deque index, or cell contents alone is not an
   identity.
2. Add a read-only `Screen`/`Parser` API that addresses the normal grid even
   while the alternate buffer is active and yields only retained records after
   a caller cursor. Each yielded view exposes ordinal, original row width,
   `wrapped`, and borrowed cells. It must not change `scrollback_offset`, copy
   the deque, or include active drawing rows. Report a gap when the requested
   next ordinal has already been evicted; do not silently restart at the deque
   front. Reject future/invalid cursors and fail closed on ordinal exhaustion.
   Normal/alternate buffer transitions preserve the epoch. RIS emits an
   explicit reset/gap notice and starts a distinct epoch even if no later row
   is appended, so a consumer cannot mistake the new empty history for a
   continuous stream.
3. After each parser feed, Panelink requests only the delta after its last
   durably accepted history ordinal. It converts borrowed cells through the
   existing sanitization policy, preserving supported rendition and wide-cell
   metadata, then advances the durable cursor only after the writer accepts
   the record. Stable IDs make a retry idempotent. A write, queue, or storage
   failure must leave the child/parser running and set a visible gap or
   capture-disabled state; it must not replay output bytes or terminal effects.
   Ordinal overflow disables extraction but does not stop or poison terminal
   parsing; history remains unavailable until a reviewed non-reused epoch or
   target generation is available.
4. A delta is bounded by the 1,024 rows still retained by this engine. If one
   16-KiB parser feed scrolls out more rows than the ring can retain before
   Panelink drains it, the ordinal gap is evidence of lost history. Publish the
   truncation/gap; do not claim rows that have already been evicted were
   persisted. A future synchronous callback or separately bounded sink could
   avoid that window, but would be a larger latency, storage-failure, and
   allocation design that needs its own review.
5. Recording disabled and engine history disabled are separate states. With
   engine capacity zero, no inserted-history records exist. If persistence is
   disabled while the in-memory engine still scrolls, mark the disabled
   interval. On re-enable, begin a new recording segment or explicitly report
   a gap; do not silently backfill it as continuous history. Checkpoints remain
   separate artifacts and alternate-screen contents remain snapshots, not
   scrollback rows.

The row payload is a normalized cell representation, never the raw PTY chunk,
formatted VT100 byte stream, query reply, OSC payload, or input. Reuse
`SnapshotCell` sanitization so control characters, bidi controls, and line or
paragraph separators become U+FFFD; preserve only supported cell attributes.
The persistence schema still needs to define width, empty cells, colors,
wide-cell continuations, wrap, record generation/ordinal, and retention/gap
markers. Do not infer a newline between rows marked as wrapped.

## Memory and vendored-component impact

The current allocation review models a maximum 240-column row, 32-byte engine
cells, a per-row cell-capacity allowance of 1,024, and a `VecDeque` capacity of
2,048 row slots; the configured history cap is 1,024. The logical occupied
cells in a full 240-column retained history total 7,864,320 bytes
(`1,024 × 240 × 32`); the conservative allocation model budgets up to
33,554,432 bytes for its retained normal-history cell backing
(`1,024 × 1,024 × 32`). Both figures are already included in the existing core
budget. At the reviewed ring capacity, a `u64` ordinal alone represents
16,384 bytes (`2,048 × 8`). One separately stored eight-byte cursor would add
another 8 bytes. These are conditional lower-bound payload estimates, not a
complete post-patch memory budget: epoch/clock state, overflow and gap flags,
possible per-grid counters, padding, actual container layout, and allocator
capacity have not been selected or measured. Keep 63,088,640 bytes as the
existing baseline only; do not claim an updated total or RSS. Recalculate the
complete model from the adopted fields and target build before integration.

The proposed borrowed delta adds no second cell array and performs no
full-history transfer per pump. A copied event queue or retained shadow history
would add memory beyond the conditional lower-bound payload estimates above;
its exact row/queue bound must be reviewed before adoption. A writer scratch
buffer is also outside this source-only estimate and must have a separately
declared maximum from the selected record encoding.

This hook changes the local vendored VT100 source. If adopted, update the
complete local patch, `vendor/vt100/PROVENANCE.json` source fingerprints, local
component revision, dependency inventory, and relevant notice/source review.
Keep the upstream MIT notice and source attribution. No dependency is proposed;
the selected component remains the checked-in VT100 fork.

The underlying review model is recorded in [P2 replacement selection](P2-REPLACEMENT.md#L21), [P2 core allocation table](P2-CORE-RESULT.md#L19), [vendored source provenance](../../vendor/vt100/PROVENANCE.json), and [the current complete local patch](../../vendor/vt100/panelink.patch).

## Required automated test matrix before integration

| Case | Expected evidence |
| --- | --- |
| Full-screen LF and automatic wrap at the bottom | Exactly one ordered row record per row actually moved into normal history; preserve cells, rendition, source width, and wrap bit. |
| Full-screen CSI `S`, including count greater than one | One ordered record per enqueued row; no duplicate IDs. |
| Identical rows and same content after later redraw | Distinct monotonic IDs; content hash is not identity. |
| Split parser feeds and multiple rows in one feed | Stable ordering and no repeat on the next delta request. |
| Scrollback cap and more than 1,024 insertions before drain | Oldest retained ID advances; consumer receives an explicit missing-ordinal gap and bounded retained suffix. |
| Partial scroll region, reverse index, CSI `T`, insert/delete lines, erase, and overwrite | No normal-history record unless the engine's exact append point ran. |
| Alternate modes 47 and 1049 | Alternate output emits no history; return to normal retains its prior IDs; 1049 clear/reset does not invent rows. |
| Width/height resize and RIS | No records for truncated drawing rows; preserve actual row width/wrap when later enqueued; RIS emits a reset/gap and changes engine epoch even with no later row; IDs cannot repeat within a target generation. |
| History capacity zero; persistence capture disabled/re-enabled | No false record while disabled; gap/segment boundary is explicit; no silent backfill claim. |
| Wide/combining cells, colors/rendition, controls/bidi, OSC and query traffic | Cell metadata survives the existing sanitizer; unsafe display controls are replaced; no escape/query/OSC/input bytes enter the record. |
| Writer error, bounded-queue full, malformed record, ordinal/epoch exhaustion, partial recovery tail | PTY and parser remain live; extraction disables if no safe non-reused epoch remains; no cursor advance past the last accepted record; a gap/error is visible; restart/retry does not duplicate accepted IDs or execute saved bytes. |

These tests would validate the source hook and recorder contract only. M5 still
needs separate checkpoint, persistence/retention, restart, recovery, and
workload evidence; no saved bytes may be replayed to a terminal or child.
