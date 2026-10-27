# Panelink: working implementation specification

Planned UI/UX direction (2026-10-27): [the consolidated plan](UI-UX-PLAN.md)
records six interview rounds selecting smooth anchored history, continuous
scrollbar updates, focus-preserving gestures, full tab controls, managed new
shells, button confirmations and a last-tab empty state. Four panes remain;
no sidebar, pane drag or zoom expansion. Final shared-understanding confirmation
is pending. This is future design intent; the implemented prototype contracts
and evidence below remain current until their respective changes are delivered.

Revision: 0.27, 2026-10-27. Status: draft for product discovery and security review planning; not implementation-ready until open decisions and feasibility gates are resolved. Bounded prototype checkpoints record current code without converting assumptions into product requirements. This revision combines four-pane layout and agent CLI work, four-pane relaunch/attach and pane exit status, saved-layout persistence with legacy migration, client IO isolation, tick-driven close, and serialized live-split launch/journal isolation. Reviewed contracts for retries, agent identity, history, child environments and clipboard requests remain in force. Provider resume and full reboot recovery remain unproved.

Development identity evidence updated2026-10-24: pinned OpenCode2.0.32 private
fixtures now also prove an API-created/UI-selected new root, a static nonempty
transcript import and native full-history fork, and a same-client project
round-trip through Ctrl-O. Native forks are top-level (`parentID` absent), with
origin in `fork`; linked children remain distinct and ineligible as top-level
bindings. See [remaining-case evidence](progress/P4-REMAINING-IDENTITY.md).
This is scoped fixture evidence. Production bridge and durable/manual recovery
acceptance remain separate; no current-at-crash guarantee is established.

The broad [agent API proposal](AGENT-API-PROPOSAL.md) remains proposed. A bounded
source implementation now covers shared per-batch UI ownership, additive pane
submit, and recognized foreground-agent list/get/read/prompt/wait commands.
Those additions have bounded private-runtime evidence; broader acceptance
remains open. See
[agent control](AGENT-CONTROL.md) and the proposal's supersession note. The
socket sandbox permission prerequisite remains unchanged.

## Agent feedback corrections — 2026-10-27

Submit honors child-enabled bracketed paste, framing text and placing the one
Enter after the closing marker. The bounded input is admitted atomically, with
unchanged deduplication and no replay of uncertain input. Applied proves a PTY
write; receipts retain `application_submission:unverified`. A private offline
Codex 0.164.0 `/status` check distinguishes raw-stream draft retention from
accepted bracketed submission without a model call.

`pane read` and `agent read` accept `--bottom --lines K` for the last K physical
visible rows, including blanks. Default short reads remain top-oriented;
scrollback remains a tail and cannot combine with bottom selection. The client
selects from one complete bounded snapshot; a byte-truncated prefix is refused
as `bottom_read_truncated`. Codex empty-placeholder detection uses the exact
observed dim style; typed drafts stay unknown and activity/approval priority
remains conservative. State remains an observation rather than task success.

Finite agent waits reuse one authenticated connection and caller-context lock
through their bounded observation loop. Process identity is checked on every
query; a connection failure is reported without reconnecting or replaying input.

Fixed control diagnostics report logical/transport failure stages and pending
operation identities without payloads, credentials or paths. Owner observations
on input errors are labeled before-operation; current ownership is unverified.

`build-info --json` exposes local package, revision, source-dirty state and a
noncryptographic FNV-1a source fingerprint covering application/vendored sources,
manifest/lockfile and embedded assets. It is not an artifact checksum or security
attestation. `server-info --json` separately queries authenticated live server
identity/generation and connection-selected capabilities via strict bounded
schema15 after optional capability256 negotiation. Older peers ignore the
optional bit; no query is sent without selection, and unknown server identity
is never replaced with client identity. See
[feedback implementation evidence](progress/AGENT-FEEDBACK-FIXES-2026-10-27.md).

## Bounded shared input and recognized-agent slice — 2026-10-27

The attached UI does not hold idle-pane claims. Each admitted UI input batch
uses separate operation keys for a short claim, input and release. Release
occurs only after known `Applied` input; failed, partial or uncertain input is
never replayed or released. This coordinates admitted batches, but does not
protect an unfinished human draft between batches. Existing
`pane claim` / `pane release` remain retained ownership; a retained claim blocks
other writers until a known release. The UI and one-shot submit do not steal or
clear retained claims.

`pane submit N TEXT [--json] [--as NAME]` is additive to the existing pane
commands. It submits nonempty printable text (maximum 32,768 UTF-8 bytes) plus
Enter. Claim, submit and release are separate operations with separate keys.
Release follows only a known `Applied` submit. Failed, partial or uncertain
input is neither replayed nor released. If release fails after `Applied`,
delivery is confirmed and release is unconfirmed.

The recognized-agent commands are `agent list`, `agent get REF`, `agent read
REF`, `agent prompt REF TEXT`, and `agent wait REF`. They require negotiated
capability 128 and the schema-14 agent messages, as well as a matching server.
Process recognition uses the native PTY foreground process PID, kernel birth
identity and executable basename `codex` or `opencode`; wrappers remain unknown.
State is a conservative screen-marker heuristic. It does not identify a
provider conversation or guarantee per-prompt completion. Wait observes state
transitions and remains bounded. Old-server compatibility can require a server
restart and UI detach/reattach. Implementation source and this contract do not
establish runtime acceptance.

## Current usability contract, 2026-10-24

Each pane reserves a one-cell margin on all four sides of its outer rectangle.
The bottom-right pane margin displays the client package version, without
changing PTY dimensions; modal notices temporarily cover it. Unpadded saved
recovery views do not show this badge.
The top margin carries pane Menu and close controls; normal-history scrollbars
use the right margin. The shared compositor geometry defines their hitboxes.
Its PTY and snapshots use the inset content size, with a minimum of 5 rows by
20 columns (7 by 22 outer cells). Root limits remain 80 by 240 outer cells.
Pointer coordinates sent to a child are relative to that content rectangle.

Normal-screen wheel scrolling is a bounded read-only history query pinned to
an exact target. It uses a separate viewport and hides the history cursor;
typing returns to live output. Alternate-screen applications receive typed
wheel input only when mouse reporting is enabled, through the same ownership,
deduplication and nonreplayable admission rules as keyboard input. Without
mouse reporting, alternate-screen scrolling stays at live output and reports
that limitation. Querying scroll mode never claims input ownership.

Scroll responsiveness update (local 0.1.2): ordinary wheel history queries return
one requested viewport without an eager range probe. An exact target/revision
extent cache is used when available; otherwise range is conservative until a
read-only seek refreshes it. Lower bounds cannot cap oldest-history navigation.
New background polls are deferred while a scroll result is outstanding. These
changes do not remove an already in-flight poll, the render cadence, or child
application latency; alternate-screen input checks are unchanged.

Scrolling presentation update (2026-10-27): pending history reads preserve the
last valid pane snapshot instead of replacing it with a loading placeholder.
Normal-buffer wheel bursts accumulate a bounded row delta while one request
is in flight; the next request uses the completed effective offset. View/target
invalidation discards this pending movement. Alternate-screen child input is
not accumulated or replayed. A vertical scrollbar occupies existing right
padding, excludes selection/copy content, and stays beneath menus. It shows the
last observed history extent (refreshed by a bounded, read-only oldest-viewport
query once per screen revision during scrolling). At live output before that
extent is known, it shows a full thumb. The pointer-controls update adds track
paging and local thumb previews that seek on release, using the read-only
history path. No wire,
retention, capture-loss, clipboard, or reboot guarantee changes.

Agents may explicitly select a durable private caller-context root with
`PANELINK_AGENT_CONTEXT_DIR`. It must be an absolute, UID-owned 0700 directory
without symlinks. The CLI never silently changes roots after an error or an
uncertain operation. This setting supplies writable metadata storage; local
control-socket access remains subject to the invoking agent's permissions.

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
| R-14 | Avoid clearing and repainting the whole screen for ordinary local changes. | Unchanged frames emit no output; local cell/cursor changes update only the affected cells/cursor. First render, resize and invalidated display state may rebuild the frame. |
| R-15 | Provide an easy scriptable API for agents to read and control other managed panes, agents and processes. | Stable targets and machine-readable CLI/local API support discovery, bounded sanitized output reads and explicit input/control operations under a documented access/ownership contract. The full proposed API remains unimplemented; the bounded four-pane CLI/runtime exposes discovery, reads, deliberate input, managed pane run and finite watches/reconciliation. See the [runtime scope reconciliation](progress/P3-RUNTIME-CONTRACT-AUDIT.md). Detailed proposed contracts and acceptance scenarios are in [AGENT-CONTROL.md](AGENT-CONTROL.md). |
| R-16 | Offer Split Left, Right, Up and Down from a pane's right-click menu. | Each action splits relative to the clicked pane and opens one fresh independent terminal session on the chosen side, preserving the original process/session. Use deterministic geometry, stable identity, minimum-size handling and a matching agent/CLI operation. The [layout design](PANE-LAYOUT.md) and [Phase3A](progress/P3A-RIGHT-CLICK-SPLITS.md) track the delivered scoped implementation; the current four-pane prototype has automated private-PTY acceptance, while distinct human four-pane host acceptance remains open. |

| R-17 | Automatically expose newly started background agents/subagents from OpenCode or compatible OpenCode wrappers in a tab named Subagents, so the user can optionally switch there and observe them (requested 2026-10-21). | Exact provider parent/child identity routes each supported worker to the correct workspace; observation never duplicates its execution. See [Phase SA plan](progress/SUBAGENTS-PLAN.md) for proposed read-only UX, version/wrapper feasibility gates and acceptance. This provider feature does not select a project sidebar or change the confirmed Project → workspace tabs → split panes hierarchy. |

