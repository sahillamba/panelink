# Planned phase SA: Subagents visibility

Date: 2026-10-21. Status: requested outcome recorded; engineering proposal.
No observer, provider bridge, tab UI, or worker viewer is implemented.

S0 execution update: [2026-10-21 attempt](SUBAGENTS-FEASIBILITY.md) verified
installed OpenCode2.0.32 CLI preflight, but server/ACP bootstrap is blocked by
local socket permissions. All child/session acceptance remains unrun; S0 is open.

## Confirmed request and proposed experience

When a managed coding agent starts a background agent or subagent, make it
visible automatically in a Panelink tab named **Subagents**. The user can
switch there to observe it while the parent continues working. The user named
OpenCode and wrappers built on OpenCode (clarified by the user this session).
This confirms a dedicated Subagents tab, without settling the remaining
workspace/sidebar hierarchy or asking Panelink to assign additional work.

Proposed defaults: one Subagents tab per workspace, created on first observed
worker; entries grouped by parent, nested for descendants; current focus stays
unchanged; a badge shows activity/attention; selecting a worker opens its live
messages and tool activity. Completed entries remain in a bounded recent list.

```text
[ Main ] [ Subagents: 2 working, 1 needs attention ]

OpenCode — project A
  Explore repository       working
  Review changes           needs attention
    Check dependencies     idle

Selected: Explore repository
  task / agent / model, where exposed
  live messages and tool activity
  observed status and freshness
```

The proposed MVP is read-only. Opening, hiding, or closing the view sends no
prompt, approval, cancellation, or provider-selection change. Highlight
permission waits and locate the provider-owned approval UI. Controls/approvals
through Panelink are later scope. Idle alone cannot establish task success.
These UX defaults remain proposals, distinct from the confirmed tab requirement.

## Provider evidence and feasibility boundary

Official documentation inspected 2026-10-21:

