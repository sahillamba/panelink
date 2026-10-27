# Proposed agent API: read and control Panelink panes

Status: Broad API design proposal, 2026-10-27. Most commands and envelopes below
remain proposals, not a description of the installed CLI or accepted product
decisions. Exception: the bounded shared per-batch input and recognized-agent
slice documented in [AGENT-CONTROL.md](AGENT-CONTROL.md) is source-implemented;
bounded private-PTY checks pass as recorded in [implementation evidence](progress/HERDR-WORKFLOW-IMPLEMENTATION.md); existing-server activation and real provider acceptance remain open. The existing socket sandbox permission
prerequisite still applies.

## Recommended experience

An agent discovers a workspace, reads an exact pane, submits input once, and
checks what happened. Viewing a pane does not reserve its keyboard. The earlier
recommendation for workspace automation mode and explicit human Take control is
a broader proposal, not part of the accepted bounded per-batch slice. See the
[ownership options](progress/AGENT-INPUT-CONTROL-OPTIONS.md) for the remaining
unresolved choices.

The normal user request should remain: "Type hi in the two panes on the right."
The agent handles target references, contexts and operation keys. The bounded
source implementation uses per-batch UI ownership and the indexed submit
command; a user should not manually manage socket paths or owner revisions
during ordinary operation.

The [Herdr comparison](progress/HERDR-API-COMPARISON-2026-10-27.md) motivates a
small ergonomic surface, explicit targets and machine-readable discovery. The
user selected shared per-batch UI input ownership and included agent
list/get/read/prompt/wait for this bounded implementation. It supersedes the
per-pane Allow agents / Take control design for this scope. It does not settle
the broader workspace access policy, human takeover UX, or remaining open
decisions. Retain Panelink's nonreplay, exact payload, ownership and capability
contracts. No competitor code is selected.

## 1. One semantic API, one authority

Ship the CLI with JSON output first, backed by the server's existing typed local
socket protocol. The operation names in this document identify semantics; they
do not imply newline-delimited JSON socket requests, HTTP, or a second daemon.
An optional MCP adapter can later call the same dispatcher, subject to separate
scope/dependency review. It must not write PTYs directly or bypass ownership.

Keep the actual hierarchy: **Project → workspace tabs → split panes**. A
workspace tab is the running workspace. Do not introduce a second nested tab
object to imitate Herdr's hierarchy. Each route retains its server/storage domain.

Separate these authorities:

1. Host permission to run the CLI, write its private context and connect to IPC.
2. Workspace grants for Read, Input, Launch, Layout and Stop, separately scoped.
3. A finite input owner for one exact managed session.

A grant can authorize repeated work without a prompt for every key. It does not
override another input owner, prove per-agent identity against the same OS user,
or approve the target application's own confirmation dialogs.

## 2. Discovery, setup and version matching

The semantic operation registry is proposed as:

```text
api.capabilities, workspace.snapshot
pane.list, pane.get, pane.neighbor, pane.read, pane.wait
pane.submit, pane.control.acquire, pane.control.status
pane.control.renew, pane.control.release, pane.control.request
pane.keys, pane.paste, pane.launch, pane.split
pane.swap, pane.zoom.set, pane.divider.set, pane.interrupt, pane.close
operation.get
```

`workspace list`, schema/skill printing, host setup and context reconciliation
also perform local registry/context work; do not pretend they are all one remote
method. Semantic API versioning is distinct from the existing wire/schema
versions. Existing typed operations can back compatible cases; new composites
and operator transitions require explicitly negotiated, reviewed extensions.

Proposed commands:

```text
panelink api capabilities --workspace WORKSPACE_REF --json
panelink api schema --json
panelink api snapshot --workspace WORKSPACE_REF --json
panelink api doctor --workspace WORKSPACE_REF --json
panelink access setup --host codex --workspace WORKSPACE_REF
panelink --skill
panelink workspace list --json
```

- `schema` and `--skill` print the release-bundled client contract/guidance and
  work offline. `capabilities` reports the live server's selected methods,
  grants, versions, limits and missing optional features. A new client never
  infers server support from its own bundled schema.
