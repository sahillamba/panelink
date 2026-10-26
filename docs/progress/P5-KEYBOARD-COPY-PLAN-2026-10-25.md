# Phase 5 keyboard copy mode plan — 2026-10-25

Status: read-only implementation contract. No implementation, build, test,
host UI, server, or clipboard action was run for this plan. Keyboard copy mode
is an independently useful remaining Phase 5 slice; it does not close the
separate Apple Terminal mouse/clipboard, reboot, or workload gates.

## Goal and boundary

Add a small attached-client keyboard path to select and copy text from the
focused pane without sending copy-mode keys, mouse actions, or pasted text to
the child. Reuse the existing visible row view, frozen snapshot selection,
selection extraction, compositor overlay, and asynchronous clipboard writer.
The first slice covers only rows currently in that pane's selected live or
history view. It does not add arbitrary saved-history search, buffer scrolling,
server protocol messages, persistence, or terminal-side clipboard access.

This is a proposed prototype key contract, not a confirmed long-term user
shortcut choice. Reserve `Ctrl-B c` for entering copy mode in the current
prefix map; confirm collisions against the active help text during
implementation. The existing `Ctrl-X` detach command remains available in
copy mode.

## User interaction contract

| Input | Behavior in the first slice |
| --- | --- |
| `Ctrl-B`, then `c` | Enter copy mode for the focused pane only when its current visible source has authoritative wrap metadata and capture succeeds. Otherwise show a fixed unavailable status and remain outside copy mode. |
| `Arrow keys` or `h/j/k/l` | Move the local caret by one cell, clamped to the captured pane rectangle. Left/right cross row boundaries; up/down preserve the cell column where possible and clamp at row ends. Wide-cell positions normalize to their lead cell. |
| `Space` | Toggle the selection anchor at the caret. With no anchor, start a selection; with an anchor, clear the selection while keeping the caret in copy mode. |
| `Enter` or `y` | If a non-empty selection exists, validate and extract it once, queue one attached-client clipboard write, clear the mode, and return to normal input. With no selection, leave copy mode without queuing a write. |
| `Esc` | Cancel and leave copy mode without copying. |
| `Ctrl-X` | Preserve normal local detach behavior; cancel copy mode before leaving. |
| Other keys, key releases, paste, and mouse events | Consume locally. They must not emit child key, paste, mouse, scroll, or clipboard-read effects. |

Repeat events may move the caret; `Enter` and `y` must be one-shot on key
press (ignore repeats) so one gesture cannot queue multiple clipboard writes.
Escape on a release or unsupported key event does nothing. There is no
keyboard-mode timeout in this first slice; detach, focus loss, geometry/view
change, buffer change, and modal/route transition cancel through existing
client lifecycle rules.

## Source and state contract

The current implementation seams were traced in the indexed checkout:

- `src/ui_client/state.rs`: `App.handle_event` owns key dispatch and consumes
  modal events; `begin_selection` obtains `visible_source`, checks the content
  rectangle and `row_wraps()`, then calls capture; `current_selection_identity`
  compares target, slot, content geometry, source kind, epoch, dimensions and
  buffer; `finish_selection` validates, calls `Selection.finish`, and uses
  `queue_copy_text`.
- `src/ui_client/selection.rs`: `FrozenDrag` owns the source identity,
  `ScreenSnapshot`, `Selection`, anchor and end point. `capture` freezes source
  cells at entry. Ordinary live revisions can continue while the captured
  cells remain stable; view identity changes invalidate the operation.
- `src/terminal_selection.rs`: `Selection::extend` updates an endpoint and
  `Selection::finish(current_identity)` extracts Unicode text, joins confirmed
  soft wraps, inserts hard line breaks, trims padding at hard boundaries,
  handles wide cells, and enforces its existing output bound.
- `src/terminal/compose.rs`: `validate_selection` checks the frozen overlay
  against the pane snapshot geometry and `paint_selection` paints the selected
  cells from the frozen source. `Compositor.render_with_selection` is the
  existing overlay entry point.
- `src/ui_client/clipboard.rs` plus `App.queue_copy_text` /
  `App.take_copy_text`: the attached client already has an asynchronous
  one-item copy path with fixed status results. Keep clipboard payloads out of
  diagnostics and keep tests on injected/fake sinks.

Implementation should add explicit copy-mode state and key interpretation at
the client layer, with copy-mode dispatch before ordinary `map_key` forwarding
and after the existing higher-priority quit/detach and route/modal guards.
Avoid turning `map_key` into a global mode-aware dispatcher: keep the mode's
small key map pure and separately unit-testable. Entry should use the same
`visible_source` and `capture` path as drag selection; initialize the caret
from the focused snapshot's cursor when it lies within the pane, otherwise
from the top-left visible cell. Store the captured source identity and frozen
snapshot for the entire mode, including after ordinary screen revisions.

