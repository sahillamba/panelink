# Phase 4 V2 identity gate — 2026-10-24

The user requested Phase 4 after approving activation of the verified Phase 3
usability build. Phase 3's distinct human four-pane Apple Terminal gate stays
open. Preserve the live workspace and all existing uncommitted work.

Execution update: the V2 implementation is deliberately bootstrap-only. Its
first live run stopped before API readiness, so no untested TUI adapter was
retained. Parent diagnostics subsequently established an owned listener returning
HTTP401. A read-only exact-bundle audit established fixture-local Basic auth;
the final authenticated parent run passed readiness, exact PID/version and two
empty-session creation/listing with cleanup. Selected-route work remains open. See [results](P4-V2-IDENTITY-RESULT.md).

## Current pin and scope

Launcher resolution found native `opencode` on PATH at
`/Users/sahillamba/.opencode/bin/opencode`. An isolated private-HOME/XDG
`--version` returns **opencode v2.0.32**. Binary SHA-256:
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
A default-home version attempt failed opening its normal log with EPERM;
only the isolated version is accepted evidence. No installed CLI update,
authentication or provider settings were changed.

The existing TUI probe correctly refuses this version because its exact gate
is 1.19.0. The OS-enforced loopback-only baseline returned
`installed_cli_not_1.19.0` before any repetition or session creation. Earlier
1.19.0 initial-route/switch results and the 1.18.32 type package do not establish
V2 compatibility. The current binary matches the earlier V2 S0 pin, but local
listener permission is being rechecked rather than assumed blocked.

Primary current documentation describes a different CLI plugin entry,
`@opencode/plugin/tui` with `Plugin.define`/`setup`, and the
`context.ui.router.current()` route accessor. Those are current docs, not an
exact-version runtime proof. See the [CLI plugin reference](https://dev.opencode.ai/v2/docs/build/plugins/cli)
and [V2 API reference](https://dev.opencode.ai/v2/docs/api).

## Bounded work and ownership

- Luna High V2 owner: only new `scripts/probe_opencode_v2_identity.py`,
  `tests/test_opencode_v2_identity_probe.py` and optional new
  `docs/progress/p4-v2-tui-plugin-fixture/` files. Pin exact version/hash, enforce
  loopback-only networking with macOS sandbox-exec, use private fresh roots,
  and never use the implicit user background service. Prove readiness and two
  empty top-level sessions before attempting selected-route observation and
  controlled keyboard A-B-A. Unsupported API or missing identity must remain
  explicit; no model prompts or provider calls.
- Luna High correlation owner: only the existing
  `scripts/probe_opencode_tui_identity.py` and its focused unit-test file.
  Remove invented selection markers and prevent an observation before an
  earlier transition from satisfying a later transition. Report actual
  attempted/unissued/missed counts and reject incomplete sequences as full
  proof. Keep the1.19.0 gate and no-prompt behavior.
- Parent: scope, primary-source verification, review, independent checks,
  roadmap/handoff and durable evidence. No worker commits. No source file may
  have two editors. No changes to the activated Rust binary are planned here.

Every live attempt has fixed process/time/output bounds and unconditional
cleanup. One diagnostic repetition precedes any repeated gate. Do not keep
retrying a blocked bootstrap. Use only fixture-owned process groups and never
send input to the user's workspace. Reports must not include IDs, credentials,
raw TUI output, API bodies, user paths or transcripts; generated identity
hashes and allowlisted booleans/counts are sufficient.

## Claims that remain disabled

Server readiness, session creation and server lifecycle events do not identify
which session a pane displays. Polling is an observation, not a barrier against
an unrecorded switch or a crash. Durable binding acknowledgment, two-agent
isolation, fork/new-session cases, bridge loss, restore behavior and crash
ordering stay separate gates. Existing pure binding/codec components are not a
live bridge or durable store. No automatic resume, `--continue`, newest-session
selection, inferred current-at-crash identity or provider credential handling
is enabled by this investigation.

Verification: focused probe tests, bounded pinned live probe and parent result
review, then Python suite and docs/whitespace checks. Re-run Rust suites only
if Rust or its build inputs change. Record blocked stages as blocked, never as
passing acceptance.
