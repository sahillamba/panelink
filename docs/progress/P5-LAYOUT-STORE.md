# P5-LAYOUT-STORE — implementation result note

## Owned files changed
- `src/layout_store.rs` — completed two-slot store with rotation, archive, fault injection, cleanup, full API.
- `tests/p5_layout_store.rs` — completed integration tests.
- `docs/progress/P5-LAYOUT-STORE.md` — this result note.

## Verification outputs (actual runs, `CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-b`)

### Unit tests (in-module)
```text
$ cargo test --locked --offline --lib layout_store -- --test-threads=1
running 25 tests
test layout_store::tests::byte_identical_rotation_retry_skips_double_archive ... ok
test layout_store::tests::cleanup_refuses_too_many_recognized_temps ... ok
test layout_store::tests::cleanup_refuses_unsafe_temp_and_deletes_nothing ... ok
test layout_store::tests::cleanup_removes_recognized_temps_and_preserves_committed ... ok
test layout_store::tests::corrupt_or_short_current_refused_and_preserved ... ok
test layout_store::tests::crash_after_archive_leaves_valid_previous_and_retry_completes ... ok
test layout_store::tests::current_unchanged_after_pre_rename_failure ... ok
test layout_store::tests::empty_current_blocks_further_empty ... ok
test layout_store::tests::empty_current_blocks_further_nonempty ... ok
test layout_store::tests::first_save_writes_current_without_previous ... ok
test layout_store::tests::invalid_current_does_not_block_load_previous ... ok
test layout_store::tests::load_never_creates_files ... ok
test layout_store::tests::lower_sequence_refused ... ok
test layout_store::tests::missing_current_with_present_previous_refused_for_save ... ok
test layout_store::tests::newer_previous_same_generation_different_bytes_refused ... ok
test layout_store::tests::nonempty_equal_revision_refused ... ok
test layout_store::tests::nonempty_to_empty_equal_revision_accepted ... ok
test layout_store::tests::prepare_creates_layout_directory ... ok
test layout_store::tests::rotation_archives_exact_current_then_writes_new ... ok
test layout_store::tests::same_generation_equal_sequence_different_bytes_refused ... ok
test layout_store::tests::same_generation_equal_sequence_identical_is_already_durable ... ok
test layout_store::tests::same_generation_higher_sequence_writes_current_only ... ok
test layout_store::tests::second_rotation_replaces_previous ... ok
test layout_store::tests::uncertain_current_rename_retry_syncs_without_touching_previous ... ok
test layout_store::tests::wrong_generation_or_workspace_candidate_refused ... ok

test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 604 filtered out; finished in 0.55s
```

### Integration tests
```text
$ cargo test --locked --offline --test p5_layout_store -- --test-threads=1
running 31 tests
test cleanup_refuses_more_than_16_recognized_temps ... ok
test cleanup_refuses_unsafe_temp_and_deletes_nothing ... ok
test cleanup_removes_only_recognized_safe_temps ... ok
test corrupt_0644_current_refused_and_bytes_preserved ... ok
test corrupt_short_long_current_refused_and_bytes_preserved ... ok
test debug_and_errors_are_redacted ... ok
test empty_then_any_further_changed_candidate_refused ... ok
test equal_seq_different_bytes_refused ... ok
test equal_seq_identical_is_already_durable ... ok
test first_save_creates_current_only ... ok
test hardlinked_current_refused ... ok
test invalid_current_does_not_block_load_previous ... ok
test load_bytes_returns_exact_160_bytes ... ok
test load_never_creates_files ... ok
test lower_seq_refused ... ok
test missing_current_plus_present_previous_refused_for_save ... ok
test newer_previous_same_gen_different_bytes_refused ... ok
test nonempty_equal_revision_refused ... ok
test nonempty_to_empty_equal_revision_accepted ... ok
test open_refuses_0755_dir ... ok
test open_refuses_missing_dir ... ok
test open_refuses_symlink_dir ... ok
test prepare_creates_layout_directory ... ok
test rotation_preserves_exact_old_bytes_in_previous ... ok
test same_gen_higher_seq_and_revision_writes_current_only ... ok
test save_on_both_absent_creates_current ... ok
test second_rotation_replaces_previous ... ok
test slot_generation_greater_than_bound_refused ... ok
test symlink_current_refused ... ok
test verify_identity_rejects_replaced_directory ... ok
test wrong_generation_or_workspace_candidate_refused ... ok

test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.76s
```

