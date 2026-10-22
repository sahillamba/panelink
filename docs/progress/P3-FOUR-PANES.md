# Phase 3 four-pane workspace slice (L15)

Date: 2026-10-22

## Delivered

- `MAX_PANES = 4` bounds layouts, retained managed sessions, client pane state,
  compositor snapshots, saved-layout panes, and pane-status replies.
- The pure layout model now stores a bounded seven-node binary split tree. A
  selected leaf can split left/right/up/down relative to itself. Per-node
  axis proportions are retained through deterministic resize, and closing a
  leaf collapses its parent while preserving the sibling subtree.
- Layout replies use a fixed schema-9 payload with four pane entries and three
  divider entries. Exact size, count, pane ordering, rectangle bounds, complete
  root coverage, non-overlap, and divider-neighbor checks are validated.
- Saved layouts write the 280-byte current representation. The 160-byte
  historical representation is accepted by the codec and migrated to the
  current representation when loaded through the layout store.
- Server pending-resize and pane status, session retention, local client
  geometry, focus navigation, menu capacity, recovery pane caches, and the
  incremental compositor support four slots.
- No dependencies were added; operation deduplication and nonreplayable input
  handling were not changed.

## Executed checks

Commands use `CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l15`.

- `cargo test --locked --offline --lib`: **640 passed, 0 failed, 4 ignored**.
  The ignored pathname-socket tests require a bind permission denied by this
  environment.
- `cargo fmt --check`: pass (re-run after final formatting edits pending).
- `cargo build --locked --offline`: pass before final recovery viewer adjustments;
  re-run pending.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: one run
  failed on two `src/agent_cli.rs` lint findings outside the assigned files;
  another found L15 findings, which were addressed. Final re-run pending.
- `python3 scripts/verify_non_ui.py`: summary reported `cargo_test: failed`,
  `crossterm_input_decoder_tests: passed`, `component_sources: passed`,
  `cargo_fmt: passed`, `cargo_clippy: failed`, `cargo_build: failed`,
  `python_unittest: passed`, and `docs_check: passed`. The cargo checks ran
  before the follow-up fixes; rerun pending.
- `python3 scripts/check_docs.py`: `Checked 208 Markdown file(s); 0 finding(s).`

The automated real-PTY four-pane integration test is present in
`tests/p3_four_panes.rs`; execution is pending. The Apple Terminal host
acceptance gate is not exercised by this worker.

## Parent integration and verification — 2026-10-22

The worker stopped with checks pending. On resume, the parent found and
fixed:

- `ui_protocol::validate_layout` rejected every nested split: a partial
  divider ending at a T-junction has another divider (not a pane) as its
  neighbour. The flank check now accepts pane-or-divider coverage, and there
  are positive and negative unit tests (sde2 worker, parent-reviewed). This
  bug showed up as `server_rejected` on the third split.
- The recovery view applied the chrome offset twice to pane 1, so it rendered
  out of bounds.
- `tests/p3_four_panes.rs` used nonexistent CLI forms. It was rewritten to use
  the real `serve` binary and `pane`/`wait` commands: right, down, down splits;
  a fifth split refused; four-way input isolation; an interior close
  collapses; survivors stay usable.
- Stale capacity-2 assertions were updated to fill to `SESSION_CAPACITY` and
  then expect refusal: in `p3_managed_launch`, `p3_multi_owner`, `p3_split`,
  `p3a_serve_pathname`, `p3a_attach_host` and two menu unit tests. An added
  `p3b_close` four-pane test that was geometrically impossible (three right
  splits at 81 columns) was removed; `p3_four_panes` covers the contract.
- Help/README text now says up to four panes.

Checks: `python3 scripts/verify_non_ui.py` **8/8**. Full `cargo test
--no-fail-fast`: **954 passed / 0 failed / 15 ignored** (summed from all
result lines). `p3_four_panes` passed 3 runs in a row. Apple Terminal host
acceptance of four panes has not been run.

## Scope note

The current recovery relaunch CLI still relaunches at most one split because
its orchestration API is outside this task's allowed files. Recovery view
geometry and rendering accept four-pane saved records. Full split-tree saved
layout writing/recovery display is supported, while the historical two-pane
format remains readable.