- `snapshot` returns one workspace's pane layout, targets, ownership and a
  cursor captured consistently by its server owner. It excludes screen contents
  and launch arguments by default. Cross-workspace registry discovery is a
  separate observation, not a globally atomic snapshot of several servers.
- `doctor` separately reports executable/build, route, context storage, socket
  permission/connectivity, version negotiation, grants and ownership. Missing
  host access is not reported as "workspace not running". A probe that cannot
  contact the server reports server state unknown rather than inventing an answer.
- `access setup` is an interactive, one-time setup workflow. It previews any host
  configuration/grant changes and requires the applicable authorization before
  applying them. It can explain or request supported host permission; it cannot
  alter the active Codex sandbox or approval policy by itself. If the host forbids
  permission requests, return one specific remediation. No silent global edits,
  server restart, replacement state root or relaxed filesystem permissions.
- Setup chooses a stable private context root/caller identity before mutations.
  It does not rotate contexts to escape an uncertain operation. The proposed
  first-slice default is consent for the current server generation: approve once
  for that workspace lifetime, not once per command. Restart requires fresh
  consent, client grants and input ownership. Host permission configuration is
  independent and can remain installed if its host supports it. A later remember-
  setting could persist a policy preference authorizing fresh generation-scoped
  grants; it would never restore old leases or replay uncertain operations.
  That persistent policy is deferred for review.

Feature support is discovered. All examples below assume the corresponding
capability is advertised and the workspace grant permits it.

## 3. Exact targeting and routing

```text
panelink pane list --workspace WORKSPACE_REF --json
panelink pane get --target TARGET_REF --json
panelink pane neighbor --target TARGET_REF --direction right --json
```

`workspace list` returns opaque `workspace_ref` locators for project/workspace-tab
routes. `pane list` returns an opaque `target_ref` for each exact live occupant,
plus geometry, process observations, source/freshness, and input ownership.
An opaque locator is an identifier, not an authorization credential.

A returned target ref binds the route/storage domain plus the existing full
`TargetRef` tuple: server generation, workspace, pane, session and occupant
revision. The wrapper binds routing without changing the existing `TargetRef`
wire bytes. ProjectId/WorkspaceTabId remain route metadata. Require an explicit
review of any new wire representation; do not silently add those IDs to legacy
target messages. Reject a target ref supplied with a conflicting workspace route.

Mutations consume the exact returned ref. They never infer the newest session,
focused UI tab, calling pane, pane title or provider name. A ref from an older
generation returns `generation_changed` without retargeting or starting a server.
Names and spatial descriptions are discovery filters; ambiguous results are
reported. Neighbor lookup returns a target and observed layout revision, so
layout changes cannot silently redirect an operation that uses that result.

Retain numbered legacy syntax as compatibility wrappers; do not change where
existing commands route. Document its resolve-at-invocation semantics. New agent
examples use target refs. A resolved ref stays with that occupant even if its
screen location changes; if a task depends on location too, pass `--if-layout`
with the observed layout revision and fail on a topology change.

IDs, generations, sequences and revision counters are decimal **strings** in
JSON, preserving u64 values in JavaScript clients. The binary wire types stay
unchanged. Text/caller labels do not substitute for server-issued identity.

## 4. Read and observe

```text
panelink pane read --target TARGET_REF --source screen --lines 80 --json
panelink pane read --target TARGET_REF --source scrollback --lines 200 --json
panelink pane wait --target TARGET_REF --text READY --timeout 20s --json
panelink pane wait --target TARGET_REF --change --after OBSERVATION_REF --timeout 10s --json
panelink pane wait --target TARGET_REF --quiet 2s --timeout 10s --json
panelink pane wait --target TARGET_REF --exit --timeout 30s --json
panelink events --workspace WORKSPACE_REF --after CURSOR --timeout 30s --json
```

Read results carry target, source, observation ref, screen/history revision,
epoch, requested/returned rows, truncation/gap state and applicable limits.
Screen and retained normal-buffer scrollback remain different sources; saved
history is accessed through the existing history family. No silent file fallback,
terminal-control replay, unproved transcript completeness or alternate-screen
history substitution. Return sanitized text. It can still contain secrets printed
by the application and must not be copied into unrelated diagnostics.

