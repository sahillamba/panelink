# Connect from a sandboxed agent

Panelink's control CLI uses a local Unix socket and durable caller contexts.
A skill describes how to use them; the agent host still controls access.
Being inside a Panelink pane does not remove Codex's command sandbox.

## Current local Codex setup — 2026-10-27

Codex 0.164.0 was configured with a named `panelink` permission profile extending
`:workspace`, write access to Panelink's existing `agent-contexts` directory and
an allow entry for the exact legacy `server/control.sock`. The profile enables
network handling with `features.network_proxy = true`; no external domains were
allowed. A sandbox-helper probe confirmed pane discovery and a bounded read,
while an unrelated external connection remained denied. The original config is
backed up under `~/.codex/config.toml.before-panelink-20261027-142414`.

This changes defaults for sessions that load the updated configuration. It does
not rewrite an already running Codex session's permission policy. Reload/select
the named profile through the host's supported permission UI, or resume with a
new Codex process that loads it. Keep the same caller/context for outstanding
operations. The allow entry covers the diagnosed legacy endpoint only; another
workspace or PANELINK_HOME requires its own explicitly scoped endpoint entry.

Read access is verified for the recorded setup. Current source implements
shared short per-batch ownership for UI input and `pane submit`; this does not
protect unfinished human drafts. Legacy retained claims still block other
writers. Capability 128 / schema 14 is required for recognized-agent commands,
and runtime acceptance is pending. The socket/context permissions described
here remain prerequisites and are not granted by the ownership change.

## Distinguish storage from socket access

| Observation | What it establishes | Next step |
| --- | --- | --- |
| `context_directory_unavailable` | The CLI could not create its private caller directory, before querying the server | Check filesystem access and the directory's ownership/permissions |
| `context_directory_unsafe` or `context_unavailable` | Context path validation/open failed | Preserve the existing context; inspect its path and permissions |
| `control_permission_denied` | The OS denied connection or private endpoint-directory access | Allow the exact endpoint through the host permission profile; verify filesystem permissions |
| `control_unavailable` | Connection or a control query failed | Inspect fixed `cause`, `stage` and `transport_stage` fields, then check the selected endpoint and host permission |
| Socket exists | An endpoint pathname exists | This alone proves neither a live server nor permission to connect |
| Normal terminal succeeds, sandbox fails | The workspace is reachable from one execution environment | Compare the two environments' paths and permissions before changing the server |
| Reads work, claim returns `input_failed` or `input_not_authorized`, and another client owns the pane | An ownership conflict is a possible cause; the generic error also covers other failures | Inspect the returned owner and matching server/client behavior; do not infer a sandbox failure |

For the default legacy workspace, the endpoint is
`$PANELINK_HOME/server/control.sock`, or
`~/Library/Application Support/Panelink/server/control.sock` when unset.
For explicitly routed workspaces, use their actual endpoint rather than assuming
the legacy route follows the visible tab. Keep the original `PANELINK_HOME`.

This baseline supports `PANELINK_AGENT_CONTEXT_DIR` as an absolute private
context root. Verify support in the chosen executable's matching source/help
when using another build. The directory and
caller subdirectory must remain owned by the current user and private (0700).
This override changes caller storage only; it does not move the server, grant
socket access, or select a different workspace.

Use one stable context root/name for the automation client. For an initial
connection before any mutation, an explicit writable private location can solve
the context error. Do not generate a new temporary context on every command or
switch roots to get past an uncertain mutation; that abandons reconciliation
state. The 7060 build accepts the explicit root, but other builds may ignore an
unknown environment variable.

## Shared per-batch UI ownership

Current source no longer claims panes while the UI is idle. Each admitted UI
input batch claims and releases ownership around that batch. The additive
`pane submit N TEXT [--json] [--as NAME]` command likewise performs a separately
keyed claim, text-plus-Enter submission and release. It releases only after a
known `Applied` write. Partial, failed and uncertain writes are never replayed
or released; if release fails after a confirmed write, report delivery as
confirmed and release as unconfirmed.

