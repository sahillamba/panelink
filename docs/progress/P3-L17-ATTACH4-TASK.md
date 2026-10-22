# L17 task: four-pane attach host acceptance (private outer PTY)

Worker: GPT-6-Luna (xhigh), launched by the parent 2026-10-22 via
`~/.opencode/bin/opencode run -m tesco-litellm/gpt-6-luna --variant xhigh`.

## Goal

Automate the client-side four-pane acceptance that otherwise needs a manual
Apple Terminal check: key routing, rendering, focus and close through the real
`panelink attach` client in a private outer PTY. Apple Terminal itself stays a
manual operator gate; do not claim it.

## Exclusive files

- tests/p3_attach_four.rs (create; copy the needed harness pieces from
  tests/p3a_attach_host.rs — do NOT edit that file)
- docs/progress/P3-L17-ATTACH4-RESULT.md (create)

Do NOT edit any src/ file. If you find a product bug, write a minimal failing
reproduction in your test, mark only that test `#[ignore = "reason"]`, and
describe it precisely in the result file for the parent to fix.

## Scenario

Outer PTY 100x24 (as in p3a_attach_host). Start `panelink serve --initialize
--allow-input` with zsh, attach.

1. Ctrl-B `r` (split right) on the focused pane, then focus left pane and
   Ctrl-B `d`, then focus the right pane and Ctrl-B `d` -> 4 panes. Use
   `panelink control ... layout` (`wait_control` on `pane_count`) to collect
   the four targets. Note the Ctrl-B nav mode times out after 4 s
   (src/ui_client/state.rs NAV_TIMEOUT) and focus moves with Ctrl-B + arrows.
2. Type a distinct marker command in each pane (moving focus via Ctrl-B +
   arrows / mouse left-click inside the pane rect) and verify via per-pane
   `control init <target>` + read that each marker reached only its pane.
3. Verify rendering: parse the host capture with vt100 and check each pane's
   marker appears inside that pane's rect (from `control layout`), and that
   divider cells exist between panes (including the T-junction).
4. A fifth split attempt via the menu shows the "Pane limit" reason and
   creates nothing.
5. Close one interior pane with Ctrl-B `x` then `y`; pane count 3, the
   sibling expands, the remaining three still accept input.
6. Ctrl-X detaches; children keep running; termios restored.

If pathname socket bind is denied, return early printing SKIPPED (same as
the existing harness) and record it as BLOCKED, never as passed.

## Verify (record actual outputs)

- `cargo fmt --check`
- `cargo clippy --locked --offline --all-targets -- -D warnings`
- `cargo test --locked --offline --test p3_attach_four -- --test-threads=1`
  three times; report each run.

Do not commit. Do not add dependencies. Other workers concurrently edit src/
files; if the build breaks outside your file, wait a minute and retry, and
report it if it persists.