An observation ref binds target, epoch and revision. `--after` provides an explicit
baseline for change and, when requested, fresh text matching: old matching output
alone cannot satisfy a fresh-read condition. It does not prove input caused that
output. Without `--after`, text matching may match the current screen immediately.
Quiet means unchanged screen revisions, not process readiness or completed work.
Exit reports a confirmed exit status or explicitly unavailable status.

Events carry metadata and cursors, not stored commands or prompts. Bootstrap
with a server snapshot/cursor and read subsequent retained events; a gap or
generation change requires reconciliation. If subscription support is not
advertised, the CLI uses documented finite watches. Do not advertise an indefinite
subscription until implemented. No event can execute or authorize a side effect.

All waits are bounded by default (proposed default 30s; server-advertised maximum).
CLI durations accept explicit units; JSON durations use integer milliseconds.
Timeout is an observation result and does not prove that an earlier effect failed.

## 5. Submit input with little ceremony

The former proposed single composite operation-key acquire/write/release
transaction is superseded for the bounded source implementation. The additive
indexed CLI command is `pane submit N TEXT [--json] [--as NAME]`; it uses three
separately keyed existing operations for claim, submit and release. Text is
nonempty, printable and limited to 32,768 UTF-8 bytes. Only a known `Applied`
submit is followed by release. Partial, failed, or uncertain input is neither
replayed nor released. A release failure after confirmed delivery reports
delivery confirmed and release unconfirmed. The UI likewise holds ownership
only for each admitted input batch. These short locks do not protect an
unfinished human draft between batches. Legacy explicit retained claims remain
exclusive and block other writers; no operation steals them.

The examples below are still proposed exact-target forms; they are not CLI
syntax for the current indexed wrapper.

```text
panelink pane send --target TARGET_REF --text hi --enter --json
panelink pane send --target TARGET_REF --stdin --enter --json
```

For a proposed exact-target API, continue to require exact target and current
ownership preconditions. The previously described one-key composite lifecycle
is not the implemented wire contract. Screen text remains observation, not
authorization.

The bounded implementation reserves and validates each existing operation
separately. Its claim, submit and release each have their own operation key and
exact payload. The new recognized-agent guarded-submit request has its own
schema-14 codec and is not a composite claim/write/release operation.

For the broader proposed exact-target API, delivery and release should be
reported separately: a successful write followed by a failed/uncertain release
must not be labelled "nothing sent". That proposed single-operation lifecycle
does not describe the current indexed CLI, whose three operation results use
separate keys.

For the implemented indexed submit, the submit status and release status are
reported separately. `delivery:pty_written` only means the defined bytes reached
the PTY interface. It does not mean the app read the message, accepted it, started
a turn or completed the task. Use a relevant observation/wait afterward. Another
application's own permission screen still needs its usual deliberate response.

Never automatically release after a failed/partial/unknown write or discard its
reconciliation state. The broader proposal has no implicit "force", suffix
resend or retry under another key. Its explicit authorized takeover remains a
proposal and is not implemented by the bounded slice.

`--stdin` avoids shell quoting and sensitive argv for larger text. Input is bounded
without silent truncation, and Panelink does not persist input/paste bodies in
contexts, journals, debug logs or receipts. Existing server-owned values and
same-user credential boundaries remain in force.

## 6. Retain control for a multi-step interaction

```text
panelink pane control acquire --target TARGET_REF --ttl 30s --json
panelink pane control status --target TARGET_REF --json
panelink pane control renew --control CONTROL_REF --ttl 30s --json
panelink pane keys --target TARGET_REF --control CONTROL_REF --keys Up Enter --json
panelink pane send --target TARGET_REF --control CONTROL_REF --text draft --json
panelink pane paste --target TARGET_REF --control CONTROL_REF --stdin --json
panelink pane control release --control CONTROL_REF --json
panelink pane control request --target TARGET_REF --json
```

A `control_ref` identifies a finite input claim with exact target, client lease
and owner revision, and is stored/reconciled under the same stable caller context.
It does not add stronger authentication. The CLI maintains revisions under the
existing context lock; callers do not guess revision numbers. It is distinct from
the longer-lived client connection lease and workspace capability grant.
Renewal returns a refreshed control ref; subsequent calls use it. A stale handle
does not silently acquire a newer owner or another claim, even for the same caller.

