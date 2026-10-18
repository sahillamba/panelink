# P3A — right-click splits and fresh terminal sessions

Status: parent-reviewed phase scope and acceptance plan, requested by the user
on2026-10-18. This phase is inserted before the queued lifecycle/stop and
history work. It is not an implemented menu, split operation, runtime route,
or host compatibility result. Exact source tasks, wire/transaction interfaces
and proposed defaults still require scoped adoption before implementation.

## Outcome and boundaries

On a supported interactive host, right-clicking an occupied pane captures that
pane's complete target identity and current layout revision. Choosing Split
Left, Right, Up, or Down creates exactly one new independent zsh PTY in the
corresponding position. The original session and process remain alive and are
resized to their new rectangle. The new pane is independently interactive.
The same actions remain reachable through a keyboard menu command if the host
or the running application consumes right-click.

This plan reuses the split tree, direction ordering, equal-ratio rounding,
target capture, layout revision, overlay behavior, and failure vocabulary in
[`PANE-LAYOUT.md`](../PANE-LAYOUT.md). It does not add another geometry/API
model or settle workspace/tab/sidebar hierarchy. The prototype supports at
most two retained sessions. Once both slots are occupied, all split actions
are truthfully disabled with a capacity reason and the server rejects a stale
client attempt before effects. Raising that cap is a separate reviewed change.
Do not add unrelated context-menu items.

The existing one-child server runtime and pathname/launch gates stay in force
until the parent separately adopts and verifies runtime integration. The
trusted `MultiSessionOwner` library tests, including two real child sessions,
are evidence for its core only; they do not establish that a socket client,
interactive terminal UI, or right-click action can reach it. In particular,
`src/server_runtime.rs::ServerDriver` still owns `StateOwner`, and its current
request dispatch does not route split or schema-6 launch requests. Do not
replace or bypass that runtime gate as part of a UI-only slice.

## Reused contracts

- Geometry is integer cells in the existing binary split tree. The divider
  consumes one cell. Initial ratios are 1:1; the first child receives the floor
  half. Left/right need at least 41 target columns; up/down need at least 11
  target rows. Every resulting PTY remains between 20 and 240 columns and 5
  and 80 rows. The existing plan's 240-column by 80-row content cap applies.
  The disabled reason is visible, and the server recomputes feasibility.
- Capture the exact clicked `TargetRef` and `layout_revision` from one state
  snapshot when the menu opens. Never retarget to current focus on activation.
  Keyboard invocation captures the focused target/revision when invoked.
  Activation submits those captured values unchanged. A stale target or layout
  rejects before PTY or tree effects.
- Use one generation-wide operation key and exact payload deduplication. A
  same-key retry returns its original result before layout/capacity
  preconditions; changed direction, target, layout revision, or launch data
  conflicts before effects. Do not replay input to either child.
- Capture one explicit client `LaunchContext` for the new child, including its
  cwd and environment. Follow `src/launch.rs` resolution/environment rules;
  never inherit the server's ambient environment or copy the existing child's
  context. Environment values and sensitive arguments remain volatile and out
  of journals, persisted metadata, and diagnostics. The initial launcher is
  exactly one fresh zsh, subject to the existing executable/path and pathname
  runtime gates. Do not run shell text from the existing pane.
- A split never implicitly claims input ownership, stops the original child,
  or grants future launch/stop authority. Client lifetime remains separate
  from server-owned PTYs. A spawned child whose result becomes uncertain stays
  retained/reconcilable under the launch coordinator contract; do not spawn a
  second child or kill the original as rollback. A proven pre-spawn rejection
  leaves the tree and both existing session state and process untouched.
  Once the layout is committed, preserve PANE-LAYOUT's distinct outcomes:
  known spawn/resize failure leaves the committed split visibly failed or
  resize-pending with Partial; uncertain spawn/result retains the acquired
  child with Unknown. Do not call a post-commit failure a rejected split or
  silently restore the old tree. Layout persistence and launch journaling need
  an explicit joint ordering contract; the current volatile ledger is not a
  durable layout store.