This short lock coordinates an admitted batch only. It does not reserve the
pane while a person edits a draft between batches. Legacy `pane claim` is still
retained ownership and blocks every other writer until known release; neither
the UI nor scoped submit steals it. Do not infer support from an older binary or
server: the new behavior has no runtime acceptance claim here.

Recognized-agent requests additionally require matching client/server support
for capability 128 and schema 14. Old-server attachment may require restarting
the server and detaching/reattaching the UI. This document records no live
restart, reattach, claim or pane-input action. The private caller-context
directory and permission to connect to the selected socket remain separate
prerequisites.

## Codex permission handling

Inspect the current session's execution permissions. Workspace-write alone can
block both writes under Application Support and local socket connections.
`approval_policy = "never"` prevents requests to go beyond that boundary; it
does not grant additional access. If the host permits per-command approval,
request it for the exact Panelink invocation and explain the required context
storage and socket connection. Follow the host's approval result and preserve
the executable, state root, caller and any existing context uncertainty.

For a local Codex CLI session whose current configuration prevents such requests,
the user can start a session with interactive approvals:

```sh
codex --sandbox workspace-write --ask-for-approval on-request
```

This enables requests; it does not automatically grant Panelink access. Have
Codex retry `pane list --json` using its supported approval mechanism, then read
the intended pane before any authorized input. For managed or embedded Codex,
the host/admin policy may control whether approval is available. Report the
specific blocked capability if it cannot be granted. Do not keep retrying under
unchanged permissions, silently edit global Codex settings, or route through
another host to evade the session's restrictions.

See the official [Codex approval guidance](https://learn.chatgpt.com/docs/agent-approvals-security)
and [permission profiles](https://learn.chatgpt.com/docs/permissions). If configuring
an integration under an explicitly authorized setup task, scope write access to
the chosen context location and socket access to the actual endpoint. New Codex
permission profiles and legacy sandbox settings do not compose; verify the
installed version before proposing configuration.

## Verified diagnosis, 2026-10-27

The running 7060 executable was reachable from an unrestricted local process:
`pane list` returned three panes and a bounded read of the right pane succeeded.
Using the installed Codex sandbox helper with the `:workspace` permission profile:

1. Default context storage returned `context_directory_unavailable`.
2. An explicit private context under `/private/tmp` reached `control_unavailable`.
3. Keeping that context and allowing the exact `control.sock` path returned
   `ok:true` and the same three panes.

The supported local diagnostic helper syntax was
`codex sandbox --permission-profile :workspace --allow-unix-socket SOCKET -- COMMAND...`.
Check `codex sandbox --help`: syntax differs across versions. These were read-only
discovery probes against the live workspace, with context metadata writes; no
claim, input, launch or close was sent. They reproduce the reported failure
sequence, rather than establishing the exact configuration of another session.
Running a nested sandbox helper inside an already restricted agent does not
grant the outer agent new permissions. Do not present that helper as a bypass.

## Wide-screen connection loss in older builds

The PR 3 repair addresses repeated connection loss while fetching large pane
screens when macOS coalesces background socket wait timers. It waits for socket
readiness within the existing deadlines. See the repository's
[CI reproduction and evidence](../../../../docs/progress/PR3-CI-SOCKET-READINESS-2026-10-27.md)
for build availability and verification. Check the executable's build; updating
source alone does not replace an existing binary. A generic connection error
still requires diagnosis, and uncertain input must never be replayed.

## Failure receipts

Use the fixed `cause`, logical `stage`, and `transport_stage` fields to separate
connect, handshake, query, request write, reply read and reconciliation failures.
A pending operation key must be reconciled in its original context; do not
change callers or repeat input. Scoped submit receipts preserve delivery even
when release fails. `application_submission:unverified` is expected until a
separate application response is observed. Owner metadata attached to a refusal
is labeled as a before-operation observation, not current lease authority.

Use `build-info --json` and `server-info --json` to distinguish the executable
from the running server. The latter requires negotiated capability 256; an old
server remains explicitly unknown. Restarting a server ends its pane processes.