Pointer and keyboard usability requirement (user, 2026-10-27): pane divider
resize must have a direct drag gesture, every pane must expose a close control,
normal-history scrollbars must support clicks and thumb dragging, and a visible
menu must make actions discoverable alongside keyboard equivalents. Touch
compatibility depends on the host translating gestures into terminal mouse
events. Native multitouch and a fixed project sidebar are not selected. The
[bounded delivery contract](progress/P5-POINTER-CONTROLS-2026-10-27.md) records
implementation progress and separate host acceptance.

Numeric latency, memory, history, and build budgets remain to be established on real hardware. The workload statement is an evaluation target, not a measured result.

Workload evaluation choice (user, 2026-10-26): ten idle OpenCode sessions
concurrent with ordinary shell output and offline fixture build output, with
no prompts or model calls. This selects the representative measurement
profile; it does not set numeric latency, capture-loss or resource budgets.

## Proposed first-release scope

Projects with workspace tabs and split panes, named sessions/processes, OpenCode and custom-command launchers, status/attention indicators, detach/reattach, retained history/search, context menus, copy mode, and selection copying. A project sidebar remains an unselected presentation option.

Tabnine is optional CLI compatibility pending an installed supported version and account access. Autonomous task assignment, worktree automation, a custom chat interface, remote runtimes, and existing tmux adoption require an explicit scope decision before inclusion.

## Proposed runtime and terminal ownership

One executable exposes client, server, and command modes. The server owns PTYs and child lifecycles. Host closure affects the client connection; its intended effect on processes is an open lifetime choice. The recommended behavior is a persistent server with explicit stop operations. The bounded four-pane runtime owns PTYs in the persistent server and supports explicit stop. Automated client death/reconnect and bounded server-death replacement evidence are recorded in the [closure audit](progress/P3-CLOSURE-AUDIT.md); broader product lifetime choices and host acceptance remain distinct.

Use one terminal engine per managed PTY, with authoritative state in the server. A pane is a view of a stable managed session, not the identity of its child process. Moving a pane or reconnecting must not relaunch the command.

Represent these concepts explicitly:

Confirmed hierarchy (user decision2026-10-25): **Project → workspace tabs →
split panes**. Each project can expose workspace tabs, and each workspace owns
its pane layout. Tabs are the workspace navigation surface; they are not a
second nested tab layer inside a workspace. Switching a tab must preserve the
server-owned processes in other workspaces. The bounded opt-in `workspace open`
client now shows a persistent row-zero project/tab strip and supports in-client
route switching. Each registered tab routes to an independent existing
four-pane server/storage/context domain. This is a bounded UI slice; full
Phase5 acceptance remains open. Projects are named groups in this slice,
without source-root discovery. Legacy commands with no workspace selector keep
the original default route; only `workspace open` without IDs follows persisted
active selection. Removal requires a stopped, unambiguous runtime and retains
its saved files. Plain `panelink` and raw attach remain legacy/headerless. See
[the routing contract and result](progress/P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md)
and [tab integration evidence](progress/P5-TAB-INTEGRATION-2026-10-25.md).

### Bounded opt-in tab UI behavior

The `workspace open` client reserves row zero for a persistent project/tab
strip. Clicking the project control opens its chooser; clicking a tab requests
that route. Ctrl-B then `w` opens the chooser;
`n`/`p` navigate tabs and `[`/`]` navigate projects. The chooser uses an
immutable catalog snapshot. A refresh does not replace the displayed active
snapshot if that active route has since been removed. This UI only navigates
registered entries; it does not create, remove, or stop projects or tabs.

Only one route preparation may be pending. While it runs, the current route is
shown and child input is frozen. A switch cancels local copy/selection work;
unsent keys are discarded with the existing notice and are never sent to the
new route. If preparation fails, the old attachment remains the actual route.
After a prepared destination is installed, it uses a fresh App and network
attachment. Ctrl-X restores the host terminal before a nonjoining best-effort
detach of the old worker. Saving the selected route is asynchronous and starts
after installation; it may finish after detach, and no rollback or completion
order guarantee is made. See [tab integration evidence](progress/P5-TAB-INTEGRATION-2026-10-25.md)
for current validation and open acceptance.

### Bounded pane operations prototype

The current pane controls use prototype bindings: `Ctrl-B z` toggles zoom,
`Ctrl-B m` opens a pane-location swap chooser, and `Ctrl-B e` opens an
ancestor-divider chooser. These bindings describe the current prototype; they
do not settle the final shortcut configuration.

Zoom is server-authoritative and scoped to one workspace. It resizes the
selected pane's actual PTY to the current root content size while the
underlying split tree and hidden panes remain in place; hidden processes keep
running at their current PTY sizes. Unzoom resizes panes as needed and restores
the exact underlying geometry for the current root size. Swapping exchanges
two exact pane targets' locations in the same tab and does not migrate, replace,
or restart their PTYs or processes. Divider mode lets the user cycle through
ancestor dividers with Tab/Shift-Tab and preview one-cell position changes
along each divider's axis with arrow keys. Enter sends one absolute-position
commit; Esc cancels the local preview.

Layout topology changes are refused while zoom is active; unzoom before
changing the split layout. Swap and divider choosers capture their target and
layout revision. If that target or revision becomes stale, the local mode
cancels instead of retargeting the action. Pane operations are asynchronous:
fixed status messages report results, and an uncertain or unknown result is
not automatically replayed. No pane input is part of these operations.

Pane-operation ratios and zoom are runtime state. They do not establish
physical-reboot restoration, and the saved-observation codec is unchanged by
this prototype. Current bindings and repeat handling are not final user
configuration claims. Typed Enter/Esc repeat and release events are handled
locally; classic streams that expose repeats as indistinguishable press events
cannot provide the same repeat guard.

As of 2026-10-26, the full serial Rust suite passes **1,399 tests, zero failed,
15 ignored** across 75 result groups. After Clippy cleanup and boxing large
worker result payloads, final client checks pass123/0 and the actual outer-PTY
target passes3/0 (tabs, clipboard-free copy, pane controls). Strict all-target
Clippy and all seven supporting checks pass. The separate real four-PTY
protocol scenario passes1/0 and verifies original shell PIDs/state, hidden
output, actual PTY dimensions, deduplication/conflict behavior and unnegotiated
denial. These automated checks do not establish human Apple Terminal,
real clipboard, physical reboot or representative workload acceptance.
M2's broader host/workload gates and Phases3/5 remain open. See
[the integration evidence](progress/P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md),
[the pane contract](progress/P5-PANE-OPERATIONS-CONTRACT-2026-10-25.md), and
[the current handoff](progress/SESSION.md).

- Workspace: stable ID, name, project directory, tab/layout roots, saved UI preferences.
- Layout: leaf pane views and split nodes with orientation and relative sizes; optional tab roots.
- Managed session: stable ID, original launch specification, PTY size, process status, terminal state, history identity.
- Launch specification: resolved executable plus arguments, working directory, environment source/profile revision, explicit overrides and unsets, required volatile variable names, and kind (shell/OpenCode/custom/optional Tabnine). Persist nonsecret launch metadata according to the environment contract below.
- Agent identity: managed session ID and launch generation, provider session ID and project identity, parent/top-level relationship, binding revision, mapping status, durable verification time, and status source/freshness.
- Client attachment: server generation, event cursor, selected workspace, viewports, input/resize ownership.

PID reuse cannot establish session identity. Never execute a restored title or terminal output as a command.

The [killed input-client acceptance](progress/P3-INFLIGHT-CLIENT-RESULT.md)
uses a real CLI process and a test-owned proxy holding a matching terminal
Applied response. It verifies durable same-key status/resync and one handled
token after SIGKILL before caller acknowledgment. This is evidence for the
existing nonreplay contract at the post-completion/pre-ack boundary; it does
not establish death-before-admission or owner-pending behavior.

### Server-death boundary — bounded prototype contract

Client detach/death and server death are separate events. Catchable shutdown
runs the existing bounded driver teardown: signal owned process groups, poll,
escalate and close owned resources. An uncatchable server kill runs no Rust
cleanup or final history/layout flush. The kernel closes the server's PTY
masters, but this does not guarantee termination of HUP-ignoring, background
or detached descendants. No supervisor, crash-loss bound or orphan-free crash
contract is established. Persisted PIDs alone are never cleanup authority.

A later explicit default-open invocation serializes endpoint inspection,
stale-socket removal and startup under its lifecycle lock. It requires the
existing endpoint classification and server-storage lock checks, and starts a
new durable generation. Direct `serve` refuses an existing endpoint; it does
not perform default-open recovery. Old full targets and lease contexts confer
no authority in the replacement generation. Choosing a fresh shell is an
explicit new launch, never replay of stored input or an uncertain prior launch.
Retained history/layout remains distinct from replacement live process state.
The [server-death acceptance contract](progress/P3-SERVER-DEATH-TASK.md)
scopes its child-exit assertion to a cooperative fixture shell, without
promoting it to arbitrary descendant cleanup or reboot recovery.

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

The disposable phase-2 core selects a locally corrected VT100 0.16.2 engine and Rustix/openpty as recorded below. A parser-only local Crossterm extraction is selected separately below. Ratatui, the complete Crossterm host framework, serialization libraries, an async runtime and SQLite remain candidates for later boundaries only when justified. Standard library sockets/threads/channels/files are the initial concurrency and storage tools. No whole UI/provider/storage framework is approved by core selection.

