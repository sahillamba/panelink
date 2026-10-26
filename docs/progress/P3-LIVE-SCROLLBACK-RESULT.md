# Phase 3 live scrollback read slice

Updated: 2026-10-23. This is a bounded live-read capability for the current
PTY parser. It does not read saved history files or expose a complete output
transcript.

## Behavior

`ReadSource::LiveScrollback` now reads borrowed normal-history rows from the
current `TerminalSession` through `history_screen()` and the vt100
`Screen::history_after` API. It does not change the emulator state or scrollback
viewport. The read remains available while the alternate screen is active
because the engine history API addresses the normal grid.

The result selects the newest retained physical rows up to `max_lines`, then
emits those rows in chronological order. Physical rows are separated by LF,
including rows marked wrapped by the terminal engine, with no trailing LF. The result
does not include visible screen rows and does not join wrapped rows. It
preserves cell spaces and combining text, skips wide-character continuation
cells, and applies the same `SnapshotCell` host-text sanitizer used by visible
screen snapshots. Raw escape sequences, control characters, OSC payloads,
bidi controls, and Unicode line separators are not returned.

The oldest retained cursor is found with a bounded binary search over the
current epoch's ordinal range. Each probe returns a borrowed exact-size
iterator; the search does not assume the configured ring capacity stayed the
same through resize. The output string is allocated only up to the caller's
byte limit, itself capped by `MAX_READ_TEXT_BYTES`. Cell sanitization uses one
fixed-size stack snapshot at a time. No history-row vector or full screen copy
is created.

`truncated` is true when current-epoch rows were evicted, `max_lines` omits
retained rows, or the byte budget omits text or rows. Byte truncation keeps
valid UTF-8 scalar boundaries; a separator is removed if the following row
cannot contribute any text. Empty history returns empty text with
`truncated=false`, including after RIS starts a new epoch. A zero-byte read of
retained rows returns empty text with `truncated=true`.

This reports only rows still retained by the live engine in its current
epoch. It makes no claim about evicted rows, earlier parser epochs, or a full
transcript. Saved history and previous-epoch data remain separate sources.

## Coverage and verification

Parser-helper coverage exercises newest-tail ordering, physical wrapped-row
separators, alternate-screen isolation, retained row width after resize,
Unicode combining and wide cells, bidi/line-separator/control sanitization,
UTF-8 byte boundaries, empty history, zero-byte reads, eviction, and RIS reset.
`tests/p3_detach.rs` updates the former unsupported-source assertion and reads
real PTY history while its alternate screen is active. The parent also added
an agent-CLI regression for the end-to-end command path.

The first focused run of
`cargo test --locked --offline --lib server_pty -- --test-threads=1` reported
5 passed and 1 failed: the eviction fixture expected `c,d`, while the engine's
two-row history ring correctly retained `b,c`. The fixture expectation was
corrected, then UTF-8 split-boundary and historical-width-after-resize cases
were added. At that source-checkout checkpoint, no final focused rerun or complete
verification result had been recorded. Combined-tree results are recorded
separately below.


## Integration on the metadata/events checkpoint — 2026-10-23

Integrated in `codex/p3-scrollback-integration`, based on `9f3d8a0`. The
source checkout `/Users/sahillamba/panelink` remains unchanged, including its
uncommitted work. The source patch touched the PTY adapter and two integration
targets; its agent CLI test was appended to preserve the newer metadata/event
fixtures. No dependency, vendor source, capability or wire-format change.

The source checkout's earlier verification is incomplete, not a passing full
run. `/private/tmp/panelink-live-scrollback-verifier.log` records a failure in
`attach_types_right_click_splits_keyboard_menu_and_detaches_leaving_children`
(4 passed/1 failed in that target; timed out waiting for `ACK42` in pane 2). The later
`/private/tmp/panelink-live-scrollback-verifier-rerun.log` passes that target
5/5 but times out at the standard verifier's 180-second Cargo deadline while
entering `p5_recovery_chooser`; the other seven checks passed. Neither run
certifies the combined tree. These are historical executions, not newly run
checks and not relabeled as passes.

Parent focused checks passed: adapter6/6 and the combined real CLI/detach/
workspace-event targets11/11 (including the existing metadata and event-gap
cases). All seven non-Cargo verifier checks passed: decoder39, Python77,
component fingerprints, fmt, strict all-target Clippy, offline build and docs.
These use the standard verifier command inventory with Cargo omitted because
the complete Cargo suite is run separately and serially. No standard8/8 claim.

The first complete serial Cargo run on the integrated source reported
1063passed/1failed/15ignored in145.23s. The last target,
`w2_server_shutdown::driver_drop_signals_both_children_before_shared_escalation_deadlines`,
failed at `tests/w2_server_shutdown.rs:231`: second child did not receive HUP.
The exact isolated rerun passed1/1 in1.07s without code changes. The fixture
and production shutdown code are unchanged by this integration; the cause of
the intermittent missing marker is not established and no fix is claimed.
The failing run remains evidence even if the final rerun passes. No Cargo
timeout occurred in this execution (600-second process-group deadline).

Final full serial rerun passed: **1064passed/0failed/15ignored** in122.73s:
library712/binary123/integration229 across50 integration targets; empty
doctests pass. Command: `cargo test --locked --offline -- --test-threads=1`,
run through the existing verifier's process-group runner with an explicit
600-second timeout. The unchanged shutdown fixture passes in1.08s in this run.
Neither full run hit its deadline. Final docs rechecked253files/0findings;
`git diff --check` passes. No further Rust edits followed final verification.

The integration remains uncommitted on this branch; full Phase3 is unfinished
and no phase-completion commit is claimed. No push. Next: specify/review
managed `pane run` on the existing launch worker with explicit client context,
stable returned targets and no shell-input substitution.

Logs:
`/private/tmp/panelink-scrollback-combined-focused.log`,
`/private/tmp/panelink-scrollback-combined-integration.log`,
`/private/tmp/panelink-scrollback-combined-noncargo.log`,
`/private/tmp/panelink-scrollback-combined-cargo.log`,
`/private/tmp/panelink-scrollback-shutdown-rerun.log`, and
`/private/tmp/panelink-scrollback-combined-cargo-final.log`.

Parent source review and independent GPT-6-Luna High review found no actionable
scrollback findings; see [review](P3-LIVE-SCROLLBACK-REVIEW.md). Native
collaboration was the launcher; no external OpenCode process was used.
Graph refreshed fast/nonpersistent for this worktree after source freeze.
All ownership released. The requested ms-rust skill remains unavailable.
Broader Phase3, human host, provider identity, physical reboot and active-agent
load gates remain open.
