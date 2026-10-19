# P3A wave 2 — trusted in-process split transaction

Status: implemented 2026-10-19 as trusted library core in
`src/server/multi_session.rs`. Worker output for parent review. It
covers the in-process owner only. There is no socket or runtime route
(`ServerDriver` still owns `StateOwner`), no CLI, no UI, no capability bit and
no durable layout store. It does not complete Phase 3A.

## API

```rust
impl<E: EffectSink> MultiSessionOwner<E> {
    pub fn layout_snapshot(&self) -> Option<LayoutSnapshot>;
    pub fn pending_resizes(&self) -> [Option<PendingResize>; 2];
    pub fn execute_split<P, PE, SE, RE, J, Prepare, Spawn, Resize>(
        &mut self, payload: &[u8], now_secs: u64, journal: &mut J,
        prepare: Prepare, spawn: Spawn, resize: Resize,
    ) -> Result<SplitResult, SplitError>
    where J: LaunchJournalPort,
          Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
          Spawn: FnOnce(P) -> Result<E, SE>,
          Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>;
    pub fn resize_layout<RE, Resize>(
        &mut self, expected_revision: u64, rows: u16, columns: u16, resize: Resize,
    ) -> Result<LayoutSnapshot, LayoutResizeError>
    where Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>;
}
impl PtyEffects { pub fn resize(&mut self, rows: u16, columns: u16) -> Result<(), Outcome>; }
```

New public types: `SplitError` (`InvalidPayload`, `GenerationMismatch`,
`Ledger`, `InProgress`, `Failed { reason, result }`, `InternalInvariant`),
`SplitFailure` (`NoLayout`, `StaleTarget`, `StaleLayout`, `Capacity`,
`Unhealthy`, `Registry`, `Layout`, `LaunchSizeMismatch`, `PreparationFailed`,
`JournalPreflightFailed`, `LaunchFailed`, `PreviouslyFailed`) and
`LayoutResizeError` (`NoLayout`, `Layout`, `ResizePending { snapshot, failed }`)
and `PendingResize { target, desired, confirmed }` (redacted target in `Debug`).
Messages are static. `Debug` redacts the ledger error and the result. Callback, journal
and resize diagnostics are discarded.

`EffectSink` has no resize hook. The resize port is therefore a caller
closure `FnMut(&mut E, rows, cols)`. The owner applies it to the concrete
retained `E` (inside `TargetState`) through the coordinator's crate-private
`session_mut`. `PtyEffects::resize` is the real implementation. It validates the
size with `TerminalSize::new`, coalesces it with `request_resize`, and applies
it through one bounded `pump`. The pump makes output progress and queues no
input. It then checks the accepted size and advances the screen revision.
A pump error fails the adapter closed and reports `Unknown`, as `poll` does.

## Layout ownership

The owner holds a private `Option<PaneLayout>`. A layout is established when
the first `execute_launch` reaches `Applied` while no layout exists. It starts at
revision 1, with that target as the single pane and the launch rows and
columns as the root. The launch result is never altered. If `PaneLayout::new`
rejects the root, the layout stays absent and splits fail with `NoLayout`.
Schema-6 bounds equal the pane bounds, so that cannot happen today. Later
`execute_launch` sessions are retained outside the layout. They are not panes
and cannot be split.

## Ordering contract

1. Decode with `decode_split_request`; malformed → `InvalidPayload`, no ledger effect. Key generation ≠ owner generation → `GenerationMismatch`.
2. `Ledger::existing_admission(key, complete split bytes)`. An exact duplicate returns the stored `SplitResult` for a committed split. For a Failed split it returns the identical `Failed` result. While the key is still accepted it returns `InProgress`. All of this happens **before** any layout, target, health or capacity check. Changed bytes return `Ledger(OperationConflict)`, even at capacity. Lease, sequence and capacity errors are returned from the ledger unchanged.
3. `Ledger::admit` the complete split bytes as the key's only ledger record. The embedded launch is never admitted separately: its key equals the split key, and a second admission with different bytes would conflict.
4. New key only (`plan_split`): the layout must exist; the target must be retained with effects and be a layout pane; `expected_layout_revision` must equal the current revision; the coordinator must be healthy; `registry.next_target()` must be available. Then `PaneLayout::preview_split(target, rev, direction, new_target)` runs. The embedded launch `(rows, columns)` must equal the planned new pane exactly. Any rejection → ledger `Failed`, `Err(Failed{reason, result})`. No ID is reserved, the layout is unchanged, and no prepare, spawn or resize runs.
5. `ManagedLaunchCoordinator::execute_admitted` runs the unchanged journaled path for exactly one child: Intent, prepare, preflight, reserve, Intent record, spawn, attach, Spawned, Applied. It keeps the retained-handle-on-uncertainty and emergency-slot behavior. Prepare or journal-preflight failure reserves nothing → `Failed`. An Intent-record or known spawn failure spends the identity (as for launches) → `Failed`, no commit.
6. **Commit point: after the child is acquired.** The layout commits only once the coordinator's retained result is `Applied` or `Unknown` for the planned target.
7. Absolute resize of the existing session to its new rectangle.
8. `Ledger::complete(key, outcome)` runs, and the `SplitResult` is retained in the owner, so a duplicate returns the identical value and encoded bytes. If completion fails, the retained result becomes `Unknown`.

