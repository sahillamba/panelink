# Herdr and Panelink: public command/API comparison

Reviewed: 2026-10-27. This is a capability inventory, not a claim of release parity.

## Scope and sources

Herdr: its current published [CLI reference](https://herdr.dev/docs/cli-reference/) and
[socket API](https://herdr.dev/docs/socket-api/), plus the linked agent, plugin, integration,
remote, session and skill guides. No Herdr binary was installed or exercised.

Panelink: the local 0.1.0 executable selected by `~/.local/bin/panelink`, which
resolves to `/Users/sahillamba/.codex/worktrees/7060/panelink/bin/panelink`.
That checkout is dirty on `codex/p5-acceptance-continuation` at `1060668`.
It differs from this older main checkout. Parent checked help, relevant source,
and a bounded read-only Luna inventory; no application edits, build, tests,
input or launch were performed for this comparison.

Inventory unit: a documented callable command path or named public method.
Argument/flag combinations, shell completion targets and integration provider
names are not separate commands. The table includes 106 CLI paths
(including the `completions` alias), 86 named socket methods,
5 terminal-stream input commands and 8 cross-cutting
selectors/guide flags. This is coverage of the reviewed public documentation,
not a schema extraction from an installed Herdr binary or a percentage-complete score.
Herdr's `api schema --json` is the way to confirm a specific installed binary.

Machine-readable per-entry mapping: [comparison data](HERDR-API-COMPARISON-2026-10-27.json).

## Main findings

- Panelink has terminal discovery/reads, finite waits/events, direct process launch,
  saved history/recovery, and typed ownership/deduplication mechanisms.
- Herdr exposes a broader recognized-agent interface, reusable layout operations,
  worktree orchestration, integrations, plugins and remote-machine control.
- Similar names can conceal different operations. Herdr `pane run` submits text
  to an existing terminal; Panelink `pane run` creates a fresh managed process in
  a new split. Panelink `pane send --enter` / low-level `submit` is the nearer
  terminal-submission counterpart, subject to ownership.
- In Panelink, swap/divider/zoom have implemented typed protocol and UI paths;
  their absence from the high-level pane CLI is an exposure gap, not missing geometry.
- Panelink's live attached UI claims and renews every pane's input. Other agents'
  claims currently fail with generic `input_failed`. Listing a send command is
  not proof that the user's visible-pane control workflow works.
- Panelink high-level legacy pane commands still route to Default/Main. Merely
  selecting a project/tab does not retarget them. Herdr exposes explicit named
  session/machine selectors and workspace-aware commands; this is an ergonomics gap.

## Status meanings

- Comparable: a similar user operation is exposed; parameters, protocols and acceptance differ.
- Partial: a narrower mechanism exists, without full behavior parity.
- UI / wire only: a UI/typed internal capability exists without the matching ergonomic agent CLI.
- Control gap: a command exists but UI-owned leases block the reproduced attached-pane workflow.
- Different semantics: a similarly named/nearby facility performs a different operation.
- No equivalent exposed: no matching public surface found in the inspected Panelink dispatch.

The status is a mapping judgement, not test evidence. JSON output does not make
Panelink's binary socket transport a JSON API. Its typed local IPC has no
corresponding published Herdr-style schema-print command or generic JSON method
endpoint. Neither this comparison nor the reviewed docs establish that a
Herdr request ID is equivalent to Panelink's operation-key deduplication contract.

## Complete documented surface mapping

Names below are functional interface identifiers. Options and response details
remain in the linked official references. Each row has a direct source link.

### Launch and discovery

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr`](https://herdr.dev/docs/cli-reference/) | Comparable | `panelink` — Open/start or reattach to the legacy workspace. |
| CLI: [`herdr status`](https://herdr.dev/docs/cli-reference/) | Partial | `doctor; pane list; control describe` — Diagnostic/status pieces, without Herdr's matching status/ping command surface. |
| CLI: [`herdr status server`](https://herdr.dev/docs/cli-reference/) | Partial | `doctor; pane list; control describe` — Diagnostic/status pieces, without Herdr's matching status/ping command surface. |
| CLI: [`herdr status client`](https://herdr.dev/docs/cli-reference/) | Partial | `doctor; pane list; control describe` — Diagnostic/status pieces, without Herdr's matching status/ping command surface. |
| CLI: [`herdr api schema`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr api snapshot`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list; control layout/panes/describe/reconcile` — Separate observations, no unified all-session bootstrap response. |
| CLI: [`herdr completion`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr completions`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr update`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr channel show`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr channel set`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Saved machines

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr machine list`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine add`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine rename`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine disable`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine enable`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine remove`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine status`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr machine reconnect`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Server

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr server`](https://herdr.dev/docs/cli-reference/) | Comparable | `serve` — Explicit foreground server. |
| CLI: [`herdr server stop`](https://herdr.dev/docs/cli-reference/) | Comparable | `stop / workspace stop` — Stops the chosen Panelink workspace server and its children; routing differs. |
| CLI: [`herdr server reload-config`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr server agent-manifests`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr server update-agent-manifests`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr server reload-agent-manifests`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`ping`](https://herdr.dev/docs/socket-api/) | Partial | `doctor; pane list; control describe` — Diagnostic/status pieces, without Herdr's matching status/ping command surface. |
| Socket: [`server.stop`](https://herdr.dev/docs/socket-api/) | Comparable | `stop / workspace stop` — Stops the chosen Panelink workspace server and its children; routing differs. |
| Socket: [`server.reload_config`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`server.agent_manifests`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`server.reload_agent_manifests`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Notifications

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr notification show`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`notification.show`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Named sessions

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr session list`](https://herdr.dev/docs/persistence-remote/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| CLI: [`herdr session attach`](https://herdr.dev/docs/persistence-remote/) | Partial | `workspace select/open; attach` — Explicit project/tab route selection and attach; legacy pane CLI remains on Default/Main. |
| CLI: [`herdr session stop`](https://herdr.dev/docs/persistence-remote/) | Partial | `workspace stop/remove; stop` — Separate stop and stopped-route removal; process, grouping and state semantics differ. |
| CLI: [`herdr session delete`](https://herdr.dev/docs/persistence-remote/) | Partial | `workspace remove` — Removes a stopped route while retaining saved data; not a named-session delete equivalent. |

### Workspaces

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr workspace list`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| CLI: [`herdr workspace create`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace project create; workspace tab create/open` — Different Project → workspace-tab model; record creation and process startup are separate. |
| CLI: [`herdr workspace get`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| CLI: [`herdr workspace focus`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace select/open; attach` — Explicit project/tab route selection and attach; legacy pane CLI remains on Default/Main. |
| CLI: [`herdr workspace rename`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr workspace report-metadata`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr workspace close`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace stop/remove; stop` — Separate stop and stopped-route removal; process, grouping and state semantics differ. |
| Socket: [`workspace.create`](https://herdr.dev/docs/socket-api/) | Partial | `workspace project create; workspace tab create/open` — Different Project → workspace-tab model; record creation and process startup are separate. |
| Socket: [`workspace.list`](https://herdr.dev/docs/socket-api/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| Socket: [`workspace.get`](https://herdr.dev/docs/socket-api/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| Socket: [`workspace.focus`](https://herdr.dev/docs/socket-api/) | Partial | `workspace select/open; attach` — Explicit project/tab route selection and attach; legacy pane CLI remains on Default/Main. |
| Socket: [`workspace.rename`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`workspace.move`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`workspace.move_block`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`workspace.report_metadata`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`workspace.close`](https://herdr.dev/docs/socket-api/) | Partial | `workspace stop/remove; stop` — Separate stop and stopped-route removal; process, grouping and state semantics differ. |

### Git worktrees

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr worktree list`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr worktree create`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr worktree open`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr worktree remove`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`worktree.list`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`worktree.create`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`worktree.open`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`worktree.remove`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Tabs

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr tab list`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| CLI: [`herdr tab create`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace tab create; workspace open` — Creates a registry tab route, then opens its server; no matching one-step root-shell API. |
| CLI: [`herdr tab get`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| CLI: [`herdr tab focus`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace select/open; attach` — Explicit project/tab route selection and attach; legacy pane CLI remains on Default/Main. |
| CLI: [`herdr tab rename`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr tab close`](https://herdr.dev/docs/cli-reference/) | Partial | `workspace stop/remove; stop` — Separate stop and stopped-route removal; process, grouping and state semantics differ. |
| Socket: [`tab.create`](https://herdr.dev/docs/socket-api/) | Partial | `workspace tab create; workspace open` — Creates a registry tab route, then opens its server; no matching one-step root-shell API. |
| Socket: [`tab.list`](https://herdr.dev/docs/socket-api/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| Socket: [`tab.get`](https://herdr.dev/docs/socket-api/) | Partial | `workspace list` — Project/tab registry plus per-workspace state; hierarchy and response shapes differ. |
| Socket: [`tab.focus`](https://herdr.dev/docs/socket-api/) | Partial | `workspace select/open; attach` — Explicit project/tab route selection and attach; legacy pane CLI remains on Default/Main. |
| Socket: [`tab.rename`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`tab.move`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`tab.close`](https://herdr.dev/docs/socket-api/) | Partial | `workspace stop/remove; stop` — Separate stop and stopped-route removal; process, grouping and state semantics differ. |

### Pane discovery

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr pane list`](https://herdr.dev/docs/cli-reference/) | Comparable | `pane list --json; panes` — Spatial entries, exact target refs, owner info and bounded direct-child metadata. |
| CLI: [`herdr pane current`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list; local UI focus` — No explicit caller-pane/current-pane high-level command or Herdr --current routing. |
| CLI: [`herdr pane get`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list; control describe` — No matching one-pane high-level get command. |
| CLI: [`herdr pane layout`](https://herdr.dev/docs/cli-reference/) | Partial | `control layout/panes; schema-13 ViewQuery` — Live layout query exists; layout show reads saved geometry, not this live query. |
| CLI: [`herdr pane process-info`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list metadata` — Direct managed-child cwd/executable; no equivalent full foreground process tree/argv query. |
| CLI: [`herdr pane neighbor`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Layout geometry / local navigation` — Can derive spatial relationships; no matching dedicated public helper. |
| CLI: [`herdr pane edges`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Layout geometry / local navigation` — Can derive spatial relationships; no matching dedicated public helper. |
| Socket: [`pane.list`](https://herdr.dev/docs/socket-api/) | Comparable | `pane list --json; panes` — Spatial entries, exact target refs, owner info and bounded direct-child metadata. |
| Socket: [`pane.current`](https://herdr.dev/docs/socket-api/) | Partial | `pane list; local UI focus` — No explicit caller-pane/current-pane high-level command or Herdr --current routing. |
| Socket: [`pane.get`](https://herdr.dev/docs/socket-api/) | Partial | `pane list; control describe` — No matching one-pane high-level get command. |
| Socket: [`pane.layout`](https://herdr.dev/docs/socket-api/) | Partial | `control layout/panes; schema-13 ViewQuery` — Live layout query exists; layout show reads saved geometry, not this live query. |
| Socket: [`pane.process_info`](https://herdr.dev/docs/socket-api/) | Partial | `pane list metadata` — Direct managed-child cwd/executable; no equivalent full foreground process tree/argv query. |
| Socket: [`pane.neighbor`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Layout geometry / local navigation` — Can derive spatial relationships; no matching dedicated public helper. |
| Socket: [`pane.edges`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Layout geometry / local navigation` — Can derive spatial relationships; no matching dedicated public helper. |

### Pane organization

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr pane focus`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Click / Ctrl-B arrows or Tab` — Focus is client-local; no matching agent CLI focus command. |
| CLI: [`herdr pane resize`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Divider UI; schema-13 DividerSet` — Absolute divider position/effective PTY resize, not matching directional ratio CLI. |
| CLI: [`herdr pane zoom`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Ctrl-B z; schema-13 ZoomSet` — Implemented typed protocol/UI path; no pane zoom CLI command. |
| CLI: [`herdr pane rename`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr pane input`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr pane split`](https://herdr.dev/docs/cli-reference/) | Comparable | `control split; pane run; UI split` — Splits left/right/up/down with at most four panes. Command shapes differ. |
| CLI: [`herdr pane swap`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Ctrl-B m; schema-13 Swap` — Same-workspace swap with pinned targets; no high-level pane swap CLI. |
| CLI: [`herdr pane move`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr pane close`](https://herdr.dev/docs/cli-reference/) | Comparable | `pane close N; close N; control close` — Terminates the selected process/pane. Closing last pane ends that workspace. |
| Socket: [`pane.split`](https://herdr.dev/docs/socket-api/) | Comparable | `control split; pane run; UI split` — Splits left/right/up/down with at most four panes. Command shapes differ. |
| Socket: [`pane.swap`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Ctrl-B m; schema-13 Swap` — Same-workspace swap with pinned targets; no high-level pane swap CLI. |
| Socket: [`pane.move`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.zoom`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Ctrl-B z; schema-13 ZoomSet` — Implemented typed protocol/UI path; no pane zoom CLI command. |
| Socket: [`pane.focus_direction`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Click / Ctrl-B arrows or Tab` — Focus is client-local; no matching agent CLI focus command. |
| Socket: [`pane.resize`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Divider UI; schema-13 DividerSet` — Absolute divider position/effective PTY resize, not matching directional ratio CLI. |
| Socket: [`pane.rename`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.input.set`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.close`](https://herdr.dev/docs/socket-api/) | Comparable | `pane close N; close N; control close` — Terminates the selected process/pane. Closing last pane ends that workspace. |

### Terminal input/output

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr pane read`](https://herdr.dev/docs/cli-reference/) | Partial | `pane read [--scrollback] --lines K --json` — Sanitized screen or retained normal-history rows; no ANSI, unwrapped or detection source parity. |
| CLI: [`herdr pane send-text`](https://herdr.dev/docs/cli-reference/) | Control gap | `pane send N TEXT; control text/paste` — Requires an input lease; current attached UI's permanent claims block agent acquisition. |
| CLI: [`herdr pane send-keys`](https://herdr.dev/docs/cli-reference/) | Control gap | `pane keys N ...; pane interrupt N` — Typed keys/interrupt exist, with a narrower key profile and the same ownership conflict. |
| CLI: [`herdr pane run`](https://herdr.dev/docs/cli-reference/) | Different semantics | `pane send N TEXT --enter; control submit` — Herdr submits into an existing terminal. Panelink pane run creates a new split/process. |
| CLI: [`herdr pane wait-output`](https://herdr.dev/docs/cli-reference/) | Partial | `wait N --text TEXT [--regex] --timeout SECS` — Visible screen and bounded regex subset; no recent/unwrapped matching-source parity. |
| Socket: [`pane.send_text`](https://herdr.dev/docs/socket-api/) | Control gap | `pane send N TEXT; control text/paste` — Requires an input lease; current attached UI's permanent claims block agent acquisition. |
| Socket: [`pane.send_keys`](https://herdr.dev/docs/socket-api/) | Control gap | `pane keys N ...; pane interrupt N` — Typed keys/interrupt exist, with a narrower key profile and the same ownership conflict. |
| Socket: [`pane.send_input`](https://herdr.dev/docs/socket-api/) | Different semantics | `pane send N TEXT --enter; control submit` — Herdr submits into an existing terminal. Panelink pane run creates a new split/process. |
| Socket: [`pane.read`](https://herdr.dev/docs/socket-api/) | Partial | `pane read [--scrollback] --lines K --json` — Sanitized screen or retained normal-history rows; no ANSI, unwrapped or detection source parity. |
| Socket: [`pane.wait_for_output`](https://herdr.dev/docs/socket-api/) | Partial | `wait N --text TEXT [--regex] --timeout SECS` — Visible screen and bounded regex subset; no recent/unwrapped matching-source parity. |

### Agent reports

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr pane report-agent`](https://herdr.dev/docs/integrations/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr pane report-agent-session`](https://herdr.dev/docs/integrations/) | Partial | `provider-binding prepare/resume/show/status` — Explicit pinned-provider identity path, not a general plugin-reported native-session/resume API. |
| CLI: [`herdr pane release-agent`](https://herdr.dev/docs/integrations/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr pane report-metadata`](https://herdr.dev/docs/integrations/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.report_agent`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.report_agent_session`](https://herdr.dev/docs/socket-api/) | Partial | `provider-binding prepare/resume/show/status` — Explicit pinned-provider identity path, not a general plugin-reported native-session/resume API. |
| Socket: [`pane.report_metadata`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.clear_agent_authority`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`pane.release_agent`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Recognized agents

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr agent list`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list; provider-binding show/status` — No generic recognized-agent catalogue or names; pinned-provider data is narrower. |
| CLI: [`herdr agent get`](https://herdr.dev/docs/cli-reference/) | Partial | `pane list; provider-binding show/status` — No generic recognized-agent catalogue or names; pinned-provider data is narrower. |
| CLI: [`herdr agent read`](https://herdr.dev/docs/cli-reference/) | Partial | `pane read` — Generic terminal text without Herdr agent identity/name resolution. |
| CLI: [`herdr agent send-keys`](https://herdr.dev/docs/cli-reference/) | Partial | `pane keys; pane send --enter` — Only generic terminal interaction, with current ownership gap; no readiness/blocked-state prompt contract. |
| CLI: [`herdr agent prompt`](https://herdr.dev/docs/cli-reference/) | Partial | `pane keys; pane send --enter` — Only generic terminal interaction, with current ownership gap; no readiness/blocked-state prompt contract. |
| CLI: [`herdr agent rename`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr agent focus`](https://herdr.dev/docs/cli-reference/) | UI / wire only | `Local pane focus` — No recognized-agent name targeting, attention acknowledgement or generic focus CLI. |
| CLI: [`herdr agent wait`](https://herdr.dev/docs/cli-reference/) | Partial | `wait --text/--quiet/--change/--exit` — Observable terminal/process conditions, not semantic working/blocked/done lifecycle. |
| CLI: [`herdr agent attach`](https://herdr.dev/docs/cli-reference/) | Partial | `attach` — Full-workspace client; no matching recognized-agent direct-terminal attach command. |
| CLI: [`herdr agent start`](https://herdr.dev/docs/cli-reference/) | Different semantics | `pane run -- COMMAND ARGS` — Directly launches a process in a new split. Does not activate/name/recognize an agent in an existing idle shell. |
| CLI: [`herdr agent explain`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`agent.list`](https://herdr.dev/docs/socket-api/) | Partial | `pane list; provider-binding show/status` — No generic recognized-agent catalogue or names; pinned-provider data is narrower. |
| Socket: [`agent.get`](https://herdr.dev/docs/socket-api/) | Partial | `pane list; provider-binding show/status` — No generic recognized-agent catalogue or names; pinned-provider data is narrower. |
| Socket: [`agent.read`](https://herdr.dev/docs/socket-api/) | Partial | `pane read` — Generic terminal text without Herdr agent identity/name resolution. |
| Socket: [`agent.explain`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`agent.send_keys`](https://herdr.dev/docs/socket-api/) | Partial | `pane keys; pane send --enter` — Only generic terminal interaction, with current ownership gap; no readiness/blocked-state prompt contract. |
| Socket: [`agent.prompt`](https://herdr.dev/docs/socket-api/) | Partial | `pane keys; pane send --enter` — Only generic terminal interaction, with current ownership gap; no readiness/blocked-state prompt contract. |
| Socket: [`agent.wait`](https://herdr.dev/docs/socket-api/) | Partial | `wait --text/--quiet/--change/--exit` — Observable terminal/process conditions, not semantic working/blocked/done lifecycle. |
| Socket: [`agent.rename`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`agent.focus`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Local pane focus` — No recognized-agent name targeting, attention acknowledgement or generic focus CLI. |
| Socket: [`agent.start`](https://herdr.dev/docs/socket-api/) | Different semantics | `pane run -- COMMAND ARGS` — Directly launches a process in a new split. Does not activate/name/recognize an agent in an existing idle shell. |
| Socket: [`agent.view.set`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`agent.view.clear`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Direct terminal/client control

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr terminal attach`](https://herdr.dev/docs/persistence-remote/) | Partial | `attach; pane read/events` — Whole-workspace attach and observation primitives; no dedicated single-terminal observe/control UX. |
| CLI: [`herdr terminal session control`](https://herdr.dev/docs/persistence-remote/) | Partial | `attach; pane read/events` — Whole-workspace attach and observation primitives; no dedicated single-terminal observe/control UX. |
| CLI: [`herdr terminal session observe`](https://herdr.dev/docs/persistence-remote/) | Partial | `attach; pane read/events` — Whole-workspace attach and observation primitives; no dedicated single-terminal observe/control UX. |
| CLI: [`herdr terminal title set`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr terminal title clear`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`client.window_title.set`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`client.window_title.clear`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Terminal stream: [`terminal.input`](https://herdr.dev/docs/persistence-remote/) | Control gap | `Typed text/keys/paste/submit` — No raw-byte JSON terminal-controller input; existing typed input has the ownership gap. |
| Terminal stream: [`terminal.resize`](https://herdr.dev/docs/persistence-remote/) | UI / wire only | `RootResize / UI resize` — Typed root/PTY resize, without Herdr's streaming JSON controller. |
| Terminal stream: [`terminal.scroll`](https://herdr.dev/docs/persistence-remote/) | UI / wire only | `UI history viewport / scroll protocols` — No matching external single-terminal JSON control stream. |
| Terminal stream: [`terminal.mouse`](https://herdr.dev/docs/persistence-remote/) | UI / wire only | `Typed app wheel input` — Wheel support differs; no general mouse JSON controller parity. |
| Terminal stream: [`terminal.release`](https://herdr.dev/docs/persistence-remote/) | Partial | `detach / context release` — Separate writer ownership and workspace detach mechanisms. |

### Session snapshot and events

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| Socket: [`session.snapshot`](https://herdr.dev/docs/socket-api/) | Partial | `pane list; control layout/panes/describe/reconcile` — Separate observations, no unified all-session bootstrap response. |
| Socket: [`events.subscribe`](https://herdr.dev/docs/socket-api/) | Partial | `events --timeout; wait; control watch/reconcile` — Metadata and layout notifications/reconciliation; finite watch transport, no matching JSON subscription API. |
| Socket: [`events.wait`](https://herdr.dev/docs/socket-api/) | Partial | `events --timeout; wait; control watch/reconcile` — Metadata and layout notifications/reconciliation; finite watch transport, no matching JSON subscription API. |

### Portable layouts

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| Socket: [`layout.export`](https://herdr.dev/docs/socket-api/) | Partial | `layout show; recovery view/relaunch` — Saved current/previous topology and fresh default-shell relaunch; no portable tree export or arbitrary declarative apply. |
| Socket: [`layout.apply`](https://herdr.dev/docs/socket-api/) | Partial | `layout show; recovery view/relaunch` — Saved current/previous topology and fresh default-shell relaunch; no portable tree export or arbitrary declarative apply. |
| Socket: [`layout.set_split_ratio`](https://herdr.dev/docs/socket-api/) | UI / wire only | `Divider UI; schema-13 DividerSet` — Absolute divider position/effective PTY resize, not matching directional ratio CLI. |

### Integrations

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr integration install`](https://herdr.dev/docs/integrations/) | Partial | `serve --opencode-bridge; provider-binding ...` — Experimental OpenCode path; no general integration installer/status family. |
| CLI: [`herdr integration uninstall`](https://herdr.dev/docs/integrations/) | Partial | `serve --opencode-bridge; provider-binding ...` — Experimental OpenCode path; no general integration installer/status family. |
| CLI: [`herdr integration status`](https://herdr.dev/docs/integrations/) | Partial | `serve --opencode-bridge; provider-binding ...` — Experimental OpenCode path; no general integration installer/status family. |
| Socket: [`integration.install`](https://herdr.dev/docs/socket-api/) | Partial | `serve --opencode-bridge; provider-binding ...` — Experimental OpenCode path; no general integration installer/status family. |
| Socket: [`integration.uninstall`](https://herdr.dev/docs/socket-api/) | Partial | `serve --opencode-bridge; provider-binding ...` — Experimental OpenCode path; no general integration installer/status family. |

### Plugins

| Surface and Herdr command/method | Panelink status | Closest Panelink interface and limit |
| --- | --- | --- |
| CLI: [`herdr plugin install`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin update`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin list`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin uninstall`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin enable`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin disable`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin link`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin unlink`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin config-dir`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin action list`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin action invoke`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin log list`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin pane open`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin pane focus`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| CLI: [`herdr plugin pane close`](https://herdr.dev/docs/plugins/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.link`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.list`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.unlink`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.enable`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.disable`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.action.list`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.action.invoke`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.log.list`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.pane.open`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.pane.focus`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`plugin.pane.close`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |
| Socket: [`popup.close`](https://herdr.dev/docs/socket-api/) | No equivalent exposed | `—` — No corresponding public command/method found in the reviewed Panelink dispatch. |

### Global selectors and bundled guidance

| Herdr flag | Panelink status | Mapping |
| --- | --- | --- |
| [`--remote`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | No Panelink remote attach command. |
| [`--machine`](https://herdr.dev/docs/connecting-machines/) | No equivalent exposed | No saved-machine API forwarding. |
| [`--session`](https://herdr.dev/docs/persistence-remote/) | Partial | Explicit directory or project/tab routes; no matching named-session selector. |
| [`--remote-keybindings`](https://herdr.dev/docs/persistence-remote/) | No equivalent exposed | No remote client/server keybinding selector. |
| [`--handoff`](https://herdr.dev/docs/persistence-remote/) | No equivalent exposed | No live binary/server upgrade handoff; unrelated to input-owner handoff. |
| [`--default-config`](https://herdr.dev/docs/cli-reference/) | No equivalent exposed | No default-config printing command. |
| [`--skill`](https://herdr.dev/docs/agent-skill/) | Partial | Repository/local skill exists; no release-bundled print-skill flag. |
| [`--version`](https://herdr.dev/docs/cli-reference/) | Comparable | panelink --version. |

## Additional semantic differences

- Agent-aware prompting: generic terminal text is not a recognized-agent API.
  Herdr documents startup readiness, blocked-state refusal and lifecycle waits.
  Panelink's text/quiet/exit waits do not establish a provider turn's completion.
  Herdr itself documents that a prompt wait does not identify individual turns.
  See [agent automation](https://herdr.dev/docs/agent-automation/) and [CLI semantics](https://herdr.dev/docs/cli-reference/#agents).
- Read sources: Herdr documents visible, recent, unwrapped and detection views.
  Panelink uses sanitized screen or bounded retained normal-history rows; saved
  files and alternate-screen contents are separate. Normal-history emptiness
  cannot establish a missing agent transcript. See [read sources](https://herdr.dev/docs/cli-reference/#read-sources).
- Public socket shapes: Herdr uses newline-delimited JSON; Panelink uses typed
  framed messages with target/lease/capability negotiation. The binary API's
  presence does not imply a general-purpose JSON or MCP adapter.
  See [transport](https://herdr.dev/docs/socket-api/#socket-transport).
- Herdr layout apply creates fresh structure/processes; it does not move live PTYs
  or restore their scrollback. Panelink's saved-layout viewer and fresh-shell
  relaunch are narrower than arbitrary declarative apply, and neither should be
  described as restoring every live process. See [layout methods](https://herdr.dev/docs/socket-api/#pane-graphics)
  and [session restore](https://herdr.dev/docs/session-state/).
- Herdr documents direct observer/controller streams, including the five terminal
  stream commands listed above. Panelink's workspace attach and typed UI protocol
  are narrower public integration surfaces. See [direct attach](https://herdr.dev/docs/persistence-remote/).
- A bundled release-matched skill/schema can reduce version drift. Panelink's
  repository skill is installed locally as a symlink; it currently teaches an
  older checkout baseline and routes newer features to their matching docs.
  See [Herdr's skill delivery](https://herdr.dev/docs/agent-skill/).

## Removed and non-equivalent APIs

The Herdr socket reference explicitly removes `pane.graphics.info`,
`pane.graphics.set`, `pane.graphics.clear` and `pane.graphics.stream`.
They are excluded from the current method count. Do not add them to a parity
backlog based on older docs. Kitty graphics via terminal output is a separate
facility. See [current graphics boundary](https://herdr.dev/docs/socket-api/#pane-graphics).

Herdr is also a local-socket/CLI product; this comparison does not invent an
HTTP or MCP layer for it. A remote SSH bridge, plugin command and direct-terminal
stream are different integration paths, not HTTP endpoint aliases.

## Panelink facilities without a same-named Herdr command

- Explicit `claim`, `release`, low-level `handoff`, expected owner revision and
  retained operation outcome/status/resync/receipt mechanics.
- Bounded saved terminal history `list/show/search/clear/cleanup`.
- Current/previous saved-layout inspection and offline recovery view.
- Direct argv/environment/cwd launch into a new managed split through `pane run`.

These differences do not prove an overall safety advantage. The ownership gap
must be fixed while preserving nonreplay and pending-input guards. The reviewed
Herdr docs do not establish an exact equivalent for every Panelink status or
retention rule, so no absence claim is made about Herdr internals.

## Priorities suggested by this comparison

1. Complete attached-UI/agent ownership handoff and specific error reporting.
2. Expose reliable explicit workspace/target routing for every agent command.
3. Expose the existing layout primitives as high-level pane CLI operations.
4. Design recognized-agent identities, prompt readiness and lifecycle reporting
   only with the required authoritative binding evidence.
5. Add schema/skill introspection and one-time host integration setup.
6. Evaluate remote/worktree/plugin scope separately against actual product needs.

These are suggested priorities, not accepted scope or implementation authorization.
The [ownership options](AGENT-INPUT-CONTROL-OPTIONS.md) remain proposed.

## Panelink evidence

- Current CLI/help: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/main.rs:675`.
- Agent CLI and owner errors: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/agent_cli.rs:125`.
- Workspace route dispatch: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/workspace_cli.rs:17`.
- Advanced control parser: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/p3_cli.rs:289`.
- Typed pane operations: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/pane_operations_protocol.rs:176`.
- Runtime pane-operation dispatch: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/multi_runtime.rs:1789`.
- Provider bindings: `/Users/sahillamba/.codex/worktrees/7060/panelink/src/provider_cli.rs:13`.
- Detailed CLI contract: `/Users/sahillamba/.codex/worktrees/7060/panelink/docs/progress/P3-AGENT-CLI.md:11`.

All inspected source belongs to the installed 7060 checkout. Markdown link and
hygiene checks validate this report's references; no new runtime acceptance is
claimed by this inventory.
