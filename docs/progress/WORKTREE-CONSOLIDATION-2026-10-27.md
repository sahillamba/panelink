# Worktree consolidation checkpoint — 2026-10-27

## Scope and preservation

User authorized combining the main checkout's uncommitted work with worktree
7060, committing and pushing the result, and opening a PR against main.
The original `/Users/sahillamba/panelink` checkout remains unchanged, including
its dirty files and local `main` at `bd84625`. Both checkouts' dirty files and
binary Git patches were snapshotted privately before integration at
`/private/tmp/panelink-consolidation-20261027-g_xd21e5`.

7060 remains on `codex/p5-acceptance-continuation`. It fast-forwarded from
`1060668` to fetched `origin/main` at `584abe4`; those two commits have identical
source trees. The checkpoint combines its existing scrolling/pointer controls,
wide-terminal repair, workload examples/reports, crash-recovery fixture and
evidence with main's unique documentation. No phase is marked complete.

## Overlap resolution

Three-way comparison used local main HEAD `bd84625` as the base, 7060's current
files as ours, and main's dirty files as theirs. Main's `server_pty.rs`,
`tests/p3_detach.rs` and `tests/p3_agent_cli.rs` live-scrollback changes were
already incorporated into 7060. Its newer metadata/events tests and guards
remain intact. README, spec, roadmap and agent-CLI conflicts were superseded
wording or historical status; 7060's current behavior and evidence were retained.

Unique imports are the repository Panelink skill and its maintenance rule,
agent input ownership proposal, Herdr comparison and their handoff. The imported
skill now describes current metadata/events, managed pane launch, workspace
routing and UI availability. Main's original pending scrollback result is
preserved in [an explicitly historical note](P3-LIVE-SCROLLBACK-ORIGINAL-HANDOFF-2026-10-23.md),
while the later integrated result remains canonical.

The global skill symlink and installed development launcher were left at their
existing targets. The launcher points at 7060; builds for this consolidation
use a private target directory and do not replace its installed debug binary
or restart existing server-owned processes.

## Release evidence

Independent read-only Luna High review confirmed the existing ignored release
binary's size/hash, linked libraries and lockfile/notices/vendor digests match
the modified manifest. Its dirty-build provenance does not capture the source
diff and predates later pointer/wide-terminal changes. The manifest is retained
as historical evidence, with that limitation in its Markdown companion.
No binary is added to Git and no release or installation is performed.

## Verification

The combined-tree canonical verifier exited 0 with **8/8 checks passed**:

```sh
RUST_TEST_THREADS=1 \
  CARGO_TARGET_DIR=/private/tmp/panelink-consolidation-20261027-g_xd21e5/target \
  python3 scripts/verify_non_ui.py
```

- Root Rust: **1,439 passed, 0 failed, 15 ignored** across 77 result groups.
- Vendored decoder: **39 passed, 0 failed**.
- Python: **233 run, 232 passed, 1 opt-in skip**.
- Component fingerprints, formatting, strict all-target Clippy, debug build
  and repository documentation (**387 files, 0 findings**) passed.
- Imported skill validation passed using the bundled quick validator in a
  private Python environment with PyYAML; its four Markdown files and agent
  interface YAML also passed validation. The initial system-Python validator
  attempt lacked PyYAML; no application dependency was added.
- The schema-3 live-history report passed its schema/count/bounds validator;
  all imported JSON reports parsed. These are artifact checks, not new workloads.

Raw combined log:
`/private/tmp/panelink-consolidation-20261027-g_xd21e5/verifier.log`.
Application/test/example/report hashes match the pre-consolidation 7060
snapshot. Original main HEAD and every snapshotted dirty file were independently
rechecked unchanged. Final documentation/skill-link and diff-whitespace checks
were rerun after this results update. Prior reports retain their original dates,
failures and scoped passes. No fresh OpenCode workload or host UI run occurred.

Human Apple Terminal, real clipboard, touch hardware, physical reboot and
representative workload budgets remain open. Existing capture-loss observations
and unresolved RSS variation remain explicit. The input ownership handoff
proposal remains unimplemented.
