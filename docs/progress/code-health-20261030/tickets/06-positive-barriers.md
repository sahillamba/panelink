# 06: Wait for positive split completion

**Status:** complete; parent implemented and verified

**Lane:** PTY fixture

**Blocked by:** 00

**What to build:** The four-pane fixture waits for affirmative evidence that the expected split target/layout and client focus/completion are installed, rather than merely absent pending text plus a fixed sleep.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [x] The four-pane fixture waits for affirmative evidence that the expected split target/layout and client focus/completion are installed, rather than merely absent pending text plus a fixed sleep.
- [x] Choose evidence the existing test can observe and that stale/default screens cannot satisfy. Preserve exact pane identities, terminal restoration, child lifecycle and timeout failure diagnostics.
- [x] Keep the wait bounded and remove the obsolete 300 ms completion sleep. Do not add product diagnostics or widen the application interface solely for this fixture.
- [x] Demonstrate that an early/stale captured screen fails the new predicate and the real private-PTY scenario succeeds. If focus cannot be observed, document the exact positive surrogate and its limits.
- [x] Run the complete affected integration target and retain any observed failure for diagnosis.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
