# P5-SAVED-LAYOUT-CODEC — implementation result note

## Owned files changed
- `src/saved_layout.rs` — full codec/model implementation; replaces all `unimplemented!()` stubs
- `docs/progress/P5-SAVED-LAYOUT-CODEC.md` — this result note

## What was implemented

- `SAVED_LAYOUT_BYTES = 160` constant
- `LayoutAxis::{Single,SideBySide,Stacked}` with Copy/Clone/Eq/PartialEq/Debug
- `LayoutRecordError` with fixed sanitized Display/Debug and `std::error::Error`
- `LayoutMetadata` with private nonzero fields, redacted Debug, constructor rejecting zeros, and read-only accessors
- `SavedLayout` with private validated fields (`metadata`, `rows`, `columns`, `axis`, `panes`), redacted Debug, and:
  - `from_snapshot` — validates snapshot revision equals metadata revision, canonical geometry/divider for count 1 and 2, target scope/identity rules, and spatial ordering
  - `empty` — canonical tombstone with rows/columns 0, Single axis, all-zero targets
  - `pane_count`, `panes`, `geometry` — read-only; `geometry` derives historical display rectangles
- `encode` — writes exactly the 160-byte format per P5-SAVED-LAYOUT.md with canonical zero padding
- `decode` — exact length check, magic/version validation, metadata nonzero checks, reserved-region zero checks, count/axis consistency, target scope/duplicate/geometry validation
- Private helpers: `root_rect`, `axis_byte`, `read_u16`, `read_u64`, `write_target`, `decode_target`, `validate_derived_geometry`, `derive_split_rects`, `validate_leaf_rect`
- `#[cfg(test)] mod tests` with 31 unit tests:
  - Empty and one-pane roundtrips
  - Four split directions via actual `PaneLayout` snapshots
  - Odd/even root dimensions
  - Minimum/maximum root sizes for count 1 and 2
  - Short records (0..159), oversize/trailing, bad magic, zero metadata fields
  - Reserved-region nonzero, count/axis mismatch, hole in pane array
  - Nonzero unused slot, target scope mismatch, duplicate pane/session
  - Zero target field, too-small and too-large roots
  - Inconsistent snapshot revision, wrong rect for one pane, wrong divider for two panes
  - Malformed snapshot not normalized
  - Time rollback independence from sequence
  - Extreme `u64::MAX` values (no wrap)
  - Debug and error redaction/sanitization
  - `geometry()` accuracy for count 1 and count 2
  - Encode offset and byte-level checks

## Verification outputs (actual)

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-a cargo test --locked --offline --lib saved_layout -- --test-threads=1
running 31 tests
test saved_layout::tests::debug_and_error_are_redacted_and_sanitized ... ok
test saved_layout::tests::decode_counts_and_axis_byte_exactly ... ok
test saved_layout::tests::decode_rejects_bad_magic ... ok
test saved_layout::tests::decode_rejects_count_axis_mismatch ... ok
test saved_layout::tests::decode_rejects_duplicate_pane_or_session ... ok
test saved_layout::tests::decode_rejects_hole_in_pane_array ... ok
test saved_layout::tests::decode_rejects_nonzero_reserved ... ok
test saved_layout::tests::decode_rejects_nonzero_unused_slot ... ok
test saved_layout::tests::decode_rejects_oversize_and_trailing ... ok
test saved_layout::tests::decode_rejects_short_records ... ok
test saved_layout::tests::decode_rejects_target_scope_mismatch ... ok
test saved_layout::tests::decode_rejects_too_large_root ... ok
test saved_layout::tests::decode_rejects_too_small_root ... ok
test saved_layout::tests::decode_rejects_zero_metadata_fields ... ok
test saved_layout::tests::decode_rejects_zero_target_field ... ok
test saved_layout::tests::empty_pane_count_and_geometry ... ok
test saved_layout::tests::empty_roundtrip ... ok
test saved_layout::tests::encode_uses_exact_offsets_and_zero_padding ... ok
test saved_layout::tests::four_directions_from_pane_layout_roundtrip ... ok
test saved_layout::tests::from_snapshot_rejects_hole_array_or_count_mismatch ... ok
test saved_layout::tests::from_snapshot_rejects_inconsistent_revision ... ok
test saved_layout::tests::from_snapshot_rejects_malformed_snapshot_not_normalized ... ok
test saved_layout::tests::from_snapshot_rejects_wrong_divider_for_two_panes ... ok
test saved_layout::tests::from_snapshot_rejects_wrong_rect_for_one_pane ... ok
test saved_layout::tests::geometry_derives_correct_rects_for_count2 ... ok
test saved_layout::tests::geometry_matches_snapshot_for_count1 ... ok
test saved_layout::tests::minimum_and_maximum_roots ... ok
test saved_layout::tests::no_wrap_with_extreme_u64_values ... ok
test saved_layout::tests::odd_and_even_root_dimensions ... ok
test saved_layout::tests::one_pane_roundtrip ... ok
test saved_layout::tests::time_may_decrease_independently_of_sequence ... ok

