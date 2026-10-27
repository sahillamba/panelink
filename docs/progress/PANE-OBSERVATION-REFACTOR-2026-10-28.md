# Pane observation refactor — 2026-10-28

## Checkout

- Worktree: `/Users/sahillamba/.t3/worktrees/panelink/t3-aa1e6159`
- Branch: `t3/pane-observation-refactor`
- Base/HEAD at start: `0baf25eefbf396b2ad76f2b237bece1cde5377ed`
- Parent-reviewed refactor prepared for an independent PR against `main`.

## Implementation

Moved finite pane-wait observation state and ordering into
`src/agent_cli/pane_observation.rs`. The module owns the target-pinned
`WorkspaceLink`, initial full-screen snapshot, original index/target, finite
deadline, event cursor, screen revisions and quiet clock. Initial and steady
layout checks use `WorkspaceLink::panes()` on that same authenticated stream;
the loop no longer opens an inventory link per check. The handler retains
argument decoding, result formatting, initial index resolution, post-error exit
discovery and exit-status inventory queries.

The full target is still checked at the original index. Geometry-only revisions
continue; missing, renumbered or replaced targets are stale. Full-screen text
reconciliation, revision-based quiet reset, explicit event-gap errors and
finite timeout remain. A failed exchange returns without reconnecting or
replaying anything. Recognized-agent waits are unchanged.

The socketpair test `repeated_pane_queries_use_one_existing_connection_without_another_hello`
serves two pane queries over one existing stream and asserts both are Requests
with sequential IDs; an additional Hello would fail the test. Observation
sequence tests cover same-target geometry, renumber/replacement staleness,
quiet-clock reset and event-gap reporting. Existing CLI resize/lower-row
regression remains in place.

The Panelink skill now documents this pane-wait connection lifetime and its
separate initial resolution and exit inventory queries independently of
`agent wait`.

## Verification

Canonical `scripts/verify_non_ui.py` passed all 8 checks on the final source:
Rust tests, decoder tests, component-source verification, formatting, strict
all-target Clippy, build, Python tests, and Markdown checks. It used
`/Users/sahillamba/.local/bin/python3.11`, external
`CARGO_TARGET_DIR=/private/tmp/panelink-pane-observation-refactor-target`,
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, and
`CARGO_PROFILE_TEST_DEBUG=0`. Python reported 233 tests with one skip; the
final documentation check covered 399 Markdown files with 0 findings. The
focused observation tests passed 3/3; the repeated-query socket test passed
1/1; `p3_agent_cli` passed 8/8, including the resize/lower-row regression.

One earlier focused `p3_agent_cli` run had a transient failure in the unrelated
workspace-event replacement test; its isolated rerun passed, and the final
canonical run passed the complete target. The additional broad
`cargo fmt --all -- --check` invocation reports rustfmt differences in existing
source formatting and vendored files. The canonical `cargo fmt --check` passed,
as did direct rustfmt checking of the new observation module. No files under
`vendor/` were changed.

## Parent review

Parent source review verified the existing-link layout checks, preserved
target/revision ordering and separate exit queries. Removed an obsolete lint
allowance, moved the target-identity documentation to the observation module,
and replaced sub-100 ms quiet-clock assertions with ordering comparisons that
remain valid under scheduler delay. Skill text now explicitly identifies the
initial selection and exit-status inventory queries as well as the post-error
exit-discovery query.

Parent focused verification passed: observation tests 3/3, repeated pane-query
connection test 1/1, pane CLI 8/8, and strict all-target Clippy. Log:
`/private/tmp/panelink-pane-observation-parent.log`. The connection test proves
two pane queries use the same stream; it does not instrument a complete wait's
handshake count. The existing CLI regression exercises resize and lower-row
matching through the real wait loop. Final formatting, whitespace, skill
validation and Markdown-link checks passed. The standard skill validator used
an external temporary Python environment with PyYAML; no project dependency
was added. No live binary activation or acceptance-gate change occurred.

