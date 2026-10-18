# P3A first implementation: pure two-pane layout

Parent-adopted bounded task. Exclusive files: src/pane_layout.rs and new
P3A-LAYOUT.md. Parent owns lib.rs exports and shared status. No process, socket,
terminal, renderer, mouse parser, wire/schema, dependency or vendor changes.
This is the first implementation slice, not the complete right-click feature.

Reuse PANE-LAYOUT.md with the current two-session bound. Implement documented
public SplitDirection::{Left,Right,Up,Down}, cell Rect, PaneGeometry (full
TargetRef and rectangle), fixed LayoutSnapshot, PaneLayout, opaque SplitPreview,
and fixed LayoutError. Choose conventional field/accessor names; preserve
these semantics and include exact API in the result note. No heap-backed tree,
Vec, HashMap or recursive layout is needed: at most two leaves and one divider.

PaneLayout::new(target,rows,columns) validates all five nonzero IDs and root
dimensions5..80 by20..240, starts revision1 and one full-root pane at x0/y0.
Root geometry always uses cell coordinates and excludes external host chrome.
Snapshots expose revision, root dimensions, up to two panes and optional divider
through immutable values. Keep mutation fields private. The pure module does
not allocate IDs or issue leases; callers supply the future target identity.

can_split(target,direction) returns a fixed eligibility/error result using the
exact full target. preview_split(target,expected_revision,direction,new_target)
returns an opaque preview without changing anything. Check exact target, expected
revision, current two-leaf capacity, new identity, geometry and revision overflow
before producing it. New target must have five nonzero fields, the same
generation/workspace as the original, and different pane and session IDs.
Reject IDs rather than normalizing/replacing them. Existing target stays intact.

Columns place first left, second right; rows place first above, second below.
Subtract one divider cell, give first floor(usable/2) and second the remainder.
Left/Up puts new target first; Right/Down puts original first. Both child
viewports must remain5..80 rows and20..240 columns. Thus left/right need41
columns and up/down11rows. All coordinates are bounded checked arithmetic.
There is no third leaf or recursive split in this prototype.

commit_split(preview) changes state only if the preview's entire before-state
still exactly matches this layout, including dimensions/revision/identities.
An old preview after resize/split must fail unchanged. A small Copy before/after
state inside the opaque preview is acceptable. Expose the planned snapshot
read-only for future transaction preflight; a preview never authorizes spawn
or ledger dispatch. A successful commit increments revision exactly once.
No global-key/dedup ledger is added here; future owner checks it before layout.

resize(expected_revision,rows,columns) validates the complete candidate tree
before mutation, recomputes both rectangles with the same ordering/rounding,
and retains exact identities. Identical dimensions are a no-op with unchanged
revision, including at u64::MAX. Changed valid dimensions increment once;
too-small/oversize/stale/overflow failure leaves every field unchanged. Test
overflow using private unit-test state, not a public arbitrary-revision setter.
Invalid physical host size is not stored as accepted layout state; future UI
must clip/show its too-small overlay independently.

hit_test(x,y) returns only an immutable pane observation for a cell inside that
pane. Divider and out-of-root cells return None. Rectangles are half-open;
there must be no overlap, gap other than divider, or focus-based targeting.
Errors/Debug must not print request/launch data; keep target IDs and revisions
redacted in custom layout/preview Debug. Geometry sizes/counts may be shown.

Tests: table all four directions with odd/even dimensions and expected exact
rectangles, minimum41/11 and one cell below, max240/80, all invalid/zero target
fields and different generation/workspace/pane/session reuse; no mutation on
preview/rejection; committed revision/identity preservation; two-leaf capacity;
stale target/revision, overflow, preview rejected after resize/commit, no-op
resize at max revision, invalid resize preserves prior two panes, all interior
cells partition correctly with divider/outside excluded, and Debug markers.
Use behavior/boundary tests, no ignored tests or production test hooks.

FIRST read AGENTS, ROADMAP, SPEC, discovery/prebuild/README and Phase3A plan.
Use graph tools before code exploration; if unavailable record and use source.
After parent exports, run cargo test --locked --offline --lib pane_layout::tests::,
cargo fmt --check, strict all-target Clippy, docs/diff checks with the established
CARGO_HOME. Record actual results, limitations and release ownership. No commits.