Phase 2's original [dependency review](progress/P2-DEPENDENCIES.md) and [source-gate evidence](progress/P2-SOURCE-GATE.md) reject Alacritty/portable-pty unchanged because of uncapped storage and launch/cleanup conflicts. The [replacement review](progress/P2-REPLACEMENT.md) approves only a bounded prototype core: VT100 `0.16.2-panelink.2` (local source corrections and bounded normal-history cursor with MIT notices/provenance), Rustix 1.1.5 and rustix-openpty 0.2.0. Chosen normal history is 1,024 rows within the experiment's 2,000-row ceiling. Its allocation estimate is conditional on the tested Rust 1.99.1 container behavior and adapter limits. No server environment mutation, custom parser or relaxed allocation contract is authorized. The [host component review](progress/P2-INPUT-REVIEW.md) now selects crossterm-input 0.29.0-panelink.1, a fixed-buffer MIT parser/types extraction from Crossterm0.29.0 with original notices, patch and fingerprints. It excludes the unbounded global readers and all upstream host IO. The pure snapshot renderer is implemented, with conservative scalar-width fallback. Ordinary updates use snapshot differences; wide-cell topology changes repaint their row. The host holds two preallocated snapshot caches (last fully written state and pending target), advances the baseline only after all frame bytes are accepted, and invalidates it after resize/discard. First/invalidated frames use a full repaint. This corrects the operator-reported full-screen reload on every change; see [incremental renderer evidence](progress/P2-DELTA-RESULT.md). A binary-private, one-shot macOS/aarch64 host IO/restoration guard and `pty-probe --terminfo <absolute-directory> -- <executable> [arguments...]` are implemented under the [bounded probe contract](progress/P2-PROBE.md); 139 application tests pass, while nine device-dependent outer-PTY scenarios remain ignored because this sandbox denies the independent `/dev/tty` open with EPERM. An initial operator host run reported four passes and five failures; the retained-session lifecycle and producer fixtures are corrected, and the user subsequently confirmed the reported failures are resolved in [operator evidence](progress/P2-OPERATOR-RUN.md), without supplying a fresh raw log. The user confirms the short redraw/Unicode/editor/resize/paste/Ctrl-C retest worked, but Ctrl-Q triggered a host window-close dialog. Ctrl-X is now the documented local probe quit key (Ctrl-Q retained as alias), with live quit/restoration confirmation pending. Separate clipboard/full-checklist results remain unspecified. [Private-PTY tests](progress/P2-RESTORE-ORDER.md) now observe exact saved termios and visual reset before the ready child finishes shutdown, for explicit cleanup and unwind Drop; this establishes scoped cleanup ordering without claiming Apple Terminal acceptance. The incremental renderer has qualitative operator confirmation from the short retest; no measured latency or broad host-compatibility claim is made. The [signal review](progress/P2-SIGNALS-REVIEW.md) adopts signal-hook0.4.5 with defaults disabled for process-lifetime atomic flags only; no unregister is performed during bounded cleanup. The child is stopped on probe exit; this disposable behavior does not change the product server/client lifetime contract. The [core result](progress/P2-CORE-RESULT.md) records core library behavior; current integration evidence records 139 passing application tests, nine ignored host scenarios and 39 separate decoder tests; [host input review](progress/P2-HOST-INPUT.md) records the separate bounded-input gate. Explicit core shutdown closes the PTY and rejects further IO, while natural child exit permits final-output draining. The [phase 2 experiment](progress/P2-EXPERIMENT.md) distinguishes proposed host checks from completed core checks. Apple Terminal compatibility has not been demonstrated.

Broader dependency selection and firm approval remain unresolved: the user identified firm security audit acceptance as the main concern. The personal core experiment has scoped source/API/build/notice/advisory evidence; obtain the firm's actual dependency/execution rules before a corporate pilot. An installed tmux backend is a separate architecture alternative discussed in research, not an accepted change to this design.

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

A scoped prototype dependency/source/API review packet now exists in `progress/P2-REPLACEMENT.md`, with exact graph, notices, integrity, build-input and dated advisory evidence. It is not a complete application security evidence bundle or a release artifact. Expand the packet before broad implementation/corporate review and update it when source or dependencies change.

The first feasibility gate must use Apple Terminal as the initial host compatibility baseline; broader terminal support follows after this baseline works. Record the tested terminal and macOS versions before claiming compatibility; the minimum supported macOS version remains undecided. Prove the engine works independently of its original GUI, can supervise a PTY correctly, and can deliver renderable snapshots to the terminal client. Also prove exact OpenCode identity capture/persistence and detect in-TUI session changes, establish environment precedence and executable resolution, and exercise lost acknowledgments and clipboard denials. Terminal queries, palette, cursor/keyboard/mouse modes, wide characters, and alternate-screen checkpoints belong in this gate. If authoritative OpenCode identity cannot be established for a supported version, advertise only manual/last-confirmed recovery for that mode; do not defer this limitation to a later status milestone.

## Historical implementation checkpoint — 2026-10-18

This section retains the historical bounded checkpoint; current four-pane and workspace-watch implementation contracts are recorded under the local protocol section and in ROADMAP.md. It does not settle the broader proposed product contracts below. See [INTEGRATION.md](progress/INTEGRATION.md) for the current verification record and [ROADMAP.md](../ROADMAP.md) for phase status.

The macOS/aarch64 prototype combines the bounded wire and typed response/session codecs, kernel same-UID authentication before handshake, a protected private-directory socket listener with existing-endpoint refusal, bounded connection workers and a single `StateOwner`, durable caller contexts and input ownership, and startup ledger admission plus a durable lifecycle-metadata Intent before child spawn. The operation ledger itself is volatile. An explicitly selected one-child PTY remains owned by the foreground server across logical detach and same-generation reconnect. Focused socketpair tests exercise a lost input acknowledgment and prove no client resend. The full agent-control CLI/API, workspace/layout model, and release runtime remain out of scope.

Storage initialization is explicit: `--initialize` applies only to an empty private directory and writes a random nonzero generation seed; normal opens require the existing lock, counter, and journal and advance the counter before use. Missing or corrupt metadata fails closed. This is not a rollback-resistant identity guarantee for copied/restored backups or a collision-free identity guarantee across independent storage domains.

On the current experimental macOS/aarch64 host, production PTY opening now
sets close-on-exec during master/slave creation and fails closed on unsupported
flags. A shared internal gate coordinates managed child spawning with the
non-atomic socket/bind/accept creation windows; socket waits, authentication,
normal IO and cleanup run outside it. This protects cooperating Panelink
paths, not unrelated native callers. Production-path PTY and gate tests pass;
a full-descriptor child probe now passes its positive and negative controls.
The actual accepted-socket/exec race fixture exists but its attempted run stopped
at pathname bind with EPERM. That acceptance remains required before API launch.
The pure schema-6 launch codec is implemented under its
[bounded contract](progress/P3-MANAGED-LAUNCH-CODEC-TASK.md), without dispatch or
authority. The schema-5 session-cursor codec is adopted for isolated library
work at the proposed offsets; runtime negotiation/migration remains pending.
No minimum macOS release floor or multi-session capability is adopted.

The isolated trusted launch coordinator now retains at most two non-reused
session identities, borrows the single global operation ledger, and records
Intent before spawn. Post-spawn journal uncertainty retains the acquired child
and blocks new launches; it does not authorize a retry. The launch preparation
adapter uses only the request's captured cwd/environment plus explicit
server-owned terminal values, without reading the daemon environment. These
are tested library components; authenticated launch dispatch remains gated.
Private reusable grants are now pinned to the complete target selected at
issuance. Resume cannot broaden that scope, handoff requires the same scope,
and another target needs a fresh lease. This conservative prototype rule does
not settle permanent cross-pane permissions. A separate trusted
[two-session library core](progress/P3-MULTI-OWNER-ASSEMBLY.md) now combines
launch, connection, control, status/receipt and bounded polling around that one
global ledger. Its caller holds the storage lock and checks launch authority;
it exposes no mutable registry or ledger. Automated real-PTY tests exercise
two children, isolated reads, duplicate launch, public ownership claims and
nonreplayed input, scoped reconciliation/event pages, and cleanup. The core
now routes the seven existing session requests and negotiates Events when
scoped reads are available with healthy metadata; older grants cannot widen
on resume. Scoped cursors bind generation, session and sequence. The running
socket/CLI server still owns one target; no multi-session endpoint is enabled.

This private-PTY proof is a separately authorized experiment while Phase 2 manual host gates remain pending. The earlier pathname CLI fixture is operator-confirmed passing outside the sandbox; expanded live-watch CLI pathname and host-terminal acceptance remain separate checks. The prototype now negotiates Events when authoritative metadata tracking is available and implements bounded metadata watches plus atomic screen/metadata/cursor reconciliation. That historical one-child checkpoint preceded the four-pane, workspace-event, metadata and live-scrollback slices; provider identity/recovery remains unproved. A same-UID boundary does not isolate one local process or agent from another.

For this prototype, `Applied` on input means all request bytes were accepted through the PTY write interface; for interrupt it means the signal call succeeded. Neither means the child application accepted the input or completed a task. The input adapter's normal drain window is 100 ms, but a PTY hard-error cleanup can add up to two seconds, so this is not a hard 100 ms whole-call deadline. The bounded journal records only operation key, managed session ID, and lifecycle state; it does not persist launch arguments, environment values, or process identity.

## Proposed local protocol contract

Scoped usability checkpoint2026-10-22: the attach client accepts typed Ctrl-letter
keys (CtrlB/X remain local, CtrlC remains Interrupt), Alt printable characters,
F1–F12, Delete/Insert/PageUp/PageDown and BackTab. Schema8 key codes1–12 keep
their bytes; codes13–20 add bounded validated variants as documented in
`src/ui_protocol.rs`. Raw host escape streams remain inadmissible. Unknown key
codes on older servers fail request validation; this is not negotiated
mixed-version support. Modifier combinations beyond the implemented profile,
application mouse pass-through and full host compatibility remain open.

