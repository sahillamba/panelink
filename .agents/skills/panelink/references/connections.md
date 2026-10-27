# Connect from a sandboxed agent

Panelink's control CLI uses a local Unix socket and durable caller contexts.
A skill describes how to use them; the agent host still controls access.
Being inside a Panelink pane does not remove Codex's command sandbox.

## Distinguish storage from socket access

| Observation | What it establishes | Next step |
| --- | --- | --- |
| `context_directory_unavailable` | The CLI could not create its private caller directory, before querying the server | Check filesystem access and the directory's ownership/permissions |
| `context_directory_unsafe` or `context_unavailable` | Context path validation/open failed | Preserve the existing context; inspect its path and permissions |
| `control_unavailable` | Connection or a control query failed; the error groups several causes | Check the selected workspace, server endpoint, and host permission to connect |
| Socket exists | An endpoint pathname exists | This alone proves neither a live server nor permission to connect |
| Normal terminal succeeds, sandbox fails | The workspace is reachable from one execution environment | Compare the two environments' paths and permissions before changing the server |
| Reads work, claim returns `input_failed`, and another client owns the pane | An ownership conflict is a possible cause; the generic error also covers other failures | Inspect the returned owner and matching server/client behavior; do not infer a sandbox failure |

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

## Attached UI ownership limitation

Confirmed against the running 7060 build on 2026-10-27: the UI's `Net::poll`
claims input on every pane, including unfocused and idle panes, then renews
those 30-second owner claims every 10 seconds. The server's `InputOwner::claim`
rejects a different holder; it does not transfer ownership. The rejection is
`Unauthorized`, mapped to `unauthorized` by the control client and then hidden
behind the agent CLI's generic `input_failed` error.

Live discovery showed client owners on all three panes. One claim against the
right-hand pane (index 3 in that layout) reproduced `input_failed`; discovery
afterward showed the same holder. No text was sent. Read access works while
input remains blocked by these leases. This is independent of the sandbox
connection failure described below. The generic error alone is insufficient
to diagnose another installation.

Report the holder and the ownership limitation. Do not repeatedly claim, change
caller identity, delete contexts or infer that Read permission grants Input.
Do not detach the user's UI, terminate it, or weaken single-writer guards as a
workaround. Cooperative UI/automation handoff and a specific ownership error
are application work still needed for controlling visible panes reliably.

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
