# ADR proposal: visible panes and agent input control

Status: Proposed; user requested options, no option accepted or implemented.
Date: 2026-10-27.
Decider: user, after reviewing the interaction and trade-offs below.

## Context and evidence

The user's workflow is to ask an agent to type into other visible panes while
keeping the Panelink UI open. Reads work, but ownership claims fail.

The installed launcher resolves to worktree `7060`, not this main checkout.
Live discovery returned three panes owned by clients 2, 4 and 3. A single
claim against the right pane reproduced `input_failed`; the owner remained
unchanged and no text was sent. The source paths below refer to that installed
worktree and are distinct from the older main implementation baseline:

- `src/ui_client/net.rs`, `Net::poll`: claims every pane, then renews the
  30-second input lease every 10 seconds, including idle/unfocused panes.
- `src/ui_client/net.rs`, `Net::split`: claims a new pane immediately.
- `src/input_owner.rs`, `InputOwner::claim`: rejects another holder.
- `src/server.rs`, `owner_rejection`: maps HeldByAnotherOwner to Unauthorized.
- `src/p3_cli.rs`, `owner_rejection_label`, and `src/agent_cli.rs`, `owner_error`:
  the agent sees a generic `input_failed` instead of a useful ownership reason.
- `InputOwner::handoff` and `SessionRequest::Handoff`: the current owner must
  initiate transfer; the recipient must have an active target-scoped input grant.
  This is not a ready-made human override. Although `Owner::Human` exists in
  the pure state machine, runtime session-info rejects it as UnsupportedOwner;
  the attached UI currently uses ordinary client leases.

Current source was inspected directly after the graph service closed. This
proposal supplies no new application test or acceptance pass. Other worktree
changes, running panes and installed binaries were not modified.

The existing [agent-control design](../AGENT-CONTROL.md) already proposes finite
input leases, a reusable workspace collaboration grant, an ownership indicator,
and explicit human Take control. Those remain proposals; the complete UI/server
integration has not been delivered. Host sandbox permission and Panelink input
ownership remain separate checks.

## Options considered

| Option | User experience | Relative work | Main trade-off |
| --- | --- | --- | --- |
| A. Explicit handoff per pane | Use Allow agents on a pane; agents then claim it. Use Take control to type manually and Allow agents to return it. | Smallest complete product fix | Deliberate switching on each pane/turn adds interaction |
| B. Workspace automation mode | Enable collaboration once for a named workspace. Its available panes stay viewable without UI input claims. Agents claim/send/release; human Take control reserves a pane until explicitly returned. | Moderate; recommended | Requires clear pane states and server enforcement across all attachments |
| C. Automatic activity-based handoff | Human activity requests priority and automation resumes when a turn is explicitly settled or reliably detected. | Largest; not recommended for first repair | A quiet pane may contain an unfinished shell command, editor action or TUI prompt; inactivity is insufficient evidence |

### A: Explicit handoff per pane

Add a pane action that deliberately releases the UI's input lease and stops
its background reacquisition for that pane. Continue rendering and reading.
The agent can then use ordinary target-scoped claim/input/release. Retaking
control must use a server-coordinated request; it must not impersonate or
silently replace another client.

This directly fixes controlling a visible pane with the narrowest behavioral
change. A user must opt each target into automation and switch modes again
after manually taking control. It is a useful first increment of option B.
It still needs pending-operation and multiple-attachment handling; simply
adding a Release button is insufficient if the next poll reacquires the lease.

### B: Workspace automation mode — recommended

Use explicit workspace collaboration setup once, then preserve the following
interaction for that workspace:

1. Existing human-held panes are released only through an explicit approved
   transition after pending input settles. Enabling the mode never clears a
   half-written command or declares it safe merely because output is quiet.
2. In this mode, opening, viewing, focusing, scrolling normal history and
   reconnecting do not automatically acquire input ownership. New panes also
   follow the selected mode. App-directed wheel input, paste and keys are
   input operations and remain subject to the writer owner.
3. Available panes can be claimed by an authorized agent. A complete text
   submission can use an optional scoped claim/submit/release lifecycle.
   Multi-step TUI interaction retains one finite lease across the whole intended
   interaction, rather than releasing after every key. The existing
   `send --claim` does not automatically release; that lifecycle is new work.
4. The UI labels each pane Available, Agent-controlled, Human-controlled or
   Switching. The label names the responsible client where known. It does
   not infer a provider conversation identity from a caller-supplied name.
5. Take control requests a server-ordered transition. Previously admitted input
   settles before ownership changes. Until it completes, user keystrokes are
   not sent or buffered for later replay. If settlement cannot be established,
   show blocked/unknown rather than claiming takeover succeeded.
