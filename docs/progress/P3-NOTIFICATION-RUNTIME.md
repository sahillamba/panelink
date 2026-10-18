# Notification runtime boundary — 2026-10-18

Status updated2026-10-18: bounded runtime watches and atomic reconciliation are
implemented under P3-LIVE-WATCH-TASKS.md; source/real-PTY evidence is in
INTEGRATION.md and P3-WATCH-*.md. This note records the constraints used for that
implementation and remaining indefinite-subscription boundaries. Events are
advertised only with authoritative tracking, never from a codec alone.

## What the metadata means

The first notification payload contains a full immutable target, nonzero
generation-scoped cursor and exactly one metadata observation: screen revision
changed, client input owner changed/vacated, or collected child exit. There is
no PTY byte stream, clipboard content, terminal input, environment, provider
identity, readiness or task-success assertion. Human-owner notification is not
represented by this initial schema; owner_lease0 means vacant only.

The journal retains64 newest metadata events (6144 logical encoded bytes with
headers). This is separate from the future large screen-snapshot writer budget,
from persistent scrollback, and from ledger result retention. A cursor before
the last evicted fact is expired; return an explicit resync condition, never a
partial stream represented as current. A current cursor or event is an
observation, not permission to dispatch any operation.

## Required runtime integration before enabling Events

The single state owner must record committed changes in cursor order. Failed
preflight, rejected admission and exact duplicate result retrieval must not
emit a fresh effect event. Ownership expiry is a change even without a client
request. Child exit means a collected status, not adapter IO failure. A screen
revision change means current display metadata changed, not that an application
finished a command. Notifications never replay child input or terminal controls.

An authoritative reconciliation result must bind full target, current screen
and owner/lifecycle metadata to its cursor atomically under the state owner.
Do not read a snapshot, then independently sample a cursor and claim they are
the same observation. Requests must bind to the original target occupant;
replacement occupants cannot fulfill an old wait or subscription.

Live delivery needs an explicitly negotiated Events capability and one writer
per connection. Bound subscriber count, pending encoded bytes, reply reserve,
read/write/owner wait deadlines and cleanup work. A slow subscriber must not
block the state owner or cause an admitted input acknowledgment to be silently
dropped. Handle expiration/overflow with explicit resync/closure, not invented
continuity. No hidden unbounded channel or automatic user input retry.

Runtime policy for sequence exhaustion must fail observation delivery closed
without claiming rollback of already accepted process effects. Keep the owned
PTY alive while reporting unavailable notifications; do not create a fresh
generation/lease or restart a child merely to reset a cursor. This component
does not implement that policy or a lifecycle rollover.

Required proof: a real authenticated socketpair with concurrent PTY output,
state-owner snapshot/cursor consistency, ownership changes, exit, disconnect,
expired/future/wrong-generation cursors, slow writer/backpressure and capability
denial. Negative assertions must show no repeat launch/input or replacement
target observation. Pathname and host acceptance remain separate evidence.
The bounded watch/reconciliation schema is now fixed in P3-LIVE-WATCH-TASKS.md.
Provider/text-condition waits and indefinite subscriptions still require their
own reviewed schemas before implementation.
