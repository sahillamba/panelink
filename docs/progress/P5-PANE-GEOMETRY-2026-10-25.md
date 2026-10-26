# P5 pure pane geometry slice

Date: 2026-10-25
Status: implementation delivered for parent serial verification; not a phase
completion record.

## Scope

This slice adds pure, same-tab pane location swap and absolute internal
divider adjustment to `src/pane_layout.rs`. It does not add wire operations,
server admission, PTY resize effects, client interaction, persistence, or zoom.
The existing public `LayoutSnapshot` fields and current tree ratio
representation remain unchanged.

The repository code graph was queried first for the indexed
`Users-sahillamba-.codex-worktrees-7060-panelink` checkout. It exposed
`PaneLayout::{preview_split,from_snapshot,resize,snapshot}` and the existing
`first_cells`/`total_cells` representation. The graph transport was available;
no direct source-search fallback was needed. Requested repository guidance and
the P5 pane operations plan were then read before editing.

## API

- `PaneLayout::swap(source, destination, expected_revision)` exchanges the
  exact `TargetRef` identities at two existing leaves. Tree shape, divider
  positions, and split proportions stay in place. An exact same-target request
  is a no-op. Every other success advances revision once.
- `PaneLayout::divider_info(target, expected_revision, divider_index)` returns
  `DividerInfo` in the same preorder used by `snapshot.dividers`. The info
  includes the split region, `DividerOrientation`, current first-child span,
  and current available cells. The exact target must be below that split.
- `PaneLayout::set_divider(target, expected_revision, divider_index,
  first_cells)` sets the first child to an absolute span in current cells,
  excluding the one-cell divider. The current available cells are used as the
  stored ratio denominator. It returns the committed snapshot.

Stale full targets, stale revisions, invalid divider indexes, unrelated
dividers, invalid spans, too-small child/subtree geometry, and revision
overflow reject without changing state. Same-target swap and setting the
existing first span preserve the revision, including when it is already at
`u64::MAX`. Split ratios continue to use the existing integer multiply, floor
division, and snapshot reconstruction behavior; no 65535-based replacement
was introduced.

## Focused coverage added

Unit scenarios cover four-pane divider traversal order, exact snapshot
roundtrip, identity swap while retaining slots and ratios, absolute divider
position followed by root resize, target ancestry and full identity checks,
invalid index/revision, descendant minimum-size rejection, atomic failures,
no-op revisions, and revision overflow.

## Verification boundary

Scoped commands and actual results:

- `rustfmt --edition 2021 src/pane_layout.rs`: exit 0, no output. Its first
  invocation reported a missing `Vertical` enum variant; that was corrected
  and this command was rerun successfully.
- `git diff --check -- src/pane_layout.rs`: exit 0, no output.
- `git diff --no-index --check -- /dev/null
  docs/progress/P5-PANE-GEOMETRY-2026-10-25.md`: exit 1 because `--no-index`
  reports a new file as a diff; no whitespace diagnostics were emitted.

No Cargo, build, integration, or broader suite was run. Parent serial
verification is deferred; the focused Rust command is
`cargo test --lib pane_layout`.

Wire schema/capability choices, server owner admission and PTY reconciliation,
client controls, and zoom remain separate work. This document records only
the implemented pure geometry slice and does not mark Phase 5 complete.

## Parent review and focused execution

Parent inspected the mutation and traversal code. The first focused build
failed because new `assert_eq!` calls required Debug for private tree nodes,
and the split menu's exhaustive LayoutError match lacked the two new variants.
Parent changed those assertions to boolean equality (retaining redacted tree
internals) and mapped divider errors to the menu's existing Unavailable state.
The initial failure remains in `/private/tmp/panelink-p5-pane-geometry-r1.log`.

Parent reran the locked/offline serial filter with isolated Cargo home/target:
`cargo test --locked --offline --lib pane_layout -- --test-threads=1`.
Result: **27 passed, 0 failed, 0 ignored**, including cross-module tests matched
by the filter; `/private/tmp/panelink-p5-pane-geometry-r2.log`.
This is focused geometry verification, not the full current-tree suite or
server/client pane-operation acceptance. Wire, PTY resizing, zoom and UI
integration remain pending.
