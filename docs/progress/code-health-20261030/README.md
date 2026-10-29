# Panelink maintainability implementation — 2026-10-30

**Complete for the authorized local scope:** packets00–08 implemented, parent reviewed and combined canonical8/8 passed. See [evidence](INTEGRATION.md), [source inventory](INVENTORY.md) and [current navigation](NAVIGATION.md). No install or push.

Start here. The user authorized implementation on 2026-10-30 in the isolated `refactor/code-health-20261030` worktree, with parent orchestration and GPT-6-Luna for bounded tasks. [PROGRESS.md](PROGRESS.md) is the execution state. Tracker publication remains outside scope. Historical draft notes below describe the planning provenance; this execution authorization supersedes their planning-only status.

## Honest assessment

Large files are a maintenance cost when they mix concepts, expose shared mutable state, or require unrelated edits to understand one behavior. Smaller files alone do not solve that. The UI state module is the strongest example: its presentation, history, input modes, workspace metadata and rendering share one large owner. That increases review scope and merge contention even though tests pass.

A correction to the earlier review: the first `#[cfg(test)]` line is NOT a valid production-code cutoff. It can mark a single helper or an early test module. Some enormous files are mostly tests. At the reviewed HEAD, the multi-session owner file has about 3,751 lines before its main test modules and about 5,600 embedded test lines. The compositor has about 2,147 lines before its main tests, followed by about 2,270 test lines. Neither needs to be broken into arbitrary tiny modules.

The goal is locality: an agent changing history behavior should normally edit the history module and its behavioral tests, rather than learn workspace lifecycle, menu state and rendering first. Keep the existing useful interfaces and place state beside the rules that govern it.

## Baseline and evidence

- Reviewed checkout: `/Users/sahillamba/panelink`, clean `main`, HEAD `daf28a63f062331234ffd63daba577101d3315fa`.
- Locally recorded `origin/main`: `a1134ec4bc69e8a97b27ba2b0b61f2e446257440`, five commits ahead. This is a local remote-tracking observation; no fetch was performed for this plan.
- Newer changes include scoped wheel delivery, capability negotiation, fresh-unsent wheel FIFO behavior, compact modal geometry and an event fixture repair. Plan against these changes, not just the older HEAD.
- At the unchanged reviewed HEAD, the prior turn's canonical verifier passed 8/8: root Rust 1,576 passed / 0 failed / 17 ignored; decoder 39 passed; Python 232 passed / 1 skipped. Log: `/private/tmp/panelink-code-health-verification.log`. That result does not validate the newer baseline or any future refactor.
- This planning turn performed source review and artifact validation, not a new application test run. No live panes, install, source edits, commits or pushes.

## Read in order

1. [Specification](SPEC.md): goals, preserved behavior, acceptance.
2. [Execution and ownership](EXECUTION.md): source pointers, checks, dispatch rules and handback format.
3. One ready ticket from the table below. Read only its additional references.

## Proposed task graph

| Ticket | Deliverable | Blocked by | Lane |
| --- | --- | --- | --- |
| [00](tickets/00-baseline.md) | Current baseline, measured source inventory and test map | None | Parent |
| [01](tickets/01-host-dispatch.md) | One host event-dispatch path | 00 | Host |
| [02](tickets/02-pane-viewport.md) | Pane viewport/history state with owned transitions | 00 | UI state |
| [03](tickets/03-workspace-presentation.md) | Workspace tab presentation and metadata state | 02 | UI state |
| [04](tickets/04-pane-interaction.md) | Pane interaction state and explicit event ownership | 03 | UI state |
| [05](tickets/05-rendering.md) | Rendering organized around bounded views and shared geometry | 04 | UI state |
| [06](tickets/06-positive-barriers.md) | Positive split-completion fixture barrier | 00 | PTY tests |
| [07](tickets/07-server-test-layout.md) | Test-heavy server modules organized by scenario | 00 | Server test layout |
| [08](tickets/08-integrate.md) | Combined review, canonical checks and current documentation | 01, 05, 06, 07 | Parent |

The UI-state chain is deliberately serial because those tickets migrate the same fields, methods and behavioral tests. Their ordering is an integration constraint, not a product dependency. The host, PTY fixture and server test-layout lanes can run alongside it with exclusive files. Start small: finish 01 and 02, review the interfaces, then continue the proposed chain. Ready means blockers completed and the parent has assigned exclusive ownership.

## Skill route and publication status

`ask-matt` routes this to `improve-codebase-architecture` for the survey, `codebase-design` for module shape, then `to-spec` and `to-tickets` for synthesis and agent-sized work. The user already endorsed the maintenance priorities; the module shapes here remain proposals to review, not confirmed product decisions.

Read sources: local `ask-matt`; recovered session-only architecture/design/setup skills at revision `59db609eb1e18aaf59b63284d0e52255aff0c63c`; `to-spec`, `to-tickets`, and `writing-for-agents` read from upstream revision `a496f5513d4a4cf889bc48e189e579f4b0bdf6aa`. No permanent skills were installed.

Repository tracker configuration was not found. Before tracker publication, run `setup-matt-pocock-skills`, choose the tracker, confirm the proposed test interfaces and ticket breakdown, then publish one ticket per issue with blocking relationships. The setup skill says “Explore, present what you found, confirm with the user, then write.” The tickets skill requires approval of the breakdown before publication. This portable draft leaves that step open and does not change agent steering files or create GitHub issues.

## Deferred follow-ups

- The CLI contains roughly 4,068 lines before its main test module: later separate cohesive command families, starting with close/status exchange, while keeping context reservation and acknowledgment rules together. Do not reorganize all commands in one change.
- The runtime driver mixes lifecycle, wire handling and dispatch. A pending-close module is worth exploring after the initial work, but only if it can own capacity/deadline/completion rules without exposing the owner's internal state.
- Keep the multi-session authority core together unless a separate design proves how ledger, input ownership, layout and lifecycle invariants stay serialized. Moving its test modules is useful navigation work; it is not proof of a better production architecture.
- Do not add a dependency, plugin framework, generic event bus or module-size CI limit to accomplish this plan.
