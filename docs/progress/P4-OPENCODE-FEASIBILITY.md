# P4 OpenCode identity feasibility — 2026-10-18

Status: the installed CLI preflight is blocked because `opencode` is not on the
active PATH. Official docs and a pinned release source review found a plausible
local TUI-plugin observation point; no live identity or switch test ran.

## Findings

The spec requires the exact selected top-level session ID, project identity,
ordered binding revisions, and durable acknowledgment before claiming a
verified resume target. Session-created/status/idle events alone do not meet
that contract. The current [server docs](https://docs.opencode.ai/docs/server/)
describe `/session` list/get/create, `/event` SSE, and TUI control routes, but
do not expose a selected-session read endpoint. The current [SDK docs](https://docs.opencode.ai/docs/sdk/)
list session operations, event subscription, and TUI controls; they do not
document a selected-session getter or selection-change event. The current
[plugin event list](https://docs.opencode.ai/docs/plugins/) includes session
lifecycle/status events and prompt/command/toast TUI events, but no focused
session event. These docs showed a last-updated date of 2026-10-17 when checked.

Pinned source review: official release [v1.18.32](https://github.com/anomalyco/opencode/releases/tag/v1.18.32)
identifies source commit `545f51d26cc39a907d2867492d498d9607ea5fa4`
(released 2026-09-21). In that revision,
[`packages/tui/src/context/route.tsx`](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/tui/src/context/route.tsx)
stores the current TUI route locally, and `navigate()` only updates that store.
[`packages/tui/src/component/dialog-session-list.tsx`](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/tui/src/component/dialog-session-list.tsx)
and [`packages/tui/src/context/local.tsx`](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/tui/src/context/local.tsx)
route session-picker and quick-switch choices through that local method.
[`packages/tui/src/app.tsx`](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/tui/src/app.tsx)
consumes incoming `tui.session.select` events, but local selection does not
publish that event. The revision's
[`TuiPluginApi`](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/plugin/src/tui.ts)
exposes `route.current`; the
[source adapter](https://github.com/anomalyco/opencode/blob/545f51d26cc39a907d2867492d498d9607ea5fa4/packages/tui/src/plugin/adapters.tsx)
maps the current route to `{ name: "session", params: { sessionID } }`. This
is a source-backed snapshot
for an in-process TUI plugin, not a documented server-plugin event. No route
change callback or persistence barrier is exposed there. Polling that getter is
a candidate for a bounded bridge experiment, but it cannot yet certify every
switch or current-at-crash identity.

Therefore the server-plugin/SDK event path is insufficient for selected-session
identity. The pinned local TUI plugin snapshot is promising enough to test next;
do not call the identity gate passed until a supported installed version
demonstrates initial route capture and idle switches (session list, quick
switch, new/fork) and Panelink durably acknowledges each observed binding.
Until then, use last-confirmed/manual recovery and never infer from recency.

## Version and acquisition scope

The local check found no `opencode` on the active PATH. The one preflight
attempt, run before bounded-runner hardening, returned
`blocked/missing_executable`; that pre-launch branch is unchanged and covered
by unit tests. The parent made one attempt to fetch the exact asset into a
private temporary directory; `curl` exited 6 (`Could not resolve host:
github.com`). No asset was downloaded or installed. No account, provider auth,
model request, or user config was used. The version reviewed above is a pinned
source sample, not a claim about the user's installed version or the newest
release.

The official v1.18.32 release lists
[`opencode-darwin-arm64.zip`](https://github.com/anomalyco/opencode/releases/download/v1.18.32/opencode-darwin-arm64.zip)
with SHA-256
`fa643f93401c13508d8d513780e54ce9cc01203d501114be9b88d62408b8101f` (listed
in GitHub's [expanded v1.18.32 release assets](https://github.com/anomalyco/opencode/releases/expanded_assets/v1.18.32)).
The tag's [LICENSE](https://github.com/anomalyco/opencode/blob/v1.18.32/LICENSE)
is MIT. A bounded acquisition option for parent review is to fetch only that
tagged asset into a private temporary/cache directory, verify the pinned hash
before extraction, and run it only from that isolated location; do not change
PATH, Homebrew state, or OpenCode config. No asset was downloaded here.

## Automated preflight

`scripts/probe_opencode.py` performs only `opencode --version` (or an explicit
`--executable` path), with a five-second deadline, no shell, no stdin, and a
temporary child `HOME`, XDG config/cache/data, and working directory. It starts
an isolated process group, reads stdout/stderr nonblocking with a shared
4,096-byte cap, and kills the group on timeout or cap overflow. It does not
inherit user credentials/config variables (child `PATH` is system-default),
expose command output or executable paths, install software, or contact
providers. JSON exposes only a parsed version or a fixed diagnostic label;
failures exit 2. An `available` result
proves only that a version string was returned; it always says
`live_identity_tested: false`.

Verification:

| Command | Result |
| --- | --- |
| `python3 -m unittest discover -s tests -p 'test_opencode_probe.py'` | Exit 0; 9 tests passed, including real noisy-output cap and descendant-held-pipe timeout cleanup. |
| `python3 scripts/probe_opencode.py` | One attempt exited 2 with `blocked/missing_executable`, `live_identity_tested: false`; see bounded-runner hardening note above. |
| `python3 scripts/check_docs.py` | To be run before handoff; checks repository Markdown. |
| `PYTHONPYCACHEPREFIX=/private/tmp/p4-opencode-pycache python3 -m py_compile scripts/probe_opencode.py tests/test_opencode_probe.py` | Exit 0. |
| `git diff --check` | Exit 0. |

This note does not enable an OpenCode adapter or close P4. The next useful
bounded task is a live, isolated TUI-plugin bridge check against an exact
approved executable, with no model request: verify `route.current` during
initial session selection and idle session switches, then test ordered durable
binding acknowledgment and stale/missing fallback before adopting an adapter.