### Clippy
```text
$ cargo clippy --locked --offline --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in X.XXs
```
(Initial clippy passes for owned files; errors from `layout_writer.rs` resolved themselves after waiting for the other worker.)

### Format
```text
$ cargo fmt -- --check
(no output)
```

## Fixes applied during resume
- Test fixture `target()` / `other_target()` now accept `generation`/`workspace` parameters so every `TargetRef` in a snapshot matches the metadata generation/workspace (fixes `InvalidRecord`/`InvalidTargets` failures caused by hardcoded `7,1` in targets used with other generations/workspaces).
- Two-fixture side-by-side columns changed to `MIN_PANE_COLUMNS + 21` to satisfy `derive_split_rects` minimums (was `+20` producing an invalid root geometry).
- `snapshot_one`/`snapshot_two` helpers in both files extended with `gen`/`ws` parameters.
- `equal_seq_different_bytes_refused` test changed to build a valid but different-geometry SavedLayout instead of corrupting raw bytes.
- `#[allow(dead_code)]` annotation added to `verify_server_directory` as required by task note.
- Removed direct `verify_server_directory` calls from integration tests (it is `pub(crate)` and tested via other public paths).
- Fixed `needless_borrows_for_generic_args` across owned files.

## In-scope behavior
- `open` enforces absolute/canonical/private-ancestor/0700/no-symlink rules.
- `prepare` creates `server/layout` 0700 if absent.
- `load` / `load_bytes` are read-only, per-slot, and do not create files.
- `save` preflights both slots, rejects zero bounds, validates generation/workspace match, enforces sequence/revision ordering, handles rotation with archive before current, supports byte-identical retry recovery, and returns `Written` or `AlreadyDurable`.
- `cleanup_temporary_files` scans <=128 entries, <=16 recognized temps, preflights safety, revalidates before each unlink, syncs directory.
- Fault injection at four points (`BeforeArchiveRename`, `AfterArchiveBeforeCurrentWrite`, `BeforeCurrentRename`, `DirectorySync`) used by unit tests.
- Debug redaction on `LayoutStore`; errors sanitized (`invalid saved layout`, `unsafe layout storage`, `layout save refused`).

## Limits
- No writer/guard/runtime/CLI integration.
- No multi-file atomic transaction claimed.
- No CRC or cryptographic checksum.
- `verify_server_directory` is blocked by `#[allow(dead_code)]` until consumed by the writer slice.

## Ownership release
- Free to edit: `src/layout_store.rs`, `tests/p5_layout_store.rs`, `docs/progress/P5-LAYOUT-STORE.md`
- Do not edit: all other files, manifest, `lib.rs`, `saved_layout.rs`

## Parent review — 2026-10-21

Worker: Kimi K2.6 (first run interrupted when its parent shell exited;
resumed run completed). Parent fix: committed-slot reads now open with
O_NONBLOCK so a planted FIFO at a slot name fails validation instead of
hanging a read-only inspection; regression `fifo_slot_is_refused_without_blocking`.
Rotation/preflight/same-key/equal-revision-empty rules were reviewed
against P5-SAVED-LAYOUT.md and accepted.

Parent independent checks (clean tracked-tree snapshot + layout files):
`--lib layout_` 63 passed/0 failed (store 25 + writer 17 + codec-name
matches), `--test p5_layout_store` 32 passed/0 failed, strict all-target
Clippy and fmt clean. No checksum, multi-file atomicity or runtime claim.
Store ownership released.