Why commit after acquisition rather than PANE-LAYOUT's "commit a `starting`
leaf before process effects": the schema-7 codec and `PaneLayout` have no
`starting`/`launch_failed` leaf state. A pre-spawn commit followed by a known
spawn failure would leave a committed pane with no session. It would also
leave a spent identity that the two-session cap can never reuse, and the
original pane would be resized for a sibling that does not exist. Committing
after acquisition keeps every known pre-acquisition failure as a clean
`Failed` with the prior tree intact. `P3A-RIGHT-CLICK-SPLITS.md` requires
exactly this for pre-spawn rejection. It also means a committed split always
contains a retained child. Consequence: a known spawn failure is `Failed`, not
`Partial`; `Partial` arises only from the existing-session resize. Parent
review should confirm this deviation from the PANE-LAYOUT recommendation.

## Outcomes

| Situation | Outcome | Layout | Child | Resize | Identity |
| --- | --- | --- | --- | --- | --- |
| Malformed / wrong generation / ledger conflict or capacity | error, not admitted | unchanged | none | none | unspent |
| No layout, stale target or revision, capacity, size mismatch, geometry, unhealthy | `Failed` | unchanged | none | none | unspent |
| Prepare or journal-preflight failure | `Failed` | unchanged | none | none | unspent |
| Intent-record or known spawn failure | `Failed` | unchanged | none | none | spent (`Failed`) |
| Applied launch, existing resize ok | `Applied` | committed rev+1 | retained | done | spent |
| Applied launch, existing resize fails | `Partial` (resize pending) | committed | retained | pending | spent |
| Spawned/Applied journal fault or attach fault | `Unknown` | committed | retained (`Unknown`) | attempted | spent |
| Exact duplicate | stored result | no change | no spawn | none | — |

The original session is never stopped, relaunched or reverted, and no input
is sent to either child. The split claims no input ownership (both owners start
with lease 0 in the real-PTY test).

`resize_layout`: `PaneLayout::resize` validates the revision and the bounds of
every resulting leaf. A stale, invalid or too-small request returns
`Layout(..)`; nothing changes and no PTY is resized. Otherwise the new
revision commits first, then each pane session gets one absolute resize.
Failures return `ResizePending { snapshot, failed[i] per pane }` with the
layout kept.

