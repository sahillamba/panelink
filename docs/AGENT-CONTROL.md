# Agent-to-pane control plane proposal

Status: proposed design for confirmed feature R-15, 2026-10-17, with a bounded implementation checkpoint dated 2026-10-18 below. The user confirmed that agents need a simple CLI/API to discover, read, and deliberately control managed pane/process sessions. The broader interface and permission policy remain proposed; cross-workspace scope and access defaults remain open. Autonomous delegation/scheduling is not implied.

## Current implementation checkpoint — 2026-10-18

The integrated macOS/aarch64 prototype now has bounded wire and typed response/session codecs, same-UID peer authentication before handshake processing, a private-directory socket listener that refuses an existing endpoint, bounded connection workers and one `StateOwner`, durable caller contexts/input ownership, startup ledger admission, and durable lifecycle-metadata Intent before PTY spawn. The operation ledger itself is volatile. Its explicitly selected one-child PTY survives logical detach and same-generation reconnect; socketpair tests cover a lost input acknowledgment without client replay. The prototype does not implement the full R-15 API described here: workspace/layout operations, indefinite subscriptions, multiple sessions, persisted scrollback, provider identity/recovery, and per-agent isolation remain out of scope. The bounded prototype now provides negotiated metadata watches and atomic screen/owner/exit/cursor reconciliation; see [live-watch tasks](progress/P3-LIVE-WATCH-TASKS.md). See [integration evidence](progress/INTEGRATION.md) for the current verification record and [the specification checkpoint](SPEC.md#current-implementation-checkpoint--2026-10-18) for lifecycle, initialization, and acceptance limits.

## Design goal

Make Panelink's managed terminal sessions addressable from another local agent without making the terminal UI the only control surface. One command should let an agent list panes, read a bounded current screen or scrollback tail, submit explicitly authorized input, and wait for a bounded observable condition. The CLI and local socket use the same typed request contract. An optional MCP adapter can map to that contract later; this proposal selects no MCP or serialization dependency.

The confirmed requirement is agent-to-pane read/control. It does not decide that Panelink should assign work on its own, choose models, approve agent prompts, create worktrees automatically, or run a task scheduler. Those remain separate product choices.

## Herdr research and design takeaways

The official Herdr documentation describes one CLI and one local socket API for scripts and agents. It separates layout operations, generic terminal-pane operations, and recognized-agent operations. A pane is a terminal location; an agent is the process recognized as its current occupant. Its documented primitives include bounded screen/scrollback reads, typed text and key input, explicit launch, event subscriptions, and waits for output or agent state. The docs also distinguish an agent name from the pane it currently aliases, and describe resolving IDs from creation responses instead of guessing them. These are useful product patterns, not Panelink API requirements. [Herdr CLI reference](https://herdr.dev/docs/cli-reference/), [Herdr agent automation](https://herdr.dev/docs/agent-automation/), [Herdr socket API](https://herdr.dev/docs/socket-api/), [Herdr concepts](https://herdr.dev/docs/concepts/).

Herdr's API documentation warns that an event stream is bounded and may report `events_lost`; clients then reconcile from an authoritative snapshot rather than treating the partial stream as current. It also documents agent waits that bind to the resolved pane occupant, so a replacement process cannot satisfy a wait accidentally. Panelink should preserve those reliability properties while using its own IDs, wire encoding, errors, and retry contract. [Herdr socket API](https://herdr.dev/docs/socket-api/), [Herdr agent automation](https://herdr.dev/docs/agent-automation/).

This proposal is independently written from public behavioral documentation. It copies no Herdr implementation, source, asset, wire schema, or code. Research used the linked official Herdr documentation pages on 2026-10-17; no Herdr software was installed or executed.

## Proposed object and identity model

Keep layout, terminal session, process, and agent identity distinct:

| Object | Meaning | Identity rule |
| --- | --- | --- |
| Server generation | One server lifetime and its operation namespace. | Use the existing nonzero `generation` value. A restarted server has a new generation; old operation keys never become valid again. |
| Workspace | A user-visible project/access scope. | Opaque server-issued `workspace_id`; its hierarchy and persistence are still open product decisions. |
| Launch location | An explicit workspace-provided insertion point for a new managed pane/session. | Opaque server-issued `launch_slot_id`; it is used only by launch and is not a target for later read/control. Initial launch does not require split-tree editing. |
| Pane view | A location in a workspace layout. | Opaque `pane_id`; moving or replacing a pane must not silently retarget an existing control request. |
| Managed session | The PTY, terminal-engine state, process lifecycle, and launch identity. | Opaque `session_id`, unique for the server generation. Never use a PID, title, newest pane, or newest session as identity. |
| Occupant revision | The incarnation currently attached to a pane view. | Monotonically increase when a pane's managed session changes. |
| Agent binding | An optional provider identity attached to one managed session. | Include provider, exact session ID, project identity, binding revision, evidence source, and freshness. Unknown or uncertain binding remains explicit. |
| Client context | A caller-side, reusable CLI/API context for one stable automation client. | A context locator selects cached generation, server-issued lease, and sequence-allocation state. It identifies no trusted process or agent and is separate from `TargetRef`. |

Every agent-facing read, watch, wait, or mutation uses a returned `TargetRef` containing an opaque reference token plus `server_generation`, `workspace_id`, `pane_id`, `session_id`, and `occupant_revision`. The server validates the token and full tuple atomically before reading or reserving an effect. A stale pane, replaced process, moved view, or old server returns a typed stale-target/generation error and performs no effect. The token is a stale-target handle, not an authorization credential. An explicit expected tuple may be used instead, but must include the generation, workspace, pane, session ID, and occupant revision; omitting any field weakens stale-target protection and is not recommended for the agent API. Input operations additionally carry `input_owner_revision`. This follows the spec's distinction between a pane view and the stable managed session.

Names are human-readable aliases only. Alias resolution is a separate read operation that requires explicit `--resolve-current` opt-in and returns the current `TargetRef`; the caller then passes that exact reference to reads, waits, and mutations. Agent-facing commands never resolve an alias implicitly, and agent-facing mutations never use an alias as their default target. This prevents a command started after pane replacement from silently acting on the replacement occupant. A long-running wait or input request pins the exact session incarnation. If the server cannot prove an agent binding, `agent` commands return `identity_uncertain` or require an explicit pane/session target. They never choose the most recently active OpenCode conversation.

## Architecture

```text
agent / script / Panelink TUI
          │
          ├── `panelink` CLI ──────┐
          ├── optional MCP facade  │ same typed operations and results
          └── local socket client ┘
                    │
          versioned bounded frames
                    │
       peer check + request validation
                    │
          single server state owner
          ├── workspace / pane / session registry
          ├── generation, lease, operation ledger
          ├── provider identity/status adapter (later M1 gate)
          ├── bounded launch/input/signal effect queues
          └── bounded event and snapshot queues
                    │
              PTY / terminal core
```

The CLI must not maintain a second policy path. It encodes the same request body sent over the socket and renders the same stable response/error model. The server state owner validates target identity, workspace policy, capability, operation class, and capacity before dispatch. Socket reader threads only decode bounded frames and submit requests; they do not touch a PTY or child directly. Each connection has one writer. Agent output never causes the state owner to dispatch an operation.

### Reusable caller context

Short-lived CLI invocations must reuse one bounded client context and its server-issued lease; allocating a fresh lease for every shell command would consume the current 4,096-ever-opened-leases-per-generation limit. A CLI context locator may come from an explicit `--context` option or `PANELINK_CLIENT_CONTEXT_ID`, resolved through a private local context registry. One-time `context init` establishes the stable locator and obtains a server lease; subsequent invocations use `context show`, `context renew`, or `context resync` to reuse and reconcile it. The registry stores only opaque context ID, server generation, lease ID, expiry/last-renewed time, and sequence high-water state. It does not store request bodies, input/paste text, environment values, credentials, terminal output, or target references. The same locator on each invocation reconnects to the same still-valid server lease; the server can renew it after rechecking the peer UID. The current proposed lease TTL is 1,800 seconds. Expiry, revocation, or a server-generation change invalidates old operation keys; the client must resync and reconcile uncertain effects before opening a replacement context/lease. If the generation's lease-record cap is exhausted, fail closed rather than restarting the server automatically.

For concurrent CLI processes sharing one context, the local context store is the sequence allocator: reserve a strictly increasing sequence under one cross-process lock/atomic transaction and persist the new high-water mark before dispatch. Hold the mutation-admission lock through the server's admitted/rejected response so sequence 2 cannot arrive before sequence 1; serialize admission, not the subsequent process work. Gaps after crashes are acceptable; sequence reuse or rollback is not. The server ledger remains authoritative for admitted operations and rejects conflicts, stale generations, expired leases, and any new sequence not greater than its accepted high-water mark. If the acknowledgment is lost, mark the context uncertain and block later mutations until read-only status/resync reconciles that operation key; never resend its input payload. If local context state is lost or disagrees with the server, stop and resynchronize instead of resetting to sequence one. This is a proposal to refine how the current “client chooses strictly increasing sequence” contract is implemented; it does not change the server-issued lease or the server's admission authority. Calls using different context IDs get distinct leases. A context marker is only a locator, never proof of caller identity or a permission grant; the implemented same-UID socket boundary does not isolate one local process/agent from another.

The caller context is not the target. Every invocation supplies both its reusable caller context (for request correlation and the operation key) and the exact `TargetRef` for the pane/session it addresses. Reusing a client context never preserves or guesses a pane target.

## API surface and phase boundary

The first useful control plane should cover generic processes even when the server cannot identify a coding agent. Agent-aware aliases and lifecycle operations are layered on top only when the provider identity/status contract is available.

| Surface | Initial essential operations | Later or gated operations |
| --- | --- | --- |
| Discovery | `workspace.list` (including currently valid launch slots), `pane.list`, `pane.get`, `session.get`, `session.status`, `api.snapshot` | Layout export and richer hierarchy as workspace/tab choices settle. |
| Read | `pane.read --source screen`, `pane.read --source live-scrollback`, bounded `pane.watch` / output events | Persisted history/search and recovery checkpoints remain a separate later storage task. |
| Launch | `pane.launch` with an explicit server-issued launch slot, cwd, argv, and environment inputs; returns server-issued pane/session IDs and a complete `TargetRef`. | Split-tree creation, named OpenCode launcher, and verified identity capture after M1. |
| Input | `pane.input.keys`, `pane.input.text`, atomic `pane.input.paste`, and atomic `pane.input.submit` (text plus the defined submit key). | Provider-specific `agent.prompt` only after the provider adapter proves exact-session targeting. |
| Lifecycle | `session.interrupt`, explicit `session.stop`, `session.detach`, absolute `pane.resize`. Stop admits a caller-selected grace period and escalation policy under the stop capability before it begins. | Pane focus, move/swap/zoom, split-tree editing, and full layout management as M2 decisions allow. Focus changes only the attached TUI's selection; it never changes the target or authorizes input. |
| Agent identity/state | No identity inference from generic screen output. Return `agent_state: unknown` when there is no authoritative binding. | `agent.list/get/read/submit/wait` keyed to a verified provider binding; M1 must prove top-level identity and session-switch behavior. |
| Collaboration tasks | Direct, explicit `pane.input.submit` is sufficient for the first cross-pane workflow. | A `task.*` record/assignment API may be added after task ownership, acknowledgment, completion, and retry semantics are chosen. It must not imply autonomous scheduling. |

The initial agent-facing CLI/API contract should include these commands before server routes freeze: `context init/show/renew/resync`, `workspace list`, `pane list/get/launch/read/watch`, `session get/status/input claim/input release/input handoff/interrupt/stop/detach`, `pane input keys/text/paste/submit/resize`, and `operation status`. Context commands inspect and repair caller lease state; they do not select a pane. Each target-bearing command requires the exact `TargetRef`; launch requires an explicit advertised `launch_slot_id`. Keep this first command set generic-process focused; defer provider commands, task assignment, split-tree editing, and other layout mutations.

`pane.read` exposes a safe decoded screen representation and bounded live scrollback, never raw terminal control bytes. The `screen` source is the current emulator state; `live-scrollback` is the terminal engine's in-memory bounded scrollback. Persisted normal history, screen checkpoints, and provider conversations remain distinct data products under `SPEC.md`; they are not silently combined into a single `read` source. ANSI output, if ever offered, is a presentation encoding produced by Panelink, not replayable child output.

`pane.watch` is a bounded subscription to screen revisions and output-advanced notifications. It returns a generation-scoped cursor, session ID, output sequence, current screen revision, and freshness metadata. Where a safe decoded text tail is included, it is explicitly marked as untrusted terminal content: an agent may use it to reason about its assigned task, but output itself never grants authority, changes access, or serves as an API command to be blindly executed or routed to another agent. Reading never calls the host clipboard or creates terminal-control side effects.

## Input, ownership, and confirmation model

Workspace access and input ownership answer different questions: a collaboration grant says whether a client may request input; an owner lease says which one writer may currently deliver it. The proposed v1 API has bounded `session.input.claim`, `session.input.release`, and `session.input.handoff` operations. Each includes the complete `TargetRef`, expected `input_owner_revision`, and caller context; claim additionally requests a finite lease duration, and handoff names the intended next owner. The server returns the new owner identity (UI or opaque client-context ID), incremented owner revision, and monotonic expiry. A grant can authorize these operations without a prompt for every key; the lease cannot exceed a server policy maximum. A reasonable prototype starting point is a 30-second lease, renewable while useful work is active, then explicit release. The server validates owner revision and expiry with every input mutation.

Only one input owner exists per managed session. The TUI's keyboard writer and an agent writer go through the same server-ordered input queue; they never interleave bytes. Each admitted keys/text/paste/submit operation is atomic at the API boundary and holds its writer position until the server reports accepted bytes and any pending suffix outcome. This does not promise that a terminal application interprets a byte sequence atomically. The server must report `Applied`, `Partial`, or `Unknown` accurately and must not hand the writer to another owner while an old operation is unresolved.

The recommended first implementation is a finite session-level input lease, rather than a claim popup or human confirmation for each operation. A workspace collaboration grant lets an agent claim an available input lease silently. If another owner holds it, the call returns `input_busy` or performs an explicit compare-and-swap handoff; it does not take over implicitly. The TUI exposes a deliberate “Take control” action. It sends a handoff with the expected owner revision, waits for the prior operation to drain, and then increments the revision and grants ownership. If a bounded drain deadline expires, the server marks the old operation `Partial` or `Unknown`, discards its not-yet-written suffix, and only then completes the handoff. No bytes are retried across the handoff. While an agent owns input, ordinary TUI keystrokes are not sent to the child or queued for later delivery; the TUI may handle its own status/control shortcuts, and the user must complete Take control before typing into the child. Keys arriving before the UI owns the writer are never buffered for a future owner. Lease expiry follows the same drain-or-mark-and-discard rule before ownership revision advances.

The human takeover and workspace-grant behavior are proposals, not settled release policy. Keep the UI ownership indicator visible and make denial/expiry discoverable so the user can tell why terminal typing is not reaching the child.

Input is typed so agents can use the level of control they intend:

| Input operation | Contract |
| --- | --- |
| `keys` | Validates the entire bounded key list before queueing it. One operation contains the exact ordered key sequence. |
| `text` | Sends literal text without adding Enter. It is not a shell command API. |
| `paste` | Sends one caller-supplied bounded payload using the target terminal's live bracketed-paste mode when supported. The API never reads the host clipboard. |
| `submit` | Sends one caller-supplied text payload plus its defined submit key as a single ordered, bounded input operation. It is an explicit request to type into that terminal, not proof the app accepted or completed it. |
| `interrupt` | A separately identified interruption operation with platform-specific, documented semantics. It is never replayed after an uncertain result. |
| `stop` | Explicitly ends the selected managed process/session using a documented graceful-stop sequence and deadline. The request declares the permitted escalation (for example, none or a specified signal) and deadline; admission checks the separate stop capability and workspace policy once. If admitted, the server may perform that escalation at the deadline without a fresh per-action human popup. The result reports `exited`, `still_running`, `escalated(signal)`, or `unknown`. It is not detach or pane close. |
| `resize` | Layout-owned absolute desired dimensions with full target and expected layout revision. For tiled panes, validate/recompute constrained leaf rectangles through the layout owner; reject unrepresentable dimensions before effects. Only computed accepted rectangles reach TerminalSession. Return actual/desired geometry and reconcile on uncertain acknowledgment; do not replay relative deltas. See [PANE-LAYOUT.md](PANE-LAYOUT.md). |

CLI payloads may be supplied inline for short, harmless examples or through bounded `--text-file` / `--text-stdin`, `--paste-file` / `--paste-stdin`, and `--env-file` / `--env-stdin` inputs. The CLI validates encoded and decoded byte limits before copying or queueing; stdin/file sources do not create an unbounded bypass. Avoid putting secret-bearing values in process arguments. Launch environment contents exist only in volatile request/staging memory, are excluded from logs and metadata, and follow the reviewed environment cap/staging contract in the Phase 3 bounds section. Secret-bearing files or stdin do not authorize persistence or weaken the clipboard-denial rule.

Only an explicit request from the caller can submit input. Do not automatically relay one pane's output into another pane. Do not replay keys, text, paste, submit, interrupts, or signals if an acknowledgment is lost or the connection drops. The client queries operation status for reconciliation, but an input status cannot prove that the child interpreted the bytes. Partial/uncertain writes are `Partial` or `Unknown`; no automatic retry follows.

The recommended prototype policy is a workspace-level collaboration grant: a trusted local workspace can enable sibling-session read and input access once, so agents do not need a human approval for every keystroke. Scope a grant to the named workspace and separately opt into cross-workspace access. Keep launch and destructive stop authority as distinct capabilities. A stop grant/policy authorizes the chosen graceful period and any allowed escalation at admission; a `SIGKILL`/force-stop escalation must be separately included in that grant or policy, but does not trigger a new popup when the deadline arrives. Stopping the server remains a different operation and may require explicit human action. Access configuration is separate from terminal input and from OpenCode's own permission prompts.

This is a usability proposal, not strong per-agent isolation. The planned P3 same-UID socket check would distinguish users, not processes or agents. Environment markers, client-supplied agent names, leases, pane labels, and workspace IDs do not create a trustworthy per-agent identity against another process running as the same Mac user. The prototype must say so plainly. If a firm deployment needs one agent to be unable to impersonate another same-UID agent, that requires a separately reviewed OS/process isolation or unforgeable capability design; it is not delivered by a 0600 socket.

Programmatic child clipboard reads/writes remain denied by the canonical spec. The API's `paste` accepts content explicitly supplied by the caller; it does not access a clipboard. A terminal screen read is a data query, not authorization to copy or paste.

## Request, cursor, and error contracts

Keep transport `request_id` separate from the existing operation key. `request_id` correlates one socket request and response; `OperationKey { generation, lease, sequence }` identifies an effect for deduplication and status. Each response repeats its `request_id`. Structured CLI output uses one stable envelope:

```json
{
  "request_id": 104,
  "server_generation": 12,
  "result": {
    "type": "pane_read",
    "target_ref": {
      "token": "tr_v1_7ac...",
      "server_generation": 12,
      "workspace_id": "ws_7d2",
      "pane_id": "pn_31a",
      "session_id": "ses_9f6",
      "occupant_revision": 4
    },
    "screen_revision": 88,
    "input_owner_revision": 7,
    "freshness": {"source": "live_terminal_engine", "age_ms": 14},
    "text": "tests passed\n"
  }
}
```

The socket and CLI share stable machine-readable error codes, with sanitized messages and no input, paste, environment, credential, or terminal output echoed in errors. Minimum codes: `not_found`, `permission_denied`, `capability_denied`, `target_stale`, `generation_mismatch`, `identity_uncertain`, `unsupported`, `capacity_exceeded`, `cursor_expired`, `resync_required`, `operation_conflict`, `operation_in_progress`, `session_exited`, `deadline_exceeded`, `input_busy`, `input_owner_revision_conflict`, `input_lease_expired`, `input_partial`, `outcome_unknown`, and `context_state_lost`. Include a retry disposition derived from the existing operation class: safe query reissue, same-key/same-payload only, never retry, or reconcile absolute state.

Use two revision domains:

- `server_generation + event_cursor` for workspace/session lifecycle and layout changes. A snapshot carries the cursor observed atomically by the state owner. A subscription asks for events after that cursor. If it is from another generation or older than retention, return `resync_required` and a full current inventory; never label a partial stream current.
- `session_id + output_sequence + screen_revision` for one terminal stream. `output_sequence` advances as the engine accepts PTY output; `screen_revision` advances when the rendered screen changes. A read returns both. Output events name a bounded sequence range and revision; clients can ask for a fresh screen instead of assuming they received every delta.

Waits are server-bounded and have an explicit condition, starting revision/cursor, and deadline. Return one of `matched`, `state_observed`, `session_exited`, `timed_out`, `cursor_expired`, `target_stale`, or `unknown`; timeout is never success. A text/output match means only that the bounded screen/output contained that text after the requested cursor. It does not mean tests passed or an agent completed successfully. Agent waits require a verified binding and exact requested state; `idle` is readiness, not success, as already required by `SPEC.md`.

## Typed schema and Phase 3 integration gates

The v1.0 wire envelope and a bounded subset of typed control/response/session schemas are implemented. The current runtime uses that subset with an authenticated local socket and state owner; it does not implement the full API below. The 65,536-byte Request/Response and 2,097,152-byte Snapshot payload limits remain distinct from the 24-byte frame header. The runtime negotiates Events only with valid authoritative metadata tracking; it delivers fixed metadata Event frames within bounded watch requests. The following are remaining design boundaries and future feature contracts, not claims that the prototype implements snapshots, watches, or events:

1. **Large launch environments (initial API policy selected):** the existing explicit host-probe input cap is 4,096 variables and 1 MiB of environment bytes; a typed Request payload is capped at 65,536 bytes, including its operation fields (the wire header adds 24 bytes). Sending a full 1 MiB environment in one Request cannot fit. Parent selects option(a) for the first server API prototype: the entire encoded launch request must fit65,536bytes, including all identity, argv and environment metadata; otherwise reject before admission with a clear capacity error. Do not truncate or inherit daemon values. The standalone probe retains its separate1MiB capture cap. Bounded staging is deferred until actual needs justify its extra lifetime/auth/partial-failure state. Future reviewed alternatives remain: (a) lower the API launch-environment cap to the encoded request budget and fail clearly; (b) extend the request/operation cap and re-review frame, ledger, queue, and effect-copy memory bounds; or (c) stage a bounded, volatile launch context across multiple <=65,536-byte operations, then reference it from the launch request. Option (c) can preserve the current probe cap without enlarging one Request, but adds context lifetime, cleanup, authorization, and partial-stage failure rules. Do not split or silently truncate environment values. The existing 1 MiB cap describes the probe capture bound, not a completed server API requirement.
2. **Snapshot frame versus queue:** a maximum Snapshot is 2,097,152 payload bytes plus the 24-byte header, or 2,097,176 encoded bytes. The selected future output budget is 2,162,736 encoded bytes per writer and 17,301,888 across eight writers (one maximum Snapshot plus one maximum control reply per writer). The prototype has bounded connection/effect queues, but does not implement this snapshot/event output queue or snapshot/event delivery. Queue accounting must reserve noncoalescible replies and close/resync a slow subscriber rather than silently dropping an input acknowledgment before those features are adopted.
3. **Unsolicited event framing (envelope implemented):** Event is numeric kind 7, request ID 0 only, with a 65,536-byte payload cap. The runtime now explicitly negotiates metadata Events for bounded watches; older private v1.0 decoders reject this kind. The initial metadata payload and bounded cursor journal are now specified and implemented in [P3-NOTIFICATIONS-TASKS.md](progress/P3-NOTIFICATIONS-TASKS.md); the bounded runtime now binds reconciliation to its cursor and delivers finite event batches; indefinite subscriptions and large snapshot queues remain unimplemented. Do not overload Response or treat request ID 0 alone as a typed event payload contract.
4. **Input payload overhead:** the full typed target, operation key, action kind, and payload share the existing 65,536-byte Request/ledger boundary. Define the maximum user text/paste bytes after accounting for typed metadata. Validate before copying or queueing; atomic paste and submit are all-or-nothing at admission.
5. **Current screen versus persisted data:** Snapshot is a live screen view. It is not scrollback, checkpoint, or agent conversation. Keep the terminal emulator's safe screen representation separate from raw PTY bytes and from the later storage schema.

The bounded codecs, same-user socket boundary, state owner, durable storage, and one-child PTY reconnect proof are implemented and described in the current checkpoint above. The remaining proposals include the full discovery/read/watch/launch/control API, snapshots/events and resync, durable workspace/layout mutations, provider-specific commands, and their corresponding acceptance work. Add provider-specific agent commands only after M1 identity evidence. Add split-tree and other layout mutation only as the workspace/tab/pane model settles in M2; initial launch can use an advertised location slot without layout editing.

## Proposed roadmap and migration tasks

| Order | Task | Dependency and migration note |
| --- | --- | --- |
| P3-A / P3-A2 | Bounded wire and typed codec slices | Implemented; response/session codecs and the control subset are present. Full API schemas and EVENT delivery remain future work. |
| P3-B1 | Global ledger bounds, expiry reclamation, and lease issuance | Implemented in the bounded prototype; see [P3-LEDGER.md](progress/P3-LEDGER.md) and [INTEGRATION.md](progress/INTEGRATION.md). |
| P3-C | Same-user local socket handshake and peer-identity check | Implemented for the private prototype. Pathname CLI acceptance remains pending because the fixture is denied with EPERM in the current environment. |
| P3-D | Bounded state owner and durable startup metadata | Implemented for the bounded prototype. The operation ledger remains volatile; metadata Intent is durable before child spawn. Full agent routes and automatic uncertain-effect recovery are not implemented. |
| Private PTY proof | Separately authorized single-child detach/reconnect experiment while Phase 2 host gates remain pending | Completed for logical detach/reconnect and lost-ack no-replay over socketpairs. This does not close Phase 2 manual gates or establish pathname CLI/Apple Terminal acceptance. |
| M1 / Phase 4 | Prove provider session identity and switching, then add agent aliases, semantic agent waits, and provider-specific submit. | A server schema can advertise the capability only when the runtime supports it; unknown identity remains generic pane/session control. |
| M2 / Phase 5 | Finalize workspace/tab/split choices; add focus and layout mutations. | A layout migration must preserve managed `session_id` values while creating fresh pane-view revisions; it must not relaunch children. |
| Later | Revisit MCP facade and task registry after the API works and the user chooses task semantics. | Both remain clients of the same control plane; neither receives direct PTY access or implies autonomous scheduling. |

At present there is no persistent server schema, client cache, or saved API payload to migrate. The first typed protocol should therefore declare its own schema version/capabilities in the handshake, reject incompatible major versions before dispatch, and define whether additive minor fields are optional or required. When persistence is later added, version the saved workspace/layout/session metadata separately from protocol payload versions; migrations must preserve provider-session identity, launch intent outcomes, and history/checkpoint separation. Do not treat the envelope's v1.0 label as evidence that typed request schemas have already been fixed.

## Example: one agent asks another to review

This proposed flow uses generic pane control, so it does not depend on claiming provider status:

```sh
# From an agent running in Panelink workspace ws_7d2, discover sibling targets.
# One-time setup creates a reusable context/lease for this automation client.
panelink context init --context reviewer-agent --json
export PANELINK_CLIENT_CONTEXT_ID=reviewer-agent
panelink pane list --workspace ws_7d2 --json
panelink workspace list --json
# The caller's JSON parser selects an explicit available launch_slot_id for ws_7d2.

# If a reviewer is not running yet, create a pane/session with explicit launch
# inputs. The location is an advertised launch slot; this initial API does not
# depend on future M2 split-tree editing. The command does not use a shell.
panelink pane launch --workspace ws_7d2 --location "$launch_slot_id" \
  --cwd /repo --env RUST_LOG=info --json -- \
  /usr/local/bin/opencode > reviewer-launch.json

# The agent runtime's own JSON parser copies result.target_ref verbatim from
# reviewer-launch.json into reviewer-target.json. No jq or other parser is a
# Panelink product dependency. The full ref contains the token and all identity
# fields; it is not a pane-name alias.
panelink pane read --target-ref-file reviewer-target.json \
  --source screen --max-bytes 16384 --json

# Claim the finite input lease once under the workspace collaboration grant.
# The caller gets the current input_owner_revision from pane/session status,
# then passes it as a compare-and-swap precondition.
panelink session status --target-ref-file reviewer-target.json --json
# The agent runtime parses result.input_owner_revision into expected_owner_revision.
panelink session input claim --target-ref-file reviewer-target.json \
  --expected-owner-revision "$expected_owner_revision" --lease-ms 30000 --json
# Parse the claim response's result.input_owner_revision into owner_revision.

# Send one explicit instruction to the reviewer. This is one non-replayable
# input operation, tied to both the exact occupant and owner revisions; it does
# not wait for success or copy output automatically. Short harmless prompts can
# use inline --text; this example reads a bounded prompt file instead.
panelink pane input submit --target-ref-file reviewer-target.json \
  --expected-owner-revision "$owner_revision" \
  --text-file review-prompt.txt \
  --json

# Wait for new terminal output after a cursor, then inspect it explicitly.
panelink pane watch --target-ref-file reviewer-target.json \
  --after out_90 --timeout-ms 120000 --json
panelink pane read --target-ref-file reviewer-target.json \
  --source live-scrollback --lines 80 --json
panelink session input release --target-ref-file reviewer-target.json \
  --expected-owner-revision "$owner_revision" --json
```

The caller must parse and use the revision returned by `session.input.claim`, not assume a constant; `$owner_revision` is set by the agent runtime in this example. All CLI reads, watches, waits, and mutations use the exact saved `TargetRef`. Convenience names require an explicit `--resolve-current` read first; the caller must save that returned reference and pass it to the subsequent command. If a replacement session occupies the pane between discovery and use, Panelink returns `target_stale` without reading or dispatching input to that replacement.

Once M1 has a verified provider adapter, `panelink agent submit <verified-agent-id> ...` and `agent wait --until <exact-state>` can be convenience aliases over the same session target. They must return the bound `session_id` and binding revision. A wait for `idle` only reports readiness; the caller must inspect findings or use a separately defined task result. Panelink does not infer that the review succeeded, send the result to another agent automatically, or schedule follow-up work.

## Proposed validation plan

These are future acceptance tests, not tests run for this document:

- **Identity and targeting:** use two panes with distinct sessions and one pane replacement; prove list/get return unique server-issued IDs and an old `TargetRef` fails without reading or sending input. Move/re-layout a pane without changing its managed session. Reject wrong server generation and stale occupant revision. Prove no test uses PID, title, focus, or “newest” resolution. Exercise `--resolve-current` as a separate opt-in read, then verify the exact returned reference is required by the follow-on command.
- **Read isolation:** read a screen containing OSC 52 and other control-sequence canaries. Verify the response is safe decoded content, no clipboard access occurs, and saved history/checkpoints/provider conversation APIs remain separate. Bound line and byte page limits before allocation.
- **Freshness/resync:** read snapshot and cursor under concurrent output; inject event loss, slow writers, cursor from a previous generation, and a cursor older than retention. Each case returns explicit stale/resync status and a complete inventory or bounded fresh screen, never false-current partial data.
- **Input admission/retry:** validate all keys before admission; test text-without-Enter, atomic paste, and submit. Lose acknowledgments before and after the PTY write, disconnect mid-request, and force partial writes. Observe no automatic replay, no duplicate dispatch, redacted diagnostics, and truthful `Applied`/`Partial`/`Unknown` outcomes.
- **Input ownership:** race two agent contexts and the TUI for one session. Assert owner revisions use compare-and-swap; only one writer is active; UI bytes and agent bytes never interleave; a takeover waits for drain or marks `Partial`/`Unknown` and discards pending suffix; owner expiry cannot revive/replay old input; and keys typed before UI ownership are not buffered. Workspace-granted claims need no per-keystroke popup.
- **Control/lifecycle:** distinguish detach, pane close, graceful session stop, escalation, interrupt, and server stop. Verify explicit target and capability checks; a layout close cannot silently stop a process. Confirm stop escalation authorization is fixed at admission, permitted escalation executes at the selected deadline without a second popup, and a denied escalation is never issued.
- **Waits and agent state:** test match after cursor, already-matched state, timeout, process exit, identity switch during wait, stale provider events, and `unknown`. Verify no wait treats `idle`, readiness, or arbitrary output as task success.
- **Access and confirmation:** within a trusted workspace grant, read/input requires no per-keystroke confirmation. Cross-workspace access, launch, and terminate follow their separate grants. Confirm docs make same-UID non-isolation clear and clipboard remains denied.
- **Reusable caller context:** issue repeated and concurrent CLI calls with the same context and verify one lease is reused, sequence allocation is atomic and strictly increasing, gaps are accepted, and concurrent duplicates are deduplicated. Test process restart, local context loss, TTL expiry, revocation, server generation change, and 4,096 lease-record exhaustion; failures reconcile or stop rather than reset sequence state or auto-restart the server. Assert the cache contains no request/prompt/environment payloads and that context locator is not treated as authentication.
- **Bounds:** exercise the 65,536-byte operation/request cap, 1 MiB environment policy choice, 2 MiB snapshot payload plus 24-byte header, output queue capacities, per-connection/global event caps, and slow-client disconnection. Capacity rejection must not mutate a ledger or create an effect.
- **CLI/socket parity:** compare one operation through CLI and direct socket and assert identical typed result/error semantics, request correlation, exit behavior, and retry disposition. Include malformed frame/schema, unknown required operation, incompatible major version, and capability negotiation.

## Open decisions and recommended defaults

| Decision still open | Recommended starting default | Why it remains a proposal |
| --- | --- | --- |
| Control scope | Workspace-level collaboration by default; explicit grant for cross-workspace targets. | The user has confirmed cross-pane agent control, not whether every agent can access every workspace. |
| Per-action access | Read and terminal input under one workspace collaboration grant; launch and termination use separate capabilities. No confirmation per keystroke or at an already-authorized stop escalation deadline. | The account-level same-UID boundary cannot isolate individual agents. Firm review rules are unknown. |
| Target and aliases | Require the full returned `TargetRef` for every agent-facing read/watch/wait/mutation. Permit alias lookup only as an explicit `--resolve-current` read, then require its returned ref. | This avoids replacement-occupant retargeting; the token and tuple schema still need protocol review. |
| Input ownership | One finite 30-second session input lease, expected-owner-revision CAS on claim/release/handoff, workspace grant for prompt-free claim, explicit UI “Take control”, drain-or-mark/discard before handoff. | Whether UI owns new/attached sessions by default and the exact lease/drain limits need prototype evidence and parent review. |
| CLI context and sequence | One stable caller context per automation client; reuse one server lease (1,800-second TTL proposal), with cross-process atomic client-side sequence allocation and server-side ledger validation. Persist only nonsecret context metadata. | The client-chosen sequence contract and cache-loss reconciliation need typed protocol review. Context locator and same-UID check are not per-agent authentication. |
| Stop escalation | A distinct stop capability/policy authorizes a specified grace/deadline and allowed escalation when the stop operation is admitted. No second popup at escalation time. | Which escalation signals are allowed by default and how the user configures the grant remain open. |
| Agent identity | Use generic pane/session APIs first; add provider agent aliases only after exact identity and switch evidence. | OpenCode is not installed in the observed shell and M1 identity capture is not implemented. |
| Task assignment | Begin with explicit one-shot `submit`; no task scheduler or automatic output forwarding. | The pending OpenCode control/delegation decision is distinct from this low-level API request. |
| MCP | Optional later adapter that calls the same typed API; no direct PTY control and no dependency assumption. | No MCP runtime or dependency is selected. |
| Read sources | Live sanitized screen and bounded live scrollback first; persistent history/checkpoints later and separately. | Retention limits and history/storage format remain unresolved. |
| Multiple controlling clients | One writer owner per managed session for the first proof; observers are separate, and takeover uses explicit owner-revision CAS. | The existing plan leaves release multi-controller policy open; the proposed lease/handoff policy needs live UI tests. |
| Large launch environment | First server API prototype: complete encoded launch request must fit65,536bytes; fail before admission when it cannot. Probe keeps its separate1MiB cap. | Parent selected the smaller initial API cap; staging remains a future reviewed extension, not a runtime feature. |
| Snapshot queue | Prefer a reviewed queue budget with explicit 24-byte frame overhead and control-response reserve; consider chunks only if measured memory requires them. | Current provisional queue limits are not large enough for a maximum snapshot frame. |

## Evidence and validation status

Completed in this planning task: read the required Panelink product and Phase 3 documents; inspected official Herdr documentation; drafted this independent proposal; confirmed the current protocol-envelope/request/snapshot limits and current Phase 2 probe environment bounds from repository files. No source implementation, schema, dependency, test, runtime compatibility, or security isolation was added or validated. Herdr was researched from its official web docs only; none of its software was installed or executed. The proposed tests above have not been run.

Primary Herdr references:

- [CLI reference](https://herdr.dev/docs/cli-reference/) — shared CLI/API access, pane operations, read sources, and input examples.
- [Socket API](https://herdr.dev/docs/socket-api/) — local socket request/response model, method families, snapshot, event subscription/resync, reads, and waits.
- [Agent automation](https://herdr.dev/docs/agent-automation/) — pane-versus-agent distinction, ID capture, explicit start/submit, and wait semantics.
- [Concepts](https://herdr.dev/docs/concepts/) — workspace, pane, agent, and client/server ownership concepts.
- [Agent skill file](https://herdr.dev/docs/agent-skill/) — Herdr's intended in-pane CLI discovery for agent users.

Panelink references:

- [Working specification](SPEC.md) — session identity, input/retry, environment, history, and clipboard contracts.
- [Phase 3 protocol plan](progress/P3-PROTOCOL-PLAN.md) — current socket, ledger, queue, and staged runtime proposal.
- [Approved wire-envelope task](progress/P3-WIRE-TASK.md) — exact opaque frame layout and kind caps.
- [Global ledger task](progress/P3-LEDGER-TASK.md) — exact ledger ceilings and retry/expiry behavior.
- [Roadmap](../ROADMAP.md) — current phase and ownership.

## Parent review and next integration

Parent reviewed the official-source research and required full target references, finite input ownership/CAS handoffs, reusable caller lease contexts with serialized sequence admission, and admission-time stop escalation authority. Those refinements are included above as proposed broader contracts. A bounded prototype subset is implemented as recorded in the current checkpoint; trusted-workspace grants, ownership durations and staging/queue choices remain proposed defaults rather than settled release policy. Confirmed R-15 is the ability to discover/read/control managed panes/processes.

The companion [four-way split plan](PANE-LAYOUT.md) for confirmed R-16 is now reviewed as a proposal. The same typed `pane.split` operation must serve the right-click UI and agent CLI, with explicit target/layout revisions. In tiled views, `pane.resize` must not resize a child PTY independently of the accepted layout rectangle; reconcile this schema with the companion plan before implementation. No separate UI-only control path should bypass the shared state owner.

R-16 adds `pane.split --direction left|right|up|down` as the shared TUI/agent durable operation, with exact TargetRef and expected layout revision, returned old/new refs, deduplication before changed-layout preconditions, and separate UI focus/input ownership. Its implementation is layout-gated; the initial flat launch-slot API remains usable before split-tree work. Socket/typed schemas should reserve this capability without advertising it as implemented.

## Initial typed foundation slice — historical checkpoint

The initial `src/control_protocol.rs` slice implemented pure schema1 request payloads for Read/Text/Paste/Submit/Interrupt, with exact borrowed nonzero numeric TargetRef tuples, operation keys and expected input-owner revisions. Its text limits, control-character handling, and read bounds are described in [P3-CONTROL.md](progress/P3-CONTROL.md). This historical checkpoint predates response/session codecs, server dispatch, and the bounded CLI; it is superseded by the current implementation checkpoint at the top of this document. The full R-15 API described above is still not implemented.

The [handshake codec](progress/P3-HANDSHAKE.md) defines Hello/Welcome and capability bits; the runtime now advertises metadata Events when tracking is available. The [macOS peer credential wrapper](progress/P3-PEER-REVIEW.md) checks the connected stream's kernel effective UID before protocol processing. A private-directory pathname listener exists and refuses an existing endpoint; the earlier pathname CLI fixture is operator-confirmed passing outside the sandbox; the expanded live-watch CLI fixture remains an external acceptance check. These prototype facts do not establish release compatibility or per-agent isolation.
