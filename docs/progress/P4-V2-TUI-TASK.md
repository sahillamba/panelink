# Phase 4 selected-route and two-client gate — 2026-10-24

Execution: scoped initial-route/A-B-A/passive-client gate passes 3/3; see
[results and remaining scope](P4-V2-TUI-RESULT.md). Opposite-client switching
and the broader provider/bridge cases remain open.


The user requested the next slices after the authenticated V2 bootstrap passed.
Read [the previous result](P4-V2-IDENTITY-RESULT.md), ROADMAP.md and SESSION.md.
Preserve the activated workspace and all accumulated dirty work. This task adds
isolated development probes, not provider recovery to the application.

## Scope and ownership

- Luna High harness owner: existing `scripts/probe_opencode_v2_identity.py`
  and its test, new `scripts/probe_opencode_v2_tui_identity.py` and its test.
  Reuse the verified bootstrap through an optional internal callback; default
  bootstrap behavior and old version guards must remain intact.
- Luna High plugin owner: only new
  `docs/progress/p4-v2-tui-plugin-fixture/plugin.mjs` and
  `tests/test_opencode_v2_plugin_fixture.py`. Inspect the exact pinned bundle
  read-only to confirm entry, configuration, route and location interfaces.
  Current online docs are supporting context, not runtime proof.
- Parent: review, live runs, integration fixes, roadmap, handoff and evidence.
  Workers do not commit or edit roadmap files. Ownership is exclusive.

## Required experiment

Use the exact 2.0.32 binary/hash, fresh private HOME/XDG/project roots and
fixture-only Basic authentication from the previous result. Enforce loopback
networking through sandbox-exec; validate listener ownership before credentials
and exact server PID/version before mutation. Never connect to the user's
implicit service or send prompts. All process, record and output lifetimes are
bounded; cleanup must retire only fixture-owned groups on every exit path.

First prove a plugin-loaded record and initial selected top-level route. Observe
the actual `context.ui.router.current()` value, not server lifecycle events or
the newest session. Record hashed session/project identity, sequence, monotonic
timestamp and per-client correlation. Poll at 100ms, cap records at2MiB and
1000 entries and lifetime at60s. Stop on malformed/missing identity. Raw IDs,
paths, terminal output, passwords and Authorization values stay out of reports.

Use one bounded diagnostic (at most60s) before repeating any passing gate. Drive
the actual keyboard picker, wait for the exact filtered fixture row before
Enter, and record only controls actually issued. Correlate observations in
order using the corrected old probe helper. Control timestamps must be in the
plugin's clock domain: the most recent preselection plugin timestamp provides
a conservative latency bound, not the exact keystroke time or a crash barrier.

Next run two concurrent private TUI clients in the same project and server,
with distinct initial top-level sessions and independent plugin record files.
Switch the first A-B-A while the second remains on B; verify both clients are
alive and the second continues fresh observations throughout. Then switch the
other client if the first gate passes. A launch `--session` argument, session
creation or a static route API signature alone is not selection proof.

## Claims and checks

Parent escalation after failed loading established the actual V2 contract:
`<OPENCODE_CONFIG_DIR>/cli.json` uses `plugins` containing a **directory** with
an extension-resolvable `tui` entry. The harness copies its original fixture to
`<private root>/identity-plugin/tui.ts` and configures that directory. The CLI
reconciler explicitly skips direct file targets. The singular `plugin` schema
belongs to legacy `tui.json` migration, not the current V2 CLI schema; an earlier
worker audit and parent correction confused those paths. Preserve those failed
attempts in evidence rather than presenting either as a passing loader proof.

The route accessor returns `{type:"session",sessionID}`; derive project identity
from successfully canonicalized `context.location.directory`, not the launch
argument alone. Confirm top-level status from the selected cached session row.
The pinned default `leader` is Ctrl-X and `session_list` is `<leader>l`;
Ctrl-O opens a different recent-session/project menu. The picker navigates
directly rather than sending an authoritative selection event.

Each JSONL row has schema1, kind, random per-plugin `tui_instance`, expected
per-client `client_nonce`, sequence, plugin-clock timestamp, route name,
selected-session hash, top-level status and project hash. Require finite,
ordered observations and correct nonce/project; unknown top-level status cannot
pass. A passive second client must keep recording its own unchanged selected
identity throughout the first client's switches.

This gate does not provide ordered durable binding acknowledgments, verified
new/forked session behavior, bridge expiry, recovery UI or automatic resume.
Polling cannot guarantee current-at-crash identity. Keep the last-confirmed/
manual fallback contract and full Phase 4 unchecked.

Run focused behavioral tests, parent independent review, then one frozen pinned
live diagnostic. Record failed stages honestly and change the identified cause
before retrying. After frozen implementation run all Python tests, compile
changed Python, docs check and whitespace check. Rust suites are unnecessary
unless Rust/build inputs change. Record exact counts and receipt locations in
the result; do not transfer automated work to the user.
