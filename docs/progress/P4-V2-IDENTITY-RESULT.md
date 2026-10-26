# Phase 4 V2 identity preflight — 2026-10-24

Historical bootstrap result. The subsequent [selected-route/two-client gate](P4-V2-TUI-RESULT.md)
now passes3/3; remaining integration/durability gates are recorded there.


Status: **Phase 4 remains open.** The installed native OpenCode 2.0.32 pin is
verified. The authenticated private-server bootstrap passes: exact owned PID
and version, two empty top-level sessions created/listed, and successful cleanup.
Earlier anonymous HTTP401 was diagnosed and resolved with a fresh fixture-only
password. TUI identity/switches, provider requests, saved bindings and automatic
resume remain untested; zero prompts were submitted.

## Delivered scope

Two bounded GPT-6 Luna High workers owned separate files; the parent reviewed
and independently tested their results. The old 1.19.0 TUI probe now rejects
unissued, malformed, extra or regressing control markers as switch proof. Each
matched observation must follow the previous match. Actual attempts, unissued
controls and missed attempts are reported separately. An empty attempt cannot
pass latency or switch checks. Its exact 1.19.0 version guard remains intact;
the current binary correctly stops at that guard before starting a repetition.

The separate `scripts/probe_opencode_v2_identity.py` is a bootstrap preflight,
not a V2 TUI adapter. It pins the executable hash and version, uses fresh
private HOME/XDG/project roots, disables inherited proxy configuration, enforces
loopback-only networking through macOS sandbox-exec, and requires the listener
to belong to its exact server child before session creation. Ownership is
rechecked before each POST. A fresh random password is supplied only to the
private child; Basic authentication is sent only after listener ownership is
verified. HTTP redirects are refused, proxies disabled, and the authenticated
info response must match the exact child PID/version. Its output is bounded and
allowlisted; process groups are retired during cleanup. Authentication rejection
stops promptly.
Even a successful mocked two-session bootstrap retains unproven identity and
CLI exit2. No polling interval or current-at-crash guarantee is introduced.

## Pin and live observations

- Launcher: native `opencode`, resolved on PATH at
  `/Users/sahillamba/.opencode/bin/opencode`.
- Isolated version: `opencode v2.0.32`.
- SHA-256: `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
- Initial worker attempt: version/hash verified, API not ready after
  12,284.694ms; no sessions created. This report was tool output only.
- Parent reviewed preflight: exit2 after 12,544.494ms, server still running,
  exact child owned the listener, cleanup completed, zero sessions/prompts.
  Receipt: `/private/tmp/panelink-p4-20261024/v2-preflight-final.json`.
- Parent added numeric HTTP-status diagnostics and ran one diagnostic attempt:
  exit2 after 12,286.706ms, **HTTP401**, server still running, owned listener,
  cleanup completed, zero sessions/prompts. Receipt:
  `/private/tmp/panelink-p4-20261024/v2-preflight-status.json`.

- Final authenticated parent run: **bootstrap passed** in 587.354ms. HTTP200
  for info, both creates and listing; exact child PID/version verified, two
  empty top-level sessions, zero prompts, cleanup completed and loopback-only
  sandbox enforced. Receipt:
  `/private/tmp/panelink-p4-20261024/v2-preflight-authenticated.json`.
  Classification is `preflight_passed_identity_not_proven`; exit2 is intentional
  because no selected TUI route or switch is proven. A bootstrap pass cannot
  satisfy Phase 4 selected-session acceptance.

The initial default-sandbox attempts could not apply sandbox-exec; permitted
executions above did enforce it. Those failures are not host socket evidence.
Likewise, the historical V2 socket-denied observation in the subagent study
does not describe this current permitted run: an owned listener now exists.
No user service, live Panelink pane, provider account or installed CLI settings
were changed. Default-home version checks encountered log-path EPERM and are
not the accepted version evidence.

Current [V2 API documentation](https://dev.opencode.ai/v2/docs/api) documents
`/api/info` and its possible unauthorized response; the live observation above
establishes the actual installed binary's response. Current documentation and
static bundle observations do not establish runtime selected-session identity.

The follow-up read-only audit found why anonymous access fails in this exact
pinned bundle. Its server middleware uses Basic username `opencode`, with a
password supplied through `OPENCODE_PASSWORD` (fallback
`OPENCODE_SERVER_PASSWORD`) or generated at startup. The bundled standalone
client supplies a fresh password to its own child. It uses `serve --stdio
--port 0`, reads a JSON URL line, holds stdin open and authenticates separately;
no password is printed in that readiness record. Relevant embedded chunks:
`chunk-3wwjbdge.js` around byte 113837007 (middleware),
`chunk-y1fqny1h.js` around 113533744 (environment),
`chunk-2388pc86.js` around 113550520 (startup), and
`chunk-k789gcvb.js` around 113588362 (standalone client). These are static
observations, not successful live authentication evidence by themselves.

## Verification and next gate

- Final Python discovery: **95 tests passed** in 6.827s (including 13 old-probe
  and 13 new-preflight tests). Covers incomplete/invalid control correlation,
  ownership failure with no request, authentication rejection with no POST,
  foreign API PID/ownership loss with no POST, Basic header/redirect refusal,
  bounded output collection and successful bootstrap remaining unproven.
- Changed Python modules compile; `git diff --check` passes and the docs
  checker reports 283 Markdown files/0 findings. The in-memory code graph was
  refreshed without writing a graph artifact.
- No Rust/build inputs changed in this Phase 4 slice. The separate activated
  Phase 3 baseline remains 1111 passed/0 failed/15 ignored, not a new V2
  acceptance result. Its human four-pane host gate stays open.

Next: add and prove the V2 selected-route adapter and ordered keyboard A-B-A
observation using the now-verified fixture-local transport. The adapter must
verify the exact project and displayed top-level identity, then demonstrate
two concurrent agents in the same project. Single-server session creation is
only bootstrap evidence. Do not retry
unchanged readiness loops or count session creation as selected-session proof.
Existing pure binding state/codec components remain separate from a live bridge
and durable store. Binding acknowledgments, two simultaneous agents, new/forked
sessions, bridge expiry and pre/post-ack crash tests remain required. There is
no provider recovery or current-at-crash claim. No commit or push.
