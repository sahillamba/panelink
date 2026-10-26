# P5 resumed integration — 2026-10-25

Status: scoped batch parent-verified. Phase3 and Phase5 remain open; scoped
Phase4 completion at `054ac3b` is retained. Work is uncommitted, with no push
or activation. The previously dirty source was preserved throughout.

## Delivered scope

- Pure bounded project/workspace index, strict metadata-only codec, private
  lifetime-locked atomic store and additive Default/Main registration.
- Opt-in workspace CLI list/create/select/open/stop/remove. Each workspace
  routes to its own server, context, lifecycle and saved-data domain; the
  shared terminfo and legacy no-selector paths remain intact. In-client tabs
  and switching are still unimplemented. Removal retains saved files, refuses
  ambiguous/live state and preserves the legacy Default/Main entry.
- Frozen drag selection with authoritative live/history row-wrap metadata,
  Unicode cell handling, selected-cell highlighting and copy-on-release.
  Normal output preserves the captured source; geometry/view/buffer/modal
  changes or missing metadata cancel it. Clipboard writes are bounded and
  asynchronous in the attached client, with restore-before-worker cleanup.
- Negotiated wrap metadata does not change saved checkpoint/history formats
  or legacy wire replies. Programmatic child clipboard requests stay denied.

## Review and corrections

GPT-6-Luna High workers implemented exclusive slices; parent reviewed source
and ran all Cargo commands serially in the existing isolated P4 target cache.
The initial fresh target failed with ENOSPC. User storage cleanup allowed
continuation; no cleanup of user files or config was performed.

Parent checks caught incomplete client imports/borrows, a post-rename test
fault consumed before its injection point, a reply guard using legacy-only
decoders, ordinary alternate revisions invalidating selection, cleanup before
terminal restoration, and routing integration compile errors. The store
review added temporary-path identity/permission/link/content revalidation.
The parent fixed the missing-namespace remove/first-open race by sharing the
route lifecycle lock before registry mutation. The final Luna source review
found inactive legacy-tab removal could orphan registry reachability; the CLI
now refuses that removal. Prior failures are preserved in the source notes
and temporary logs; they are not relabeled as passes.

## Validation before final full run

All commands use locked/offline Cargo with
`CARGO_HOME=/private/tmp/panelink-p4-cargo-home` and
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.

- Library841passed/0failed/4ignored in18.79s.
- Focused client74/0; compose19/0; UI codec21/0; actual wrap transport1/0;
  reply negotiation/exact-target guard1/0; store fault/substitution2/0.
- Private-PTY four-pane2/0, scroll2/0 and attach5/0, including70 attachment
  cycles. These do not certify human Apple Terminal interaction.
- Selection/model/codec/store/snapshot/composite integration targets40/0,
  excluding the nested child lock-probe result from the aggregate.
- Actual two-workspace routing5/0 before the final legacy-removal regression.
- Python232run/231pass/1opt-in native skip; component fingerprints match.
  Documentation and all-source checks must be repeated after final edits.

Logs are `/private/tmp/panelink-p5-parent-*.log`. The first full serial run
stopped at the recovery chooser helper: its fixed PATH selected Apple
Python through a missing Xcode installation. The helper now uses the invoking
tool PATH while retaining the application child's explicit minimal environment.
The unchanged PTY/termios/no-start assertions pass7/0; final routing including
legacy-entry preservation passes6/0. No system developer setting was changed.
The final full serial rerun passes **1304 tests,0 failed,15 ignored**, across
70 result groups (`panelink-p5-parent-full-final.log`). Subsequent style-only
Clippy corrections have final client74/0, live-wrap1/0, host9/0 and routing6/0
regressions passing. All seven supporting checks pass in
`panelink-p5-parent-supporting-pass.log`: decoder39, component fingerprints,
repository formatting, strict all-target Clippy, isolated build, Python232run/
231pass/1opt-in skip and Markdown348/0. Earlier six-of-seven supporting runs
are preserved separately; the lint findings and corrections were not skipped. No worker test claim
substitutes for the parent-run checks.

The repository-required `cargo fmt --check` passes after sorting one new
module declaration. A broader `cargo fmt --all -- --check` attempt also
inspected pinned vendored sources and reported their existing formatting
differences; those sources were not rewritten and fingerprints still match.

## Remaining gates

Full Phase3 human four-pane Apple Terminal acceptance, real drag/clipboard
acceptance, in-client workspace tab UI, keyboard copy mode, physical reboot
and representative workload/budget acceptance remain open. Orderly restart
and synthetic workloads are separate evidence. No provider auto-resume,
current-at-crash identity, terminal-effect replay, remote execution, project
root discovery or automatic worktrees are introduced.

The next live task is Luna High `resume-20261025-tab-ui-design-r1`, read-only
code exploration and one new UI integration plan note. Review its plan before
assigning implementation. No phase-completion commit is justified by this
scoped batch; all source remains dirty and the active binary is unchanged.
