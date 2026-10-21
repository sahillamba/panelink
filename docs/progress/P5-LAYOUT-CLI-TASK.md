# P5 read-only `panelink layout show` — bounded task — 2026-10-21

Status: assigned (stage 5 of [P5-SAVED-LAYOUT.md](P5-SAVED-LAYOUT.md), read
"Read-only restart view contract"). Runs in parallel with the writer task;
this slice only reads slots written by `LayoutStore`.

## Exclusive ownership

ONLY: `src/main.rs` (dispatch + help text only), a new function block in
`src/default_open.rs` (`pub(crate) fn layout_show(previous: bool) -> i32` and
private helpers; do not change existing functions), new
`tests/p5_layout_cli.rs`, new `docs/progress/P5-LAYOUT-CLI.md`. No other edits,
no dependencies, commits or pushes.

## Behavior

- `panelink layout show` -> current slot; `panelink layout show --previous`
  -> previous slot. Anything else (extra/duplicate/unknown args, missing
  `show`) -> existing `report_cli_error(CliError::InvalidArguments)` (exit 2).
- Dispatch in `run` next to `history`, BEFORE any `default_open::open`,
  `ensure_server`, `prepare` or live-startup path.
- Resolve state root via existing `resolve_root(PANELINK_HOME, HOME)`, open
  `LayoutStore::open(&state.server.join("layout"))` (never `prepare`: must not
  create directories), `load(slot)`. No ServerStorage open/initialize, no
  generation change, no server start/attach/stop, no cleanup, no lock.
- Validate entire record before writing ANY stdout; build the full output in
  a String then print once.
- Output (stdout, exit 0), exact lines:
  ```
  saved layout: current|previous
  generation: G
  workspace: W
  snapshot sequence: S
  layout revision: R
  observed at unix ms: T
  size: ROWSxCOLUMNS
  orientation: single|side-by-side|stacked
  pane 1: G:W:P:S:O
  pane 2: G:W:P:S:O
  history: rows/screens are separate read-only sources; use `panelink history show TARGET`
  ```
  Empty tombstone: replace size/orientation/pane lines with
  `panes: none (empty workspace snapshot)`. Pane lines only for present panes.
  Never print anything for the slot not requested; no fallback from current
  to previous.
- Errors (stderr, exit 1, no stdout): `panelink: no saved layout` (directory
  or slot absent), `panelink: saved layout invalid` (invalid/unsupported/unsafe
  slot), `panelink: layout directory unavailable` (root resolution failure).
  Fixed text; no paths/IDs/bytes.
- Help text: add one line for `layout show [--previous]` in the existing
  help output style.

## Tests (`tests/p5_layout_cli.rs`, run the built binary via
`env!("CARGO_BIN_EXE_panelink")` with `PANELINK_HOME` set to a fresh 0700 temp
root under /private/tmp; follow tests/p5_history_corruption_cli.rs style)

- absent root/server/layout: exit 1 `no saved layout`, and afterwards NO new
  directories/files exist (full recursive listing unchanged), no `server.lock`,
  `generation` unchanged when ServerStorage pre-initialized; a configured
  `SHELL` marker script is never executed (marker file absent).
- valid current (two-pane side-by-side, stacked, one-pane, empty) written via
  `LayoutStore::prepare` + `save`: exact stdout; `--previous` absent ->
  `no saved layout` even though current exists.
- rotation: save gen1 then gen2 -> `--previous` prints gen1, default prints gen2.
- invalid current (corrupt bytes) -> exit 1 `saved layout invalid`, empty stdout,
  but `--previous` valid still prints.
- read preserves bytes, mode, mtime of both slots and directory listing.
- usage errors: `layout`, `layout show extra`, `layout show --previous
  --previous`, `layout list` -> exit 2.

Run and paste ACTUAL output into P5-LAYOUT-CLI.md:

```text
cargo test --locked --offline --test p5_layout_cli -- --test-threads=1
cargo test --locked --offline --bin panelink -- --test-threads=1
cargo clippy --locked --offline --all-targets -- -D warnings
cargo fmt --check
```

Record the known pre-existing binary failure (pathname socket bind EPERM)
separately if it appears; do not count it as yours or as a pass.