Typing without Enter and multi-step key/paste interactions require explicit
retained control. Do not interleave another writer between pieces of unfinished
input. A complete submit under retained control does not automatically release it.
Release is deliberate: the server cannot tell whether a partially composed
application command is safe to abandon merely because its PTY writes completed.

`request` creates a bounded metadata-only request for the current controller/UI
to grant or yield control. It carries no queued terminal input and changes no
owner itself. Expiry/cancellation does not later deliver a stale request. A
refusal is `input_owned_by_human` or `input_owned_by_client` with a safe owner
reference and an action hint; it is not an opaque `input_failed`.

## 7. Human control and the attached UI

The former per-pane Allow agents / Take control option is superseded for this
bounded scope. Current source implements no idle UI claims and short per-batch
input ownership; it has no explicit yield, draft reservation, or takeover
transition. The remaining human-control policies below are proposals only.

Proposed states are Available for automation, Agent-controlled, Human-controlled
and Switching. The server owns policy and transition order across all attachments;
a local badge alone does not authorize writes.

- Observing, focusing and reconnecting do not claim every pane in automation mode.
  Human-held panes stay protected until the user explicitly yields them. Enabling
  automation grants policy only; it does not release those panes. Each pane needs
  its own explicit yield after pending input settles and the human deliberately
  gives up its interaction, including any unfinished draft. No draft is cleared
  or declared safe by an idle heuristic. New splits follow the selected policy.
- While an agent holds input, the UI shows the owner and a Take control action.
  Ordinary keys/paste/application mouse input are neither sent nor queued for
  future replay. Local navigation, copy and normal-history viewing still work.
- Take control stops new old-owner admissions and renewals, settles the admitted
  operation, proves its writer quiescent, then advances the owner revision.
  It cannot be implemented as "claim from another owner" or by declaring a
  generic agent client to be human.
- If a bounded drain expires, an explicitly authorized takeover may finalize
  Partial/Unknown and discard the old unsent suffix only after proving no further
  old-writer IO can occur. Keep the sender's original outcome. If quiescence
  cannot be established, remain Switching/blocked; do not report successful control.
- Explicit yield returns the pane to availability. Timeouts, Enter and focus
  loss do not automatically end a human editing turn. Reconnect does not revive
  queued keys or stale input ownership.

This needs an authenticated, scoped operator transition at the server boundary;
the existing current-owner-only handoff method is insufficient. The first release
must state its same-OS-user trust limit and must not claim to distinguish hostile
same-user automation through an environment flag or caller name.

## 8. Launch, interrupt and layout operations

```text
panelink pane launch --anchor TARGET_REF --direction right --cwd DIR --json -- EXE ARG...
panelink pane split --target TARGET_REF --direction down --if-layout REV --json
panelink pane swap --target TARGET_REF --with OTHER_REF --if-layout REV --json
panelink pane zoom --target TARGET_REF --on --if-layout REV --json
panelink pane resize --target TARGET_REF --divider INDEX --first-cells N --if-layout REV --json
panelink pane interrupt --target TARGET_REF --control CONTROL_REF --json
panelink pane close --target TARGET_REF --grace 2s --escalation none --if-layout REV --json
```

`launch` starts a new argv-defined process in a new split. `split` starts a default
shell. Neither submits a shell string into an existing terminal. Keep the current
`pane run` as a compatibility alias for its existing launch semantics; do not
silently redefine it to match Herdr. `send --enter` is the existing-terminal path.
No automatic client focus change is required for agent creation operations.

Launch uses the invoking explicit cwd/environment context and server-owned
terminal identity rules, preserves the current aggregate bounds and Intent-before-
spawn discipline, and returns the creation target plus operation result. Startup
success is not application readiness. Partial/Unknown may retain an existing child;
neither result permits a replacement launch with a new key.

Layout operations wrap current typed layout capabilities while preserving exact
targets, expected layout revision and duplicate lookup before stale checks. An
absolute divider setting is not replayed as a relative resize delta. Retain the
advertised pane limit and minimum geometry; this proposal does not expand them.

