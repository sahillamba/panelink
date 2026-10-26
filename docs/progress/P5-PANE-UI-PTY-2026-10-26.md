# P5 attached pane UI and PTY acceptance

Date: 2026-10-26
Status: R2 test correction authored; parent serial target run is pending. This owner did not compile or execute tests.

## Scope and source grounding

The exclusive test change is `tests/p5_workspace_tabs.rs`. It preserves the two existing attached workspace navigation and keyboard-copy tests, including their shared `Client::wait_marker` behavior, and revises only the new pane-operation acceptance test and its dedicated helpers. This note is the only other file in scope.

The test uses its private `Fixture`, real `workspace open` client, outer PTY, parsed `vt100::Parser` cells and fixture-owned route cleanup. It creates a real second shell with Ctrl-B `r`; it does not construct a layout view or issue a pane-operation protocol request. The first post-split shell command is withheld until the rendered vertical divider is present and the hardware cursor is inside the right pane beside a visible shell prompt. That command explicitly sets the private fixture's prompt and emits a short readiness marker.

Pane marker, PID and `stty size` observations are parsed from cells inside the current pane bounds. Marker lines require exact trimmed line equality, and size fields must occur immediately before their exact pane-local marker. The divider is recognized only as a vertical run of pipe cells at one column across multiple pane rows; a pipe in a one-line navigation banner cannot satisfy it. Expected PTY dimensions come from the rendered root and divider geometry after subtracting the one-cell pane borders: with a 24x100 root and one header row, content height is 21 rows; a pane's content width is its outer span minus two border cells.

## Scenario and observable checks

The scenario starts two actual shell PTYs, records their PIDs, verifies each pane's initial `stty size`, zooms A, checks the zoomed PTY size against root content cells, and unzooms. It emits fresh short markers before swap, then verifies those exact markers exchange pane sides and checks A's PID after the swap.

The divider preview moves the vertical divider with Right, the key mapped to a left/right divider. The test checks that the rendered divider moves before commit, waits for the selection overlay to close, then verifies both real shell PIDs and PTY sizes against committed pane cells. A second preview is canceled with Escape; its divider returns to the committed column, its local selection overlay closes, and a pane-local shell barrier proves normal input resumed. Ctrl-X must exit successfully, restore the outer PTY termios modes, and leave the fixture-owned route server alive until normal fixture cleanup.

The test performs no clipboard or paste operation, user-service configuration, or activation. It sends no shell command while NAV or a pane-operation mode is active. No arbitrary sleep is used as an input barrier.

## R1 failure evidence and correction

The parent compiled and ran the original R1 test: the two existing tests passed and the new test failed at `P5B_INIT` (`/private/tmp/panelink-p5-pane-ui-pty-r1.log`). The rendered split had executed the initial shell in A; the divider-only wait did not prove focus had transferred to B. R1 also searched for exact markers across whole rendered rows, although a pane row includes cells from the adjacent pane and the divider. The revised test waits for the right-pane cursor and visible prompt before typing, then performs exact pane-local marker checks.

The correction also addresses three source-grounded issues found during review: pane PTY sizes exclude the pane's two border cells; Right moves an internal vertical divider while Down is ignored; and zoom's navigation banner can contain `|`, so it cannot stand in for divider cells or for leaving NAV. Old startup markers are not used as post-reflow evidence after zoom; short pane-local markers are refreshed before swap and PTY-size checks are emitted after the divider commit. No R1 result is relabeled as a pass.

## Verification boundary

This owner may run only:

```text
rustfmt --edition 2021 tests/p5_workspace_tabs.rs
rustfmt --edition 2021 --check tests/p5_workspace_tabs.rs
git diff --no-index --check /dev/null tests/p5_workspace_tabs.rs
git diff --no-index --check /dev/null docs/progress/P5-PANE-UI-PTY-2026-10-26.md
```

Rust formatting and formatting-check commands passed. The two no-index whitespace checks emitted no findings; their expected exit status was normalized because each scoped file is untracked in this shared tree. These checks do not compile or execute the corrected test and establish no passing UI/PTy acceptance. The parent owns the serial target run after integration:

```text
cargo test --locked --offline --test p5_workspace_tabs -- --test-threads=1
```

This attached two-pane scenario is separate from actual four-PTY protocol continuity. It does not establish native Apple Terminal acceptance, physical reboot behavior, clipboard behavior, workload budgets, or Phase 5 completion.

## Parent final acceptance — 2026-10-26

R2's actual outer-terminal target passed3/0 (existing tabs and clipboard-free
copy plus new pane controls). Parent removed two unused fixture bindings.
After worker-result boxing for strict Clippy, the final serial target again
passes **3 tests, 0 failed**, in21.90s:
`/private/tmp/panelink-p5-pane-ui-pty-final.log`. The complete Rust suite also
passed1399/0/15ignored before that lint cleanup; final client123/0 and strict
Clippy pass afterward. This closes the bounded automated outer-PTY gate,
not human Apple Terminal or full Phase5 acceptance.
