# ADR-LAUNCH-01: Serialized launch work outside the live owner

Status: accepted for bounded implementation, 2026-10-22.
Decider: parent integrator under the user's approved architecture/build task.

## Context

The live schema-7 split route currently opens and reloads the launch journal,
resolves executable/context/terminfo, synchronizes Intent, starts a PTY child,
and synchronizes Spawned/Applied while holding the server owner thread.
Any slow filesystem or process-start call delays existing panes and control.
The schema-6 direct launch route is disabled; initial workspace launch occurs
before the interactive owner loop starts. This slice targets live splits.

## Decision

Keep all grants, ledger admission/results, registry identities, input ownership,
layout and lifecycle observations on the owner. Add one dedicated worker for
launch preparation, journal IO and process creation. A single active split
reserves the geometry and one session identity; no independent launch backlog
is admitted. A capacity-one command channel and one completion slot bound
worker traffic. Existing owner/connection queue limits continue to apply.
Worker payloads contain at most one validated split request (65,536 bytes),
kept only in volatile memory. No arguments or environment enter the journal.

The owner authorizes and checks exact duplicates before checking worker/active
capacity. New available work admits the exact split key/bytes before geometry
validation, preserving the existing sequence-consuming Failed rejection policy.
Geometry rejection spends no session identity or process. Exact duplicates
observe the same pending or terminal operation; changed bytes conflict.
A competing new split is refused before admission. Client timeout or
disconnect never cancels admitted work.
At most eight finite reply waiters use existing connection bounds. No wire
schema changes or terminal success acknowledgments before completion.

1. Owner reserves target, preview and target-state seed, then submits Start.
2. Worker resolves launch context, opens/preflights the sole journal, durably
   writes Intent, checks cancellation, and starts exactly one child through the
   existing atomic PTY opener/shared spawn gate. It does not retry a spawn.
3. Worker publishes the acquired TerminalSession in a bounded completion slot.
   The owner consumes it and immediately attaches it to the reserved registry
   record. Only then does it submit Finalize to the worker.
4. Worker writes Spawned and Applied; failed sync after acquisition reports
   Unknown and disables new launches. No success is inferred from visible bytes
   after a failed sync. Owner retains the child for either Applied or Unknown,
   commits the split, resizes the existing pane, and records the terminal
   ledger/result. Failed survivor resize preserves Partial semantics.

The worker transfers only the newly acquired TerminalSession, whose Send
property is compile checked. Existing PtyEffects and owner state never move
to the worker. A worker-owned journal guard duplicates the held singleton and
directory descriptors with atomic CLOEXEC, binds the same generation, and
excludes direct journal handles and another launch worker. Initialization and
guard acquisition occur before the live owner loop, not inside split dispatch.

While a split is active, root resize and close/new topology mutations are
refused with explicit busy behavior; input, reads, watches and detaching remain
available. Existing panes may exit naturally; their lifecycle facts are
recorded while topology collapse waits for the reserved preview to settle.
This applies to splits from one, two or three existing panes in the current
four-pane model. Empty-workspace publication and
workspace-end detection are suppressed until the admitted split settles.
An acquired child that exits during finalization stays Exited rather than
being moved backward to Applied. Final layout reconciliation follows normal
owner polling. No dead process is revived or input replayed.

Shutdown stops admissions and requests worker cancellation immediately.
After child-stop processing it waits only within the existing shared 250 ms
metadata deadline and joins a finished worker; a timeout reports incomplete
launch shutdown. A storage call or spawn already entered may finish; the worker
checks cancellation before beginning subsequent effects. An unconsumed child
is cleaned up on the worker while it retains the singleton. A transferred
child is owned by normal driver shutdown. The completion slot is retained by
the worker so abandoning the driver cannot run child cleanup on a live owner
tick or release exclusion before late work/cleanup ends. Shutdown does not
claim that an uninterruptible syscall has stopped or that pending work drained.
The retained lock and late cleanup apply while the server process remains
alive. Detached threads cannot survive process exit. An incomplete graceful
shutdown therefore does not certify child cleanup, and forced process exit,
SIGKILL, abort and physical power loss remain outside that guarantee.
Cancellation without a terminal worker completion is restricted to driver
destruction. A live owner handles submission failure or worker death by
terminalizing the reserved operation; uncertain worker loss is Unknown even
without an adopted child. The current split wire codec cannot represent an
uncommitted Unknown split, so this case returns a fixed uncertainty error;
exact retries observe that uncertainty without launching again.

## Options and consequences

Keeping synchronous dispatch is smaller but leaves unrelated pane traffic
blocked. Moving the whole owner to another thread only moves the stall and
would require sharing its mutable authority. Running multiple launch workers
would expand journal ordering, capacity and spawn races. The selected single
worker adds explicit handoff/failure states while preserving serialized
launches and owner authority. Startup launch remains synchronous; this is not
a general asynchronous filesystem framework or concurrent-launch policy.

## Acceptance

Deterministic delayed-worker tests must prove existing pane input/output and
read/control progress before release; pending duplicates/conflicts must never
create another job or process. Cover failure before Intent, Intent-sync refusal
with zero spawns, post-acquisition sync uncertainty with retained child, child
exit while finalization waits, client disconnect/timeout, and shutdown during
blocked preparation/journal/spawn work. Verify singleton retention until late
work and unconsumed-child cleanup end. Run existing split/close/layout/history,
descriptor-gate and client checks plus formatting/strict Clippy. Record blocked
host/socket acceptance separately. Parent review precedes enabled adoption.