- The menu overlay owns its mouse and keyboard events through close/activation;
  menu navigation and activation bytes must never enter the child input queue.
  Ordinary output uses incremental rendering. Full repaint is appropriate for
  initial/invalidated state or dimension changes per the existing renderer,
  not as the ordinary split redraw strategy.
- Selecting the new pane for focus after a successful split is a proposed
  prototype default, not a confirmed product decision. It changes only local
  presentation focus; it does not transfer the input-owner grant.

The split request and its wire representation are not implemented. The
existing `ControlRequest` schema has no layout operation, and the existing
schema-6 managed-launch request does not by itself express the captured layout
revision/direction preconditions. Parent review must define the exact split
operation payload so exact-byte deduplication covers the complete request.
Do not treat a launch-only key/payload as sufficient split deduplication or
freeze a new schema in this planning task.

## Source-backed implementation stages

Each stage should have one exclusive owner and a parent-reviewed interface
before the next stage consumes it. The paths below are proposed ownership
areas; shared module exports and integration remain parent-owned.

1. **Pure two-pane layout and admission contract.** Add a small, dependency-
   free layout module (proposed `src/pane_layout.rs`) and its unit tests. Reuse
   the exact node/order/rounding rules in `docs/PANE-LAYOUT.md`. Implement
   layout revision overflow rejection, full-leaf min/max validation, one
   split only, and capacity-disabled state. Add no terminal IO or process
   launch here. Parent owns any `src/lib.rs` export and reviews the request and
   result types. This is groundwork, not phase completion.
2. **Trusted split transaction and existing runtime adoption.** Extend the
   new multi-session core in `src/server/multi_session.rs` only after the
   parent adopts a split payload and ordering contract. It must preserve
   full-payload/global-key dedup before layout preconditions, reserve the
   second identity/capacity before effects, call the already-reviewed managed
   launch path for exactly one child, and retain uncertain post-spawn handles.
   Then, in a separately owned `src/server_runtime.rs`/startup integration
   slice, the parent must deliberately migrate routing from the current
   `StateOwner` driver to the multi-session owner while holding the existing
   storage lock for the owner's whole lifetime. Preserve authentication,
   pathname gates, bounded workers, and existing one-child behavior until
   that migration passes its gate. Do not reinterpret the current `zsh` launch
   confirmation as blanket launch or stop authorization.
3. **Interactive pane host, menu, and client routing.** There is no current
   production multi-pane UI to extend. `src/p2_probe.rs` is a disposable
   one-child loop; it is not the product UI and must not become a shortcut
   that moves PTY ownership into the client. After parent selects the
   interactive host entrypoint, add the smallest new UI module/entrypoint
   needed to own the layout snapshot, mouse hit testing, overlay, focus, and
   incremental pane composition; keep PTY handles in the adopted server
   runtime. Use the existing bounded parser/types and `terminal::render`
   contracts rather than adding a host input framework. At menu-open time,
   retain the clicked target and revision; at submit, send those exact values
   plus the explicit captured client launch context through the reviewed
   split route. Keyboard fallback invokes the same menu and action handler.
   Document the actual Apple Terminal mouse setup only after testing it.
4. **End-to-end proof and phase exit.** Parent integrates the exclusive slices,
   runs the complete automated matrix below, and performs the available
   supported-host fixture/checks. Phase exit requires the user-visible
   right-click action to produce two real interactive child sessions while
   preserving the first, plus a working keyboard fallback. A pure geometry
   suite, rendered/mock menu, or two-child core test alone is explicitly
   insufficient. Record any environment-blocked host acceptance as blocked,
   not passed; do not bypass the existing pathname/runtime launch gate.

## Required automated checks

Tests should use deterministic fake effects for pure ordering and real private
PTY/socketpair fixtures where the existing test harness supports them. Keep
the ownership and assertions specific:

- **Geometry:** each of the four directions places the new pane on the
  expected side; odd dimensions use first-child floor rounding; exactly 41
  columns/11 rows succeeds and one cell less fails; each leaf's 20x5 minimum
  and 240x80 maximum is enforced; divider is one cell; revisions increment
  once, stale/overflow revisions reject; unrelated pane geometry is stable.
