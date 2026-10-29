# 04: Own pane interaction state and event priority

**Status:** authorized; pending blockers and parent assignment

**Lane:** UI state

**Blocked by:** 03

**What to build:** Group pane menus, confirmations, swap/divider interaction and selection/copy state around their captured identity and transitions. First document current modal priority from source and tests.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [ ] Group pane menus, confirmations, swap/divider interaction and selection/copy state around their captured identity and transitions. First document current modal priority from source and tests.
- [ ] Preserve that event priority: route freeze, dialogs, pointer capture, pane modes, actions/help/confirmation/menu/picker, copy and ordinary input. Preserve actual source ordering if it differs from this navigation list.
- [ ] All modal keys/paste/mouse consumption, opening-event and repeat/release latches, cancel-default close behavior and captured-target validation remain identical.
- [ ] Selection keeps one owner and a frozen viewport identity; route/focus/buffer/size changes invalidate it as before. Copy attempts/results stay source-target/request scoped and preserve Unicode/wrap behavior.
- [ ] Preserve one-way handoff of commands to existing effects and pending-operation tracking. This is not a wholesale event framework or concurrent command redesign.
- [ ] Behavioral tests prove exact child effects, default-cancel and stale target cancellation; remove superseded root interaction state. Event routing becomes readable policy rather than duplicated per-mode state mutations.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
