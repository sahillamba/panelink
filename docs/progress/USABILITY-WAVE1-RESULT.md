# Usability wave 1 integration — 2026-10-22

User requested 2–3 Luna agents at high/medium for the usability recommendations.
Parent assigned three exclusive tasks under
[USABILITY-WAVE1-TASKS.md](USABILITY-WAVE1-TASKS.md), reviewed all source diffs,
integrated the cleanup helper, and added independent real-PTY acceptance.
All worker file ownership is released. No dependency/vendor changes, commit,
push, hierarchy selection, provider adoption or broad phase completion.

## Delivered behavior

- **U1, Luna high:** typed Ctrl-letter input, Alt printable characters, F1–F12,
  Delete/Insert/PageUp/PageDown and Shift-Tab in the attach client. CtrlB/X stay
  local and CtrlC remains Interrupt. Schema8 original key codes remain stable;
  added keys are strictly validated and old servers reject them before effects.
  Terminfo entries describe the corresponding core encodings. This is a bounded
  input profile, not arbitrary escape-stream forwarding, mouse pass-through,
  modified function/navigation-key support, or full host compatibility.
  [Worker result](U1-INPUT-RESULT.md).
- **U2, Luna medium:** CtrlB then `?` opens discoverable help using the existing
  incremental compositor. Esc/`?` dismiss; CtrlX detaches. Help consumes other
  input without hidden child effects, survives NAV expiry and clips on resize.
  [Worker result](U2-HELP-RESULT.md).
- **U3, Luna high:** teardown discard now returns after at most16 reads of a
  4096-byte stack buffer so the caller can check child status/deadlines even
  when output stays readable. Only actual EOF sets EOF; errors/exhaustion do
  not. Parent connected this helper into `TerminalSession`. Remaining async
  owner/client responsiveness work has a staged design, not an implementation.
  [Worker result and design](U3-CLEANUP-RESULT.md).

The disposable probe keeps its narrower host event map. New key encodings
require updated client/server implementations; no mixed-version negotiation
or transparent server replacement was added. Input remains nonreplayable.

## Parent evidence

| Check | Actual result |
| --- | --- |
| `cargo test --locked --offline --lib -- --test-threads=1` | 555 passed, 0 failed, 4 ignored. |
| Root test stage in populated-cache verifier | Same library result; binary98 passed/1failed on known pathname socket EPERM; Cargo stops before remaining integration targets. |
| `cargo test --locked --offline --test usability_input_pty -- --test-threads=1` | 2 passed/0failed/0ignored: exact wire-to-real-PTY bytes and HUP-ignoring continuous-output cleanup. |
| `cargo test --locked --offline --test p2_pty -- --test-threads=1` | 11 passed/0failed/0ignored. |
| `cargo test --locked --offline --test p3a_runtime --test p3b_close_runtime --test p3b_close -- --test-threads=1` | p3a_runtime1pass; p3b_close2pass; p3b_close_runtime0pass/5fail, each unable to start pathname server before attach raw mode. |
| Final `cargo fmt --check` and strict all-target Clippy | Passed after the parent removed two Boolean comparisons in help tests. |
| Vendored decoder / Python / component fingerprints | 39 decoder tests pass; 45 Python tests pass; both component fingerprints match. |
| Offline build in populated-cache verifier | Passed. |
| Final aggregate verifier | 7/8 checks pass; root Cargo test fails only at the recorded binary pathname-socket EPERM test. Earlier populated-cache run6/8 retained while Clippy correction was pending. |

The parent PTY test sends a validated schema8 request through the public typed
core to a raw child; an independent byte-to-hex observer confirms exact control,
Alt UTF8, function and navigation sequences. The second test waits for a
HUP-ignore readiness marker before shutting down the continuously writing child
and observing KILL escalation/exit within3seconds. Deterministic helper tests
prove the16-read cap; that is bounded work with a nonblocking master, not a
hard wall-clock bound on arbitrary filesystem/kernel operations.

The worker help tests use a recording fake for modal event isolation,
pre-help pending-input flush, focus preservation, CtrlX, expiry, tiny viewports,
status priority and unchanged-frame/no-full-clear rendering. These are automated
component checks, not a live Apple Terminal claim.

## Corrections, environment and remaining gates

The first standard verifier selected the writable but incomplete temporary
Cargo cache, causing missing libc/bitflags resolution. Direct Cargo uses the
populated normal registry successfully, although its Cargo home is read-only
to this session. Parent supplied a new writable temporary Cargo home referencing
that registry for the subsequent offline aggregate checks, without copying
credentials/config or changing the verifier. Initial failures remain in the logs.

The populated-cache run initially exposed two `bool_comparison` Clippy warnings
in new help tests. Parent corrected them and strict all-target Clippy passed.
Earlier worker fixture/compile corrections are preserved in their result notes.

Known pathname-socket/host startup failures remain failures, not passes or
ignored successes. Provider identity/Subagents S0, broader host input/mouse/
clipboard compatibility, full reboot/layout recovery and representative latency
remain open. The major synchronous owner launch/close and host-loop disk/socket
findings are not fixed by bounding the discard loop. Selection/history UI,
tabs/sidebar and provider status remain separate slices.

Logs are preserved in [usability-wave1-evidence](usability-wave1-evidence/),
including the [final aggregate log](usability-wave1-evidence/verifier-final.log).
No application payloads, credentials or environment dump are recorded.
Graph index refreshed fast/nonpersistent after source changes. ms-rust remains
uninstalled; no guideline compliance claim. Final docs183/0 and diff whitespace
checks pass. The full aggregate remains a failure; no phase-completion commit
is claimed while required acceptance is incomplete. No manual verification
task is assigned to the user.
