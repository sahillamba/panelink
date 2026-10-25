# Phase 4 modal result row bounds — 2026-10-25

Status: **the result-row counting defect is corrected and verified by the
managed native probe; foreign-project refusal and return remain unproven**.

## Bounded change

The old counter treated every nonblank screen line after `Sessions` as a
selectable session row whenever `Projects` was absent. It therefore counted
lower application chrome as six unknown choices in r6 and withheld Enter.

The new extractor projects the fixed 100-column probe PTY onto the pinned
large-dialog bounds: width `min(88, terminal_width - 2)`, centered, with the
title at the source-derived three-column inset. It requires one `Sessions`
category inside those bounds, counts only contiguous nonblank rows after it,
and requires the first blank modal row as the end boundary. Lower chrome
outside the modal columns and rows after that blank are excluded. A missing
boundary, invalid geometry, standalone overflow ellipsis, or nonblank modal
right edge leaves the projection untrusted and prevents Enter. The actual
unknown contiguous result count and known fixture/competitor checks remain
strict; `Projects` still blocks filtered activation.

The basis is pinned OpenCode `v2.0.32` source:

- [`dialog-open.tsx`](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/component/dialog-open.tsx)
  constructs session options with title and footer and no `details` rows, then
  groups all session options under `Sessions` before `Projects`.
- [`dialog-select.tsx`](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/ui/dialog-select.tsx)
  renders each option as one `DialogSelectRow`; category groups have a blank
  row gap, and overflow uses a separate `…` indicator row.
- [`dialog.tsx`](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/ui/dialog.tsx)
  sets `DIALOG_INSET=3`, `DIALOG_LIST_INSET=4`, large width 88, a terminal-width
  cap of width minus two, and the `DialogLayout` blank gaps and bottom row.

This evidence supports the extractor's bounded row semantics; it does not
relax identity, uniqueness, stable-read, or same-instance observer checks.

## Focused verification

- `python3 -m unittest -v tests.test_panelink_provider_bridge_probe` — 29
  tests run, 28 passed, 1 opt-in native test skipped, 0 failures.
- `python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.
- Scoped `git diff --check` for the probe and test files — passed.
- No build was run. The existing Panelink binary SHA-256 remained
  `1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`.

Regression cases cover a single target row followed by a blank modal row and
lower chrome outside the centered modal; an unknown contiguous second result;
the `Projects` category; a standalone overflow ellipsis; a missing blank
boundary; and title/footer ellipsis content that is distinct from the pinned
standalone overflow row.

## Native managed probe

Launcher: `/Users/sahillamba/.opencode/bin/opencode` (PATH lookup did not
provide `opencode`; the pinned fallback was used). OpenCode 2.0.32 matched
SHA-256 `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
Every attempt used the existing Panelink binary, enforced loopback-only
sandbox, empty fixture sessions, zero prompt submissions, and completed
cleanup.

- r7: `/private/tmp/panelink-p4-20261025/modal-rows-r7.json` — blocked before
  Enter. The initial projection classified an ellipsis-related row condition
  as overflow; `unknown_selectable_row_count` was null and the projection
  failed closed.
- r8: `/private/tmp/panelink-p4-20261025/modal-rows-r8.json` — blocked before
  Enter. The fixed diagnostic identified `session_rows_guard_reason=ellipsis`.
  The pinned source distinguishes a standalone overflow marker from
  title/footer row content, so the extractor was narrowed to reject only the
  standalone marker. Exact title visibility and right-edge clipping checks
  remain required.
- r9: `/private/tmp/panelink-p4-20261025/modal-rows-r9.json` — the foreign
  target passed the filtered-row guard and Enter was sent. The observer
  acknowledged the exact foreign project/session on the same TUI instance at
  sequence 114, greater than the preceding sequence. `foreign_title_visible`
  was false on the settled surface, although the picker was closed, the pane
  was live/applied, and input owner remained `client` at revision 3.
  Consequently `managed_foreign_project_refused=false`; the expected
  refusal/preserved-binding evidence did not pass, and return-to-A was not
  attempted (`managed_foreign_project_returned_to_a=false`). The final
  classification is `blocked`, reason
  `foreign_project_scope_refusal_not_proven`.

The independent ordinary routes passed in r9: initial A binding, A-B-A,
passive B unchanged, Home return, new-root selection, imported-fork selection,
fork return to A, and historical-after-stop identity. Current-at-crash and
automatic-resume remained false. User-message absence checks passed before and
after; cleanup completed.

The actual modal-row blocker is resolved. The managed foreign refusal and
return gate is still open; this report does not claim Phase 4 completion. No
Rust/assets/helper/canonical files, builds, commits, pushes, activation, or
user configuration were changed.