6. A human-held pane stays protected even when typing stops. The user explicitly
   returns it to automation. Do not infer that Enter, focus loss or a timeout
   means a generic terminal application is at a safe interaction boundary.
7. Turning automation off stops new agent admissions immediately; existing
   admitted operations are settled/reported before switching the writer.
   Other attached clients must respect the same server-owned policy.

For "type hi in both", after setup and with both targets available, the agent
discovers exact targets, submits once to each under its lease, verifies results
and releases. Both panes remain visible. These are two independent operations;
one success and one refusal must be reported separately, not treated as an
atomic multi-pane transaction or retried as a pair.

This offers low interaction during normal agent work while protecting deliberate
human typing. An optional remember-setting can be considered during setup;
this proposal does not select a persistent, cross-restart authorization policy.

### C: Automatic activity-based handoff

A server arbiter could prioritize explicit human activity and grant short
automation turns between human interactions. The displayed UI would observe
without keeping a permanent claim. An admitted batch would still have exactly
one writer and a pinned operation result.

However, serialization alone cannot protect a command being composed over
several key batches: an agent's `hi` could be appended to a user's unfinished
shell command or land inside a TUI dialog. Enter is not a universal completion
signal. Reliable automatic turn boundaries would require more interaction state,
application integration or explicit turn-ending actions. Without that evidence,
the arbiter must refuse/pause, reducing the promised automation benefit.

This is a possible later enhancement of B, not a safe "remove the locks" patch.

## Common requirements and implementation touchpoints

- Diagnose before admission: distinguish missing Input capability, owner conflict,
  stale owner/target, pending input, context storage failure and uncertain outcome.
  Do not relabel all Unauthorized failures as InputBusy: wrong-scope and missing
  grant failures must remain authorization errors. Return a truthful owner,
  target/revision and actionable disposition without exposing input contents.
- Integrate UI mode and control actions through `ui_client/state.rs`,
  `worker.rs`, `net.rs` and `conn.rs`. Cover startup, reconnect, new split,
  multi-window/multiple-attachment behavior, paste and app-directed wheel paths.
- Mediate cooperation/takeover through the server ownership/lease admission
  boundary (`server.rs`, `server/multi_session.rs`, `input_owner.rs` and relevant
  session/UI codecs). A generic client cannot gain special takeover rights by
  declaring itself human. Same-UID access is not per-agent security isolation.
- Preserve exact target and owner-revision checks, deduplication before stale
  preconditions, pending-input protection and existing operation-key semantics.
  Unknown/partial input is never resent under the same or a new key. No automatic
  expiry-based handoff of unresolved input and no replay of old user keys.
- Keep stop/close/launch and host permission setup distinct from input ownership.
  A working MCP or socket connection alone does not repair UI-held leases.
- Update canonical spec/agent-control contracts, CLI help, skill and current
  worktree handoff when a choice is implemented. This proposal does not adopt it.

## Validation required before claiming control works

Use private fixtures and parsed terminal cells with readiness barriers:

1. Attach a live UI, then discover/read/claim/submit/release from another client;
   observe output in the intended visible pane without detaching.
2. Race two agent claims and a human control request. Prove exactly one writer,
   correct owner revision and no cross-target or replacement-pane input.
3. Enter a partial human command, leave it quiet, and attempt automation.
   Verify no concatenation, clearing or secret logging.
4. Exercise typing, paste, app mouse wheel, new split, focus change, reconnect
   and two UI attachments under each supported control mode.
5. Lose the input acknowledgment; verify reconciliation with the original key
   and zero resubmission. Exercise pending/partial/unknown input at takeover,
   expiry, disconnect and shutdown.
6. Simulate one successful target and one refused target in the user's two-pane
   workflow. Report independent results without resending the first operation.
7. Verify specific errors for ownership conflict versus missing capability,
   stale targets, denied socket access and unwritable context storage.

## External behavior references

[tmux's manual](https://man.openbsd.org/tmux) documents read-only attached clients
and toggling that state; observing a pane therefore need not mean controlling
its keyboard. [WezTerm's send-text reference](https://wezterm.org/cli/cli/send-text.html)
documents explicitly targeted pane text input. These are behavioral references
for visible observation and agent ergonomics, not evidence that those products
implement Panelink's lease, takeover or nonreplay contracts. No third-party
implementation is proposed for copying or adoption.

## Recommendation and consequences

Choose B as the product direction, with A as its bounded first implementation.
Deliver truthful ownership diagnostics and cooperative per-pane switching first,
then the workspace automation policy and full takeover integration. Do not ship
the new mode as complete until attached-UI input and uncertainty scenarios pass.

This adds explicit ownership state to the UI and some server policy work. It
removes the permanent idle-UI ownership conflict without exposing uncoordinated
writers or relying on guessed application readiness.
