# Phase 3 server-death acceptance result — 2026-10-23

The existing default-open SIGKILL regression now checks the bounded crash and
replacement contract, beyond the earlier stale-socket-only observation. See
[accepted task](P3-SERVER-DEATH-TASK.md), [source audit](P3-SERVER-DEATH-AUDIT.md)
and [independent review](P3-SERVER-DEATH-REVIEW.md).

## Delivered behavior and evidence

The test starts a private default workspace, binds an old control context and
full target/generation, and has its zsh write its PID and one private append
record. Split printf output proves the command completed, rather than merely
being echoed. It installs a cooperative HUP exit trap, detaches the client,
then sends SIGKILL only after matching the private server PID record to the
current process owner and kernel start time. The server exits and leaves its
socket; this cooperative shell also exits. Zombies count as exited without a
claim of immediate reaping; uncertain process probes do not count as success.

A later explicit default-open invocation, using the fixture's fresh-shell
selection, recovers the stale endpoint. The server identity and full target
change and its durable generation advances. The saved old control context is
refused and remains byte-for-byte unchanged. A fresh context succeeds; its
live pane read lacks the old marker. One new command produces one new marker
in the authoritative pane snapshot. The old external append record stays
exactly once through replacement input and detach. Explicit replacement stop
then removes the socket and ends the server.

Only tests/p3a_default_open.rs changes executable test behavior in this slice.
Its failure-cleanup SIGKILL also validates PID/start-time/owner identity.
Application source and prior pane-run, scrollback, owner/client-lifetime tests
remain hash-unchanged. No dependencies, wire/storage format, supervisor or
production crash policy changed. The canonical spec is revision0.23.

## Verification

- Parent exact SIGKILL/recovery test: 1/1 pass in3.81s before independent
  review fixes; 1/1 pass in3.73s with PID validation/context preservation.
- Seven supporting offline checks pass: decoder39, Python77, component
  fingerprints, formatting, strict all-target Clippy, build, docs263/0 at
  that run. Formatting and strict Clippy pass again after review fixes.
- Complete final locked/offline Cargo with `--no-fail-fast` and serial test
  threads: **1088 passed/0 failed/15 ignored** in **153.17s**. Library712,
  binary142, integration234 across52 targets. No deadline expired.
- Final documentation check after evidence updates:265 Markdown files/0
  findings; `git diff --check` passes.

The worker's exact test compiled but failed at initial server startup inside
its restricted run (0pass/1fail,0.69s), before the new assertions. Its fixture
removed the server log; the underlying startup error was not captured. Parent
runs with private sockets/PTYs enabled pass; the worker failure is not relabeled
as successful acceptance or assigned an unproved root cause. One intermediate
worker compile error was corrected before that run. Parent review corrected
probe false-success/raw-render marker assertions before final verification;
independent review's PID-identity issue is resolved. Prior1088-pass checkpoint
intermittent ACK42 and history-writer admission failures remain historical
unresolved conditions, recorded in P3-OWNER-LIFETIME-RESULT.md.

Local raw logs: `/private/tmp/panelink-p3-server-death-worker.log`,
`-parent.log`, `-reviewed.log`, `-noncargo.log`, `-final-clippy.log` and
`-full.log` with the same `panelink-p3-server-death` prefix. Temporary files
are local evidence; this note preserves the durable outcomes.

## Limits and handoff

SIGKILL runs no destructor, bounded group escalation or final flush. One
cooperative shell exiting does not prove that arbitrary HUP-ignoring,
background or detached descendants die. No orphan-free crash, in-flight
operation recovery, crash-loss bound, physical reboot or provider recovery
claim is made. Retained history/layout is separate from new live process
state. No cleanup signals are derived from arbitrary saved process metadata.

Native GPT-6-Luna High workers owned test implementation and a separate
read-only audit/review. Parent defined the contract, reviewed and corrected
assertions, updated shared documentation and ran independent checks. Native
collaboration was the launcher; all ownership is released. The requested
ms-rust skill remains unavailable; no compliance claim is made.

Full Phase3 remains open. All accumulated work remains uncommitted on
codex/p3-scrollback-integration based on9f3d8a0. No phase-completion commit or
push is claimed; the original source checkout is untouched. Preserve this
working tree and read ROADMAP.md plus SESSION.md before continuation.
