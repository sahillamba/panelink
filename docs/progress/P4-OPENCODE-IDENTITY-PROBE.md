# P4 installed OpenCode identity probe — 2026-10-21

Status: live, offline, isolated server/session feasibility check completed;
selected TUI identity and idle switch remain **not proven**. This supersedes
the missing-executable preflight in [P4-OPENCODE-FEASIBILITY.md](P4-OPENCODE-FEASIBILITY.md)
only for the installation observed here. No Panelink adapter or resume claim.

## Boundaries and observations

`scripts/probe_opencode_identity.py` resolved and executed the installed
OpenCode CLI (reported as `launcher=opencode`) with a fresh mode-0700 temporary HOME,
private XDG config/data/state/cache directories, a fresh single project directory,
minimal environment (including models-fetch/update disabling), no inherited
credentials and no real user config. CLI capture follows the preflight's bounded
capture pattern with unconditional process-group termination, a 4,096-byte
combined cap and 8-second command deadline; server startup has
an 8-second readiness deadline, loopback requests have at most 2-second deadlines,
response bodies are capped at 16 KiB and never emitted. The server binds only
127.0.0.1 on a random port. No model request, provider account or network-dependent
step was performed; the loopback HTTP requests are local. CLI/API bodies and
session identifiers are not printed or saved. An SSE connection is closed after
checking headers; no event body is saved. The server process group is killed in
`finally` before the temporary tree is removed.

Launcher resolution is: explicit `--executable` (invalid explicit paths fail
closed), `opencode` on PATH, the executable at `~/.opencode/bin/opencode`, then
`tescode` on PATH. The earlier 2026-10-18 preflight recorded `opencode` absent
from its active PATH and `blocked/missing_executable`; **this live 2026-10-21
probe did not hit missing_executable**. The first version of this script used
the known installation path directly; this follow-up removed that hard-coded
path. The command below resolved the native launcher and ran native OpenCode
commands. For a wrapper-only installation, the probe runs only isolated
`tescode version` and `tescode sessions --help` with bounded non-interactive
capture. Successful wrapper version output is classified
`wrapper_version_command_passed` (not a pinned native OpenCode version); native
serve, session list, export, SSE and selection steps are
`blocked_wrapper_only`. The wrapper branch is tested with mocks, **not** a live
wrapper-only run. No auth, SSO, update, reset or uninstall commands are used.

Actual command and output (rerun with resolver after the follow-up):

```text
$ python3 scripts/probe_opencode_identity.py
{"cli_export": "exit_0", "cli_session_list": "exit_0", "event_sse": "http_200_text_event_stream", "launcher": "opencode", "selected_after_creation": "unvalidated_http_200", "selected_route": {"/session/current": "http_500", "/tui/session": "unvalidated_http_200", "/tui/session/current": "unvalidated_http_200"}, "serve_help": "available", "server": "responded", "session_list": "http_200", "sessions_listed": true, "switch": "not_proven", "two_sessions": "two_sessions_created_selection_not_proven", "version": "1.19.0", "version_status": "proven"}
```

Focused verification after the launcher update:

```text
$ python3 -m unittest tests.test_opencode_identity_probe -v
Ran 9 tests in 0.257s
OK
$ python3 scripts/check_docs.py
Checked 189 Markdown file(s); 0 finding(s).
```

**Proven in this isolated run:** `--version` returned `1.19.0`; `serve --help`
exited zero; a loopback server responded to `GET /session` with HTTP 200; `POST
/session` twice returned two distinct valid top-level IDs, both were found in a
subsequent `GET /session` list. `opencode session list` and `opencode export` of
one newly created session each exited zero, without model calls. `GET /event`
returned HTTP 200 and `text/event-stream` headers. This proves stream presence,
not which events appear or their ordering. Both queried `/tui` GET paths returned
HTTP 200 but did not return a validated top-level session object; `/session/current`
returned HTTP 500. These are probe observations, not documented selected-session
routes. Responses were not interpreted as a selected ID.

**Installed-package surface (read-only inspection, distinct from CLI proof):**
local `@opencode-ai/plugin` package metadata reports `1.18.32`, older than CLI
`1.19.0`. Its `dist/tui.d.ts` lines 13–24 define a route with
`params.sessionID`; lines 459–476 expose `route.current` and `route.navigate` in
`TuiPluginApi`; lines 505–510 define an in-process TUI plugin entry. The same
file's event bus (407–411) has no explicit selected-session callback. Its
`dist/index.d.ts` lines 36–56 show a separate server plugin and general event
hook. The local SDK `dist/v2/gen/sdk.gen.js` lines 3019–3043 exposes a POST
`/tui/select-session` control operation; this is an action, not a selected-state
getter or evidence that local interactive TUI switches publish it. The local
package is **not** the installed executable's verified 1.19.0 API revision.

**Not proven / blocked:** A headless `serve` instance has no interactive TUI
selection. Neither the session list, lifecycle SSE availability, CLI list/export
nor successful creation attributes a selected session to an actual pane. An
in-TUI idle selection/switch, plugin loading on CLI 1.19.0, ordered switch
notifications, durable acknowledgment, crash boundary and automatic recovery
were not exercised. No real TTY was available in this bounded headless test;
making a model call to force selection would violate the no-provider constraint.
The next automated gate is a **private PTY**, isolated config with a local
1.19.0-compatible TUI plugin fixture, two precreated same-project session IDs,
scripted TUI navigation between those IDs without prompt submission, and a
bounded local event receiver that correlates plugin `route.current` snapshots to
each controlled transition. If idle navigation cannot be automated, record that
gate blocked and continue last-confirmed/manual recovery. Polling alone cannot
establish the ordering barrier specified in [SPEC.md](../SPEC.md); a separate
durable acknowledgment/crash-order test is required before any current-at-crash
claim. This run therefore leaves P4 identity acceptance open.
