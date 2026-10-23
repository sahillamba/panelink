# Phase 3 owner discovery and client lifetime — 2026-10-23

The owner-discovery slice and abrupt attach-client death regression are delivered
on the integrated scrollback/pane-run tree. Full Phase 3 remains open.

`pane list --json` now exposes neutral server-issued `input_owner_info`:
generation-scoped lease identity, caller comparison, owner revision and raw
server-clock deadline. Legacy self/none labels remain; another lease is labeled
client rather than an inferred agent role. Text includes the same observation.
The server remains authoritative for claim/release/input; no identity registration,
authentication policy, wire/storage format or dependency changed. See the
[accepted contract](P3-OWNER-IDENTITY-TASK.md) and
[independent source review](P3-OWNER-IDENTITY-REVIEW.md).

The real private outer-PTY regression SIGKILLs the exact live attach PID without
Ctrl-X, closes its outer PTY, attaches a fresh client, and proves the server,
full target, managed shell PID and prior output survive. A unique new marker and
the earlier marker each appear once. The second client detaches normally and
restores its terminal; explicit server stop removes the managed child. No
restoration of the SIGKILLed client's terminal is claimed. See the
[lifetime review](P3-CLIENT-LIFETIME-REVIEW.md).

## Verification

Final complete serial locked/offline Cargo with `--no-fail-fast` and
`--test-threads=1`: **1088 passed/0 failed/15 ignored** in **156.25s**.
Library 712, binary 142, integration 234 across 52 targets. This run includes
the final PID assertion and attach-host timeout diagnostics. No deadline expired.
Earlier failed runs below remain distinct evidence.

- Parent owner helper tests: 2/2 pass; real owner CLI acceptance: 1/1 pass.
- Parent attach acceptance: 2/2 pass before the final explicit PID assertion.
- Seven supporting verifier checks pass: decoder 39, Python 77, component
  fingerprints, formatting, strict all-target Clippy, build and docs (260/0 at
  that run). Final documentation check: 261 Markdown files/0 findings; final
  formatting and strict all-target Clippy also pass after timeout diagnostics.
- Prior scrollback source/test hashes remain unchanged. Pane-run dispatch and
  tests remain integrated; no files were reset to HEAD.

Intermediate failures are preserved. The first owner fixture expected a narrow
error label while the existing rejected-claim path returns `input_failed`;
the corrected fixture proves nonzero failure and unchanged authoritative owner
state instead. The first killed-client fixture waited for its supervisor before
writing the release marker; the parent fixed release ordering and closed the
first outer PTY before reattach. The corrected targeted runs above pass.
Formatting checks caught intermediate import/assertion formatting; formatting
was corrected before the successful supporting check run. No production bug
was inferred from those fixture failures. The first full serial run stopped at
the unchanged `p3a_attach_host` case waiting for pane-2 `ACK42` (126.19s
total; 906 passed/1 failed/14 ignored before Cargo stopped, so this was
not a complete-suite result). Its isolated rerun passed 1/1 in 3.10s without source changes. A second full serial run stopped at the same marker (123.66s;
906 passed/1 failed/14 ignored before stopping). The parent added timeout-only
pane/client/exit diagnostics, without altering input or readiness behavior; the
whole attach-host target then passed 5/5 in 19.16s. The cause remains
unestablished; no production fix or timing guarantee is claimed. The diagnostic
full run stopped earlier in `history_writer::tests::backend_panic_fails_current_receipt_and_closes_writer`: nonblocking submission returned `Full`
before the expected panic scenario (711 passed/1 failed/4 ignored, 14.80s).
This is separate from the attach-host timeout. Its isolated rerun passed 1/1 in 0.10s. The complete no-fail-fast serial
run above then passed without production changes; this does not establish
that either intermittent condition has been fixed.

Logs live under `/private/tmp/panelink-p3-`: `owner-pure-parent.log`,
`owner-cli-parent.log`, `owner-cli-corrected.log`, `lifetime-parent.log`,
`lifetime-corrected.log`, `owner-lifetime-full.log` and
`owner-lifetime-noncargo.log`, `owner-lifetime-attach-retry.log` and
`owner-lifetime-full-retry.log`, `owner-lifetime-attach-target.log` and
`owner-lifetime-full-diagnostic.log`, `owner-lifetime-history-retry.log`,
`owner-lifetime-complete.log` and `owner-lifetime-final-clippy.log`. Temporary logs are local evidence, not durable
artifacts; this note records their results.

## Scope and continuation

Native GPT-6-Luna High workers owned implementation, owner acceptance and
client-lifetime acceptance in exclusive files. A Luna Medium worker audited
closure gates/reviewed owner code; a separate Luna High source review covered
the lifetime test. Parent reviewed and corrected fixtures, updated the canonical
contract and ran integrated checks. All ownership is released; no live worker
is required to resume. Native collaboration was the launcher. The requested
ms-rust skill remains unavailable; no guideline compliance claim is made.

This closes automated post-ready attach-process death and reattach coverage.
It does not cover killing a client during an in-flight request, server-crash
cleanup, human Apple Terminal four-pane acceptance, provider identity or reboot
recovery. Use [the closure audit](P3-CLOSURE-AUDIT.md) before choosing another
bounded Phase 3 gate. Preserve this uncommitted tree on
`codex/p3-scrollback-integration`, based on `9f3d8a0`; no phase-completion commit
or push is claimed. The original source checkout remains untouched.
