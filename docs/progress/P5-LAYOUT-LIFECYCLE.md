# P5 saved-layout driver lifecycle acceptance

Status: completed (2026-10-21).  Parent integration unchanged; this task added
acceptance tests only.

## Files produced

- `tests/p5_layout_lifecycle.rs` — 10 acceptance tests covering all required
  scenarios.
- `docs/progress/P5-LAYOUT-LIFECYCLE.md` — this note.

No `src/` files were edited. No new dependencies. No Cargo.toml/Cargo.lock
changes.

## Scenarios implemented

| # | Test name | Technique |
|---|-----------|-----------|
| 1 | `startup_one_pane_current_is_single_with_sequence_1` | Single `/bin/sh` child, no socket |
| 2 | `split_produces_two_panes_with_correct_axis_and_sequence` | Socketpair client, schema-7 split route with real `/bin/zsh` children |
| 3 | `unchanged_ticks_do_not_advance_sequence` | Single child, pump 50 ticks |
| 4 | `root_resize_changes_geometry_and_sequence` | Socketpair client, schema-8 RootResize |
| 5 | `close_one_pane_of_two_leaves_survivor_and_new_sequence` | Socketpair client, schema-9 close request |
| 6 | `natural_exit_of_last_pane_becomes_empty_tombstone` | Single child script exits naturally |
| 7 | `driver_drop_with_live_panes_preserves_non_empty_topology` | `drop(driver)` after durable sequence 1 |
| 8 | `restart_rotation_preserves_previous_bytes` | `ServerStorage::open` advances generation, `load_bytes` comparison |
| 9 | `drop_with_history_and_layout_completes_within_shared_deadline` | Both stores enabled, `drop` timed |
| 10 | `layout_show_cli_is_read_only_and_matches_saved_record` | Binary `layout show` via `CARGO_BIN_EXE_panelink` |

All scenarios use:
- Private `/private/tmp` fixture directories with `0o700` mode.
- Real child processes (`/bin/sh` or `/bin/zsh`); no mock sessions.
- Deadlines on every loop so tests never hang.
- UnixStream socketpairs for UI protocol clients (never pathname sockets).

## Defects and notes

No ignored-defect tests were required. All 10 scenarios pass cleanly.

### Timing observation (scenario 9)

`drop(driver)` with both history and layout enabled consistently completes in
well under 1 s on the test host (observed ~0.20–0.40 s across runs), staying
comfortably inside the 3 s assertion.

## Verification commands and actual output

### `cargo test --locked --offline --test p5_layout_lifecycle -- --test-threads=1`

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-e cargo test --locked --offline --test p5_layout_lifecycle -- --test-threads=1
   Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.40s
     Running tests/p5_layout_lifecycle.rs (/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-e/debug/deps/p5_layout_lifecycle-bd7fdf37c9789368)

running 10 tests
test close_one_pane_of_two_leaves_survivor_and_new_sequence ... ok
test driver_drop_with_live_panes_preserves_non_empty_topology ... ok
test drop_with_history_and_layout_completes_within_shared_deadline ... ok
test layout_show_cli_is_read_only_and_matches_saved_record ... ok
test natural_exit_of_last_pane_becomes_empty_tombstone ... ok
test restart_rotation_preserves_previous_bytes ... ok
test root_resize_changes_geometry_and_sequence ... ok
test split_produces_two_panes_with_correct_axis_and_sequence ... ok
test startup_one_pane_current_is_single_with_sequence_1 ... ok
test unchanged_ticks_do_not_advance_sequence ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
```

### `cargo clippy --locked --offline --all-targets -- -D warnings`

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-e cargo clippy --locked --offline --all-targets -- -D warnings
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
```

### `cargo fmt --check`

```text
$ cargo fmt --check
(no output)
```

### Counts

- **Tests:** 10 passed, 0 failed, 0 ignored.
- **Clippy warnings:** 0 ( exits successfully with `-D warnings` ).
- **Format check:** clean.
