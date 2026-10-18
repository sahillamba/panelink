# P3A first implementation: pure two-pane layout

Status: implemented as a dependency-free, fixed-capacity library module. This
is layout groundwork only. It does not implement a menu, split wire operation,
launch transaction, server route, renderer, or host compatibility behavior.

## API and state

The new module is src/pane_layout.rs. It uses the existing
crate::control_protocol::TargetRef, including all five identity fields. Its
public types are:

- SplitDirection: Left, Right, Up, and Down.
- Rect: half-open x/y/rows/columns cell geometry with a checked contains test.
- PaneGeometry: a complete TargetRef paired with a Rect.
- LayoutSnapshot: revision, root rows and columns, two optional spatially
  ordered panes, and an optional one-cell divider. Its pane_count method
  reports the occupied entries.
- PaneLayout: private mutable state with new(target, rows, columns), snapshot,
  can_split(target, direction), preview_split(target, expected_revision,
  direction, new_target), commit_split(preview), resize(expected_revision,
  rows, columns), and hit_test(x, y).
- SplitPreview: opaque, Copy before/after state. planned_snapshot returns a
  copy of the planned snapshot for transaction preflight; it does not authorize
  dispatch or process effects.
- LayoutError: fixed sanitized variants InvalidTarget, InvalidDimensions,
  StaleTarget, StaleRevision, TargetScopeMismatch, TargetIdentityConflict,
  LayoutTooSmall, LayoutTooLarge, PaneLimitExceeded, RevisionExhausted,
  PreviewStale, and InvalidLayoutState.

All root and child geometry uses terminal content cells. Root dimensions must
be 5..=80 rows by 20..=240 columns. The pane array and all intermediate layout
state have fixed capacity; production code uses no heap-backed layout tree.
New pane and session IDs are supplied by the caller and are never normalized
or allocated here.

## Geometry and mutation rules

The first spatial child receives floor((dimension - 1) / 2) cells. The divider
uses the remaining one cell, and the second child receives the balance. Left
and Up put the new target first; Right and Down keep the existing target first.
Both resulting pane rectangles are checked against the full 5..=80 by
20..=240 viewport bounds. The exact minima are therefore 41 columns for a
left/right split and 11 rows for an up/down split.

The new target must contain five nonzero IDs, share generation and workspace
with the existing target, and use different pane and session IDs. Admission
also checks the full current TargetRef and expected revision. A preview does
not mutate the layout. Commit compares the complete private before-state,
including dimensions, revision, pane identities, geometry, and split ordering.
Only an exact match can commit, and commit advances the revision once.

Resize validates the whole candidate layout before assignment and preserves
both targets and the original split direction. Identical dimensions are a
no-op even at u64::MAX. Invalid, too-small, oversize, stale, or overflowing
changes leave the accepted state unchanged. The module reports invalid host
dimensions; clipping and a too-small UI overlay remain future UI work.

Debug output for PaneLayout, LayoutSnapshot, PaneGeometry, and SplitPreview
redacts target IDs and revisions. LayoutError carries no request fields and
uses static Display messages.

## Verification

Focused unit tests cover all directions and exact rectangles, odd/even
rounding, divider and hit-test partitioning, the 41-column and 11-row
thresholds, root and leaf limits, every zero identity field, target scope and
reuse checks, stale full-target/revision checks, read-only previews, exact
commit matching, two-pane capacity, resize and its no-op case, overflow, and
debug redaction.

Actual checks used CARGO_HOME=/private/tmp/panelink-p2-cargo-home:

- cargo test --locked --offline --lib pane_layout::tests:: passed: 12 passed,
  0 failed, 0 ignored (365 filtered out).
- cargo fmt --check passed with exit status 0.
- cargo clippy --locked --offline --all-targets -- -D warnings passed with
  exit status 0.
- RUSTDOCFLAGS=-D warnings cargo doc --locked --offline --no-deps passed with
  exit status 0 and generated the crate docs.
- git diff --check passed with exit status 0. The two new files were also
  checked against /dev/null with git diff --no-index --check; both returned
  status 1 with no output, which indicates the expected new-file diff and no
  whitespace findings.

An initial standalone rustc test harness compiled successfully but found two
bad test fixtures: up/down splits used roots below the documented 11-row
minimum. Those fixtures were corrected to use 11- and 12-row roots; the exact
Cargo test command then passed all 12 cases. No implementation behavior was
weakened to make the fixtures pass.

No UI, runtime, PTY, wire, launch, or Apple Terminal acceptance is claimed by
this slice. Parent-owned src/lib.rs export is required to make this module part
of the crate.