Interrupt requires applicable input/control authority and is nonreplayable.
Close/stop requires its separately scoped destructive authority, explicit target,
grace and permitted escalation. A workspace input grant does not imply stop,
shell launch, cross-workspace access or a right to approve another app's prompts.

## 9. Results, failures and reconciliation

Proposed JSON envelope (identifiers and counters are strings):

```json
{
  "api_version": "1.0",
  "ok": true,
  "action": "pane.submit",
  "request_id": "req-41",
  "target_ref": "opaque-target-reference",
  "operation": {
    "generation": "15642535525154316803",
    "client_lease": "12",
    "sequence": "41",
    "status": "completed",
    "outcome": "applied"
  },
  "result": {
    "delivery": "pty_written",
    "control_release": "released",
    "task_completion": "not_observed"
  },
  "error": null
}
```

For scoped synchronous writes, `ok:true` requires Applied delivery and confirmed
release. If delivery succeeds but release is unconfirmed, return `ok:false`,
`effect:confirmed` for the input, the original operation, delivery details and an
actionable `control_release_pending` or `outcome_unknown` error. Retained-control
submit can succeed while intentionally holding its claim and labels that fact.
Read/wait `ok:true` means the requested observation/condition succeeded.
There is no implicit async success mode in this first proposal: incomplete
operations return a structured in-progress/unknown result and original key.

An error includes a stable `code`, sanitized `message`, `effect` of none/possible/
confirmed, relevant target/owner metadata, `action_required` and `retry_policy`.
It must never echo input, environment values or credentials. An error before key
reservation has `operation:null`; an uncertain submitted mutation retains its
original key even if its target result is unavailable.

| Error | Meaning and next action |
| --- | --- |
| host_permission_required | Host denied context writes or IPC; request the specific permission if the host allows it |
| context_storage_unavailable | Durable caller state cannot be safely read/reserved; preserve it and fix the specific access problem |
| workspace_not_running | Endpoint absence/refusal is established; do not infer this from a generic permission failure |
| capability_denied | Required workspace method/grant missing; a read grant does not imply input |
| unsupported_method | Live server lacks the method; consult capabilities, without silently substituting effects |
| stale_target / generation_changed / layout_changed | Rediscover and re-establish intent; do not retarget a submitted operation |
| input_owned_by_human / input_owned_by_client | Another controller holds input; request/yield control, without stealing it |
| control_expired / stale_owner | Reconcile the exact owner and outstanding operation before new input |
| input_pending | A writer has not settled; observe its original operation |
| payload_too_large | Refused without truncation; report the advertised limit |
| operation_in_progress / input_partial / control_release_pending / outcome_unknown | Inspect original status; do not resend input or invent a replacement key |
| observation_timeout / events_gap | Observation is incomplete; reconcile, without treating an earlier write as undelivered |

CLI exit 0 means the synchronous requested operation/condition succeeded;
exit 1 covers refusal, timeout, partial, unknown or incomplete outcomes; exit 2
means invalid syntax. JSON consumers must inspect delivery, lifecycle and
observation fields rather than translating an error into "no effect".

```text
panelink operation get OPERATION_REF --json
panelink context resync --as CLIENT_NAME --json
```

These are read/reconciliation operations. The CLI durably reserves each mutating
key before dispatch and retains the current nonreplay rules. A transport
`request_id` is not a deduplication key. Complete key/payload equality and the
existing ledger determine duplicate outcomes. Server/lease expiry does not
prove an effect never happened. Context locks serialize sequence admission.
The first proposal keeps one outstanding mutation per target/caller context;
no unbounded write backlog or automatic queued-input retry is added.

Receipt only handled terminal results under the existing context rules. Do not
retire a result merely because a transport reply arrived or a local write failed.
An abandoned/dead server's uncertain operation stays explicitly unobservable;
new generations never translate or replay it.

## 10. Concrete two-pane workflow

These are proposed commands, executed individually with results inspected:

```sh
panelink pane list --workspace "$workspace_ref" --json
panelink pane read --target "$first_ref" --source screen --lines 40 --json
panelink pane read --target "$second_ref" --source screen --lines 40 --json
panelink pane send --target "$first_ref" --text hi --enter --json
panelink pane send --target "$second_ref" --text hi --enter --json
```

