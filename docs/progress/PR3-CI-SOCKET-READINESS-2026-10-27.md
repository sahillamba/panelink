# PR 3 hosted CI failure and socket readiness repair — 2026-10-27

## Hosted observation

[Run 40449868873](https://github.com/sahillamba/panelink/actions/runs/40449868873)
failed at head `ec09eb0`. Seven verifier checks passed. Root Rust stopped in
`p3a_default_open`: six passed, one failed, one ignored. The wide-terminal test
waited for pane controls and a shell prompt but displayed `connection lost`.
Later root test targets were not run by that fail-fast Cargo invocation.
The earlier local 8/8 result remains a local pass, not a hosted pass.

## Reproduction and diagnosis

The unchanged exact wide-terminal test passed locally in 2.33 seconds. Running
its entire fixture at background priority reproduced connection loss, but also
throttled the outer-PTY reader. That broad experiment is not the final control.

Temporary fixed-metadata tracing showed successful resize/claim exchanges and
repeated screen-query failures at roughly 400 ms. Nominal 1 ms socket sleeps
actually lasted about 40–55 ms under background timer coalescing. Extending only
the snapshot deadline to two seconds still failed; that experiment was rejected
and the original deadline restored. All temporary tracing was removed.

The focused regression backgrounds only the private server after checking its
PID, UID and kernel start identity. The UI and PTY drain retain normal priority.
It keeps the existing 64x269 host, 240-column workspace assertion, visible controls
and shell prompt, successful detach, and identical full shell target on reattach.
With the old transport this exact case fails with connection loss (13.19 s);
with descriptor readiness waiting it passes (2.33 s). No assertion or timeout
was relaxed. This supports the same large-transfer failure mechanism as the
hosted symptom; a fresh hosted run remains necessary to confirm that repair.

## Repair

Replace blind sleep polling after nonblocking socket WouldBlock with `poll`
for the exact descriptor and read/write direction. Listener accept and pending
connect also wait for readiness after releasing the spawn gate. The same
absolute deadlines remain authoritative, including post-poll expiry checks,
EINTR retries within the original deadline, and fail-closed partial-frame errors.
HUP/error readiness returns to the existing I/O or connect error handling.

The scoped libc call uses a borrowed live descriptor and one initialized stack
pollfd. The existing pinned libc dependency is sufficient; no manifest, lockfile,
wire format, input replay, queue capacity or authorization changes are introduced.

Added transport regressions cover a full-size snapshot through a 1,024-byte send
buffer, and expired readiness even when bytes are already available. Existing
stalled writer, poisoning, preallocation limits and spawn-gate tests remain.

Independent Luna High review found no material issue in poll safety, deadline
handling, connection errors or gate scope. This is source review, not runtime
verification. A first new test build found a scoped-thread borrow error in its
post-send assertion; moving that assertion after scope completion fixed the
fixture. Focused transport: **39 passed, 0 failed, 2 ignored**.

## Verification and logs

The first combined verifier attempt passed six of eight checks. Root Rust
stopped at `pane_read_scrollback_returns_live_history_separately_from_screen`:
its initial producer command returned `control_unavailable` before input was
submitted. A single isolated diagnostic run passed (0.78 s); no root cause is
claimed for that intermittent connection failure. The fixture now preserves a
bounded private server-stderr tail on failure, instead of discarding diagnostics.
Its assertions and timeouts are unchanged. Strict Clippy also rejected manual
modulo arithmetic in the new millisecond rounding; the equivalent standard
`is_multiple_of` expression corrects that lint without relaxing checks.

Final canonical locked/offline verifier exited 0 with **8/8 checks passed**.
Root Rust: **1,441 passed, 0 failed, 15 ignored**, 77 result groups; decoder:
**39 passed**; Python: **233 run, 232 passed, 1 opt-in skip**. Fingerprints,
formatting, strict all-target Clippy and the private-target debug build pass.
Repository docs: **388 files, 0 findings**. The updated skill reference and its
links pass separate Markdown checks. This includes the background-server
wide-terminal regression, the agent CLI target and the new transport tests.

The source was frozen for this final run; only evidence text was updated
afterward, followed by documentation and staged-whitespace checks. No installed
binary or live workspace was replaced. The fix is published as a follow-up to
PR 3; a new hosted result is pending, not inferred from local checks. Full
phase/host/clipboard/reboot/workload gates stay open.

Local logs under `/private/tmp/`:

- `panelink-pr3-ci-40449868873.log`: downloaded hosted failed step.
- `panelink-pr3-wide-baseline.log`: original normal-priority exact test pass.
- `panelink-pr3-wide-background.log`: broad background reproduction failure.
- `panelink-pr3-wide-snapshot-experiment.log`: rejected timeout-only experiment.
- `panelink-pr3-server-only-background-old.log`: old-transport focused failure.
- `panelink-pr3-server-only-background-fixed.log`: fixed focused pass.
- `panelink-pr3-transport-focused.log`: initial new test compile failure.
- `panelink-pr3-transport-focused-r2.log`: final focused transport pass.

Temporary transport/timer traces remain private under the consolidation snapshot
root; they contain only fixed request metadata and elapsed waits, not input text.

Additional logs: `panelink-pr3-readiness-verifier.log` records the first combined
failure, `panelink-pr3-agent-scrollback-diagnosis.log` the isolated pass, and
`panelink-pr3-readiness-verifier-r2.log` the final combined run.
