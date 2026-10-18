# Operator host-suite report — 2026-10-17

Source: terminal output pasted by the user, from Apple Terminal outside the agent command sandbox. This is operator-reported evidence, not a parent-run check.

Command:

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p2_host -- --ignored
```

Result: exit101, 4 passed / 5 failed / 0 ignored / 2 filtered, 8.58 seconds. This supersedes the earlier statement that all nine scenarios were unrun; they remain ignored by default for the restricted agent environment.

Passed:

- `host_hangup_exits_within_deadline`
- `external_sigterm_is_polled_and_hangup_ignoring_child_is_killed_within_budget`
- `local_ctrl_q_remains_available_while_child_exit_output_is_draining`
- `typed_keys_and_one_decoded_paste_reach_the_child_once`

Failed:

- `oversized_paste_drops_suffix_and_flushes_already_queued_outer_input`: retained-slave descriptor ioctl returned errno25 (ENOTTY) at tests/p2_host.rs:518.
- `normal_child_exit_restores_termios_and_never_forwards_osc_controls`: post-exit tcgetattr returned ENOTTY at line270.
- `resize_is_applied_to_the_child_and_an_undersized_resize_restores_the_host`: same post-exit ENOTTY.
- `ctrl_q_restores_the_outer_terminal_and_stops_the_child`: same post-exit ENOTTY.
- `output_stall_diagnostic_and_child_cleanup_are_bounded_without_outer_drain`: harness exceeded its five-second no-drain deadline at line261; no exit status was observed before the assertion.

The four passes cover only their implemented assertions. In particular, the external-signal fixture did not handshake after installing its HUP trap, so it does not establish KILL escalation or restoration timing. The clipboard sentinel/manual full-screen checklist was not part of this run.

## Diagnosis and corrections

The confirmed retained-slave lifecycle defect has been corrected. A disposable safe-Rustix reproduction inside the sandbox confirmed that a retained slave descriptor returns ENOTTY from tcgetattr after its controlling session leader exits, while fcntl_getfl still succeeds. The original harness made the probe itself that session leader. The corrected harness must keep a supervising session leader alive until post-probe termios/flags/input assertions finish; it must preserve failures rather than swallow ENOTTY. See P2-HOST-DEBUG.md for implementation and actual checks. Output-stall diagnosis is separate in P2-STALL-DEBUG.md.

The application output loop has an added isolated unread-PTY regression: it returns OutputStalled in 1,001 ms and restores termios/reaps its child through bounded cleanup. This does not reproduce the earlier whole-process timeout. The corrected supervisor fixture must be rerun from the operator terminal; no source-derived inference or local component pass establishes that all nine operator cases now pass.

The final harness also drains queued private output only during supervisor teardown, after probe exit and restoration assertions. A suppressed-output regression confirms the child status can be observed without draining, then the supervisor drains/reaps on release. Re-run the corrected nine scenarios serially with `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1` from Terminal. Local full verification is recorded in the latest INTEGRATION.md.

## Operator confirmation after corrections

The user subsequently replied "all corrected" following the requested serial rerun. Record this as operator confirmation that the reported host-test failures are resolved. No new raw test output, exact counts or timings were provided, so no additional measured pass count is asserted. This supersedes the outstanding correction-confirmation request, while retaining the initial failing run above. Manual Apple Terminal shell/editor/Unicode/paste/resize/clipboard validation and the explicit restoration-before-child-wait observation remain pending.

## Partial manual feedback and redraw issue

The user subsequently reported that most manual terminal checks were fine, but did not provide itemized outcomes or a new host/version record. Ctrl-Q stopped the disposable probe before they finished; they do not yet understand the child-process timing check and chose to defer it. This records qualitative partial feedback, not all manual cases passing. Ctrl-Q is the intentional whole-probe stop action; the checklist now distinguishes it from leaving `vi` with Esc, `:q!`, Enter. The pending timing observation is that saved host input/display state must be restored before waiting for a slow child to stop.

The user also observed that every change reloads the entire screen. Source confirmed the old renderer erased and repainted every frame. The incremental renderer and bounded host cache integration correct this in source; parser replay and host-frame unit evidence are in P2-DELTA-RESULT.md. Apple Terminal retest of the changed path remains pending. No new per-item manual pass count is asserted.

## Short retest after incremental rendering

User reports Ctrl-Q triggers a host keyboard/window-close confirmation and everything else in the supplied short retest works. Record operator confirmation for redraw, Unicode, editor/resize, paste and Ctrl-C, without an invented test count, timing or host-version update. The short retest omitted the original separate clipboard sentinel scenario, so that result remains unspecified. The application local-quit branch does not request a host window close; actual host key mapping is unverified. Added Ctrl-X as the documented local quit key, retaining Ctrl-Q compatibility, with decoder/dispatch verification that it queues no child input. Final Ctrl-X and outer-shell restoration confirmation remains pending.

## Ctrl-X and single-word launcher confirmation — 2026-10-18

After being asked to run `panelink`, press Ctrl-X and confirm return to the
normal outer shell without a close dialog or broken typing, the operator
replied "works". Record that scoped launcher/local-quit/restoration check as
operator-confirmed passing. No process timing, raw output or new host-version
record was supplied. The separate clipboard sentinel remains unconfirmed; this
response does not establish every item in the original full checklist.

## Clipboard reply injection corrected; zsh launcher — 2026-10-18

The operator's manual OSC52 read produced `52;c;` at the shell prompt, then
`52` and `c` command-not-found errors. Source confirmed that the clipboard
callback queued an empty OSC52 response into child input. Although it contains
no clipboard data, a shell with no clipboard-response consumer can interpret
its bytes as input. This is a failed manual acceptance observation, not proof
that clipboard contents were exposed or changed.

The callback now discards clipboard reads without responding, matching the
write/clear denial. The canonical spec, core contract and experiment were
updated; four supported CSI queries retain their existing behavior. The former
empty-response contract and source-only tests missed shell behavior. Unit and
real-PTY regressions both failed before the fix; the PTY observed16injected
bytes for BEL/ST reads. After the fix both pass with zero reply bytes, and an
exact hex comparison confirms only the explicitly submitted next line reaches
the shell. No clipboard API, host control forwarding or payload storage added.

Full locked/offline suite:249passed/13ignored; strict all-target Clippy, build,
format checks pass. The executable used by the launcher was rebuilt. Actual
clipboard sentinel/manual behavior still requires an operator retest. Earlier
source-only empty-response evidence is historical and does not certify the new
policy or the failed operator scenario. Phase completion remains pending.

At the user's request, the single-word development launcher now starts
`/bin/zsh` rather than `/bin/sh` (macOS Bash3.2). Explicit CLI arguments are
unchanged. The interpreter for the launcher itself remains portable `/bin/sh`;
the visible child shell is zsh. No home shell configuration or login-shell
default was changed. Shell syntax and argument forwarding were checked; live
zsh host behavior awaits the next operator launch.