- **Menu and input ownership:** pointer hit tests choose the pane under the
  pointer even when another pane is focused; captured target/revision remain
  unchanged if focus moves before activation; keyboard invocation selects the
  focused target once. Menu open/navigation/activation/close produce no child
  input bytes. An inner application in mouse-reporting mode cannot receive a
  menu-owned event. All four directions remain visible and disabled reasons
  fit/clamp within the bounded overlay. Capacity-full and too-small states
  disable truthfully.
- **Dedup and ordering:** a retry after a committed split returns the same
  target/session and layout revision and launches exactly one child. Same-key
  changed direction/target/layout/launch payload conflicts before stale
  target/layout checks and before prepare/spawn. A distinct stale-layout
  operation rejects without ID, layout, resize, or process effects. A third
  session attempt reports the exact bounded-capacity error without spawn.
- **Two real children:** run the production launch-preparation/spawn route
  with two zsh PTYs. Use unique READY/PID and request/ACK sentinels to prove
  both shells are alive and accept input independently, each response arrives
  only from its target, and a split does not relaunch or terminate the original
  PID. Repeat a lost-ack split request and prove one child/one committed split.
  Keep the storage lock alive for the owner lifetime and assert both children
  are reaped on owner teardown. Do not resend any PTY input as a retry.
- **Failures and resize:** rejection before layout commit leaves the prior
  layout/session intact; a known spawn failure after commit retains the failed
  pane with Partial. Injected post-spawn journal/result uncertainty
  retains the child and blocks duplicate spawn. Verify absolute resize reaches
  both children at their assigned geometry, ordinary output changes use the
  existing diff renderer, and a terminal resize below a leaf minimum preserves
  the last accepted layout and children until enlargement.
- **Host fixture:** where a deterministic host fixture is available, inject
  actual right-button input through the event decoder/dispatch boundary and
  prove it opens the menu and can launch the second PTY. Also drive the
  keyboard fallback through that same action route. Otherwise preserve these
  as explicitly unverified host checks; mocks do not establish Apple Terminal
  delivery.

## Source evidence and unresolved gates

The existing design source is [`PANE-LAYOUT.md`](../PANE-LAYOUT.md), with the
canonical R-08/R-16 requirements and launch-environment rules in
[`SPEC.md`](../SPEC.md). Source discovery used the repository graph before
reading definitions. Current implementation facts relevant to scope:

- `src/server/multi_session.rs::MultiSessionOwner::execute_launch` provides a
  trusted two-session launch core and tests, but expressly leaves runtime
  routing and launch policy to its caller. The latest
  [`INTEGRATION.md`](INTEGRATION.md) says the two-child real-PTY proof is
  in-process core evidence only.
- `src/server_runtime.rs::ServerDriver` still contains a single
  `StateOwner<E>` and dispatches control/session/watch request schemas, not a
  split action or schema-6 launch route. The pathname bind/accept proof is
  still recorded blocked by EPERM in the latest integration checkpoint.
- `src/p2_probe.rs::run_loop` owns one disposable local `TerminalSession`;
  `dispatch_event` dispatches keyboard/paste events. It is neither the
  persistent multi-pane UI nor a place to move server-owned PTYs. The current
  renderer in `src/terminal/render.rs::render_diff` supplies incremental
  snapshot output, not pane-tree composition or mouse menus.
- `src/launch.rs::LaunchContext`, `build_environment`, and `resolve_launch`
  provide explicit client-context environment construction and executable
  resolution. A future UI must capture/pass that context for each new launch;
  server ambient environment is not a fallback.

The `MultiSessionOwner` constructor and split wire/runtime route require parent
adoption before any new UI code can claim the feature. The user-requested phase
does not decide how to widen existing lease grants to future targets, nor
does a grant or right-click authorize stop. Current launch authority, pathname
proof, and host mouse delivery remain separate gates. Focus-new-pane is the
only noted presentation default and remains a prototype assumption.
