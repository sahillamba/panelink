# Panelink: working implementation specification

Revision: 0.3, 2026-10-17. Status: draft for product discovery and security review planning; not implementation-ready until the open decisions and feasibility gates are resolved. This revision resolves five reviewed contract gaps: command retries, agent identity, history semantics, child environments, and terminal clipboard requests.

## Purpose and requirement authority

Panelink is a Rust terminal multiplexer and workspace runtime for agents and ordinary processes on Apple Silicon Macs. A user runs it inside an existing terminal application and can reconnect from a compatible terminal later. OpenCode is the first supported agent, using its existing terminal interface.

Confirmed requirements come from the user's answers recorded in [discovery](PANELINK-DISCOVERY.md). The design below is proposed where the user has not chosen details. The [research](PANELINK-RESEARCH.md) explains current source evidence and dependency candidates. The historical desktop research is superseded.

## Confirmed product requirements

| ID | Requirement | Observable acceptance |
| --- | --- | --- |
| R-01 | Operate inside existing terminal apps. | Launch from a shell in the agreed host matrix; no Panelink desktop window is required. |
| R-02 | Support Apple Silicon macOS in Rust. | Produce an arm64 executable for the agreed minimum macOS version; document a reproducible build. |
| R-03 | Manage agent workspaces and ordinary processes. | Keep 10 OpenCode sessions plus representative shell/process panes usable; record Panelink resource overhead separately. |
| R-04 | Use the agent's own terminal interface. | OpenCode input, output, resize, interruption, and permission prompts work inside a pane. |
| R-05 | Support local saved layouts/settings/history. | Restart after a normal shutdown and recover saved workspace metadata and retained history within documented bounds. |
| R-06 | Offer recovery after reboot. | Restore retained layout/history; offer exact-ID OpenCode resume only from an acknowledged durable identity mapping, clearly labeled with its verification time. Missing, stale, or ambiguous identity uses explicit manual recovery. Prior processes and uncheckpointed changes are not preserved. |
| R-07 | Provide tmux-style keyboard operation and Herdr-like interaction. | All essential navigation and pane actions are accessible through configurable prefix bindings and mouse controls. |
| R-08 | Provide right-click menus. | In each tested compatible host, right-click on Panelink content opens the appropriate in-terminal menu; keyboard equivalent remains available. |
| R-09 | Copy selected text automatically. | Drag a pane selection and release; clipboard contains exactly the documented selected text without Cmd-C. |
| R-10 | Keep application code original and MIT intended. | No imported competitor implementation/assets; maintain dependency justifications and required notices. Add the license after copyright details are settled. |
| R-11 | Limit third-party dependencies. | Each direct dependency has a concrete saved-effort justification, license, pinned version policy, and transitive review. |
| R-12 | Develop privately before public release. | Source history and branches live in the private [sahillamba/panelink](https://github.com/sahillamba/panelink) repository. |
| R-13 | Support review by the user's firm security audit team and similar workplaces. | Map the actual review criteria to documented architecture, dependencies, data flows, release evidence, and feature controls. Approval remains the reviewing organization's decision. |

Numeric latency, memory, history, and build budgets remain to be established on real hardware. The workload statement is an evaluation target, not a measured result.

## Proposed first-release scope

Project workspaces, split panes, optional tabs/sidebar according to the user's pending layout choice, named sessions/processes, OpenCode and custom-command launchers, status/attention indicators, detach/reattach, retained history/search, context menus, copy mode, and selection copying.

Tabnine is optional CLI compatibility pending an installed supported version and account access. Autonomous task assignment, worktree automation, a custom chat interface, remote runtimes, and existing tmux adoption require an explicit scope decision before inclusion.

## Proposed runtime and terminal ownership

One executable exposes client, server, and command modes. The server owns PTYs and child lifecycles. Host closure affects the client connection; its intended effect on processes is an open lifetime choice. The recommended behavior is a persistent server with explicit stop operations.

Use one terminal engine per managed PTY, with authoritative state in the server. A pane is a view of a stable managed session, not the identity of its child process. Moving a pane or reconnecting must not relaunch the command.

Represent these concepts explicitly:

- Workspace: stable ID, name, project directory, tab/layout roots, saved UI preferences.
- Layout: leaf pane views and split nodes with orientation and relative sizes; optional tab roots.
- Managed session: stable ID, original launch specification, PTY size, process status, terminal state, history identity.
- Launch specification: resolved executable plus arguments, working directory, environment source/profile revision, explicit overrides and unsets, required volatile variable names, and kind (shell/OpenCode/custom/optional Tabnine). Persist nonsecret launch metadata according to the environment contract below.
- Agent identity: managed session ID and launch generation, provider session ID and project identity, parent/top-level relationship, binding revision, mapping status, durable verification time, and status source/freshness.
- Client attachment: server generation, event cursor, selected workspace, viewports, input/resize ownership.

PID reuse cannot establish session identity. Never execute a restored title or terminal output as a command.

### Child-process launch environment

Every new launch receives an explicit launch context from the requesting client or an explicitly selected in-memory environment profile. For a TUI action, that context is the environment of the Panelink client process at attachment; for a CLI launch invoked from a pane/shell, it is the environment of that invocation. The background server's startup environment is not the implicit baseline for later children. Attaching a new client neither changes existing processes nor replaces another client's launch context.

Construct the child environment from scratch, with these layers from lower to higher precedence:

1. Minimal account defaults from the OS: HOME, USER, LOGNAME, and default shell identity.
2. The selected client launch context, subject to the effective environment policy.
3. Explicit user global defaults, then workspace overrides, then one-launch overrides/unsets. An unset removes the lower-priority value.
4. Panelink-owned values for the child PTY and managed session: TERM and supported capabilities, size-related values where supplied, and PANELINK identifiers. Do not blindly copy the host terminal's claimed capabilities or accept caller-supplied PANELINK identity.

Organization constraints apply to the resulting environment and executable; fail with a named policy error on conflict instead of silently changing credentials or using the server's stale PATH. Client environment profiles must remain scoped to that client/explicit launch. A different shell's context is adopted only through an explicit new launch/profile choice. Refreshing a context affects future launches only. Panelink does not automatically source shell startup files to discover credentials; an explicitly launched user shell can follow its normal startup behavior.

Resolve an unqualified executable using the effective child PATH before spawning. Require absolute PATH entries by default; reject empty or relative entries unless explicitly enabled by applicable policy. Absolute configured executables bypass PATH search. Resolve relative launch directories against the requesting client's declared absolute working directory, record the absolute result, and fail if missing/inaccessible. Record the selected executable path and resolved target; never silently choose a different binary on retry. Named-executable launchers use argument arrays, without shell expansion or aliases. Shell evaluation is reserved for an explicitly configured shell-command launcher.

Persist executable/working-directory/profile references, required variable names, and explicitly designated nonsecret overrides. Do not persist the inherited environment, credentials, auth tokens, SSH_AUTH_SOCK, or sensitive overrides in metadata, command journals, or diagnostics. Sensitive launcher arguments use required-at-launch placeholders rather than saved values. Operation deduplication may retain necessary payload equality data in bounded volatile memory; it must not serialize secret-bearing launch payloads to disk. Review rules for classifying explicit persistent values before release; variable names alone do not establish that values are safe.

After server restart/reboot, request a fresh launch context and any required volatile values before explicit relaunch/resume. Re-resolve and show any changed or unavailable executable; require confirmation of the new launch rather than substituting silently. A live reattachment keeps the existing child's environment. Provider-owned credential files remain provider-owned; a fresh Panelink launch context does not guarantee those credentials are valid.

## Proposed implementation boundaries

Start with a small Cargo workspace or modules: `core`, `server`, `client`, `terminal`, `protocol`, `storage`, and `integrations/opencode`. Split crates only when ownership or build boundaries justify it.

Recommended dependency candidates are Ratatui, Crossterm, `alacritty_terminal`, Serde, and serde_json. First evaluate the engine's PTY facilities; portable-pty is an alternative if needed. These are research candidates, not installed or fully audited dependencies. Standard library sockets/threads/channels/files are the initial concurrency and storage tools. An async runtime and SQLite remain optional decisions supported by evidence.

Dependency selection is explicitly unresolved: the user identified firm security audit acceptance as the main concern. Obtain the firm's dependency/execution rules and provide concrete per-dependency review evidence before adopting the list. An installed tmux backend is a separate architecture alternative discussed in research, not an accepted change to this design.

## Proposed security review requirements

This section proposes evidence and controls to make review practical. It does not assert compliance with an unknown firm policy, certification, or a completed audit. Keep the user's personal history/clipboard requirements while supporting policy-driven restrictions where required.

| ID | Proposed requirement | Review evidence |
| --- | --- | --- |
| SEC-01 | Review the complete dependency graph, including build-time components and native code. | Pinned versions/features, lockfile, component/SBOM inventory, licenses, justification, build scripts/procedural macro inventory, and advisory results with dates and dispositions. |
| SEC-02 | Make releases traceable to reviewed source and build inputs. | Source revision, toolchain/SDK versions, controlled build procedure, approved registry or optional vendor inventory, artifact hashes, and signing/notarization evidence where required by the delivery path. |
| SEC-03 | Keep the initial Panelink core local and user-level. | Document Unix socket endpoints, file paths and permissions, executable launches, and environment inheritance. Propose no core internet access, TCP listener, telemetry, self-update downloads, or privileged helper. |
| SEC-04 | Treat agents and their providers as separately governed software. | Installed executable/version and activation method, provider/auth ownership, child-process network behavior, and organization approval status. Do not claim Panelink isolates arbitrary same-user agents or enforces their network policy. |
| SEC-05 | Make saved history and clipboard behavior explicit and configurable. | Capture enable/disable, retention/clear behavior, access permissions, copy-on-selection toggle, and diagnostic-content policy. Do not promise local storage cannot contain secrets or that heuristic redaction removes them all. |
| SEC-06 | Control agent integration and local command authority. | Opt-in bridge activation, correlated identity, bounded/validated messages, peer checks and socket access, no unsolicited global config edits, and no treating terminal output as commands. |
| SEC-07 | Enforce terminal side-effect boundaries. | Deny programmatic clipboard reads and writes in the first release, including while detached; keep explicit user copy/paste separate. Verify terminal query handling, safe history rendering, bounded control sequences, and stale-client handling against the contracts below. |
| SEC-08 | Maintain an ongoing vulnerability/update process. | Document owner/contact, supported versions, assessment and remediation process, approved update distribution, and organization-specific exceptions. A clean advisory scan is evidence of known findings, not proof of safety. |

Offline/locked dependency resolution is supported by Cargo; offline builds also require the approved toolchain, SDK, native dependencies, and build-script inputs. Reproducible binaries are a separate measured property, not an automatic consequence of a lockfile. Consult the [research sources](PANELINK-RESEARCH.md) for the Cargo and NIST references.

Configuration restrictions should be visible in effective settings and cannot be overridden by project config when an agreed organization policy applies. If the firm needs enforcement against local changes, use its approved managed deployment/policy mechanism; an ordinary user-owned file alone does not supply that guarantee. The exact deployment and enforcement model remains open.

No security evidence bundle exists yet because no application dependency graph or release artifact exists. Produce the first architecture/dependency review packet before a broad implementation, and update release evidence when source or dependencies change.

The first feasibility gate must prove the engine works independently of its original GUI, can supervise a PTY correctly, and can deliver renderable snapshots to the terminal client. It must also prove exact OpenCode identity capture/persistence and detect in-TUI session changes, establish environment precedence and executable resolution, and exercise lost acknowledgments and clipboard denials. Terminal queries, palette, cursor/keyboard/mouse modes, wide characters, and alternate-screen checkpoints belong in this gate. If authoritative OpenCode identity cannot be established for a supported version, advertise only manual/last-confirmed recovery for that mode; do not defer this limitation to a later status milestone.

## Proposed local protocol contract

Use a private per-user Unix socket. Restrict directory access and verify peers where supported. A same-user socket is not isolation against every process running as that user.

Each message has a protocol version, type, bounded payload, request identity when applicable, and stable domain IDs. The server has a distinct generation identity on each start. State updates are ordered within that generation; clients detect gaps and request a new snapshot.

### Command identity, acknowledgments, and reconnect retries

Transport request IDs correlate replies; they do not establish whether a side effect happened. Every mutating command additionally carries an operation key `(server_generation, client_lease_id, command_sequence)`. The server issues the lease and clients use strictly increasing sequences for new commands. Retransmission uses the identical operation key, target IDs, preconditions, and semantic payload. Reusing a key with a different payload is `operation_conflict` and never executes the changed command.

Reserve the key and execution state atomically before dispatching any side effect. Concurrent duplicates share that record: return the prior result or `in_progress`, without redispatch. Keep the result and assigned resource IDs until the client receipts that terminal result. A cumulative receipt retires results through a known completed sequence; retain a retired high-water mark so any later use of a retired key returns `result_expired` without executing. Bound unreceipted commands to 1024 per lease initially; backpressure rejects admission before side effects, never by silently evicting deduplication records.

A lease survives client disconnection for 30 minutes, advertised during handshake; an active connection renews it. Expiration retires the lease permanently for that server generation. A new lease cannot reuse old operations. Accepted commands can finish after a disconnect; lease expiry does not retroactively cancel them. Server restart changes generation and rejects every old-generation mutation. No mutation may be transparently translated to a new generation, lease, or operation key.

`operation_status(key)` is read-only and returns `not_admitted`, `accepted`, `applied`, `failed`, `partial`, `unknown`, or `result_expired`, with target/resource IDs and relevant counts. `not_admitted` is safe to assert only while the original lease is valid and the server still has complete sequence/admission knowledge. Expired/retired keys or lost server state do not prove nonexecution; return `unknown`/`result_expired` as appropriate. After a disconnect, query operation status and synchronize state before allowing a new command that could duplicate the outstanding action. Give unresolved actions a visible “outcome unknown” state.

Acknowledgments mean:

- `accepted`: validated and reserved/queued in the current server; not proof of completed execution or durable storage.
- `applied`: the defined side effect finished. Workspace/layout mutations require committed metadata. Launch success requires the assigned managed-session record, successful child spawn, PTY ownership, and committed launch-result metadata. This reports launch, not agent readiness or task success.
- `input_written`: the effect detail of an applied input operation, with a count accepted by the PTY write interface. This does not prove the child read them or that a shell command completed. Partial writes report the count and `partial`; do not resend a guessed suffix.
- `failed`: failure before side effects only when the server can prove none occurred. Otherwise record partial/unknown outcome rather than implying a safe fresh retry.

Persist a stable managed-session ID and launch intent before spawning, then the launch result after ownership is established. A crash between OS side effects and metadata commit remains an uncertain outcome; filesystem records and PTY writes are not one atomic transaction. Restored launch intents are reconciled/marked interrupted or unknown and never executed again automatically. Same-generation deduplication is not an exactly-once guarantee across server crashes. Do not durably log raw keystrokes, paste contents, or secret-bearing request payloads for retry purposes.

| Operation class | Automatic reconnect behavior |
| --- | --- |
| Read-only list/status/snapshot/history queries | May be reissued, with a new observation timestamp/generation. |
| Create/split/launch/explicit resume and durable workspace mutations | May retransmit only the identical key/payload to the same server generation while the original lease remains valid. Deduplicate before reevaluating state preconditions. Never create a replacement key or auto-rerun after server restart. |
| Absolute desired-state updates such as viewport size | Reconcile attachment ownership and current state first; the newest desired size can then be issued under current ownership as a new observation-based update, not a replay/translation of the old mutation. Relative resize/move actions do not qualify. |
| Keystrokes, paste, mouse actions sent to a child, interrupt/terminate signals, approval answers, clipboard side effects, commands submitted to an existing shell, or future agent prompts | Never resend automatically after connection loss, including with the same key. Query outcome where available and require a fresh explicit action if the user still wants it. Creating a managed shell-command process uses the launch row above; submitting command text to an existing process uses this row. |

Discard unsent buffered user input on disconnect; do not flush an old paste after reattachment. Finish or explicitly account for already accepted input before handing input/resize ownership to a new attachment. Event replay supplies observations only and can never redispatch the commands that produced them.

The first protocol draft should define these operations and their failure behavior before client/server implementation diverges:

| Area | Operations |
| --- | --- |
| Connection | Hello/capabilities and leases, attach, detach, snapshot, subscribe/resync, operation status/result receipt, error. |
| Workspaces | List, create, rename, activate, close. |
| Panes | Create shell/OpenCode/custom, split, move, resize, zoom, rename, close. |
| Processes | Status, interrupt, terminate, explicit server/session stop. |
| Terminal | Input bytes, paste, mouse, viewport/scroll, screen updates, mode updates. |
| History | Page/search retained content, select/copy text, clear/export according to settings. |
| Integration | Correlated OpenCode session/status/attention/error events and supported resume metadata. |

Represent terminal byte data losslessly; do not assume all output chunks are complete UTF-8 strings. Keep presentation updates bounded. A lagging client can lose superseded rendering deltas only if the protocol requires resync; emulator input must remain complete. Avoid unbounded queues and repeated full-history transfers.

For the first release, recommend one interactive attachment owner for a workspace/session, with explicit takeover rather than two clients racing input and resize. A read-only observer can be a later capability. Ownership granularity remains a protocol decision.

## Proposed interaction contract

Default prefix: Ctrl+B, configurable. Ctrl+B twice forwards a literal Ctrl+B. Provide discoverable help and menu actions. Binding names and exact split shortcuts will be finalized after the layout choice; essential actions include create/split/focus/resize/zoom, workspace switching, history/search/copy, detach, and explicit stop.

Mouse hit testing distinguishes sidebar, tab strip, pane border, pane content, menu overlay, scrollbar, and resize handles. Overlay actions must not leak input into the underlying process.

Right-click opens an original context menu with only applicable operations. Initial proposals: new shell, start OpenCode, split right/down, rename, zoom/unzoom, move, copy, paste, close. Closing a live process must use explicit stop semantics and a confirmation appropriate to that action; ordinary detach needs no stop confirmation under the recommended lifetime policy.

Selection starts in one pane, stays within its content, supports wrapped lines and Unicode cell widths, and copies after mouse release. Preserve selection across ordinary output where possible; resize or buffer changes must have defined behavior. Recommend a snapshot of selected text at copy time with stable history coordinates, clearing selection on resize or screen-buffer switch in the first release. Keyboard copy mode must work while output continues.

Copy/paste should use macOS clipboard commands from the local attached client without interpolating text into shell code. Clipboard errors are visible and do not exit the session. Forward paste with the active pane's bracketed-paste semantics. Do not blindly forward host input escape sequences to a pane with different terminal modes.

### Clipboard authority and terminal requests

Clipboard actions initiated by the user are separate from requests emitted by a program inside a pane. The proposed [Alacritty event API](https://docs.rs/alacritty_terminal/latest/alacritty_terminal/event/enum.Event.html) exposes both clipboard store and clipboard load events; the latter can return clipboard data to a child PTY. Engine capability is not permission to perform either action.

| Source | First-release behavior |
| --- | --- |
| User finishes a drag selection or invokes Copy | Copy selected text under the effective copy-on-selection/clipboard policy. Only the attached local client accesses the system clipboard. |
| User invokes Paste | Read the local clipboard for the explicitly focused/input-owned session and send one user-authorized paste. Deny if no valid attachment/ownership or policy disables it; never queue for a future attachment. |
| Child emits OSC 52 or equivalent programmatic clipboard read | Deny by default and throughout the first release. Do not invoke a host read, pbpaste, or a load callback with real clipboard data. For a supported query return a protocol-valid empty response, bounded/rate-limited; ignore unsupported requests. Do not forward the query to the host terminal. |
| Child emits a programmatic clipboard write/clear | Deny throughout the first release. Do not change the clipboard, emit an OSC clipboard write to the host, or show a consent prompt triggered by arbitrary output. |
| Programmatic request while detached, during reconnect, in saved history, or received from a non-owning client | Same denial. Drop the side-effect request without deferred execution or prompts; do not replay it on attach. |

Explicit user Copy/Paste authorization never enables future programmatic reads or writes. A future opt-in programmatic-write feature requires a separate reviewed contract; it is not included by adding an engine flag. Apply limits to control-sequence size and request rate before expensive allocation or host actions, and do not log clipboard contents in denial diagnostics. Initial denial behavior must be verified while the child continues producing output and across attach/detach transitions.

Mouse-reporting inner applications need a defined pass-through gesture or per-pane mode so Panelink selection does not make editors unusable. Specify this after checking the chosen host terminal's modifier behavior. A host can consume a right-click before Panelink sees it; document setup and a keyboard menu fallback.

## Proposed OpenCode integration

Phase one launches the installed executable with the explicit child environment and preserves provider-owned authentication. Missing executable or invalid directory produces a local actionable error, without silently installing tools or replacing the shell.

Reliable identity capture is an M1 requirement for the supported managed OpenCode mode. Rich status/attention reporting can arrive later. A bridge failure must not kill the user's agent, but it must revoke verified identity/readiness and degrade recovery promises explicitly. Users who disable the bridge or use an unsupported version can still use a generic terminal pane, with reduced recovery capabilities displayed.

### Durable provider-session identity

The [OpenCode CLI](https://docs.opencode.ai/docs/cli/) accepts an explicit session ID for resume. That flag does not identify which session a particular pane currently displays. The [plugin documentation](https://docs.opencode.ai/docs/plugins/) lists session events, but a session-created/updated event alone is not proof of the TUI's active selection.

The selected adapter must establish an authoritative binding between managed-session ID/launch generation, canonical project identity, and the exact current top-level OpenCode session ID. Store binding revision, verification source/time, supported provider version, and state (`unbound`, `pending`, `verified`, `stale`, `ambiguous`, or `unavailable`). Child/subagent activity cannot overwrite the displayed top-level binding. Never infer identity from pane title, process PID, newest session in a directory, or latest activity timestamp.

On initial launch/resume, treat identity as pending until the adapter obtains the exact selected ID and verifies the expected project/session. Commit that mapping atomically with the managed-session record before acknowledging `identity_saved` or advertising verified resume eligibility. Disk failure leaves the agent alive with an explicit “identity not saved” indicator. If using an explicit pre-created/resume ID, still verify the displayed binding; providing a launch argument does not prove the session cannot later change.

On an in-TUI session switch, fork, new conversation, or project switch, the adapter must report the new selected top-level identity with ordered binding revisions. Mark a detected transition pending, persist the new binding, then acknowledge the new revision and expose it as verified. Reject stale/out-of-order updates from old launches. Heartbeats must confirm the selected identity, not just bridge liveness. On event gaps, heartbeat expiry, or inconsistent observations, mark the mapping stale/ambiguous until a fresh authoritative reconciliation is persisted. Fix the heartbeat interval and expiry in the versioned adapter contract before M1 is complete.

The mapping acknowledged as saved is the recovery boundary. A crash before a switch update is durably acknowledged can leave only the previous verified mapping. After restart present it as “last confirmed conversation” with verification time, not as guaranteed current-at-crash identity. A claim of current-session recovery additionally requires the supported adapter to demonstrate an ordering barrier for session selection and durable identity recording. If the provider cannot supply or reliably observe all switches, disable that claim for that mode. An optional plugin or a periodic heartbeat alone does not prove the barrier.

Missing/stale/ambiguous identity uses a visible manual recovery flow: retain the pane layout/history; offer a provider-owned session picker or explicit user-supplied ID and project; validate that choice through a supported provider interface; save the chosen mapping before a new explicit resume. Never silently fall back to `--continue`, choose the directory's latest session, start a replacement agent, or retry an uncertain resume with a new operation key. Deleted/inaccessible provider sessions produce an actionable error and preserve Panelink history. Resume starts no new prompt or automatic work merely because an old conversation was reopened.

M1 must demonstrate two concurrent sessions in the same project, initial identity persistence, idle in-TUI switches, new/forked sessions, stale events, bridge disable/failure, and crashes before/after mapping acknowledgment. If the chosen provider version/integration cannot support the required identity contract, record that limitation and use the explicit last-confirmed/manual fallback before calling recovery supported. No undocumented TUI event or unsupported interception API is assumed to exist.

Process states (starting/running/exited/failed) and agent states (unknown/working/needs-input/idle/error) are distinct. Idle or turn-ended is not proof the user's work succeeded. Exited processes cannot continue to appear working. Reconcile pending permission events and clear stale states on disconnect/error; preserve evidence/freshness for unknown states.

Keep approvals and model/account choices inside OpenCode. Resume must target a specific supported saved session ID; do not use an ambiguous “continue latest” when several agents share a project. Server API integration is an alternative if the plugin path does not meet reliability requirements, with its dependency and lifecycle costs recorded.

## Proposed storage and recovery contract

All Panelink metadata and retained terminal history stay on the local Mac. Recommended baseline: a per-user application data directory with restrictive permissions, versioned JSON metadata, and separate bounded history segments. Exact location, line/byte/age limits, and rotation policy remain engineering decisions to document.

### Meaning of saved history

Panelink retains a bounded reading/search record, not a recording of every terminal frame or a provider conversation database. Distinguish these data types explicitly in storage and UI:

| Data type | What is retained and recoverable |
| --- | --- |
| Normal-buffer scrollback | Text/rendition rows that actually enter the engine's normal scrollback, with stable record IDs and wrap metadata. Search covers only retained rows. Text overwritten or erased before reaching scrollback is not automatically preserved. |
| Screen checkpoints | At most the latest committed visible normal-screen checkpoint and latest committed alternate-screen checkpoint per managed session, tagged with dimensions, buffer identity, capture time, and output sequence. Preserve the last alternate checkpoint when returning to the normal screen; do not treat alternate output as an append-only transcript. |
| Live emulator state | Complete in-memory state while the server is healthy, used for live reattach. Disk screen checkpoints are read-only recovery views, not full process/emulator checkpoints from which arbitrary TUIs continue executing. |
| Agent conversations | Owned and retained by OpenCode according to its configuration. Panelink stores verified session references and relevant metadata only; conversation resume/search depends on that provider. Terminal search does not imply access to every agent message, tool result, or conversation turn. |

Separate normal/alternate buffers are a terminal compatibility fact; see [xterm's buffer API](https://xtermjs.org/docs/api/terminal/interfaces/ibuffernamespace/) as an illustrative reference, not a Panelink engine choice. The selected engine's exact normal/alternate history behavior must be proved in M1.

Proposed checkpoint cadence: capture changed visible screens at a target interval of at most five seconds during ordinary operation, plus on detach, orderly shutdown, and before an alternate buffer is discarded on exit/reset. Persist only when capture is enabled, coalescing intermediate frames. This is a target cadence, not a crash-loss bound; recovery shows the last successful durable checkpoint time and any known gap after it. A capture/flush failure is visible without stopping PTYs.

After reboot, history search covers retained normal scrollback plus text in those latest committed screen checkpoints, clearly labeled by source/time. Search results from checkpoints may repeat text in scrollback and must be distinguished. Earlier versions of overwritten screen cells, older overwritten checkpoints, transient progress/status text, and unsaved frames cannot be recovered unless their text separately entered retained scrollback. Do not merge alternate-screen redraws into fabricated conversation lines. Continuous full-screen recording is outside the first-release scope.

History records contain normalized text and explicitly supported rendition/cell data, not executable terminal escape sequences or raw input logs. Render recovery views through Panelink's cell renderer; no stored OSC clipboard commands, query replies, or process inputs may be executed during search, copy, or restore. Expose retention/truncation and capture-disabled indicators. Clearing a session's saved history removes its retained rows and checkpoints under the retention/deletion policy, without changing OpenCode's own conversation store.

Use one metadata writer, atomic replacement with a documented sync/recovery protocol, generation IDs connecting metadata to history, and handling for partial final history records. Storage failures must preserve running PTYs and display a visible persistence error. Disable or reduce history recording per session through settings; clearing Panelink history must not delete provider conversation databases.

Live reattachment rebuilds the screen from server state and continues existing processes. Reboot/server-loss recovery opens retained history read-only and restores the layout, with explicit resume/relaunch actions. Historical bytes are never blindly replayed to a host terminal or shell as active control instructions. Store a safe history representation; terminal escape sequences may carry side effects such as clipboard operations.

Do not automatically rerun arbitrary prior commands. Mark normal process relaunch separately from supported agent conversation resume. Store provider session IDs without copying credential stores.

## Development milestones and exit evidence

| Milestone | Deliverable | Exit evidence |
| --- | --- | --- |
| M0 | Resolve focused product questions and source hosting. | Decisions recorded; dependency policy and host matrix clear. Repository blockers documented until resolved. |
| M1 | One managed shell/OpenCode-compatible pane with background lifetime, reconnect, and a proven identity contract. | Full-screen input/resize/queries; lost-ack deduplication and input nonreplay; host termination/reattach; authoritative identity capture, session switching, durable binding acknowledgment and honest recovery fallback; environment isolation; clipboard denials and screen checkpoints. |
| M2 | Workspace/tab/split model and keyboard navigation. | Several processes continue while switching, moving, resizing, and zooming panes. |
| M3 | Mouse selection, copy, context menus, and history search. | Clipboard and mouse routing work in agreed hosts, including ongoing output and Unicode selection. |
| M4 | Durable local layouts/settings/history and explicit recovery. | Partial writes, bounded scrollback/checkpoints, checkpoint gaps, search scope, server restart, and explicit reboot resume obey the defined guarantees. |
| M5 | OpenCode status/notifications layered on the identity contract proved in M1. | Real versioned activity/permission events map truthfully; missing bridge and provider failures revoke readiness without losing the user's running terminal. |
| M6 | Personal alpha and release preparation. | Representative 10-agent workload, resource/build measurements, installation, notices, and upgrade behavior documented. |

The validation plan should include process cleanup, terminal restoration after client failure, protocol ordering/resync, history rotation/recovery, wide-character selection, and clipboard behavior across the supported hosts. These are proposed checks for later implementation. No tests or benchmarks have been run for an app that does not yet exist.

### Acceptance scenarios added by contract review

| ID | Required scenario and outcome |
| --- | --- |
| V-RETRY | Lose a launch acknowledgment after spawn; reconnect/retransmit the same eligible key and observe one child and the same session ID. Concurrent duplicates do not redispatch; changed payload conflicts; expired/retired keys and new server generations do not execute old commands. |
| V-INPUT | Lose acknowledgments before/after complete and partial PTY writes; no keystroke, paste, signal, or mouse input is automatically resent. Status counts describe write-interface acceptance only. Buffered unsent input is discarded. |
| V-IDENTITY | Run two OpenCode sessions in one project, switch/fork conversations while idle, and inject stale events/bridge loss/storage failure. Only acknowledged verified mappings become resume candidates; every unsupported or uncertain case visibly falls back to last-confirmed/manual selection. |
| V-HISTORY | Alternate-screen content changes from A to B between checkpoints, then the server stops/reboots. Recover only committed checkpoints/normal scrollback; report that unrecorded overwritten A is unavailable. Search and restore cause no stored terminal side effects. |
| V-ENV | Start the server from shell A, attach from B with a different PATH/auth/SSH agent, and launch explicitly from B. Only B's selected context plus documented overrides applies; existing A children are unchanged. Reboot requires fresh volatile values and detects changed executables; no secrets are persisted. |
| V-CLIPBOARD | A child emits clipboard read/write requests while focused, unfocused, detached, and reconnecting. Reads never return system clipboard contents; writes never alter it; attachment never releases queued clipboard actions. Explicit user Copy/Paste still follows its separate policy. |

These are acceptance requirements for future implementation, not completed executable tests or evidence that the provider identity capability exists.

## Open decisions before implementation

User choices: primary host terminals; OpenCode control versus delegation; lifetime/stop behavior; workspace/sidebar/tab hierarchy; remote/tmux scope; firm review criteria, OpenCode approval, allowed dependencies, and history/clipboard policy. The private repository destination is confirmed as [sahillamba/panelink](https://github.com/sahillamba/panelink).

Engineering choices after prototype: pinned dependency versions/features, PTY API, viewport/delta schema, terminal capability baseline, minimum macOS, data format/retention limits, input ownership, mouse pass-through gesture, OpenCode adapter/version matrix, and numeric quality budgets.

Every future task must identify owned modules, required protocol changes, observable behavior, failure handling, and documentation updates. Do not turn pending options or prototype assumptions into confirmed user requirements. Do not begin full application implementation from this draft without resolving its required choices.
