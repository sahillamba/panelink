# BTN06 parent integration and acceptance

Source checkout: t3-baecf477, branch t3/terminal-button-design-research; base
fdd8664d3085958430f1fbd91ec066d868c33050 plus uncommitted feature/docs.

## Parent corrections and review

- Help now uses shared bounds for hit/render and valid captured release.
  Drag-away, Escape, focus loss, resize and reopening cancel. Full state170/0
  passed after this correction; reduced renderer fixtures2/0 also passed.
- Independent read-only source review identified in-bounds left dragging
  canceled workspace capture. Fixed retention and added a raw Down/in-bounds
  Drag/Up regression (1/0). No remaining material source findings reported.
- First full canonical run passed7/8 supporting gates but root cargo_test
  failed at workspace capture's previous expectation that even in-bounds drag
  invalidates a press. Preserved [initial full log](logs/btn06-canonical-initial.log).
  Corrected that cancellation scenario to drag to the half-open right edge
  outside the control, preserving its cancellation/feedback/effect assertions.
  The separately added in-bounds regression retains positive activation proof.
  Corrected cancellation matrix passes1/0, [log](logs/btn06-parent-cancel.log).
- Canonical spec, README, CLI help, older UX plan pointer, repository usage
  skill and interactive reference now describe actual source behavior and
  inactive-tab select-first guard. Skill frontmatter/local links and docs
  checker463/0 passed before final evidence file creation.

## Final combined gate — passed

`RUST_TEST_THREADS=1 CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target python3 scripts/verify_non_ui.py`
exited **0**, all **8/8** checks passed. [Full canonical log](logs/btn06-canonical-verified.log).

- Root Rust: **1626 passed, 0 failed, 20 ignored**, across84 result groups.
- Vendored decoder: **39 passed**, zero failures.
- Component-source fingerprints match.
- Formatting, strict all-target Clippy and build pass.
- Python: **233 run, one existing skip**, successful.
- Documentation: **464 Markdown files, zero findings**.

Ignored/skipped tests are not counted as acceptance passes. Earlier baseline
provider WouldBlock errors were not diagnosed or claimed fixed; the current
complete serial library and full suite pass without provider-source changes.
Independent bounded source review reports no remaining material findings after
parent's in-bounds-drag correction. Parent retains the initial red logs and all
fixture corrections described below. The full gate includes those final changes.

## Runtime and milestone boundaries

No install, live user pane operations, runtime/server restart, push or native host
acceptance occurred. Apple Terminal, real light/dark host palettes, touch and real
clipboard/provider acceptance retain their separate roadmap gates. Source
reduced-color fixtures are synthetic ANSI/parser evidence. Startup recovery
chooser stays line-mode, offline recovery viewer stays key-only. Inactive tab
close asks the user to select it first and cannot silently retarget a process.

## Integration fixture correction

The second canonical run passed library941/0 and binary343/0, then failed the
real four-pane attach fixture at its legacy `[Menu]`/`[x]` readiness strings.
Captured output showed the correct new filled labels and focused second pane.
Parent changed ONLY these label expectations in tests/p3_attach_four.rs; the
new-target/focus/readiness/process assertions remain. Proactively updated the
same stale `[Menu]` check in the wide default-open fixture. These are approved
appearance changes, not timing relaxations. Full second red log retained in
[second canonical log](logs/btn06-canonical-second.log). Focused suites are running
before the next complete canonical run. No application-source fix is claimed.

Focused legacy-fixture gates after label/title migration: four-pane attach2/0,
attach-host6/0, default-open8/0/1existing ignored real-environment test. Both attach
fixtures now wait for the grouped Layout: Split / Resize title; their full
process, focus, no-replay/input isolation and readiness predicates remain. Logs
logs/btn06-parent-{attach-four,attach-host,default-open}.log. The four-pane initial
post-label failure on the old title is retained in logs/btn06-attach-four-old-title.log.
The final full canonical run after these focused checks passed8/8 as recorded above.
