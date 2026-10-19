# P3A wave 1 — pure menu, split codec and pane composition

Status: parent-specified 2026-10-18. Three parallel bounded tasks with
exclusive files. All are pure library work: no sockets, PTY launch, runtime
routing, host terminal IO or dependency changes. They are groundwork for the
later split transaction (wave 2), runtime migration and interactive client
(wave 3) and end-to-end acceptance (wave 4). None completes Phase 3A.

Delegation note: the parent dispatched these through the available
implementation subagents (`sde2`). The harness does not expose model
selection, so no specific model (e.g. gpt-6-luna) is claimed for this wave.

Shared rules: follow `AGENTS.md`; std only; no new crates; fixed or explicitly
bounded storage; redacted `Debug` for any type holding a `TargetRef`; static
error messages carrying no request data; workers do not commit, do not edit
`src/lib.rs`, `ROADMAP.md`, `INTEGRATION.md` or files owned by another task.
Verification for each: focused tests, `cargo fmt --check`,
`cargo clippy --locked --offline --all-targets -- -D warnings`.

## A — split menu model (`src/split_menu.rs`, `docs/progress/P3A-MENU.md`)

Pure, terminal-independent context-menu state for the four split actions,
consuming `crate::pane_layout::{PaneLayout, LayoutSnapshot, SplitDirection,
Rect, ...}`. Opening by pointer hit-tests the pane under the cell (divider or
outside opens nothing); opening by keyboard uses the supplied focused target.
Both capture the full `TargetRef` and the layout revision from one snapshot;
activation returns exactly those captured values plus the direction, never
current focus. All four items always listed in order Left, Right, Up, Down,
each enabled or disabled with a reason (`Split Left needs 41 columns; this
pane has 38`, pane-limit reason when two panes exist). Own input enum
(navigation, activate, cancel, direct letter keys, mouse press at cell, other)
returning Consumed / Closed / Activated(intent); every input while open is
consumed. Overlay rectangle clamped inside the viewport with the compact
five-row fallback described in `docs/PANE-LAYOUT.md`; status text clipped,
never wrapped. Exposes fixed-width ASCII overlay rows with style
(normal/selected/disabled) for a renderer.

## B — schema-7 split codec (`src/split_protocol.rs`, `docs/progress/P3A-SPLIT-CODEC.md`)

Borrowed, bounded codec. Request kind 1: 96-byte prefix (schema 7, kind,
operation key, full target, expected layout revision, direction, zero
reserved) followed by one complete embedded schema-6 launch request (validated
with `managed_launch_protocol::decode_launch_request`) whose key must equal the
outer key and whose rows/columns are the caller's planned new-pane size. Total
≤ `MAX_OPERATION_PAYLOAD_BYTES`; exact-byte payload therefore covers target,
revision, direction and launch data. Status query kind 2 and fixed-size result
kind 0x8001 (outcome, committed layout revision, existing target, optional new
target). `OperationClass::DurableMutation` for request, `Query` for status.
How wave 2 admits this payload in the ledger is not decided here.

## C — pane composition renderer (`src/terminal/compose.rs`, `src/terminal/render.rs`, one `pub mod compose;` line in `src/terminal/mod.rs`, `docs/progress/P3A-COMPOSE.md`)

Offset-aware rendering of up to two pane snapshots at their `Rect`s, a
one-cell divider and an optional ASCII overlay drawn last and clipped to its
rectangle, with the cursor placed in the focused pane (hidden while overlay is
open). Incremental: per-pane diff against the previous snapshot; full repaint
only on first frame, geometry change or overlay close/open over that region.
Existing `render`/`render_diff` public behavior and tests stay unchanged.
Sanitization guarantees of `render.rs` (no child-origin controls) apply to
every pane cell.
