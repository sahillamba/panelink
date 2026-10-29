# 07: Organize embedded server scenarios

**Status:** authorized; pending blockers and parent assignment

**Lane:** Server test layout

**Blocked by:** 00

**What to build:** Move the existing multi-session main, split and runtime-helper test modules into named private test files while preserving their assertions, fixtures, ignored reasons and execution behavior.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [ ] Move the existing multi-session main, split and runtime-helper test modules into named private test files while preserving their assertions, fixtures, ignored reasons and execution behavior.
- [ ] Production owner fields and authority/lifecycle logic remain unchanged. Keep test-only helpers private through child-module visibility; avoid exposing internals or creating a generic test framework.
- [ ] Capture the test list before and after; every scenario remains accounted for, allowing documented module-prefix renames. Inspect the diff for pure relocation plus minimal declarations.
- [ ] Run every relocated scenario. The server core remains a single authority owner; describe this result as navigation improvement, not production decomposition.
- [ ] Further runtime/compositor test movement is a separately leased follow-up after this small packet succeeds. Do not expand this ticket across all large files.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