For the first rendering slice, reuse the inverse selection overlay for the
anchor-to-caret range. When no anchor is active, show the caret using a local
cursor/overlay path that does not alter the child snapshot. Keep cursor and
selection coordinates in pane-content cells, then translate through the
existing pane content rectangle during composition. Do not change the
compositor's base cell snapshot or renderer protocol. If the current overlay
shape cannot express a caret without a selection, add the smallest explicit
copy-caret field/path alongside the existing selection overlay; do not broaden
this into a compositor refactor. Mark the view dirty on entry, motion, anchor
toggle, exit, or cancellation so the compositor redraws/clears the affected
overlay cells.

While mode is active, ordinary live revisions must keep showing the frozen
captured source and move/copy against that frozen identity, matching the
existing drag guarantee. Revalidate current identity before clipboard queueing;
target removal, slot/content geometry change, live/history source switch,
epoch change, resize, buffer change, tab/route switch, focus loss, or opening a
help/menu/confirmation overlay cancels and clears the local overlay without
copying. Unknown or changed row-wrap identity fails closed. A failed or
unavailable copy reports only a fixed status string, never selected text.

## Ownership and implementation prompt

Suggested next implementation task for one exclusive owner:

> Implement the first attached-client keyboard copy-mode slice in the shared
> Panelink worktree. Own only `src/ui_client/keys.rs`, `src/ui_client/state.rs`,
> `src/ui_client/selection.rs`, `src/ui_client/state_tests.rs`,
> `src/terminal/compose.rs`, its focused compositor tests (or a dedicated
> `tests/p5_keyboard_copy.rs`), and a new `docs/progress/P5-KEYBOARD-COPY-RESULT-2026-10-25.md`.
> Do not edit README, SPEC, DISCOVERY, ROADMAP, SESSION, shared module roots,
> protocol/server files, Cargo files, other active owners' files, or this plan.
> Use the contract in `P5-KEYBOARD-COPY-PLAN-2026-10-25.md`. Preserve existing
> mouse drag behavior and tests. Keep `Ctrl-B c` a clearly labeled prototype
> assumption. Add no dependency. Do not run tests/build/format unless the
> parent explicitly assigns verification; return the diff summary, exact
> touched files, acceptance evidence available from source-only review, and
> unresolved points. No commit or push.

The parent owns cross-file integration, key-help/README/SPEC updates if the
behavior is adopted, any shared roots, verification, and evidence review.
Avoid concurrent ownership of the source files above while implementation is
active.

## Automated acceptance to request from the parent

Keep these as separate application checks; do not call them host acceptance:

1. Key-map unit coverage for `Ctrl-B c`, accepted movement/anchor/copy/cancel
   keys, unsupported modifiers, release events, repeat handling, and preserved
   `Ctrl-X` detach.
2. Copy-mode lifecycle coverage proving entry fails closed without confirmed
   wrap metadata; movement is pane-clamped; keyboard selection extraction
   matches drag extraction for Unicode combining marks, wide glyphs, soft
   wraps, hard newlines, and selected spaces.
3. A frozen-source regression where ordinary live output revises cells after
   mode entry: visible selected cells and copied content still come from the
   press/entry snapshot. An identity regression proves copy is cancelled on
   target, geometry, source/history, epoch, dimensions, buffer, route/tab, and
   modal/focus changes.
4. Input-isolation coverage for copy-mode keys, paste, and mouse showing zero
   child key/paste/scroll/mouse effects, and no input on canceled or empty
   selections. Successful `Enter` and `y` queue at most one copy payload.
5. Renderer coverage for caret placement/translation, anchor-to-caret inverse
   overlay, and clearing/repainting both after movement and after mode exit,
   including wide-cell boundaries and small pane rectangles.
6. Clipboard tests use an injected fake sink and assert only fixed status
   messages. Never read or write the user's real clipboard for automated
   acceptance.

The parent should run only the checks permitted by the current orchestration
and record exact commands/results in the implementation result. Full-suite or
host checks remain distinct evidence classes.

## Separate host acceptance gates

After automated implementation acceptance, Apple Terminal validation still
needs to observe the actual delivered key sequences and verify the proposed
prefix does not conflict with the host or an inner application. Confirm entry,
movement, selection visibility, copy-once, Escape, Ctrl-X detach/restoration,
continued output during copy mode, resize/focus/modal cancellation, and mouse
drag behavior with a mouse-reporting child. Verify clipboard contents only as
an explicitly interactive host check. Record macOS and Terminal versions and
actual outcomes; do not infer this from fake-sink tests or PTY fixtures.

This design neither enables programmatic child clipboard requests nor reads
the clipboard except through explicit user copy/paste paths already owned by
the attached client. Phase 5 and all outstanding reboot, human host,
representative-workload, and physical recovery gates remain open until their
own evidence is recorded.
