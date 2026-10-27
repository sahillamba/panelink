# Herdr input-control study — 2026-10-27

User requested understanding Herdr before selecting Panelink input-control policy.
No policy selection is inferred. The per-pane/global handoff implementation task
was cancelled before edits; scrolling work continues independently.

## Evidence

Current official agent automation, socket API, integrations and skill guides were
read. Source inspected at Herdr v0.9.5, commit
`2ecf19a92fc3a7fff75b6ce3655ef04f4625ec01`, in a separate read-only study checkout
`/private/tmp/herdr-control-study-20261027`. No Herdr binary installed or run.
No Herdr code copied into Panelink.

- [Socket API](https://herdr.dev/docs/socket-api/): local Unix socket on macOS,
  injected socket/workspace/tab/pane context; CLI targets the current server.
- [Pane input handler](https://github.com/herdrdev/herdr/blob/v0.9.5/src/app/api/panes.rs#L1930):
  resolves pane, encodes input, validates submission and enqueues bytes, without
  a Panelink-style current-controller lease check in this path.
- [PTY actor](https://github.com/herdrdev/herdr/blob/v0.9.5/src/pty/actor/unix.rs#L110):
  human and API input use the pane runtime/actor queue. The write gate is for
  accepting/shutdown/handoff state; it is not an editing-turn owner identifier.
  Combined submissions can order text and Enter; this does not protect text
  already typed by a human at the application prompt.
- [Agent validation](https://github.com/herdrdev/herdr/blob/v0.9.5/src/app/api/agents.rs#L140):
  checks resolved recognized foreground agent, pending startup and blocked state.
- [Automation guide](https://herdr.dev/docs/agent-automation/): raw pane input
  addresses the terminal; agent input checks the recognized occupant. Prompt
  waits observe lifecycle transitions, not guaranteed per-prompt completion.
- [Codex integration](https://herdr.dev/docs/integrations/#codex): installs session
  identity hook/config entries; screen manifests provide lifecycle state. Its
  documented installation is not a grant to bypass Codex's tool sandbox.
- [Socket restriction](https://github.com/herdrdev/herdr/blob/v0.9.5/src/api/server.rs#L29):
  local socket uses owner-only mode 0600. HERDR_ENV is an instructional guard,
  not a secure per-agent authentication credential.

Interpretation: the inspected raw-input path is a shared local control surface,
not the same human/agent ownership model proposed for Panelink. Multiple writers
can enqueue input to the terminal; queue ordering alone does not protect an
unfinished human draft. Herdr's server-upgrade handoff is a different concept
from transferring a human editing turn to an automation client.

## Current Panelink diagnosis (separate from Herdr)

Existing live workspace remains reachable; all three panes have attached-client
input owners. In Codex 0.164.0, :workspace with a stable writable temp context
reproduces control_unavailable; allowing the exact control.sock permits discovery.
The named panelink permission profile, extending :workspace, additionally grants
only agent-contexts writes and the current legacy endpoint. network_proxy is
explicitly enabled; an unrelated direct external connection was denied in the
verified helper probe. Default-profile discovery and a bounded read succeed.
User config backup: ~/.codex/config.toml.before-panelink-20261027-142414.
Existing Codex session policy is not retroactively changed by editing that file.
Input ownership remains a separate unresolved product choice; no user-pane input
was sent, no server restarted, no per-pane yield implementation enabled.

Official permission reference:
https://learn.chatgpt.com/docs/permissions