CtrlB then `?` opens a bounded local help overlay, independent of NAV expiry;
Esc/`?` dismisses it and CtrlX detaches. While open, help consumes keys, paste
and mouse without dispatching provider or child actions; rendering stays
incremental. Cleanup discard work is now capped per iteration so a continuously
readable PTY returns to the existing shutdown deadline checks. Wave1 alone does
not resolve the separate owner/client responsiveness findings. See
[wave1 tasks](progress/USABILITY-WAVE1-TASKS.md)
and [integration result](progress/USABILITY-WAVE1-RESULT.md) for exact scope and
checks; no broader UI phase or provider acceptance is implied.

Wave2 checkpoint2026-10-22: active attach socket/context work now has one
bounded IO owner, while the host owns rendering, focus and local controls.
Thirty-two admitted commands/results share a4MiB payload/reservation budget,
including in-flight work; fixed transport/decode and screen buffers are
additional. Durable key reservation still precedes send, and uncertain input
is never replayed. Unadmitted keys remain pinned under overload; CtrlX always
detaches and reports any abandoned unsent batch after restoration. Startup
connection remains synchronous before raw mode, and one IO worker serializes
both panes. Delayed-fsync and complete host-terminal acceptance remain unproved.

Live multi-pane close now admits one transaction, sends HUP and advances exit/
KILL phases on owner ticks. Closing targets reject input/resize; final capture,
collapse and ledger/result completion precede the terminal schema9 reply.
Worker loss does not cancel admitted close. Unknown retains ownership and
disabled input; process exit and PTY EOF remain distinct. Driver shutdown uses
shared child deadlines. Legacy synchronous core APIs remain separate.
See [Wave2 integration](progress/WAVE2-RESULT.md) for evidence
and remaining gates; no general latency budget is certified.

Live-split launch checkpoint2026-10-22: one bounded worker performs launch
preparation, journal reads/syncs and PTY spawn outside the multi-pane owner.
The owner retains authorization, exact-byte ledger admission, reserved target,
layout preview and results. Durable Intent precedes spawn; an acquired child
is attached to the owner's registry before the worker writes Spawned/Applied.
Post-acquisition journal failure retains the child and records Unknown.
Disconnect or reply timeout cannot cancel/repeat the admitted launch. One
active split, one command slot, one completion slot and at most eight finite
reply waiters bound this path. No independent launch backlog is accepted.

Root resize, close and competing topology changes are busy while the preview
is reserved; existing-pane input/read/watch and local detach remain available.
Natural exits stay recorded, including children exiting during finalization;
the owner reconciles topology after the split settles. Worker cancellation
starts at shutdown, with any wait sharing the existing250ms metadata deadline.
Late IO/unclaimed-child cleanup retains singleton exclusion while the process
lives; deadline expiry reports incomplete shutdown and cannot certify cleanup
after process exit. Initial startup and legacy direct-core launch calls remain
synchronous. No wire, dependency, provider or general latency contract changes.
See [the design](progress/ASYNC-LAUNCH-DESIGN.md) and
[implementation evidence](progress/ASYNC-LAUNCH-RESULT.md).

The next bounded notification foundation provides a fixed72byte metadata event
payload and a volatile64event journal with generation-scoped cursors and explicit
expiry. A later bounded runtime slice now delivers metadata Events during finite watch
requests and provides screen/owner/exit/cursor reconciliation in one owner turn.
Events require explicit negotiation, eight pending watches maximum, at most64
events per result and a shared250ms batch-write budget. Persistent subscriptions
and provider/text-condition waits remain unimplemented. See [notification tasks](progress/P3-NOTIFICATIONS-TASKS.md)
and [runtime integration boundary](progress/P3-NOTIFICATION-RUNTIME.md).

Workspace layout notification contract (2026-10-23, bounded prototype):
capability bit 8 is separately negotiated for schema-11 workspace reconciliation
and finite watches. Existing pane Events retain their target-scoped schema.
The owner holds a separate 64-record journal of layout revision invalidations,
including positively confirmed terminal-empty transitions, with generation,
workspace and sequence cursors. Layout snapshot and current cursor are copied
in one owner turn. Invalidations carry no terminal content, launch data or
operation receipts and never execute/retry a mutation.

Read plus capability 8 and a live grant in the requested workspace are required;
retiring that grant's original pane does not change its workspace authority.
Resume cannot widen an existing grant. A different generation/workspace or
future cursor is refused; eviction returns an explicit resync hint and requires
fresh reconciliation. Checked sequence failure disables notifications without
rolling back layout or stopping children; ordinary layout queries remain usable.
Workspace and pane finite watches share at most eight pending requests; waits
are at most 1,000 ms. A complete watch response contains at most 64 events /
1,576 payload bytes, validated before output; reconciliation is 472 bytes.
No persistent subscriber queue or dependency is added.

The agent CLI uses the new path when negotiated, emits `layout_changed` and
explicit `layout_resync` records, and reconciles current topology after changes
or gaps. Derived pane observations can coalesce intermediate layouts; this is
not a complete lifecycle transcript. Pane screen/owner/exit watches continue
independently. `events --pane N` pins the initially resolved full target and
never follows a replacement at index N. Servers/old leases without capability 8
use the documented bounded polling fallback. See
[the accepted contract](progress/P3-LAYOUT-EVENTS-TASK.md) for exact wire sizes,
permissions, failure behavior and acceptance boundaries.

Managed-child metadata contract (2026-10-23, bounded prototype): optional
capability16 and schema12 expose the current direct child's cwd and executable
path through a separate full-target Read-authorized query. Each field is
independently sampled, has a fixed unavailable reason, and includes a shared
generation-relative monotonic sample-start time. This does not identify a
foreground descendant or an atomic process-state pair. No launch-path fallback,
argv/environment query, lossy path conversion, or persistent metadata is used.
Paths must be absolute UTF-8 without control/NUL characters, at most4095 bytes
each; the maximum reply is8262 bytes. PID plus captured OS birth identity is
verified before and after native sampling, and authority/full-target/liveness
is revalidated before publication. A single bounded worker isolates libproc
from PTY pumping; after500ms the owner reports timed_out and rejects new work
as busy until the late query completes. No kernel cancellation is claimed.
See [the accepted metadata contract](progress/P3-PANE-METADATA-TASK.md).

The workspace observer reconnects at25 minutes to renew its same30-minute
lease, requesting Read plus workspace events only and preserving its cursor.
Its original pane may retire without changing workspace authority; a retired
pane resume cannot acquire pane input/events. Three transport-only retries
are spaced15 seconds apart; expiry, generation change or invalid grant fails
closed, with no replacement lease or mutation replay.

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

### Input owner discovery — bounded prototype contract, 2026-10-23

`pane list` exposes each pane's server-observed owner as
`input_owner_info:{kind,id,is_caller,revision,expires_at_server_secs}`. A vacant
owner uses kind none with null ID/deadline and false is_caller. A client owner
uses kind client and opaque ID generation:lease; is_caller compares the
current-generation pane link's reusable lease. Revision and expiry come from
the same exact-target SessionInfo observation. The deadline is on the server
runtime's monotonic seconds clock, not wall time or a client countdown.
Pending input can retain an owner past that deadline; readers must not infer
permission to write or bypass server owner-revision CAS from this metadata.

The older `input_owner` string remains none/you for those cases; another holder
is now client:<lease>, replacing the unsupported agent-role label. Text output
uses neutral client:<generation>:<lease> plus revision and raw server deadline.
No caller name, process identity, UI/human role or provider identity is inferred
or persisted. Same-macOS-user clients remain mutually trusted, not isolated
agents. Pane ownership/metadata samples are point-in-time and not an atomic
cross-pane snapshot. No wire/capability/storage/authority changes are added.
See [accepted implementation contract](progress/P3-OWNER-IDENTITY-TASK.md).

### Managed pane run — bounded prototype contract, 2026-10-23

`panelink pane run [--pane N] [--direction left|right|up|down] [--cwd DIR]
[--as NAME] [--json] -- COMMAND [ARG ...]` creates a fresh managed process via
the existing schema7 split/embedded schema6 launch worker. The prototype
defaults are spatial pane1 and right; this does not settle workspace hierarchy.
The default workspace must already run. Existing INPUT negotiation and
full-target grant rules apply; a separate input-owner claim is not required
and no input is delivered to the anchor. Four-pane/geometry limits remain.

The invoking CLI supplies raw executable/argv, cwd and filtered environment.
Relative cwd resolves against that invocation, and unqualified executables
resolve through its effective PATH. Panelink options end at a mandatory --;
all later tokens are literal child argv. No implicit shell evaluation, daemon
environment fallback, argv/env persistence or new wire/storage format. Bounds
are 80 CLI tokens, 65,536 aggregate argv bytes, 1,024 variables/48KiB filtered
environment and 65,536 bytes for the entire encoded request. Overflow refuses
before key reservation. Returned operation and full target identify creation;
Applied does not claim readiness or task success. An error after key
reservation still returns the operation identity and an explicit uncertain
outcome; null target/zero layout revision mean unobserved, not absence.
Partial/Unknown may retain
a child and must not be presented as safe-to-repeat failure.

Resolve the anchor once, pin its full identity and expected layout revision,
and validate both at server admission. Reserve the durable context key before
send; exact server deduplication stays in force. The CLI never automatically
resends or creates a replacement key after uncertain acknowledgment. A later
invocation may reconcile uncertainty but stops before any new launch in that
invocation. Contexts and journal persist identity/status metadata only.
See [the accepted implementation contract](progress/P3-PANE-RUN-TASK.md).

### Bounded live normal-history reads — 2026-10-23

