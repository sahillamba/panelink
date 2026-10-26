# Phase 5 keyboard copy mode — implementation result, 2026-10-25

Status: bounded keyboard copy passes parent automated verification. Phase 5,
Apple Terminal acceptance and real clipboard acceptance remain open.

Parent focused verification now passes **108 client tests** and **25 compositor
tests**, at `/private/tmp/panelink-p5-copy-client-r5.log` and
`/private/tmp/panelink-p5-copy-compose-r2.log`. Earlier compile and fixture
failures remain in r1–r4 logs; parent corrected the event-kind import,
temporary-buffer lifetimes, checkpoint fixture byte order, anchor-toggle and
flush setup, and header-style expectations. The exit-key latch accepts a new
explicit Press as a new gesture. Final full-tree and clipboard-free PTY
results follow below; the real clipboard has not been exercised.

## Implemented slice

Parent final checkpoint: full locked/offline serial Rust **1356passed/0failed/
15ignored**, 71 result groups, `/private/tmp/panelink-p5-copy-full-final.log`.
The clipboard-free actual PTY target passes2/0, including workspace switching.
All seven supporting checks pass: decoder39, source fingerprints, root fmt,
strict all-target Clippy, isolated build, Python232run/231pass/1opt-in skip and
Markdown361/0 before this evidence update. Logs are
`/private/tmp/panelink-p5-copy-{decoder,fingerprints,fmt,clippy,build,python}-final.log`
and `panelink-p5-copy-docs-r1.log`. No real clipboard, activation, configuration
change, commit or push was performed. These are scoped automated checks;
Phase3/Phase5 host, physical reboot, workload and remaining M2 pane-operation
gates stay open. Earlier source failures and fixture corrections remain recorded.

- `Ctrl-B c` enters keyboard copy mode only when the focused pane's current
  visible live/history snapshot has confirmed row-wrap metadata and capture
  succeeds. The key remains a prototype shortcut assumption.
- Arrows and `h/j/k/l` move a local, pane-clamped caret. Left/right cross row
  boundaries; right skips a wide-character continuation; all caret locations
  normalize to wide-character leads. Up/down retain the column where possible.
- Space toggles the selection anchor. Enter or `y` copies once only when a
  non-empty anchor-to-caret range exists; otherwise it exits without copying.
  Escape cancels. Ctrl-X keeps the existing local detach path.
- Copy-mode keys, unsupported events, paste and mouse are consumed locally.
  Repeated movement is accepted; repeated Enter/`y` is ignored. Captured
  content remains frozen through ordinary live revisions, and copy is
  revalidated against the existing view identity before using the asynchronous
  attached-client copy queue.
- Resize, focus change/loss, geometry, buffer, route, modal, missing/changed
  wrap metadata and accepted history-view changes cancel the local mode.
  Selection extraction continues to use the existing bounded Unicode and
  soft-wrap/hard-line semantics.
- `Compositor::render_with_copy_mode` adds an optional local caret overlay
  without changing `ComposeFrame` or existing render APIs. The active pane is
  composed from the frozen snapshot, including with the opt-in header offset.
  The caret is inverse-rendered, wide-cell aware, and hides the child cursor.
- The active help text spells out `Ctrl-B c`. Recovery-availability checks are
  skipped while copy mode owns events. Ordinary status/navigation notices are
  suppressed during copy mode so they cannot cover the caret; modal overlays
  retain their existing higher priority and cancel through the event path.
- The frozen render guard compares the full `ViewIdentity`, including buffer
  identity. The initial caret uses the snapshot cursor only when visible and
  within the captured content rectangle; otherwise it starts at the top-left.
- Space toggles the anchor only on a press. After Enter/`y` exits, matching
  protocol Repeat events are consumed until the matching Release; a different
  explicit new Press clears the latch and follows normal dispatch.

The bounded view is only the pane's currently selected visible live or history
snapshot (up to existing 240×80 renderer bounds). There is no arbitrary history
search/scroll, storage, server message, replay, or programmatic terminal
clipboard access in this slice.

## Files owned and changed

- `src/ui_client/keys.rs`
- `src/ui_client/state.rs`
- `src/ui_client/selection.rs`
- `src/ui_client/state_tests.rs`
- `src/terminal/compose.rs`
- `docs/progress/P5-KEYBOARD-COPY-RESULT-2026-10-25.md`

Existing dirty tab, header, and workspace work in these shared files was
preserved. No other paths were edited. No dependency, commit, push, clipboard
activation, or user configuration change was made.

## Added acceptance coverage and correction history

The Rust tests now cover local event consumption, one-shot copy and repeat
handling (including exit-key Repeat/Release isolation), no-anchor/escape
cancellation, wrap-metadata refusal, frozen live
revisions, history-source copying, resize/focus/buffer/route/modal/wrap
cancellation, Unicode combining/wide text, hard newline and soft-wrap
extraction, and header-offset caret placement. Compositor tests parse actual
VT100 output cells for caret movement, clearing, wide-cell normalization, and
writer failure followed by a full recovery frame. Existing mouse-drag tests
remain in place.

The source-only R1 handoff recorded 11 client compile errors in
`/private/tmp/panelink-p5-copy-client-r1.log`: one `Rect` versus
`Option<Rect>` frozen-render guard comparison, plus ten borrowed checkpoint
temporaries in new tests. This revision reuses the already matched content
rectangle and binds generated checkpoint bytes before decoding. The focused
compositor handoff recorded 24 passes and one failure in
`/private/tmp/panelink-p5-copy-compose-r1.log`: `vt100` represents the wide
continuation cell's topology but does not expose its rendition independently.
The test now checks the parsed wide lead and continuation topology and the
actual emitted inverse SGR immediately before the two-column glyph. This is a
source-level correction subsequently verified by the parent's passing run.

Parent review also found recovery probing could synchronously perform
recovery-file work before local copy dispatch, non-modal status could hide the
caret, repeated exit events could fall through to child input, and the frozen
identity guard did not compare buffer identity. The implementation and
regressions above address these findings and pass the parent runs above.
The worker ran no Cargo/build/test commands. The first scoped rustfmt
attempt rejected the non-ASCII wide glyph in a byte-string assertion; it was
changed to explicit UTF-8 bytes, after which scoped Rust 2021 formatting and
`git diff --check` both passed.

The supplied parent source-freeze baseline (1,344 Rust passed, 0 failed, 15
ignored; seven supporting checks passing; three consecutive live fixture passes
without the response probe) predates this implementation and does not cover
these changes; the later 1,356-test checkpoint does. Human Apple Terminal key delivery,
mouse behavior, asynchronous clipboard operation, and real clipboard content
remain separate acceptance gates.
