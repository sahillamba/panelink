# 08: Verify the combined design and update current guidance

**Status:** authorized; pending blockers and parent assignment

**Lane:** Parent

**Blocked by:** 01, 05, 06, 07

**What to build:** Review all combined changes against the preserved contracts, the deletion test and actual state ownership. Reject helpers that merely accept the whole application and relocate complexity.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [ ] Review all combined changes against the preserved contracts, the deletion test and actual state ownership. Reject helpers that merely accept the whole application and relocate complexity.
- [ ] Confirm no protocol/JSON/storage/dependency or user-visible behavior change slipped into a structural ticket. Any necessary bug fix has its own failing reproduction and review scope.
- [ ] Run the canonical verifier on the combined tree with serial test execution and an isolated build directory. Record exact results, failures, ignored/skipped checks and environment.
- [ ] Update current roadmap/handoff entries and code-navigation references, retaining dated historical evidence. Validate the repository Panelink skill and its links; update paths or guidance affected by the refactor and distinguish implementation from open acceptance.
- [ ] Record before/after production/test inventory and demonstrate a history edit, tab-completion edit and confirmation edit each have a clear owner with fewer unrelated touchpoints.
- [ ] Prepare the parent handback with source identity, module ownership, test evidence and unresolved host/provider/performance gates. Commit/push/install only under applicable user authorization; this planning bundle does not authorize them.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