The existing `LiveScrollback` read source now returns the newest retained
normal-buffer physical rows in chronological order, at most the caller's
`max_lines` and bounded UTF-8 text budget. It excludes visible screen rows and
alternate-screen content without changing the active screen or viewport.
Wrapped rows remain separate; blank/wide cells and safe combining text follow
the screen-read normalization policy. No terminal control sequences are returned.
The engine retains at most 1,024 normal-history rows. The existing response
`truncated` flag covers omitted rows/text and observed eviction within the
current epoch. Reset clears history and begins a new epoch; the flag makes
no cross-reset continuity claim. No extra retained queue, wire schema,
capability, dependency or disk-read fallback is added. Existing same-UID,
full-target, lease and read-capability checks remain authoritative.
See [implementation and verification](progress/P3-LIVE-SCROLLBACK-RESULT.md).

## Bounded pointer controls prototype — 2026-10-27

The attached client renders Menu and close controls in each pane's top margin.
Menu and Ctrl-B s open the same captured-target actions, with viewport-bounded
pages and Back/More/Cancel controls. Right-click retains the split context menu.
Close requires a separate explicit Close click or keyboard y. The last-pane
warning stays visible, and stale layout changes cancel the confirmation.

Divider dragging previews locally and submits one absolute schema-13 request
on release. The menu also exposes divider selection, minus/plus adjustment and
Apply/Cancel controls; Ctrl-B e preserves keyboard operation. Minimum subtree
sizes constrain motion. Escape, focus loss, host resize, stale layout/route and
buffer changes cancel captured gestures; uncertain mutations are not replayed.

Normal-history tracks page and thumb drags preview a position before read-only
seeking on release. Ctrl-B PageUp/PageDown/Home/End supplies keyboard access.
A separate read-only worker command cannot turn a queued seek into application
wheel input if the server changes buffer modes. Alternate-screen application
wheel handling remains separate. Controls share renderer geometry and retain
modal priority; no extra PTY content cells are consumed. Help and local menus
have pointer exit controls, and keyboard copy can return to the pane Menu.

These are terminal-cell pointer controls; touch depends on the host reporting
mouse gestures. Automated private-PTY evidence does not establish native touch
hardware, human Apple Terminal or real clipboard acceptance. See
[plan and results](progress/P5-POINTER-CONTROLS-2026-10-27.md).

## Proposed interaction contract

Default prefix: Ctrl+B, configurable. Ctrl+B twice forwards a literal Ctrl+B. Provide discoverable help and menu actions. The bounded opt-in tab UI uses `w`, `n`/`p`, and `[`/`]` for chooser, tab, and project navigation. The current pane-operation prototype uses `z`, `m`, and `e` after the prefix; these are not final bindings. Other binding names and exact split shortcuts remain subject to the interaction design; essential actions include create/split/focus/resize/zoom, history/search/copy, detach, and explicit stop.

Mouse hit testing distinguishes sidebar, tab strip, pane border, pane content, menu overlay, scrollbar, and resize handles. Overlay actions must not leak input into the underlying process.

Right-click opens an original context menu with only applicable operations. Confirmed split actions are Split Left, Split Right, Split Up and Split Down, relative to the clicked pane. The proposed detailed contract is in [PANE-LAYOUT.md](PANE-LAYOUT.md), prepared independently of the unresolved workspace/tab hierarchy. Other initial menu proposals are new shell, start OpenCode, rename, zoom/unzoom, move, copy, paste and close. Closing a live process must use explicit stop semantics and a confirmation appropriate to that action; ordinary detach needs no stop confirmation under the recommended lifetime policy.

Prototype lifecycle decision (2026-10-20): an authenticated same-UID client may
stop an individual pane without a separate stop grant. This does not confer
launch or input authority. The interactive menu and Ctrl-B then `x` prompt for
confirmation; `panelink close N` is an explicit command. Stop sends HUP to the
pane's process group, waits one second, then sends KILL if still alive and
waits one more second. Exit, escalation, and uncertainty have separate results.
If one pane's shell exits, its live sibling expands to fill the layout and a
later split receives a fresh session identity. The last shell's exit ends the
workspace. This is a scoped two-pane behavior, not a decision about the
broader workspace/agent authorization policy.

Selection starts in one pane, stays within its content, supports wrapped lines and Unicode cell widths, and copies after mouse release. The bounded drag implementation freezes sanitized cells and authoritative row-wrap flags at press, retaining that source across ordinary output. Resize, target/layout geometry changes, displayed history/view changes, buffer switches, modal ownership and lost wrap metadata cancel it. Selected cells use the frozen source for highlighting; other cells keep updating. Empty selections do not copy.

### Bounded keyboard copy mode prototype

The attached client also has a bounded keyboard path for the pane's currently
visible live or history view. `Ctrl-B c` enters when capture succeeds and
authoritative row-wrap metadata is available; this shortcut remains a
prototype assumption, not a settled configurable binding. Arrows or `h`/`j`/
`k`/`l` move the local caret. Space sets or clears the selection anchor. Enter
or `y` copies a non-empty anchored range once and exits; with no anchor it
exits without copying. Esc cancels, while Ctrl-X detaches. Other keys, paste,
and mouse events remain local during the mode.

Copy mode holds its captured cells stable while ordinary live output continues.
Resize, focus, geometry, buffer or visible-view, route, modal, or wrap-identity
changes cancel it. Copy uses the existing attached-client asynchronous
clipboard queue and fixed status messages. This slice adds no arbitrary
history scrolling/search, server message, persistence, or terminal clipboard
access. Typed-protocol Repeat events for Enter/`y` are guarded after the copy
key press; classic streams that encode repeats as indistinguishable Press
events cannot promise the same one-gesture repeat behavior. Focused client
(108 passed), compositor (25 passed), and clipboard-free private-PTY (2 passed)
checks pass after compile/fixture corrections. The keyboard-copy checkpoint,
before the later pane-operation changes, passed the full serial tree with
1,356 tests and 15 ignored. Apple Terminal and real clipboard acceptance
remain open. See the [keyboard copy result](progress/P5-KEYBOARD-COPY-RESULT-2026-10-25.md).

The live-screen/history reply extension negotiates row-wrap metadata separately;
legacy wire replies and persisted checkpoint/history bytes retain their formats.
An old peer without wrap metadata disables drag copying rather than guessing
line boundaries. The attached client submits at most a bounded selection to an
asynchronous `/usr/bin/pbcopy` worker with fixed redacted results and no shell.
Detach cancels pending work; terminal restoration precedes blocking worker
cleanup. The implementation and automated acceptance are tracked in
[the integration plan](progress/P5-SELECTION-INTEGRATION-PLAN-2026-10-25.md) and
[client result](progress/P5-SELECTION-CLIENT-RESUMED-2026-10-25.md). Human Apple
Terminal drag behavior and real clipboard acceptance remain separate gates.

Copy/paste should use macOS clipboard commands from the local attached client without interpolating text into shell code. Clipboard errors are visible and do not exit the session. Forward paste with the active pane's bracketed-paste semantics. Do not blindly forward host input escape sequences to a pane with different terminal modes.

### Clipboard authority and terminal requests

Clipboard actions initiated by the user are separate from requests emitted by a program inside a pane. The prototype's selected [VT100 callbacks](../vendor/vt100/src/callbacks.rs) expose clipboard copy/paste requests; the adapter must discard reads, writes and clears without sending clipboard response bytes to child input. Engine capability is not permission to access the host clipboard.

| Source | First-release behavior |
| --- | --- |
| User finishes a drag selection or invokes Copy | Copy selected text under the effective copy-on-selection/clipboard policy. Only the attached local client accesses the system clipboard. |
| User invokes Paste | Read the local clipboard for the explicitly focused/input-owned session and send one user-authorized paste. Deny if no valid attachment/ownership or policy disables it; never queue for a future attachment. |
| Child emits OSC 52 or equivalent programmatic clipboard read | Deny by default and throughout the first release. Do not invoke a host read, pbpaste, or a load callback with real clipboard data. Ignore clipboard queries without a response; even an empty OSC reply can become unintended shell input. Do not forward the query to the host terminal. |
| Child emits a programmatic clipboard write/clear | Deny throughout the first release. Do not change the clipboard, emit an OSC clipboard write to the host, or show a consent prompt triggered by arbitrary output. |
| Programmatic request while detached, during reconnect, in saved history, or received from a non-owning client | Same denial. Drop the side-effect request without deferred execution or prompts; do not replay it on attach. |

Explicit user Copy/Paste authorization never enables future programmatic reads or writes. A future opt-in programmatic-write feature requires a separate reviewed contract; it is not included by adding an engine flag. Apply limits to control-sequence size and request rate before expensive allocation or host actions, and do not log clipboard contents in denial diagnostics. Initial denial behavior must be verified while the child continues producing output and across attach/detach transitions.

Mouse-reporting inner applications need a defined pass-through gesture or per-pane mode so Panelink selection does not make editors unusable. Specify this after checking the chosen host terminal's modifier behavior. A host can consume a right-click before Panelink sees it; document setup and a keyboard menu fallback.

## Proposed OpenCode integration

The first supported provider implementation launches the installed executable with the explicit child environment and preserves provider-owned authentication. Missing executable or invalid directory produces a local actionable error, without silently installing tools or replacing the shell. Provider integration is not part of the completed foundation or current terminal-core task.

Reliable identity capture is an M1 requirement for the supported managed OpenCode mode. Rich status/attention reporting can arrive later. A bridge failure must not kill the user's agent, but it must revoke verified identity/readiness and degrade recovery promises explicitly. Users who disable the bridge or use an unsupported version can still use a generic terminal pane, with reduced recovery capabilities displayed.

### Durable provider-session identity

