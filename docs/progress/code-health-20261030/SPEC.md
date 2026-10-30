# Proposed specification: behavior-preserving maintenance

## Problem statement

Maintainers and coding agents must navigate large mixed-purpose modules to change one terminal interaction. Shared state makes correctness reviews broad and makes parallel edits collide. Raw file length also overstates some problems because embedded tests account for much of the code.

## Solution

Keep Panelink's current behavior and public interfaces while moving cohesive state and transitions into deep private modules. Keep one small coordinator for cross-module ordering. Organize test-heavy modules separately where it improves navigation. An extraction earns its place when it hides rules and improves locality; passing the entire application to a helper does not qualify.

## User stories

1. As a maintainer, I can find all normal-history transitions together so stale replies and cursor changes are reviewed in one place.
2. As a maintainer, I can distinguish fresh unsent wheel gestures from uncertain admitted input so refactoring does not cause replay.
3. As a user, I retain the same scrolling, focus, copy, menus, tab dialogs and detach behavior.
4. As an agent, I receive an exclusive work area and exact blockers so parallel edits do not overwrite another worker.
5. As a reviewer, I can verify one extraction without reviewing a simultaneous protocol or product change.
6. As a maintainer, I can change rendering without reimplementing pointer hit geometry.
7. As a maintainer, I can locate a server scenario's tests without scanning thousands of unrelated test lines.
8. As an integrator, I get evidence from the combined tree and preserve all separate host/provider acceptance gates.

## Proposed implementation decisions

- Keep the existing App/effects interface as the primary behavioral test surface during migration. Production worker and recording test adapters already justify that seam. Prefer private concrete modules for in-memory state; introduce no extra traits without a real second adapter.
- Let a pane viewport module own live/history identity, bounded caches, cursor arithmetic, in-flight history requests, coalesced normal motion and fresh-unsent wheel intent state. It exposes narrow transitions and read-only presentation views. The application coordinator decides modal/route admission and applies returned notices; network submission continues through the existing effects adapter.
- Keep workspace tab presentation, picker/dialog state, request identity and metadata completion rules together. Let route preparation and disk/socket work stay in their existing worker adapters. The presentation module produces bounded intents; it performs no host or storage I/O.
- Group pane interaction state around captured targets, modal event consumption, cancellation and selection identity. Preserve the present precedence rather than replacing it wholesale with a new global state-machine design. Decide selection ownership after the viewport extraction; there must be one authoritative owner and an explicit invalidation path.
- Render from immutable views of these modules. Keep frame construction separate from frame acceptance/commit. Use one geometry definition for rendering and pointer hit targets. Reuse the compositor's existing frame-diff implementation.
- For each migration, replace the old state owner and its transition logic in the same green change. Temporary compatibility methods are acceptable within a ticket, but remove pass-through scaffolding before its acceptance. Avoid duplicated caches, expensive screen cloning, new dynamic allocation in hot paths or broader visibility solely to satisfy extraction.

## Preserved behavior

- Exact target identity includes generation, workspace, pane, session and occupant revision. Captured target and layout revision validation remain authoritative.
- Input admission, durable metadata reservation, acknowledgment, delivery and provider completion remain distinct. Reserve/send once; reconcile uncertain operations by existing receipts/status. No uncertain input replay or ownership stealing.
- Preserve the latest scoped wheel capability negotiation and authoritative server checks. Fresh unsent alternate-screen gestures may use the existing bounded FIFO (32 entries, one-second age limit); preserve order, coordinates, expiry and invalidation. Admitted or uncertain events never return to that FIFO. Preserve refusal of retained claims, including same-caller claims, and legacy/new-peer behavior. History seeking remains read-only. Do not change queue/TTL limits in a structural refactor.
- Normal-history coalescing, cursor/RIS epoch handling, newest absolute seek and stale-reply rejection remain unchanged. Preserve cross-pane scheduling gates, one-in-flight rules and alternate-wheel/seek/normal-motion priority; a per-pane owner must not accidentally remove application-wide admission checks. Keep visible content while a read is pending. Wheel/scrollbar/divider operations retain typing focus.
- Preserve copy's frozen view, Unicode/wrap semantics, source-scoped feedback, repeat/release swallowing and cancellation on route/buffer/layout/focus changes.
- Preserve cancel-default close confirmations, captured close targets, opening-event consumption, modal key/paste ownership and compact render/hit geometry.
- Preserve request-ID and route matching for metadata completions, unknown create intent reconciliation, partial/unknown tab-close visibility, empty workspace behavior, and confirmed-server-end exit.
- Keep the bounded render viewport, preallocated screen caches, output baseline advancing only after accepted writes, host restoration order and client/server lifetime separation.
- Wire schemas, JSON output, storage records, dependency graph and public commands remain unchanged.

## Testing decisions

Keep behavioral tests at the existing App/effects and real private-PTY interfaces. Move tests with the behavior they protect only when the new module is the real caller interface; preserve scenario intent, assertions and identifiers where practical. Add a focused characterization only for a meaningful uncovered invariant; test counts alone are not acceptance. Preserve observable rendering, exact effect counts/targets, failure outcomes and restoration bytes. Test expiry using supplied monotonic time and completion using positive observations rather than new sleeps.

The deletion test applies to each new module: deleting it should spread its rules back into callers. If deleting it merely removes a forwarding layer, redesign or abandon that extraction. Review interface facts and ordering requirements, not just method count.

## Acceptance

Each ticket lands independently verifiable, respects its file lease and removes its superseded implementation. The final combined tree passes the canonical non-UI gate. Reviewers can trace a history change, a tab metadata completion and a pane confirmation through their owning modules without understanding unrelated state. Report actual before/after production/test sizes and cross-module accesses; no arbitrary line-count quota may drive a split. Native host, clipboard, reboot, provider and performance acceptance remain separate.

## Out of scope

New features; changed input ownership/retry semantics; revised shortcuts or layout limits; changing phase gates; installing or restarting Panelink; live pane input; new dependencies; performance guarantees; sweeping renames; deleting historical evidence; committing, pushing or publishing tracker issues as part of this planning task.
