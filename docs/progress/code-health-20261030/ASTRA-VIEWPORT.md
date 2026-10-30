# Astra viewport source review

Reviewed frozen HEAD `80c9ffb0204c42efafe70a4ca904d4c2ca67541a` against base `a1134ec4bc69e8a97b27ba2b0b61f2e446257440` in the requested checkout. Read AGENTS.md and the code-health SPEC.md. Used the named codebase-memory project for discovery, then verified current source against `git show` of the base.

## Result

No material findings within the completed scope below. This is a bounded behavior-preservation source review, not whole-integration approval.

## Completed scope

- `src/ui_client/state/viewport.rs:199–348` and `state.rs:1718–1738`: exact full TargetRef plus view-epoch completion matching, stale replies, copy cancellation timing, history/failed/mouse/no-mouse outcomes, and status/dirty propagation. A stale epoch clears only that exact target's fresh alternate queue, without clearing its current pending read; absent targets do not mutate a replacement occupant. This matches the base.
- Cursor arithmetic and RIS/rollover handling in `viewport.rs:233–259, 645–683`: same-epoch forward ordinal deltas saturate offsets/extents at the protocol maximum; backward ordinals do not move the live base backward. Changed cursor epochs preserve frozen content and mark truncation. A history reply from a different cursor epoch is superseded rather than installed. Latest queued absolute seeks continue to suppress older frames. These are the original policies, including their distinction from the independently incremented view epoch.
- Checkpoint/wrap failure ordering in `viewport.rs:260–303, 610–688` and `state.rs:1976–2012`: live checkpoint/wrap loading precedes target/revision publication and buffer-change handling; history extent/base bookkeeping precedes decode/load. Invalid history wraps clear wraps and queued intents, publish the error, and return before committing offset/revision. Neither path was made transactional by this extraction; its existing mutation/error order is retained.
- `ViewportChange::finish_extent` preserves the original early returns: false for stale identity/epoch and invalid history wraps, true after the remaining completion branches, including decode/load errors. The App applies dirty/status before conditionally invoking `finish_unknown_extent_capture`, matching the original control flow.
- Extent and seek reply integration in `state.rs:1740–1833` and `viewport.rs:120–145, 374–418`: unknown extents do not cap future reads, known extents clamp against the greater of total/current offset, latest requested offsets replace queued seeks, and deferred thumb release is consumed once. Extracted `queue_seek` preserves the original requested-offset comparison. Extent capture and output-driven pointer refresh retain their geometry and layout checks.
- Lifecycle scopes in `viewport.rs:112–198, 349–373` and corresponding App route freeze, host resize, `set_layout`, return-to-live, and load-screen calls: route/host invalidation clears pending/queued work and advances every slot epoch without erasing visible caches; geometry change resets history on affected exact targets; removal clears slot identity/live revision; return-to-live clears the target's history/request state and advances its epoch. Buffer changes cancel history and gestures before App selection/divider/pointer cleanup. Layout reconciliation still receives the old layout before App publishes the replacement.
- Live update admission at `state.rs:1673–1709` retains exact layout-target membership and only refreshes pointer seek after successful loading.

Supporting assertions read, not executed: `state_tests.rs:3114–3214` (cursor delta, epoch truncation, eviction clamp, older-server fallback), `3220–3297` (extent discovery and one-time released seek), `3580–3625` (visible content while pending), and `3647–3681` (typing/layout/failure invalidation and stale completion). They support the source trace; they are not fresh test evidence.

## Unresolved assigned paths and evidence limits

Unresolved paths within this assignment: none. Flush scheduling and the full alternate FIFO pass were deliberately left with the earlier ASTRA-REVIEW.md partition. No tests, builds, panes, installs, commits, pushes, or nested agents were run. Parent-reported canonical 8/8 is supporting evidence only and was not independently reproduced. Only this report was written.