Managed implementation update2026-10-24: opt-in `serve --opencode-bridge`
now provisions an original TUI plugin through a private launch configuration
for the exact pinned2.0.32 executable. A same-user one-use stream grant binds
the complete managed target, canonical project, provider version and random
source instance. The worker requests challenge-bound selected-route snapshots
every500ms with a300ms operation deadline and3000ms host-receipt lease; startup
accept is bounded to15s. Failure revokes verification without killing the PTY.
There is no reconnect or implicit regrant. Ordinary launcher panes and later
generic shell splits do not enable the adapter. The exact version, timings,
scope and failure rules are recorded in the [pinned adapter contract](progress/P4-PINNED-ADAPTER.md).

The owner now saves confirmed identity and managed-launch metadata in a single
atomic record, acknowledging only the matching private worker completion.
Restoration is always historical/stale. `provider-binding prepare` validates an
explicit exact top-level ID/project and durably prepares a one-shot choice;
the separate `resume` action revalidates, durably spends its ticket before
navigation, then waits for fresh selected-route confirmation and a saved joint
record. Lost replies stay uncertain; neither navigation acknowledgment nor a
prepared choice is selected-state proof. See the [operator contract](progress/P4-MANUAL-RECOVERY.md)
and [native acceptance](progress/P4-NATIVE-MANUAL-RECOVERY.md). Scoped Phase4
acceptance is complete for pinned2.0.32: actual managed A-B-A/new/fork/Home,
foreign project selection, explicit structured manual refusal with A retained,
and same-instance return with exact newer runtime/disk save all pass Luna and
independent parent. See [integrated result](progress/P4-INTEGRATED-RESULT.md).
The private fixture keymap is test apparatus; native shortcut conflicts and
separate host/reboot/product gates remain. The historical component notes below
describe their earlier narrower scope. Current-at-crash and automatic resume
are disabled.

Transport component update2026-10-24: the independent `PLBRDG01` bounded wire
codec and same-user, grant-scoped Unix endpoints are implemented and verified
through private socketpair/pathname fixtures, admission and binding storage.
See [wire/channel evidence](progress/P4-BRIDGE-TRANSPORT-RESULT.md). A consumed
issuer grant binds complete target/project/version/source instance; it is not
provider executable attestation or a global replay/revocation guarantee.
Endpoints accept supplied streams and create no global listener. At that
component checkpoint, live source/grant delivery, owner/worker integration,
bounded queues, fresh snapshot reconciliation and production heartbeat policy
were still open; the scoped managed implementation above now supplies them.
Host receipt alone
does not prove when a route was observed; buffered metadata is not a selection
or reconciliation barrier. These components enable no recovery or automatic
resume behavior.

Component implementation update2026-10-24: a typed protocol-v1 receiver now
wraps the binding core with source-instance sequencing, host-time expiry and
explicit reconciliation. A separate private single-writer store synchronizes
bounded metadata before confirmation. Parent component tests and three actual
process-kill boundaries pass; every restored record starts stale. See
[implementation evidence](progress/P4-BRIDGE-STORE-RESULT.md). The caller must
supply trustworthy source identity/clocks, nonreused instances and an IO policy.
These exported components add no live transport, authenticated plugin, state-
owner dispatch, atomic managed-session linkage or recovery UI. The lease remains
bounded caller-selected prototype policy; production heartbeat timing and the
full integration requirements below remain open. A saved newer record may
exist after an acknowledgment was lost, without any current-at-crash guarantee.

Development evidence (2026-10-24): the pinned OpenCode2.0.32 private two-TUI
[selected-route gate](progress/P4-V2-TUI-RESULT.md) passes three fresh runs of
initial identity and keyboard A-B-A with passive-client isolation. Its100ms
polling is an observation experiment, not the product heartbeat contract or a
durable ordering barrier. No live Panelink provider bridge, saved binding or
provider recovery is enabled by this result; the requirements below remain.

The subsequent [extended fixture cases](progress/P4-V2-EXTENDED-RESULT.md)
pass opposite-client switching and controlled observer-delivery loss/expiry/
explicit reconciliation in three fresh runs each. Keyboard new-conversation
navigation yields Home without a selected ID. These observations add no durable
binding or production heartbeat guarantee; allocation timing and the remaining
new-ID/nonempty-fork/same-client project-switch cases remain unproven.
Scoped linked-child and separate-client directory isolation evidence is recorded
in [the follow-up](progress/P4-V2-ISOLATION-RESULT.md).


