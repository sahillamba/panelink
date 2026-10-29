# 02: Own pane viewport and history transitions

**Status:** complete; parent implemented, independently reviewed and verified

**Lane:** UI state

**Blocked by:** 00

**What to build:** The pane viewport module owns the live/history snapshots, identity/cursors, request state and pending motion rules; callers no longer mutate these fields independently.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [x] The pane viewport module owns the live/history snapshots, identity/cursors, request state and pending motion rules; callers no longer mutate these fields independently.
- [x] Migrate one complete scroll/read cycle and its screen-loading/invalidation lifecycle through the new owner. The application supplies bounded context and the existing effects adapter, not unrestricted mutable application state.
- [x] Preserve normal-motion coalescing, newest absolute seek, cursor rollover, RIS epochs, stale replies, visible pending content, target replacement and route/resize invalidation.
- [x] Preserve fresh-unsent alternate-screen FIFO order/coordinates, capacity/expiry, pop-before-admission semantics and failure discard; no admitted/uncertain input is reinserted. Preserve current capability/ownership behavior in existing network adapters.
- [x] Keep shared render/hit geometry and typing focus stable. Give selection one explicit identity/invalidation contract; avoid duplicate selection or cache owners.
- [x] Existing behavioral viewport, wheel and private-PTY tests pass. A test fake observes exact effects and targets. No large snapshot cloning or extra screen cache is introduced.
- [x] Remove old viewport fields/logic from the root owner. Review one normal read, one alternate gesture and one stale reply end-to-end before acceptance.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