Pending-resize state (reviewer fix, 2026-10-19): the owner keeps bounded
`[Option<PaneSizeState>; 2]` keyed by full target. Each entry holds the last
confirmed `(rows, columns)` and, while pending, the desired layout rectangle.
It is seeded at layout establishment (the launch size) and at split commit
(the new child's spawn size). Any failed absolute resize, whether the
split's existing-pane resize or a root resize, marks that pane pending; any
success clears it. `resize_layout` at the **current** size keeps the
revision. It retries only the pending panes, once each, at their current
rectangle, and returns `ResizePending` again (same revision) if a retry fails.
With nothing pending it has no PTY effect. Non-pending panes are never
resized by a same-size call. `pending_resizes()` exposes the desired versus
confirmed size so a UI can show a resize-pending pane.

## Coordinator refactor

`execute` keeps its public behavior and all existing tests unchanged. The
post-admission body moved into private `run_reserved(.., ledger:
Option<&mut Ledger>, ..)`. With `None`, the caller owns ledger completion.
`finish_pre_spawn`, `finish_post_spawn_unknown` and the emergency path complete
through a `complete_owned` helper, which is a no-op for `None`. The test
helper `retain_emergency_after_attach_error` is kept, wrapping
`retain_emergency_inner`. New crate-private methods: `execute_admitted`,
`retained_result` and `next_target`. `execute_admitted` refuses a key the
coordinator already holds or a target that differs from the next identity,
before any callback.

## Tests

- `src/server/multi_session.rs` `split_tests` (14): layout establishment; Applied split in all four directions (rects, revision 2, spawn once with the launch size, one resize of the existing pane to its new rect, effect order); duplicate after commit returns identical value/bytes with no effects after the revision advanced; direction/target/revision/one-env-byte conflicts at capacity with no effects; stale revision/target, size mismatch and too-small geometry → `Failed` with IDs, registry and layout unchanged, retries identical; third session → `Capacity`; no layout → `NoLayout`; malformed/wrong generation not admitted; prepare and journal-preflight failure → `Failed`, unspent, redacted; known spawn failure → `Failed`, no commit; Spawned/Applied journal fault → `Unknown`, committed, child retained, duplicate does not spawn; resize failure → `Partial`; `resize_layout` no-op, too-small, stale, success sizes and per-pane pending; no input recorded anywhere.
- `src/managed_launch.rs` (+2): `execute_admitted` runs the journaled path while leaving the caller's key `InProgress`; it refuses a repeat key, an unplanned identity, and prepare/preflight failure without spending.
- Reviewer-fix tests (replacing the earlier workaround test that changed dimensions): split with a failing resize → `Partial`, with `pending_resizes()` showing the existing pane's desired/confirmed size. A same-size `resize_layout` that still fails → `ResizePending`, revision unchanged, only the pending pane retried. Then success → `Ok`, revision unchanged, pending cleared, exactly one call. A further same-size call has no effect. The same sequence is covered for a failed root resize (only the failed pane retried).
- `tests/p3_split.rs` (2 real-PTY tests). The required proof runs `/bin/zsh -f` (NO_RCS) for both children. A cheap `/bin/sh` variant shares the same scenario. Explicit environment: `TERM`, `PATH=/usr/bin:/bin`, and `HOME`/`ZDOTDIR` pointing at an empty private 0700 directory (asserted empty). Nothing else is inherited. READY also prints a `$ZSH_VERSION`-derived sentinel, asserted present for zsh and absent for sh. A mutation that swapped zsh for sh made the zsh test fail. The server storage lock is held for the owner's lifetime. Each scenario checks: first child READY/PID; Split Right via schema 7; second child READY with a distinct PID; first PID unchanged and alive; claim plus Submit on each, with each ACK only on its own target; `stty size` reports 24x40 for both; exact lost-ack replay → identical result with no prepare, spawn or resize, revision unchanged, no ACK duplicated, and the third launch hits the cap; `resize_layout` to 30x101 → both report 30x50; a too-small resize is rejected; drop reaps both PIDs.

## Verification (2026-10-19, macOS aarch64, Rust 1.99.1)

- `cargo test --locked --offline --lib server::multi_session`: 31 passed.
- `cargo test --locked --offline --lib managed_launch`: 36 passed.
- `cargo test --locked --offline --test p3_split`: 2 passed (zsh and sh), repeated 5× green.
- `cargo test --locked --offline`: 513 passed, 0 failed, 14 ignored (pre-existing `#[ignore]` tests). The known flakes did not fire on this run.
- `cargo fmt --check`, `cargo clippy --locked --offline --all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps`: clean.

## Limits and open items

- No runtime route: unreachable from sockets, CLI or UI until the parent migrates `ServerDriver`.
- Volatile only: the ledger, layout and retained split result live in memory. A crash between Intent and layout commit can leave a journaled child that has no layout. There is no cross-crash exactly-once claim, and layout/journal persistence ordering remains open.
- The original `Failed` reason is not retained; a duplicate reports `PreviouslyFailed` with the identical result.
- Pending sizes are owner-observed: "confirmed" means the resize port reported success, not an independently measured PTY size. The state is volatile, and a resize-pending pane is not yet rendered or routed by any UI.
- `src/server_pty.rs` received the `PtyEffects::resize` method (outside the listed files; required for the real resize port).
