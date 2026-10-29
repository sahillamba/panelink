# 01: Unify host event dispatch

**Status:** authorized; pending blockers and parent assignment

**Lane:** Host

**Blocked by:** 00

**What to build:** Ordinary decoded events and completed lone-Escape events follow one shared dispatch sequence for recovery availability, application handling, metadata queues, route requests and catalog refresh.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [ ] Ordinary decoded events and completed lone-Escape events follow one shared dispatch sequence for recovery availability, application handling, metadata queues, route requests and catalog refresh.
- [ ] Preserve distinct handling of Quit and ViewRecovery, the documented lone-Escape assumption, input work budgets, decoder/input timeouts and local detach responsiveness.
- [ ] Use existing application methods; this packet does not change their interface while the UI-state lane runs. Keep worker-result processing in place unless a small private helper clearly improves locality.
- [ ] Behavioral host/private-PTY tests pass; add a focused recording test only if the shared sequence has an uncovered ordering requirement. Confirm no event is processed twice and one path cannot miss a queued intent.
- [ ] The duplicated dispatch sequence is removed. Keep terminal restoration and server-ended/unknown-tab-close behavior intact.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
