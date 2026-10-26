# P5 workspace tab view model — 2026-10-25

Status: bounded pure library slice; this is not the in-client tab UI or route
switch implementation. It preserves the confirmed **Project → workspace tabs
→ split panes** hierarchy and reads display names, order, and per-project tab
selection from a borrowed `WorkspaceIndex` snapshot.

## Contract

- `WorkspaceRouteId` is a client route pair of stable `ProjectId` and
  `WorkspaceTabId`; it is not a `TargetRef` and contains no path or runtime
  identity.
- `navigate` validates the complete route and returns another pair without
  mutating index selection. Tab navigation wraps within one project. Project
  navigation follows index order, skips projects with no tabs, and uses the
  destination project's selected tab (then its first tab as a deterministic
  fallback). Invalid routes and missing navigable projects return fixed errors
  that contain no names.
- `layout` clamps the screen to 80 rows by 240 columns, reserves row zero, and
  sets the pane root to `(0, 1, rows - 1, columns)`. A zero-row or zero-column
  viewport returns no header cells or hits and a zero-sized root. A one-row
  viewport retains the strip and yields a zero-height root.
- `fits_single_pane` requires the reduced root to meet the existing 7-row by
  22-column outer pane minimum. Therefore a host needs 8 rows by 22 columns to
  fit one pane below the strip; no PTY or `TerminalSize` minimum changes.
- Project and tab labels are capped at 24 terminal cells each. Tabs are shown
  in index order; overflow reserves bounded left/right controls and includes
  the current tab whenever at least one tab cell remains. A narrower viewport
  may show only a clipped project control. Hit regions cover only painted
  whole-cell labels and controls and return typed IDs/actions; row one and
  below never hit the strip.
- Label processing is deliberately conservative without a grapheme crate:
  controls become `?`, Unicode width is measured with the existing
  `unicode-width` dependency, width-two glyphs use a lead and continuation
  cell, and zero-width marks append to the preceding glyph up to the
  compositor's 32-byte cell payload bound. The complete cluster width is
  recomputed after each appended mark and the 24-cell label cap is enforced
  against that width. A mark with no preceding glyph is represented as `?`;
  overlong clusters are clipped before the next glyph. ZWJ is rendered as a
  visible `?` boundary because scalar widths alone cannot safely model a
  joined emoji across independently emitted cells; other complex emoji
  sequences are not promised to be one grapheme cell group. Emission uses the
  compositor-validated `HeaderCell` width for placement and continuation
  creation.

The model uses only immutable index views and in-memory bounded geometry. It
does not read or write the registry, perform route resolution, open sockets,
change index selection, access environment variables, own terminal processes,
or alter compositor/UI state. The parent-owned module export and subsequent UI
integration remain separate work.

## Local evidence

Inline unit scenarios cover wrapping tab/project navigation, empty-project
exclusion, deterministic invalid-route errors, selection immutability, host
clamping, pane minimums, zero/one-cell geometry, typed hit mapping, overflow
visibility, wide glyph continuation integrity, combining-mark grouping,
variation-selector width changes, bounded long combining clusters, conservative
ZWJ fallback, and header topology across every host width from 0 through 240.
This worker did not run Cargo or build/test commands; the parent serializes
Cargo work in the existing shared target directory. Suggested focused command
after the parent exports the module and the compositor API is integrated:
`cargo test --lib workspace_tabs::` with the parent's configured
`CARGO_HOME` and target directory.

Parent verification: locked/offline serial focused run passed **8/0** at
`/private/tmp/panelink-p5-tab-view-parent-r3.log`. The preceding r2 run failed
two fixture setups: the empty-project case had retained its initial tab, and
the long combining name exceeded the registry's 64-byte limit. Parent removed
the inactive fixture tab and used a valid long name; the direct glyph test
still exercises strings beyond the cell payload cap.