The refs come from discovery, not hard-coded pane numbers. A routing/target/control
failure stops dependent actions. Inspect whether each pane is a shell, Codex or
another TUI before deciding what `hi` means; do not submit shell commands to a
provider prompt by accident. Each target has its own outcome: if the first send
succeeds and the second is refused, report that split result. Never resend the
first as part of a guessed "retry both" transaction. Verification should use an
applicable completion observation, not command echo or arbitrary sleeps.

## 11. Recognized-agent layer

The prior proposed provider-binding layer is superseded for this bounded source
slice by the screen-observation commands `agent list/get/read/prompt/wait`.
These use foreground process identity and conservative terminal markers; they do
not prove provider conversation identity. Their presence in source does not
establish runtime acceptance. Details and limits are recorded in
[AGENT-CONTROL.md](AGENT-CONTROL.md).

Do not treat the implemented process-bound refs as provider conversation refs.
Unknown, stale or blocked observations refuse prompt input. Do not infer authority
from a pane title, caller label or generic screen text.

An adapter may report working/ready/blocked/unknown with source/freshness. State
waits are labelled state observations. A claim that a particular prompt's turn
completed requires an adapter-supported correlation/barrier; otherwise report
the limitation. No general prompt queue, model selection, autonomous delegation,
remote machine management or task scheduler is implied by this pane API proposal.

## 12. Delivery and acceptance

Recommended sequence for the remaining broad API work:

1. Host diagnostics, version/capability/skill discovery and exact workspace routing.
2. Decide broader grants, retained-control and explicit human takeover policy;
   the selected bounded per-batch implementation does not settle these choices.
3. Add bounded observations/cursors and high-level layout/launch wrappers.
4. Establish separate runtime, host and provider acceptance for the source slice.

Keep the binary dispatcher and clients under one schema/version contract. Bound
requests, rendered JSON replies, event pages, rows, text bytes, control requests
and waits. Advertise actual selected limits, preserving the current 65,536-byte
request budget and other layer-specific bounds until explicitly revised. JSON
escaping expansion needs its own bounded-output policy; never assume the wire
limit alone bounds every CLI allocation. No new serialization/runtime dependency
is selected by this document.

Acceptance must exercise a visible attached UI and independent caller: read and
submit in the intended panes without detach; human unfinished drafts; two-agent
claim races; two UI attachments; paste/wheel input; split/reconnect/expiry; exact
refs after route/layout/generation changes; u64 JSON roundtrips; denied storage
and socket access; missing capabilities; oversized payloads; and lost input,
release and launch acknowledgments without replay or duplicate children.
Use private fixtures, shell-readiness barriers and parsed terminal cells.
No live user pane is a disposable acceptance fixture.

Documentation acceptance is separate from these runtime gates. Implementation
must update this proposal's accepted status only after a user decision, then the
canonical spec, agent-control contracts, help/schema/skill and the current
worktree's durable handoff. No phase or release is completed by proposing names.

## Sources and compatibility

- [Current Panelink agent-control design](AGENT-CONTROL.md) supplies the existing
  ownership, caller-context, targeting and retry contracts. This proposal refines
  agent ergonomics; it does not replace shipped behavior.
- [Herdr agent automation](https://herdr.dev/docs/agent-automation/) motivates
  separating terminal locations from recognized agents and returning targets.
- [Herdr socket reference](https://herdr.dev/docs/socket-api/) motivates schema
  discovery, snapshots, explicit methods and event reconciliation. Their
  transport/acknowledgment model is not adopted as a substitute for Panelink's
  typed protocol and operation ledger.
- [Detailed comparison](progress/HERDR-API-COMPARISON-2026-10-27.md) records the
  current published Herdr surface versus the installed 7060 Panelink build.

Open choices: acceptance of workspace automation mode and the proposed session-only
consent default; any later persistence of consent;
exact operator takeover authority/handshake; opaque-ref encoding; compatibility
and schema revision plan; supported host setup mechanisms; and provider adapter
evidence. These are explicit design choices to settle before implementation.
