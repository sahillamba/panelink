# 03: Own workspace tab presentation and completions

**Status:** complete; parent checks and independent review pass; see [evidence](../PACKET03.md)

**Lane:** UI state

**Blocked by:** 02

**What to build:** The workspace presentation module owns picker/dialog/filter state and pending create/rename/close metadata identity. Existing route and metadata workers remain the I/O adapters.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [x] The workspace presentation module owns picker/dialog/filter state and pending create/rename/close metadata identity. Existing route and metadata workers remain the I/O adapters.
- [x] Produce intents and consume completions through a small interface that includes source route, request ID and expected revision. Keep a single authoritative current-route relationship.
- [x] Preserve empty workspace New/Detach, tab catalog/navigation/filtering, compact dialog behavior and inactive-tab close restrictions.
- [x] Late or mismatched metadata results cannot change a replacement route/dialog. Unknown create reconciliation uses the same intent only on explicit user action. Unknown/partial close retains the current visibility behavior.
- [x] Route switches freeze/discard exactly the same unsent actions and retain local resize/detach. Existing tab, lifecycle and recovery tests pass.
- [x] Remove duplicate root tab/picker/pending-metadata fields and forwarding scaffolding. Rendering consumes an immutable bounded view.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
