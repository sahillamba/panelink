# Phase 3 baseline and workspace layout notifications — 2026-10-23

Status: scoped implementation, parent review and acceptance complete. Final
combined verifier passes 8/8. Parent Phase 3 remains open.

## Baseline diagnosis

This continuation began on clean detached HEAD at `bd84625`, after `70f589c`.
The old index-resolution next action was stale: there were no unmerged paths.
The parent created `codex/p3-integration-layout-events`. See
[the bounded audit](P3-BASELINE-AUDIT.md); the original merge failure report is
preserved as historical evidence.

The fresh restricted full sweep ran 52 result groups (including doctests):
989 passed / 28 failed / 15 ignored. This reproduces the binary pathname-bind
failure and the same nine failing integration targets. Direct isolated AF_UNIX
and loopback bind probes returned EPERM inside the sandbox and passed in the
permitted context. Running all nine affected targets there, without product
changes, yielded 35 passed / 0 failed / 1 ignored. This clears those failures
for that permitted test context; it does not turn restricted runs into passes.

The initial permitted verifier's Cargo test command passed 1,017 tests with
0 failures and 15 ignored on the baseline compiled tree. Later checks in that
same verifier overlapped implementation edits and reported in-progress
compile/format errors. That run is not an 8/8 verifier pass. The final
post-integration run below supersedes it.

## Delivered behavior

The parent accepted [ADR-P3-LAYOUT-01](P3-LAYOUT-EVENTS-TASK.md) after reviewing
[the proposal](P3-LAYOUT-EVENTS-PROPOSAL.md). Schema 11 and optional capability
8 provide workspace reconciliation and finite watches, separate from existing
target-scoped pane events. The owner publishes ordered layout revision and
terminal-empty invalidations, with a separate 64-record / 1,536-byte volatile
journal. Snapshot and cursor are copied in one owner turn. A response holds
at most 64 events / 1,576 payload bytes; pending workspace/pane watches share
the existing eight-entry cap. No dependency or stored history format changed.

Live Read plus workspace-events authority, workspace/generation scope, lease
expiry and resume non-escalation are enforced before observation. Expired
history requires explicit reconciliation; future/wrong-scope cursors fail
closed. Notification failure preserves processes and ordinary layout queries.
The worker validates complete scoped replies before writing their first byte.
There is no subscriber thread that owns a PTY or replays a mutation.

The agent CLI uses a dedicated workspace context when the capability is
selected, emits `layout_changed`/`layout_resync`, and reconciles topology after
changes/gaps. Existing grants/servers without the capability keep bounded
polling. Pane output/owner/exit events remain independently observed during
quiet workspace watches. `--pane N` pins the initial complete target rather
than following a new occupant at N. Spatial labels update after reorder.
Confirmed empty layouts end observation; transport loss alone does not prove
child exit on the new path. Derived pane observations may coalesce intermediate
layouts and are not a complete lifecycle transcript.

## Delegation and parent corrections

Built-in collaboration launched GPT-6 Luna Medium for the initial evidence
audit and runtime adapter, and GPT-6 Luna High for the protocol/journal,
client and bounded owner/runtime review. OpenCode was not launched. Each worker
had exclusive files and left Git commits to the parent. No cost telemetry is
available. All worker ownership is released. Component records:
[codec/journal](P3-LAYOUT-EVENTS-COMPONENT-RESULT.md),
[runtime](P3-LAYOUT-EVENTS-RUNTIME-RESULT.md),
[client](P3-LAYOUT-EVENTS-CLIENT-RESULT.md),
[independent review](P3-LAYOUT-EVENTS-REVIEW.md).

Parent review corrected incomplete runtime response cursor checks, duplicate
stop settlement, client handling of quiet watches, the initial snapshot/cursor
race, skipping journal pages after a newer snapshot, retained spatial labels,
and successful/failed reconciliation at terminal empty. The parent also removed
unnecessary retired-target accumulation on the workspace path and bounded the
legacy positive-exit fallback to current panes. A final stale-read assertion
initially raced the server's shutdown linger; the fixture now observes process
exit and socket removal before expecting `no_workspace`, preserving the actual
assertion rather than weakening it.

Parent owner tests cover initial snapshot/cursor, no-op/rejected resize,
revision publication, terminal empty without a revision change, scope/expiry,
no resume escalation, rollover/resync/future rejection and ordinary queries
after notification failure. Existing duplicate-split and multiple-exit async
tests now assert no duplicate notification and one event per committed
revision. Runtime acceptance observes an actual pending watch while another
connection queries/resizes, then wake, gap recovery and stop settlement.
CLI acceptance observes ordinary output without topology changes and keeps a
filtered stream on its original target after same-generation index replacement.
The added gap scenario waits for a real `layout_changed`, stops only its spawned
observer, commits 70 alternating root resizes, resumes it and observes exactly
one `layout_resync` followed by the quiet deadline. A guard kills/reaps the
observer even if an assertion unwinds while it is stopped. The parent corrected
four strict-Clippy findings in this test helper (argument count, exclusive file
creation and parity checks), and made initial readiness time-bounded.

## Final verification

Final standard verifier (permitted socket/private-PTY context):

```text
RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py
8/8 checks passed; exit 0
root Cargo suite: 1,036 passed / 0 failed / 15 ignored
  library 691 passed / 4 ignored; binary 119 passed
  50 integration targets: 226 passed / 11 ignored
decoder: 39 passed / 0 failed
Python: 77 passed / 0 failed
strict all-target Clippy, fmt, build, component fingerprints: passed
docs: 243 Markdown files / 0 findings
```

All source and acceptance tests were frozen for this final run, including the
real CLI journal-gap regression. Earlier integrated runs also passed 8/8;
they were repeated after the final production corrections (initial-empty and
bounded exit observations), then after the added gap test and its strict-lint
corrections. The final result above is one actual complete run, not a sum of
reruns. The CLI integration target passes 4/4 within that run.

Additional normally ignored Phase 3 fixtures, run separately in the permitted
context: three library pathname peer/authentication/accepted-descriptor-race
cases pass 3/3; `p3_pathname` context-reuse scenario passes 1/1. The internal
child-only lock probe, nine Phase 2 outer-PTY cases, and real-user-environment
smoke were not directly invoked by these extra commands. The standard suite's
15 ignored remain its own result; the four extra passes are separate evidence.

Raw logs are temporary `/private/tmp/panelink-layout-events-complete-verifier.log`,
`panelink-layout-events-pathname-lib.log`, and
`panelink-layout-events-pathname-cli.log` (all under `/private/tmp`). No
per-user state or provider/model requests were used by these fixtures.

## Subsequent continuation

The metadata and proactive-renewal limitations below describe this original
layout-events checkpoint. The subsequent slice is documented in
[P3-PANE-METADATA-RESULT.md](P3-PANE-METADATA-RESULT.md); its verification is
recorded separately and does not rewrite the original counts.

## Limits and next work

Private PTY/socketpair/pathname fixtures are automated evidence; no human Apple
Terminal, physical reboot or provider identity acceptance is inferred. The dedicated
workspace connection does not proactively renew its 30-minute lease; expiry
ends the stream with an error. These are retained limits, not skipped passes.

Next bounded Phase 3 work: define and expose safe live pane cwd/executable
metadata (currently null), without arguments or environment values. Live
scrollback, managed `pane run`, richer owner identity and wider host/lifetime
contracts remain open. Phase 4 provider proof and Phase 5 hierarchy, selection
and reboot/history acceptance remain separate. This continuation does not
complete the parent phase or claim a phase-completion commit. No push was made.