- [OpenCode V2 agents](https://opencode.ai/v2/docs/agents): subagents run in
  foreground/background child sessions. A worker need not be a separate process.
- [Plugin overview](https://dev.opencode.ai/v2/docs/build/plugins/): session
  reads/context and server event subscription provide candidate observation
  surfaces. Choose an adapter only after a pinned-version spike.
- [ACP](https://dev.opencode.ai/v2/docs/cli/acp/): child updates identify child,
  parent, depth and optional title. Activity updates stop after the parent
  prompt ends, though permission requests/questions may continue. ACP owns a
  private server; it does not establish observation of an independently running
  TUI's workers. ACP alone is insufficient proof for all background activity.
- [HTTP API](https://dev.opencode.ai/v2/docs/api): exposes session reads,
  context/messages and an experimental server-wide event stream. Disconnects
  miss events and slow consumers can overflow; reconciliation and location/
  project/parent filtering are necessary.

These are documentation observations, not local runtime acceptance or selected
dependencies/transports. V1 and V2 require distinct evidence. Do not silently
upgrade the installed provider. A wrapper is supported only when its exact
version exposes compatible child identity, activity and reconciliation APIs;
the OpenCode base alone does not prove wrapper compatibility.

Panelink must observe the exact worker already started by the provider. Never
rerun its task to produce a visible terminal. Distinguish two representations:

1. Provider child session: bounded activity/transcript view, separate from
   managed terminal sessions and the current two live-PTY slots.
2. Independently launched terminal worker: view its existing managed PTY only
   with an exact ownership/identity link. Extra PTYs need reviewed capacity work.

A fully interactive provider TUI per child needs proof that a second viewer
can attach without resuming, forking, prompting, or changing execution/input
ownership. The MVP proposes a read-only activity view; it does not claim that
interactive attachment exists.

“As soon as started” means promptly after authoritative observation. A proposed
target is visibility within 250 ms of receiving a valid start observation;
measure provider-start-to-observation latency separately. An after-the-fact
event cannot guarantee atomic placement before execution. Reconciled workers
may appear later after reconnect and must be labeled accordingly.

## Architecture and constraints

```text
Provider TUI/service
  -> versioned observer: events + reads + reconciliation
  -> bounded validated updates
  -> server-owned worker registry
  -> client snapshot/deltas
  -> Subagents tab + selected worker detail
```

Identity includes server generation, managed parent launch, provider instance,
project, exact root/parent/child IDs, and execution/run identity where available.
A child session can run multiple tasks; do not retain an old completion as the
state of a new execution. Preserve provider revisions/cursors when supported;
local observation revisions describe only local ordering. No title/PID/latest-
session heuristics. Child activity must never replace the top-level
`ProviderBinding` used for parent recovery.

Separate relationship, execution status, and freshness. Deduplicate by exact
identity. Bound child-before-parent buffering, then reconcile. Gaps, overflow,
bridge restart and disconnect make data stale until authoritative rereads.
Test subscription/snapshot races; do not assume the event stream is resumable.
Unsupported capabilities show unavailable state with no invented live content.

Provider IO/decoding must run outside the PTY owner and host render loops.
Use bounded queues and batched updates; fetch selected-worker detail on demand
and virtualize the list. Proposed initial limits: 64 active/unknown workers,
128 recent terminal entries, 256 KiB sanitized selected-detail cache. Excess
workers get explicit overflow indication/reconciliation, never provider
cancellation. Review these limits through measurements before adoption.

Render titles/messages/tool output through safe text/cell paths. No raw control
sequence replay. Keep prompts, tool arguments, secrets and transcript contents
out of diagnostics. Prefer memory-only viewer caches initially; provider
conversations remain distinct from Panelink terminal history. Viewing installs
no global provider config and starts no replacement process.

## Delivery slices and acceptance

| Slice | Deliverable | Exit evidence |
| --- | --- | --- |
| S0: capability spike | Pin exact OpenCode version; prove parent binding, child discovery and read-only content | Two parents in one project; foreground/background/nested workers; observation after parent turn ends; missed-event recovery; zero duplicate execution. Record unsupported cases. |
| S1: pure registry | Bounded tree, run identity, status/freshness and reconciliation | Duplicates, stale/out-of-order updates, missing parents, rapid starts/ends, restart, quotas and cross-project rejection. |
| S2: observer runtime | Opt-in versioned adapter, bounded server delivery | Delayed/failed provider calls and event floods do not block PTY input/output/control; no replay or parent-binding mutation. |
| S3: tab/view UI | Badge, grouped list, selected read-only activity, keyboard/mouse navigation | No focus stealing; updates while hidden; opening/closing causes no provider action; safe text, incremental rendering and unavailable states. |
| S4: live acceptance | Supported provider end-to-end workflow | Concurrent/nested workers, permission wait, explicit results, detach/reattach, parent switches and observer reconnect; measured latency and bounded memory. |

Losing a parent pane or observer is not evidence that workers completed. Use
provider/lifecycle evidence, otherwise show unknown/stale. Define last-parent-
pane exit and empty-main-tab behavior before S3 adoption, so it cannot silently
destroy the only worker view. Closing Subagents itself must not stop work.

## Roadmap placement and unresolved choices

Phase **SA — Subagents visibility** spans Phase 4 provider proof and Phase 5 tab
presentation. Planning this phase does not reorder the saved-layout codec task.
S0 can be an isolated spike when implementation is requested. Runtime adoption
requires authoritative identity, a bounded observer channel, a minimal tab/view
model and responsiveness checks addressing the
[architecture review](../reviews/2026-10-21-architecture-review.md).

Do not silently extend the fixed 160-byte two-pane saved-layout format. Persisting
this tab later needs explicit versioning/separate UI metadata. Reboot worker
relaunch and transcript persistence are outside this MVP; historical views
never execute work.

The read-only activity view is the proposed first slice. An interactive
provider TUI for each worker requires an explicit scope choice and additional
attachment proof. Bootstrap checks ran but no required child/session acceptance has run;
the slices above are future work, not completed milestones.