The [OpenCode CLI](https://docs.opencode.ai/docs/cli/) accepts an explicit session ID for resume. That flag does not identify which session a particular pane currently displays. The [plugin documentation](https://docs.opencode.ai/docs/plugins/) lists session events, but a session-created/updated event alone is not proof of the TUI's active selection.

The selected adapter must establish an authoritative binding between managed-session ID/launch generation, canonical project identity, and the exact current top-level OpenCode session ID. Store binding revision, verification source/time, supported provider version, and state (`unbound`, `pending`, `verified`, `stale`, `ambiguous`, or `unavailable`). Child/subagent activity cannot overwrite the displayed top-level binding. Never infer identity from pane title, process PID, newest session in a directory, or latest activity timestamp.

On initial launch/resume, treat identity as pending until the adapter obtains the exact selected ID and verifies the expected project/session. Commit that mapping atomically with the managed-session record before acknowledging `identity_saved` or advertising verified resume eligibility. Disk failure leaves the agent alive with an explicit “identity not saved” indicator. If using an explicit pre-created/resume ID, still verify the displayed binding; providing a launch argument does not prove the session cannot later change.

On an in-TUI session switch, fork, new conversation, or project switch, the adapter must report the new selected top-level identity with ordered binding revisions. Mark a detected transition pending, persist the new binding, then acknowledge the new revision and expose it as verified. Reject stale/out-of-order updates from old launches. Heartbeats must confirm the selected identity, not just bridge liveness. On event gaps, heartbeat expiry, or inconsistent observations, mark the mapping stale/ambiguous until a fresh authoritative reconciliation is persisted. Fix the heartbeat interval and expiry in the versioned adapter contract before M1 is complete.

The mapping acknowledged as saved is the recovery boundary. A crash before a switch update is durably acknowledged can leave only the previous verified mapping. After restart present it as “last confirmed conversation” with verification time, not as guaranteed current-at-crash identity. A claim of current-session recovery additionally requires the supported adapter to demonstrate an ordering barrier for session selection and durable identity recording. If the provider cannot supply or reliably observe all switches, disable that claim for that mode. An optional plugin or a periodic heartbeat alone does not prove the barrier.

Missing/stale/ambiguous identity uses a visible manual recovery flow: retain the pane layout/history; offer a provider-owned session picker or explicit user-supplied ID and project; validate that choice through a supported provider interface; save the chosen mapping before a new explicit resume. Never silently fall back to `--continue`, choose the directory's latest session, start a replacement agent, or retry an uncertain resume with a new operation key. Deleted/inaccessible provider sessions produce an actionable error and preserve Panelink history. Resume starts no new prompt or automatic work merely because an old conversation was reopened.

For the explicit one-shot workflow, saving the choice means durably recording a
prepared candidate with its complete managed launch scope, source instance,
prospective revision and ticket. It must not replace the last-confirmed selected
binding or claim selection before navigation. The separate resume action
revalidates the candidate and durably marks that ticket spent before issuing at
most one action. Only a later fresh selected-route observation may atomically
update the joint managed/provider binding and acknowledge selected-and-saved.
An interrupted or lost-acknowledgment operation remains uncertain and is never
automatically replayed, even if its prepared candidate still exists.

M1 must demonstrate two concurrent sessions in the same project, initial identity persistence, idle in-TUI switches, new/forked sessions, stale events, bridge disable/failure, and crashes before/after mapping acknowledgment. If the chosen provider version/integration cannot support the required identity contract, record that limitation and use the explicit last-confirmed/manual fallback before calling recovery supported. No undocumented TUI event or unsupported interception API is assumed to exist.

Process states (starting/running/exited/failed) and agent states (unknown/working/needs-input/idle/error) are distinct. Idle or turn-ended is not proof the user's work succeeded. Exited processes cannot continue to appear working. Reconcile pending permission events and clear stale states on disconnect/error; preserve evidence/freshness for unknown states.

Keep approvals and model/account choices inside OpenCode. Resume must target a specific supported saved session ID; do not use an ambiguous “continue latest” when several agents share a project. Server API integration is an alternative if the plugin path does not meet reliability requirements, with its dependency and lifecycle costs recorded.

## Proposed storage and recovery contract

All Panelink metadata and retained terminal history stay on the local Mac. Recommended baseline: a per-user application data directory with restrictive permissions, versioned JSON metadata, and separate bounded history segments. Exact location, line/byte/age limits, and rotation policy remain engineering decisions to document.

Prototype history defaults chosen 2026-10-20: `history/` under the private
per-user Panelink application directory; at most 1,024 retained normal
scrollback rows and 8 MiB on disk per managed session, plus one latest normal
and one latest alternate screen checkpoint. Target capture of changed screens
at least every five seconds while running, plus detach and orderly stop;
retain for 30 days after session end and offer a per-session clear operation.
The scoped two-pane history slice is implemented under
[P5-SAVED-HISTORY.md](progress/P5-SAVED-HISTORY.md): bounded normalized normal
scrollback, separate latest normal/alternate visible-screen checkpoints,
gap/reset indicators, read-only CLI show/search, 30-day ended-session pruning
and explicit stopped-workspace clear. These are prototype defaults, not a
claim that the full retention/recovery contract works. Complete layout
restoration, provider conversation resume and V-HISTORY reboot acceptance
remain unproved; committed partial-tail refusal is specified below. No stored
bytes are replayed.

### Scoped saved-layout/restart design (bounded snapshot, read-only recovery and explicit relaunch prototype)

Recovery viewer/relaunch user decisions (2026-10-21, prototype scope): plain
`panelink` offers a chooser when a saved layout exists (non-interactive or
Escape/timeout keeps today's fresh shell); the read-only viewer is reachable
from that chooser, `panelink recovery view [--previous]` and an in-workspace
Recovery menu item; a too-small terminal shows a scaled topology outline;
explicit relaunch starts the user's default shell in every pane with fresh
invoking-client context, is refused while a workspace runs, keeps old-to-new
links for the 30-day history retention, and never resumes provider
conversations (manual pick only). See [P5-RECOVERY-VIEWER.md](progress/P5-RECOVERY-VIEWER.md).

Implementation status (2026-10-22): the codec, two-slot store, guarded async
writer, owner/driver observation with freeze-before-stop and one shared
250 ms metadata deadline, `serve` wiring and read-only
`panelink layout show [--previous]` are implemented and tested; see
[P5-LAYOUT-SLICE.md](progress/P5-LAYOUT-SLICE.md). This storage slice does not
restore historical targets into live control or relaunch processes/providers;
the separate explicit fresh-shell relaunch prototype is described above.
Machine-reboot acceptance remains open.

[ADR-P5-LAYOUT-01](progress/P5-SAVED-LAYOUT.md) scopes a dependency-free
historical snapshot for layouts of up to four panes, with current and
immediately preceding-generation slots. The current codec uses fixed 280-byte
records and migrates supported legacy 160-byte records; migration preserves
historical metadata and does not restore targets to live control. It stores only
root dimensions, spatial historical targets, orientation and explicit snapshot
metadata. Client-local focus, launch data, environment values, titles, PID and
provider identities are excluded. An independent snapshot sequence handles
last-pane exit without assuming a layout revision changes. Ordinary pane
removal may publish an empty snapshot; driver/workspace teardown preserves the
pre-stop topology and must distinguish shutdown-caused exits.

The fixed 280-byte current codec and legacy 160-byte migration path use strict
identity, geometry, reserved/unused-field and whole-record validation; see the
[component result](progress/P5-SAVED-LAYOUT-CODEC.md). They handle historical
data only and perform no live restoration. The integrated
store/async writer/guard/runtime and read-only CLI are separate components.
The workers share one total250ms metadata-drain wait, retain singleton exclusion
through late IO and expose last confirmed rather than assumed current-at-crash
state. Layout/history files are not an atomic snapshot. Missing/corrupt history
sources remain separate from valid historical layout metadata.
Abandoning the layout writer or its pending shutdown handle cancels queued
saves and retries without waiting; an already-running storage call can finish
while retaining its singleton guard.

Before the first current-slot save from a higher generation, the store
must preserve the exact previous current bytes into the previous slot and
finish that directory sync before replacing current. Normal fresh-shell
startup therefore keeps the preceding saved generation available for explicit
read-only inspection. This is bounded two-slot retention and ordered durability,
not a multi-file transaction; interrupted rotation may expose duplicate old
snapshots. Corrupt/unsafe/unsupported slots are preserved and refuse mutation;
readers never silently substitute previous for invalid current.

The bounded persistence, read-only view, recovery chooser and explicit fresh
shell relaunch prototypes are implemented. Relaunch creates new live targets
and fresh client context; saved historical targets never become dispatch
identities. This does not restore live PTYs, checkpointed emulator state,
provider conversations or arbitrary prior processes. A physically rebooted
host has not been accepted, and full V-HISTORY acceptance remains open.

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

Use exactly one exclusive writer per independently owned metadata stream, atomic replacement with a documented sync/recovery protocol, generation IDs connecting metadata to history, and handling for partial final history records. The saved-layout design separates layout files from history files; its writer and the existing history writer retain duplicate handles to the same server singleton, with distinct exclusive stream claims. Generation counters and launch journal retain their existing owner. This refines the earlier single-writer wording without allowing concurrent writers to any one stream or claiming cross-stream atomicity. Storage failures must preserve running PTYs and display a visible persistence error. Disable or reduce history recording per session through settings; clearing Panelink history must not delete provider conversation databases.

The prototype's recovery retry checks (2026-10-21) exercise software-injected
failures before rename and at the subsequent directory sync. A renamed record
can be visible while its write still reports failure; an identical retry must
complete the directory sync before acknowledging success. Repeated session
initialization and end recording obey this boundary too. End retries retain
the first stored end time, and screen retries retain their original capture
metadata while target, buffer, output sequence and screen revision are
unchanged. Conflicting content remains rejected. See
[fault evidence](progress/P5-RECOVERY-FAULTS.md) and
[fresh-process history reads](progress/P5-RESTART-ACCEPTANCE.md). These checks
do not establish physical power-loss durability or full reboot/layout recovery.

Prototype committed-record corruption policy (2026-10-21): row segments and
screen checkpoints use whole-file atomic replacement, not an append journal.
Only a fully validated bounded committed file is readable. Truncated headers,
lengths or bodies, complete-row prefixes missing declared rows, extra trailing
bytes, malformed metadata/ordering/cells, mismatched identities and unsafe
entries fail closed. Do not salvage a valid-looking prefix, infer an end time,
or silently truncate, repair, rename, delete or replace the invalid committed
file. Mutation preflights also refuse it; clearing is not an override for
corruption. Retain the bytes for a separate explicitly designed recovery tool.

The current codecs contain no checksum or authentication. A format-valid text
or metadata alteration may remain readable: structural validation cannot prove
originality, detect all bit rot or distinguish a torn tail from corruption.
A future integrity-format migration or forensic export/repair requires its own
review; no such tool or format change is included here. The selected policy
favors preserving uncertain evidence over presenting uncertain rows as saved.

Read-only CLI show/search validates the requested rows and both existing screen
components before output. Any invalid component fails that entire target
request with status1, fixed content-free diagnostics and no partial stdout.
Missing components are absence, not malformed files. Valid other targets remain
readable by explicit ID; individual store buffer reads remain independent.
Target listing is not a certificate that every component is valid. Readers
never clean up or promote temporary files, launch processes or replay effects.
Existing locked maintenance discards only recognized uncommitted temporaries,
even when a committed destination is invalid; it never substitutes the temporary.
Maintenance across multiple files/targets is not atomic and may have completed
permitted earlier effects before a later failure. See
[policy and automated acceptance](progress/P5-COMMITTED-CORRUPTION.md).

Prototype interrupted-write policy (2026-10-21): a dot-prefixed file is never
a committed history record. Before enabling capture, the server holds its
storage lock and discards only files whose names exactly match its generated
row/screen temporary format. `panelink history cleanup` performs the same
operation while holding lifecycle and server-storage maintenance locks; it
requires the workspace to be stopped. `HistoryStore::open` and read-only
history list/show/search never clean up.
Complete orphan temporaries are discarded too, even if no committed slot
exists. Committed rows/checkpoints, including corrupt ones, are never replaced,
truncated or repaired by cleanup.

Cleanup scans at most 16,384 directory entries and preflights at most 4,096
recognized temporaries before deleting any. Each must be a private, single-link
regular file within its row/screen size limit. Candidate opens and unlinks use
the held directory descriptor, with no symlink following or temporary-body
reads; inode/mode/size and named-directory identity are rechecked. Unknown
names are preserved. Unsafe recognized entries or exceeded bounds fail closed.
The directory sync must succeed before cleanup reports success, including an
empty retry after an uncertain deletion. A later failure can leave partial
cleanup; rollback is not claimed. The caller must exclude other writers; this
does not isolate against malicious same-user filesystem races.

If startup cleanup fails, shells continue with history capture unavailable and
a fixed diagnostic. This safe discard policy handles interrupted temporary
writes; it is not salvage of corrupt committed records or a complete
power-loss/reboot acceptance claim. See the scoped
[cleanup evidence](progress/P5-TEMP-CLEANUP.md).

The [synthetic throughput baseline](progress/P5-HISTORY-THROUGHPUT.md) records
two actual release-mode runs with full durability syncs. Median full-segment
append storage calls took approximately 22–49 ms across the measured cases
and runs. These measurements precede the live writer integration; they do not
set a release budget or prove UI/ten-agent throughput. Encoding sanitized rows
and screens still runs on the owner thread; history filesystem operations
after startup preparation run on the worker.

The accepted [background-writer component contract](progress/P5-HISTORY-WRITER.md)
uses one worker with at most 16 accepted batches and 32 MiB of encoded payload,
including in-flight work. A held per-server claim excludes a second worker
until the first exits, including after shutdown timeout. Submission returns a receipt, not a durable cursor
acknowledgment; only completion of every ordered storage stage commits it.
Failed stages retry identical data at most three times before failing closed.
Accepted batches own sanitized records independently of pane lifetime. They
are ordered sequences of individually durable writes, not atomic multi-file
transactions. Rejected submissions return ownership to their caller.

An explicit drain waits at most 250 ms; a stuck storage syscall cannot be
canceled. An outstanding worker must retain a duplicate singleton lock until
it can no longer write, excluding startup and maintenance after the original
owner drops. Timeout/cancellation never counts as a durable receipt. These
component limits exclude bounded storage scratch and metadata; they are not
a total-memory or crash-loss budget.

After an explicit shutdown timeout only, canceled accepted batches with a
validated known nonzero end time may receive metadata-only finalization on the
same guarded worker, in FIFO order across at most sixteen accepted items.
No further row-append or screen-save stage starts for canceled work. Plain
`Drop`, disconnect and prior fail-closed/panic paths retain strict cancellation.
An already executing final stage that succeeds may still commit normally.
Metadata-only success leaves receipts canceled, durable payload cursors
unchanged and the drain outcome timed out. It is not durability at caller
return. The worker retains exclusion through finalizers, receipt/diagnostic
publication and backend teardown. Filesystem calls have no total wall-time
bound; the 250 ms limit bounds the caller's drain wait.

The existing codec atomically stores a conservative gap and first end time,
preserving compatible rows and both screen files. Missing history becomes an
empty ended gap; an older live epoch resets incompatible rows. A matching
already-ended epoch preserves its first timestamp and existing gap and retries
directory sync. Corrupt, unsafe, mismatched, capture-disabled or newer records
are refused. The gap means continuity is uncertified, not proof of missing
rows. Each helper gets at most three attempts with fixed 10 ms worker delays;
a terminal failure closes the writer and prevents later finalizers. Unsupported
backends fail immediately without a storage-error/retry-exhaustion label.
Fixed `canceled_end_metadata_finalized` and
`canceled_end_metadata_finalize_failed` reasons distinguish actual helper
results; neither clears timeout/failure. Unknown or unsubmitted end intent is
not recovered. This changes cancellation scheduling without changing payload
quotas, storage format or caller budget. See the
[finalization evidence](progress/P5-DRAIN-METADATA.md).

The live two-pane driver adopts this worker for both existing and split-created
sessions. Its bounded coordinator keeps submitted and durable cursors/checkpoint
identities separate; only a matching committed receipt advances durable state.
At most eight full-target capture slots retain work across pane retirement.
Each permits one ordinary and one final accepted batch, reserving eight of the
writer's sixteen count slots for terminal work. Slot exhaustion disables
capture with a sticky failure while the otherwise valid shell/split continues.
While ordinary work is pending, rows are resampled later from the emulator's
bounded ring; overflow or reset marks a gap and retains only the surviving
suffix. Each slot coalesces at most one latest normal and one latest alternate
checkpoint, each within `MAX_SCREEN_CHECKPOINT_BYTES`, outside the accepted
payload quota. This adds at most sixteen deferred checkpoint records, about
14.1 MiB of encoded bytes, excluding metadata and transient encoding scratch.

The coordinator retains the first end time, epoch and gap independently of
payload admission. An actual final encoded-byte rejection permits one additional
nonblocking attempt using the newest complete-row suffix that fits currently
available capacity, while retaining all candidate screen checkpoints and the
first end time. The complete original batch is validated before discarding any
prefix. The accounting snapshot releases its lock before row scanning or
destruction, and another ordinary submission cannot intervene. Worker activity
can only release accepted capacity during this one-producer sequence.

If the original full batch now fits, retry it unchanged without reporting
loss merely from the earlier rejection. Any discarded prefix retains a gap
and sticky failure; `final_payload_dropped_bytes` means partial or complete
final-payload loss. A suffix retains the newest row boundary, and only its
matching committed receipt advances durable state. A screen-only retry does
not invent a new row cursor. If capacity cannot be sampled, no complete row
fits alongside all screens, or the retry fails, preserve the existing visible
failure and metadata-only gap/end fallback. Initial count or accounting-lock
contention rejection retains that fallback directly. Snapshot contention alone
is not an admission event. This policy adds no retained row queue outside the
accepted quota, wait for capacity, public API or storage-format change. See
the [bounded contract](progress/P5-FINAL-TAIL-TASK.md) for scope and evidence
requirements.

Unconfirmed exit time remains unknown. Detach
does not end capture. Driver teardown stops/captures children before closing
admission, then uses one total history drain budget of 250 ms with no shared
coordinator lock held across waits. A shutdown timeout sets the same sticky
failure diagnostic, including after the driver has dropped. The existing
explicit synchronous component API remains for compatibility; live driver
capture does not use it. See [live integration evidence](progress/P5-LIVE-WRITER-RESULT.md).
This scope does not establish representative workload performance, physical
power-loss durability or full reboot/layout recovery.

The [live workload observations](progress/P5-LIVE-HISTORY-BENCH.md) add two
release runs through the real driver/store: paced 80-column output, burst
240-column output and active-output shutdown, each with two 24-row panes.
All 96 final input probes were child-acknowledged before observed producer
completion. Both final burst cases raised sticky capture failures; final end
markers were present, while a preserved preliminary run had missing end
markers after writer settlement. These observations motivate distinguishing
admission, storage and drain failure reasons before changing scheduling. No
numeric release budget or queue-occupancy claim is selected by this evidence.

The [diagnostic contract](progress/P5-HISTORY-DIAGNOSTICS-TASK.md) adds a
fixed set of twenty-one sticky reason bits and an actual writer shutdown result
(`not_requested`, `drained`, `failed`, `timed_out`). A cloned observer obtained
before enabling capture remains valid after driver teardown and detached worker
settlement. Recording uses fixed-width atomics; labels contain no terminal data,
target IDs, paths, input, environment values or storage error text. Concurrent
snapshots are nontransactional. A timed-out shutdown stays timed out if work
finishes later.

Ordinary admission contention/count/byte rejection is an observation and need
not set the existing capture-failure flag. Final-payload loss records its own
paired rejection cause. Storage errors, retry exhaustion, receipt/worker
failure, actual canceled work, pending accepted receipts at the deadline and
unsubmitted final intent are distinct observations. A drained writer queue can
coexist with lost final payload or unsubmitted metadata; it does not establish
complete capture. The CLI emits bounded fixed labels at the first observed
failure and a final shutdown summary. The diagnostic layer itself adds no storage format, scheduling,
queue-limit, retry, cancellation or shutdown-budget change.

Row retry acknowledgment applies to the durable bounded retention result;
it does not promise every submitted row remains saved. Every incoming row is
validated, including an evicted prefix. Retained ordinal overlap must match,
including across gaps; missing interior ordinals cannot be backfilled. An
evicted prefix is accepted only with a verifiable retained overlap or a new
tail, with a gap preserved. Entirely stale, evicted batches with no overlap
are rejected. A new gap must be durably recorded even on a duplicate-only
batch. No discarded cells are reconstructed or treated as verified content.

Live reattachment rebuilds the screen from server state and continues existing processes. Reboot/server-loss recovery opens retained history read-only and restores the layout, with explicit resume/relaunch actions. Historical bytes are never blindly replayed to a host terminal or shell as active control instructions. Store a safe history representation; terminal escape sequences may carry side effects such as clipboard operations.

Do not automatically rerun arbitrary prior commands. Mark normal process relaunch separately from supported agent conversation resume. Store provider session IDs without copying credential stores.

## Development milestones and exit evidence

Execution priority update2026-10-18: the user inserted Phase3A in
[ROADMAP.md](../ROADMAP.md) before the previously queued lifecycle/stop and
history work. Bring forward the bounded two-pane/right-click subset of M2/M3:
split the clicked pane and start a fresh terminal on the new side. This does
not select the remaining workspace/tab/sidebar design, close M1, or waive
runtime launch and host-input gates. Existing milestone IDs remain stable.

| Milestone | Deliverable | Exit evidence |
| --- | --- | --- |
| M0 | Resolve focused product questions and source hosting. | Decisions recorded; dependency policy and host matrix clear. Repository blockers documented until resolved. |
| M1 | One managed shell/OpenCode-compatible pane with background lifetime, reconnect, and a proven identity contract. | Full-screen input/resize/queries; lost-ack deduplication and input nonreplay; host termination/reattach; authoritative identity capture, session switching, durable binding acknowledgment and honest recovery fallback; environment isolation; clipboard denials and screen checkpoints. |
| M2 | Workspace/tab/split model and keyboard navigation. | Several processes continue while switching, moving, resizing, and zooming panes. |
| M3 | Mouse selection, copy, context menus, and history search. | Clipboard and mouse routing work in agreed hosts, including ongoing output and Unicode selection. |
| M4 | Durable local layouts/settings/history and explicit recovery. | Partial writes, bounded scrollback/checkpoints, checkpoint gaps, search scope, server restart, and explicit reboot resume obey the defined guarantees. |
| M5 | OpenCode status/notifications layered on the identity contract proved in M1. | Real versioned activity/permission events map truthfully; missing bridge and provider failures revoke readiness without losing the user's running terminal. |
| M6 | Personal alpha and release preparation. | Representative 10-agent workload, resource/build measurements, installation, notices, and upgrade behavior documented. Status 2026-10-22: delivered for the personal alpha scope — idle real-OpenCode 10-session workload (no model calls, user decision), build/size measurements, locked release manifest + install/upgrade/uninstall script, license inventory + cargo-audit, SEC-01..08 review pack; see progress/P6-*.md. Active-agent load, signing/notarization and firm approval remain open. |

The validation plan should include process cleanup, terminal restoration after client failure, protocol ordering/resync, history rotation/recovery, wide-character selection, and clipboard behavior across the supported hosts. Foundation tests and disposable engine/PTY source/API probes have run as recorded in `progress/`; these broader integrated acceptance checks and representative benchmarks remain pending. No prototype check establishes the complete M1 milestone.

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

## Open decisions beyond the bounded prototype

User choices still pending: exact agent-control access scope/defaults and autonomous scheduling; lifetime/stop behavior; project-sidebar presentation and shortcut configuration beyond the bounded tab UI; remote/tmux scope; firm review criteria, OpenCode approval, allowed dependencies, and history/clipboard policy. Apple Terminal is the first host baseline. The private repository destination is confirmed as [sahillamba/panelink](https://github.com/sahillamba/panelink).

Core dependency pins and the PTY API are selected only for the bounded prototype. Release-level dependency/capability choices, host input, viewport/delta schema, minimum macOS, data format/retention limits, input ownership, mouse pass-through gesture, OpenCode adapter/version matrix, and representative numeric quality budgets remain open.

Every future task must identify owned modules, required protocol changes, observable behavior, failure handling, and documentation updates. Do not turn pending options or prototype assumptions into confirmed user requirements. On 2026-10-17 the user authorized implementation and model delegation with durable progress tracking. `../ROADMAP.md` records the first bounded foundation tasks and explicit prototype assumptions; resolve further choices before implementing the affected features.
