# Phase 5 keyboard copy mode — private PTY acceptance scenario, 2026-10-25

Status: source corrected; the corrected scenario is pending parent verification.
This note distinguishes the earlier live attempt from the current test source.

Parent verification: the corrected copy-mode scenario and existing workspace
navigation both pass, **2/0**, at `/private/tmp/panelink-p5-copy-pty-r2.log`.
Parent explicitly sets the private shell's PS1 before capture so the rendered
mode-exit prompt barrier is independent of system zsh defaults. Strict
all-target Clippy also passes. Full current-tree verification is running.

## Scenario

`tests/p5_workspace_tabs.rs` uses the existing private `Fixture` and `Client`
helpers to open an opt-in workspace route on a real outer PTY. It observes the
copy caret by scanning inverse-styled cells in one-pane content (row 2 onward,
excluding the outer border columns). The hardware cursor must be hidden while
copy mode is active; `cursor_position()` is not used as the copy caret.

An arrow moves the local caret. The test waits on one parsed screen snapshot
for an inverse content cell at a different position and for the old cell to
clear. While copy mode is active, it sends only `>P5_FLAG` and unanchored Enter.
The payload has no space or `y`/`Y`, so it cannot set an anchor or request copy.
Before typing each normal-shell `printf` barrier, the test waits for the
hardware cursor to be visible, the inverse content caret to be absent, and the
fixture prompt to be visible. After the Enter barrier it verifies that
`fixture.home/P5_FLAG` remains absent. It then re-enters copy mode, sends
Escape, waits for the same normal-shell state, sends a second barrier, and
detaches with Ctrl-X. The existing restoration check and live server assertion
remain in place.

All barrier commands are sent outside copy mode. No anchor key is sent while
copy mode is active. `wait_marker` requires the complete marker on one screen
line. No real clipboard is read or written.

## Prior live attempt

The R1 live PTY attempt failed while waiting for the initial caret at
`/private/tmp/panelink-p5-copy-pty-r1.log`. Its predicate treated the hardware
cursor position (which the renderer hides in copy mode) as the copy caret;
`last_cursor_position` records the last painted cursor position and is not the
copy caret either. The test never reached the in-mode payload or Enter, so no
clipboard operation occurred. The separate workspace tab-navigation test
passed.

## Limits and execution

This scenario covers local caret movement, child-input isolation, unanchored
Enter exit, Escape exit, Ctrl-X detach, and terminal restoration in the
existing private fixture. It does not test anchored selection, copy extraction,
clipboard effects, Apple Terminal key delivery, or real clipboard contents.
The corrected Rust test has not been run here; parent serial verification is
pending.
