# P5 saved-layout codec — next implementation handoff — 2026-10-21

Status: queued; not implemented or executed by the preceding design slice.
Read ROADMAP.md and SESSION.md first, then AGENTS.md, README.md, SPEC.md,
PREBUILD.md, PANELINK-DISCOVERY.md and the accepted engineering scope in
[P5-SAVED-LAYOUT.md](P5-SAVED-LAYOUT.md). Parent phases remain open.

## Exclusive ownership

Future Luna xhigh owner may edit ONLY new src/saved_layout.rs and new
P5-SAVED-LAYOUT-CODEC.md result note. Parent owns src/lib.rs export, independent
integration checks, canonical docs/roadmap and final verification. Do not edit
other sources, manifests, vendor or prior reports; no dependencies, commits,
pushes or real-user files. Use graph-first code discovery and Rust2021 style.
The requested ms-rust skill is still absent. Worker does not mutate ROADMAP;
parent owns shared status, an explicit exception to append-only worker rules.

## Exact API to implement

Use crate::control_protocol::TargetRef and pane_layout::{LayoutSnapshot,Rect,
PaneGeometry,MIN_PANE_ROWS,MAX_PANE_ROWS,MIN_PANE_COLUMNS,MAX_PANE_COLUMNS}.
No persistence, threads, environment, clock, launch, runtime or CLI integration.

- `pub const SAVED_LAYOUT_BYTES: usize = 160`.
- `LayoutAxis::{Single,SideBySide,Stacked}`: Copy/Clone/Eq/PartialEq/Debug; labels
  contain no IDs. Single is the no-divider axis for both zero/one pane.
- `LayoutRecordError::{InvalidLength,UnsupportedVersion,InvalidMetadata,
  InvalidGeometry,InvalidTargets,InvalidRecord}`: fixed sanitized Debug/Display,
  std::error::Error. No payload/error-string fields.
- `LayoutMetadata` private fields generation/workspace/sequence/revision/time,
  allu64. `new(generation, workspace, snapshot_sequence, layout_revision,
  observed_at_unix_ms)->Result<Self,LayoutRecordError>` rejects anyzero.
  Copy/Clone/Eq/PartialEq; redacted Debug. Read-only accessors with matching
  field names. Metadata contains no ambient source/global clock.
- `SavedLayout` private metadata,rows:u16,columns:u16,axis:LayoutAxis,
  panes:[Option<TargetRef>;2]. Copy/Clone/Eq/PartialEq; redacted Debug.
  `from_snapshot(metadata:LayoutMetadata,snapshot:&LayoutSnapshot)->Result<Self,
  LayoutRecordError>` validates snapshot revision equals metadata revision,
  dimensions, slots, targets, exact equal geometry and divider before returning.
  `empty(metadata:LayoutMetadata)->Self` creates canonical zero geometry/slots
  and Single axis. No zero-target placeholder gets exposed as a valid pane.
  Accessors: metadata(),rows(),columns(),axis(),pane_count(),panes().
  `geometry()->[Option<PaneGeometry>;2]` derives historical display rectangles
  with fixed capacity; empty returns[None,None]. This is not a live PaneLayout
  constructor or a TargetRef dispatch adapter. Do not implement From<SavedLayout>
  for PaneLayout or expose a method that allocates/installs live identities.
- `pub fn encode(layout:&SavedLayout)->[u8;SAVED_LAYOUT_BYTES]`: no Result because
  only validated private state is constructible; all unused bytes canonicalzero.
- `pub fn decode(bytes:&[u8])->Result<SavedLayout,LayoutRecordError>`: exact
  version/length/zeroreserved/canonicalshape/target/geometry validation before
  returning. Roundtrip identity must preserve sequence/revision/time, including
  earlier wall-clock time; never use timestamp to order/normalize records.

All validation must be checked and fail closed; use fixed arrays. Metadata
validation is separate from filesystem scope/order checks reserved for the next
store slice. No heap allocation in production codec paths. Avoid manual unsafe
or duplicating unrelated terminal parser/state machinery. Snapshot orientation
must be inferred from exact canonical geometry/divider; equal rectangles with
wrong target placement/divider or a hole in the pane array are rejected.

## Format and acceptance

Use exactly the160-byte offsets and invariant table in P5-SAVED-LAYOUT.md.
The later current/previous file rotation does not change the per-record codec
and is outside this pure slice.
Count0 retains metadata with rows/columns0, Single axis, allzero target slots.
Count1 uses first slot/fullroot, axis0, second slotzero. Count2 axis1/2,
spatial order, shared header scope, distinct pane/session IDs and canonical
equal division including one-cell divider. Nonempty roots5..80x20..240;
children same bounds; two-pane minima41columns or11rows. No ratio/tree/version
extension or target generation remapping.

Tests: one-pane and empty roundtrips; derive snapshots from actual PaneLayout
for all four split gestures/spatial orders, odd/even roots and min/max cases;
all159+zero short record lengths, oversize/trailing, bad magic/version,
metadatazero, flags/reserved, counts/axis/hole/nonzero-unused, scope/duplicate/
zero target fields, too-small/large roots/leaves, inconsistent snapshotrevision,
rect/divider/array order; metadata time may decrease independently of increasing
sequence; no arithmetic wrap; full debug/error redaction. Tests are component
acceptance, never a saved-layout persistence or reboot claim. Include explicit
counterexamples that malformed public LayoutSnapshot cannot get normalized.

Run and paste ACTUAL results in owned result note:

```text
cargo test --locked --offline --lib saved_layout -- --test-threads=1
cargo test --locked --offline --lib pane_layout -- --test-threads=1
cargo clippy --locked --offline --lib -- -D warnings
rustfmt --edition 2021 src/saved_layout.rs
```

Coordinate parent export before Cargo compiles the module; do not edit lib.rs
yourself. Notify freeze before parent independent/full checks. Record exact
counts, corrected development failures, limits and ownership release. Parent
runs relevant full permitted verification and preserves blocked host gates.
No proposed acceptance scenario counts as an executed pass.
