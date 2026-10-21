# P5 read-only `panelink layout show` — implementation evidence — 2026-10-21

Status: CLI read-only slice complete. Files and tests accepted by automated
verification; no commits or pushes performed.

## Exclusive ownership

- `src/main.rs` — dispatch + help only (added `layout` branch before `history`,
  help line in `print_help`)
- `src/default_open.rs` — new function block appended at end before `#[cfg(test)]`
  (`layout_show`, `layout_show_result`, `format_layout_output`) plus imports
- `tests/p5_layout_cli.rs` — new integration tests
- `docs/progress/P5-LAYOUT-CLI.md` — this file

No Cargo.toml/Cargo.lock changes. No new dependencies. No unsafe.

## Actual verification outputs

```text
$ cargo test --locked --offline --test p5_layout_cli -- --test-threads=1
running 10 tests
test absent_layout_with_pre_initialized_storage_leaves_identity_unchanged ... ok
test absent_root_server_or_layout_reports_no_saved_layout_without_mutation ... ok
test empty_workspace_snapshot_exact_output ... ok
test invalid_current_reports_invalid_but_previous_still_readable ... ok
test read_preserves_bytes_mode_mtime_and_directory_listing ... ok
test rotation_previous_shows_older_generation ... ok
test usage_errors_return_exit_two ... ok
test valid_one_pane_current_exact_output ... ok
test valid_two_pane_side_by_side_exact_output ... ok
test valid_two_pane_stacked_exact_output ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

```text
$ cargo test --locked --offline --bin panelink -- --test-threads=1
running 95 tests
test result: ok. 95 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

```text
$ cargo clippy --locked --offline --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.78s
```

```text
$ cargo fmt --check
(no output)
```

Known pre-existing binary failure (pathname socket bind EPERM): **not observed**
in this run and not counted as a pass.

## Behavior implemented

- `panelink layout show` -> current slot; `panelink layout show --previous` -> previous slot.
- Anything else -> existing `report_cli_error(CliError::InvalidArguments)` (exit 2).
- Dispatch in `run` next to `history`, BEFORE any `default_open::open`, `ensure_server`, etc.
- Resolve state root via existing `resolve_root`, open `LayoutStore::open` (never `prepare`),
  `load(slot)`. No ServerStorage, no generation change, no server start/attach/stop.
- Validate entire record before writing ANY stdout; builds full output in a `String` then prints once.
- Exact stdout format matches the bounded task spec; empty tombstone replaces size/orientation/pane
  lines with `panes: none (empty workspace snapshot)`.
- Errors use fixed sanitized text with no paths/IDs/bytes.

## Tests

All 10 integration tests pass and cover every criterion in the task document:

1. `absent_root_server_or_layout_reports_no_saved_layout_without_mutation`
1. `absent_layout_with_pre_initialized_storage_leaves_identity_unchanged`
1. `valid_one_pane_current_exact_output`
1. `valid_two_pane_side_by_side_exact_output`
1. `valid_two_pane_stacked_exact_output`
1. `empty_workspace_snapshot_exact_output`
1. `rotation_previous_shows_older_generation`
1. `invalid_current_reports_invalid_but_previous_still_readable`
1. `read_preserves_bytes_mode_mtime_and_directory_listing`
1. `usage_errors_return_exit_two`

## Parent review — 2026-10-21

Worker: Kimi K2.6. Dispatch precedes every live-startup path; output is
built fully before stdout; no ServerStorage/lock/prepare is touched.
Parent fix: an unsafe/unreadable layout directory (e.g. mode 0755) was
reported as `no saved layout`; only NotFound is absence now, anything else
is `saved layout invalid`. Regression
`unsafe_layout_directory_is_invalid_not_absent`.
Parent rerun: `--test p5_layout_cli` 11 passed/0 failed. CLI ownership released.