test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 582 filtered out; finished in 0.00s
```

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-a cargo test --locked --offline --lib pane_layout -- --test-threads=1
running 18 tests
test pane_layout::tests::capacity_is_fixed_at_two_and_rejects_a_third_pane ... ok
test pane_layout::tests::collapse_gives_either_survivor_the_root_in_every_direction ... ok
test pane_layout::tests::collapse_rejections_leave_layout_unchanged ... ok
test pane_layout::tests::exact_minimum_splits_succeed_and_one_cell_less_fails ... ok
test pane_layout::tests::four_directions_use_spatial_order_one_cell_divider_and_floor_rounding ... ok
test pane_layout::tests::hit_testing_partitions_every_root_cell_and_excludes_divider_and_exterior ... ok
test pane_layout::tests::layout_debug_redacts_targets_and_revisions ... ok
test pane_layout::tests::preview_and_rejection_failures_leave_the_layout_unchanged ... ok
test pane_layout::tests::preview_is_read_only_and_commit_changes_revision_once ... ok
test pane_layout::tests::resize_recomputes_both_panes_and_no_op_preserves_revision ... ok
test pane_layout::tests::revision_overflow_rejects_changes_but_allows_no_op_resize ... ok
test pane_layout::tests::root_and_every_resulting_leaf_respect_core_minimum_and_maximum ... ok
test pane_layout::tests::stale_invalid_oversize_and_too_small_resizes_preserve_two_panes ... ok
test pane_layout::tests::survivor_can_split_again_with_a_fresh_target_after_collapse ... ok
test pane_layout::tests::target_validation_checks_every_field_and_scope_identity_rules ... ok
test saved_layout::tests::four_directions_from_pane_layout_roundtrip ... ok
test ui_protocol::tests::planned_split_size_matches_pane_layout_geometry ... ok
test ui_protocol::tests::two_pane_layout_must_be_one_exact_partition ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 595 filtered out; finished in 0.00s
```

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-a cargo clippy --locked --offline --lib -- -D warnings
error: method `verify_server_directory` is never used
   --> src/layout_store.rs:222:19
    |
153 | impl LayoutStore {
    | ---------------- method in this implementation
...
222 |     pub(crate) fn verify_server_directory(&self, server_directory: &Path) -> io::Result<()> {
    |                   ^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: `-D dead-code` implied by `-D warnings`
    = help: to override `-D warnings` add `#[expect(dead_code)]` or `#[allow(dead_code)]`

error: could not compile `panelink` (lib) due to 1 previous error
```

**Clippy blocked by another worker's file (`src/layout_store.rs`).** This file is not in my ownership list. I waited 60s and retried as required; the error persists. `src/saved_layout.rs` itself has no clippy warnings or errors.

```text
$ rustfmt --edition 2021 --check src/saved_layout.rs
(no output)
```

## Observations and limits
- No unsafe code in `saved_layout.rs`
- Zero heap allocation in encode/decode/from_snapshot paths
- No new dependencies; `Cargo.toml`/`Cargo.lock` untouched
- No git operations performed
- `verify_server_directory` and layout_store warnings are outside owned files
- Timestamp rollback and `u64::MAX` are handled without arithmetic wrapping
- All verification commands executed; actual counts recorded above

## Ownership release
- Free to edit: `src/saved_layout.rs`, `docs/progress/P5-SAVED-LAYOUT-CODEC.md`
- Do not edit: all other files

## Parent review — 2026-10-21

Worker: Kimi K2.6 (user-requested model for this run), parent-orchestrated.
Parent source review found and fixed two validation gaps before acceptance:

- `decode` count1 did not check the single target's generation/workspace
  against the header (two-pane path did). A format-valid foreign-scope
  one-pane record was accepted. Fixed; regression
  `decode_rejects_one_pane_target_scope_mismatch`.
- `from_snapshot` count2 did not bound the ROOT to 5..80 x 20..240 (only
  leaves); e.g. an 81-row stacked root with 40-row leaves was accepted.
  Fixed; regression `from_snapshot_rejects_oversized_two_pane_root`.
- Removed an unused test helper that failed strict all-target Clippy.

Parent independent checks (clean snapshot of tracked tree + codec, isolating
a concurrently edited store file):

```text
cargo test --locked --offline --lib saved_layout -- --test-threads=1
test result: ok. 33 passed; 0 failed; 0 ignored; 557 filtered out
cargo test --locked --offline --lib pane_layout -- --test-threads=1
test result: ok. 18 passed; 0 failed; 0 ignored; 572 filtered out
cargo clippy --locked --offline --lib --tests -- -D warnings   -> Finished (0 warnings)
rustfmt --edition 2021 --check src/saved_layout.rs              -> clean
```

Component acceptance only; no persistence, runtime or reboot claim.
Codec ownership released.
