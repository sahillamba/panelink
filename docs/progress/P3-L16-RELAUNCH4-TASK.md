# L16 task: four-pane saved-layout relaunch

Worker: GPT-6-Luna (xhigh), launched by the parent 2026-10-22 via
`~/.opencode/bin/opencode run -m tesco-litellm/gpt-6-luna --variant xhigh`.

## Goal

`panelink recovery relaunch [--previous]` (src/default_open.rs
`recovery_relaunch`, ~line 1424) currently relaunches at most one split. Saved
layouts now hold up to `MAX_PANES = 4` panes as rects + dividers
(`SavedLayout::geometry()`, `dividers()`; tree rebuilt by
`PaneLayout::from_snapshot`). Make relaunch replay up to four panes.

## Exclusive files

- src/default_open.rs (only the relaunch functions: `recovery_relaunch`,
  `perform_relaunch_split`, `current_matches`, status/line building, and new
  private helpers)
- src/recovery_view.rs and src/recovery_reader.rs (only to expose saved pane
  rects to relaunch; do not change view rendering behaviour)
- tests/p5_recovery_relaunch.rs
- docs/progress/P3-L16-RELAUNCH4-RESULT.md (create)

Do NOT edit any other file. If you need a public accessor in
src/pane_layout.rs, do NOT add it; instead derive the split plan from rects
and dividers in a private helper inside src/default_open.rs (pure function,
unit-tested in that file's test module or in the integration test).

## Required behaviour

1. Build a pure split plan from the saved rects/dividers: an ordered list of
   `(source pane index in the then-current live layout, direction)` steps that
   reproduces the saved tree topology. Use only Right/Down so each new pane
   lands after its source in spatial order. Unit-test 1-, 2-, 3- and 4-pane
   topologies (including a nested T-junction layout).
2. Replace the single-split branch with a loop: after each applied split take
   the committed layout revision, re-query layout, recompute the planned split
   size, use a fresh durable operation key. Never retry an uncertain split
   under a new key (existing contract). On failure, stop: remaining saved
   panes are reported `failed` (or `unknown` for an uncertain outcome) and
   no link is written as Applied for them.
3. Generalize `current_matches` and the old→new line/link mapping to N panes
   by spatial index. Proportions are always 1:1 (no ratio op exists): compare
   topology, not exact rects; record this limitation.
4. Preserve all existing contracts: refusal while running, missing/invalid/
   empty refusal, fresh launch context (no saved cwd/env), secrets never in
   links, "provider conversations are not resumed" notice.

## Tests

Add to tests/p5_recovery_relaunch.rs a `three_pane` and `four_pane` saved
layout builder (valid PLLAY current-format record via the public
`panelink::saved_layout` API / layout store, matching the existing builders)
and a test that relaunches a four-pane nested layout and asserts 4 `(applied)`
lines, 4 shell starts, pane count 4 via `panelink panes`, and 4 Applied links
sharing the new generation, in spatial order. Keep existing tests passing.

## Verify (record actual outputs)

- `cargo fmt --check`
- `cargo clippy --locked --offline --all-targets -- -D warnings`
- `cargo test --locked --offline --bin panelink`
- `cargo test --locked --offline --test p5_recovery_relaunch --test p5_recovery_cli --test p5_recovery_view_model --test p5_recovery_reader -- --test-threads=1` (twice)

Do not commit. Do not add dependencies. Other workers concurrently edit other
files in this repo; ignore unrelated build errors outside your files only if
they clearly come from another worker's in-progress edit, and report them.
Write actual results (counts, failures, limitations) to the result file.
