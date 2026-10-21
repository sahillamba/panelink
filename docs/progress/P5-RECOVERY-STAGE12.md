# P5 recovery view model and reader — Stage 1/2

**Status:** Implemented and focused tests executed 2026-10-21. This is a
bounded model/reader slice only; no CLI, TUI, startup chooser, relaunch, or
provider recovery was implemented. Phase 5 remains open.

## Delivered

- `recovery_view` models a selected historical slot, validated layout metadata,
  spatial panes, exact historical targets, and independent row/normal-screen/
  alternate-screen source availability.
- Viewport planning uses checked split arithmetic, subtracts viewer chrome,
  clamps supported pane/outline boxes to 80x240, and preserves spatial order.
  When regular leaves do not fit, hosts of at least 3x10 receive a bounded
  proportional topology outline with fixed pane labels and no content. Only
  smaller hosts return `TooSmall`. The outline carries no saved cells or text.
- `recovery_reader::read_recovery` reads only the explicitly selected current or
  previous layout and each pane's exact target. A corrupt requested layout
  returns one sanitized error; row and checkpoint faults are isolated by pane
  and source. A gap is reported as a possible reset/discontinuity, because the
  persisted format also uses it for skipped or evicted rows and does not encode
  reset cause separately.
- The reader creates no paths, opens no `ServerStorage`, takes no lock, emits no
  output, and performs no cleanup or writes. Absent `history/` remains absent.
  Invalid history storage is labeled invalid per source. Existing store readers
  enforce no-follow regular-file, mode, link-count and bounded-length rules.
- Reader fixtures contain synthetic data under `/private/tmp`. No real user
  history, config, provider account, or marker shell was used.

## Actual verification

Toolchain/target requested: Rust 1.99.1, macOS aarch64. Cargo target directory:
`/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l2`.

The combined focused command was executed:

```text
CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l2 cargo test --locked --offline --test p5_recovery_view_model --test p5_recovery_reader -- --test-threads=1
```

Final actual test output: `p5_recovery_reader`: 7 passed, 0 failed, 0 ignored;
`p5_recovery_view_model`: 7 passed, 0 failed, 0 ignored. The required library
filter reported 0 tests selected, 0 failures, and 635 filtered out (the new
recovery behavior is covered by the integration targets above). Strict
all-target Clippy completed with no diagnostics. `cargo fmt --check` returned
exit status 0.

Initial focused attempts exposed and corrected a wrong short-axis test fixture,
test helper issues, an incorrect expected host-clamp value, and strict-Clippy
findings. Those failed attempts are not counted as passes; the final required
commands above completed successfully.

`python3 scripts/check_docs.py` was executed and reported:
`Checked 190 Markdown file(s); 0 finding(s).`

Host-terminal acceptance, CLI behavior, reboot recovery, provider identity, and
relaunch remain unimplemented/unverified.
