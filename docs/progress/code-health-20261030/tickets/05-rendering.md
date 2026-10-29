# 05: Render bounded module views with shared geometry

**Status:** complete; parent review and focused checks pass; see [evidence](../PACKET05.md)

**Lane:** UI state and chrome

**Blocked by:** 04

**What to build:** Frame construction consumes immutable viewport, workspace and interaction views. Keep display decisions with the state module that owns their meaning; keep frame assembly and compositor interaction in the renderer.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [x] Frame construction consumes immutable viewport, workspace and interaction views. Keep display decisions with the state module that owns their meaning; keep frame assembly and compositor interaction in the renderer.
- [x] Render and hit testing share the same geometry for pane chrome, compact tab dialogs, pane confirmations, scrollbar and header offsets. Extract the existing pure chrome geometry only if doing so improves navigation; keep existing public exports stable.
- [x] Preserve terminal bounds, small-host placeholders, Unicode/wide-cell handling, selection overlays, copy feedback and version/history indicators.
- [x] Output baseline/commit remains after accepted writes; frame generation does not commit output or mutate business state. Preserve preallocation and avoid screen clones.
- [x] Existing geometry, compositor delta/damage and UI behavior tests pass. Move surviving tests to the real module interface only with equivalent assertions; do not retain duplicate old/new tests just to increase counts.
- [x] The root renderer no longer contains all modal layouts inline or reaches through mutable state internals. Report the resulting responsibilities and remaining long functions without enforcing a file-size quota.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
