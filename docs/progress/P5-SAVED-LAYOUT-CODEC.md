# P5 saved-layout codec result — 2026-10-22

Status: pure fixed-record codec/model implemented and component checks passed
after a parent review found and corrected a snapshot-validation gap.
This is not saved-layout storage, runtime integration, restart recovery, or
Phase 5 completion.

The implementation is in `src/saved_layout.rs`. It provides the frozen
160-byte `PLLAY001` record, private validated metadata/layout values, canonical
empty records, exact snapshot geometry validation, big-endian encode/decode,
read-only historical geometry derivation, and fixed sanitized errors. Production
paths use fixed arrays and no allocation, filesystem, clock, thread, environment,
launch, or live-pane construction. Debug output for metadata and saved layouts
redacts identities and time. The parent added the crate-root export.

Executed checks:

```text
cargo test --locked --offline --lib saved_layout -- --test-threads=1
8 passed; 0 failed; 0 ignored; 559 filtered out

cargo test --locked --offline --lib pane_layout -- --test-threads=1
17 passed; 0 failed; 0 ignored; 550 filtered out

cargo clippy --locked --offline --lib -- -D warnings
Finished dev profile successfully; 0 warnings

rustfmt --edition 2021 src/saved_layout.rs
exit 0; no output
```

Parent review identified that `from_snapshot` accepted geometrically canonical
two-pane rectangles without checking the minimum size of each leaf. Such a
record could encode successfully but fail its own decoder. The validator now
checks both leaf bounds before accepting either orientation; regression cases
cover a 5x20 side-by-side snapshot with 9/10-column panes and a 5x20 stacked
snapshot with 2-row panes. Both are rejected as `InvalidGeometry`.

One initial codec run caught two test expectation issues: a mutation from a
10-row root to 5 rows was still a valid canonical split, and the external
`TargetRef` type's own `Debug` intentionally prints its public fields. The test
was corrected to exercise an invalid 40-column root, and the unsupported direct
`TargetRef` redaction assertion was removed. The codec's `SavedLayout` and
`LayoutMetadata` Debug implementations remain redacted. The corrected exact
codec command above passed.

The component tests cover empty/one/two-pane roundtrips, all four spatial split
orders, odd/even and min/max examples, all short lengths and trailing data,
header/version/metadata/reserved/tag/canonical-shape errors, invalid target
scope and duplicate pane/session identities, malformed geometry/divider/order,
sequence/time independence, and redaction. The model is deliberately historical;
no persistence, ordering policy, storage scope, checksum/authenticity, rollback
resistance, live restoration, or provider/session recovery is claimed. Parent
independent integration and broader permitted checks remain separate.

Worker ownership is released after this note. No dependency, vendor, runtime,
manifest, commit, or push changes were made by this worker.

Parent integration: eight component tests were independently rerun after the
leaf-size correction. Two additional independent wire-fixture and exhaustive
single-byte canonicality tests pass. The final integrated library, strict
all-target Clippy and selected integration checks pass; broader verification
limits are recorded in WAVE2-RESULT.md. Storage/runtime remain unimplemented.
