# Pane layout and four-direction split plan

Status: proposed design for the confirmed right-click-menu requirement and the user's four-direction split request. This document is planning only: it defines no implemented UI, command, wire schema, or runtime behavior. The wider workspace, tab, sidebar, and session-lifetime choices remain open.

Execution priority2026-10-18: the user inserted
[Phase3A](progress/P3A-RIGHT-CLICK-SPLITS.md) before the next lifecycle/stop
and history work. A split must create a fresh terminal session on the chosen
side while preserving the original session. The phase starts with the actual
two-session core capacity; the larger resource figures below remain proposals,
and no menu/runtime implementation or host acceptance is inferred.

## Scope and authority

The user confirmed right-click context menus in requirement R-08, requested four pane-relative split directions, and confirmed the agent-to-pane control feature in R-15. This proposal adds a split operation to the R-15 CLI/API design and uses its full `TargetRef`, operation key, retry, launch, and input-ownership contracts. It does not settle the surrounding workspace/tab hierarchy or the process lifetime after a client exits.

The scope is one split-tree leaf and its newly inserted sibling. Splitting preserves the existing pane view and its managed session; it creates one new pane view and one new managed terminal session using an explicit launch request. It does not introduce autonomous agent scheduling, copy terminal input, replay shell commands, or infer an agent identity.

Parent review is required before this design is added to the canonical specification or typed R-15 protocol schemas. The current Phase 3 envelope has opaque payloads, and [P3-PROTOCOL-PLAN.md](progress/P3-PROTOCOL-PLAN.md) puts layout mutation after the typed target contract and the Phase 2 host gate.

## Proposed layout model

Represent one layout root as a pure binary tiling tree:

```text
LayoutNode = Leaf(PaneViewId)
           | Split { axis, ratio, first, second }
```

`axis=columns` places `first` left of `second`, separated by one terminal cell. `axis=rows` places `first` above `second`, also separated by one cell. The tree root is whichever layout container owns the targeted pane in the eventual hierarchy; this document does not choose whether that container is a workspace, tab, or another product object.

Each split initially uses an equal `1:1` ratio. Layout uses integer cell counts only. For an odd usable width or height, the first child gets the floor half and the second gets the remainder. Thus the stable rounding order is left-to-right or top-to-bottom. The divider consumes one cell. Nested ratios are evaluated from the parent rectangle at each node, so inserting a split changes only the targeted leaf's rectangle; unrelated leaves retain their positions and sizes.

Increment `layout_revision` whenever a committed tree/ratio update or a viewport/chrome change alters pane geometry or split feasibility. Focus changes and terminal output do not increment it. An empty initial launch slot is not a `TargetRef` and cannot be split; `pane.split` requires an occupied leaf. Initial launch from an advertised empty slot remains the separate `pane.launch` operation in R-15. This plan does not create two empty child slots or redefine workspace hierarchy.

The revision is a nonzero monotonically increasing `u64` within its layout root. Refuse a mutation before effects if incrementing would overflow; never wrap a revision back to a previously valid value.

| Menu action | Axis and child order | Existing target | New pane |
| --- | --- | --- | --- |
| Split Left | columns: `first`, divider, `second` | second / right | first / left |
| Split Right | columns: `first`, divider, `second` | first / left | second / right |
| Split Up | rows: `first`, divider, `second` | second / bottom | first / top |
| Split Down | rows: `first`, divider, `second` | first / top | second / bottom |

```text
Before: [ existing pane ]

Split Left:     [ new pane ][ existing pane ]
Split Right:    [ existing pane ][ new pane ]
Split Up:       [ new pane ]
                [ existing pane ]
Split Down:     [ existing pane ]
                [ new pane ]
```

To split a leaf, replace that leaf in place with a new `Split` node containing the same existing `PaneViewId` and one server-issued new `PaneViewId`. Keep the existing `session_id`, pane identity, launch generation, and occupant revision. The existing process receives an absolute resize to its new cell rectangle; it is never relaunched. Reserve tentative pane/session IDs together with all other capacity for a new admission, and publish them only when the operation is admitted. A rejected admission must not consume committed ID watermarks. Store the assigned IDs with the admitted operation result so an accepted duplicate returns the same IDs.

The tree and its revision are independent of keyboard focus. A `TargetRef` continues to identify the clicked pane/session incarnation according to R-15. The split request also carries `expected_layout_revision`; the server validates both atomically. A menu opened on one target cannot silently retarget the currently focused pane or a replacement occupant.

## Geometry and feasibility

Compute pane geometry from the current TUI content rectangle after subtracting whatever chrome the implementation actually displays. Do not bake a sidebar or tab strip into this model; those remain unresolved. For the currently tested terminal core, each managed terminal viewport must be at least **5 rows by 20 columns** and no larger than **80 rows by 240 columns**. The divider is one cell and is not part of either PTY viewport.

For a left/right split, let `W` be the target pane width in columns after chrome allocation. The operation is geometrically feasible only when `W - 1 >= 40`, so the minimum target width is **41 columns**. For an up/down split, target height `H` must satisfy `H - 1 >= 10`, so the minimum target height is **11 rows**. The ratio rule then assigns each child at least 20 columns or 5 rows respectively. Before admission, validate every resulting leaf, including maximum dimensions, against the terminal-core bounds.

For the prototype proposal, cap the managed root content rectangle at 80 rows by 240 columns. If the host terminal offers more cells, leave the excess outside the managed pane area; do not pass dimensions above the reviewed core limit. Revisit this presentation choice with the M2 host UI. A host resize that would make any current leaf smaller than the minimum must report `layout_too_small`; it must not kill a session or silently change its size to an unsupported value.

The context menu always lists all four directions. A direction that cannot fit both resulting panes is disabled with a direction-specific reason, such as `Split Left needs 41 columns; this pane has 38`. Other known constraints may disable an item with a clear reason. A server-side check remains authoritative because the size, layout, or capacity may change after the menu opens.

## Right-click and keyboard interaction

When a right-click reaches Panelink, hit-test the pointer against the current pane rectangles and capture the exact `TargetRef` under that point together with the current `layout_revision`. The clicked pane is the operation target even if another pane has keyboard focus. If the user opens the same menu from a keyboard binding, capture the focused pane's current `TargetRef` at that moment. The binding sequence is still an open shortcut choice; it must be discoverable and configurable as required by R-08.

The menu is an overlay that owns its keyboard and mouse events until it closes. It must not send menu navigation or activation keys into the pane. Position it within the TUI content bounds: move it left or upward when it would overflow, clamp it to the available rectangle, and use a compact full-content overlay when a small viewport cannot fit the normal popover. Keep all four split labels visible, including disabled labels and their reasons. At the proposed five-row minimum, the compact overlay uses four rows for the split labels without borders/title and a fifth status row for the selected item's reason. Clip/ellipsize the status text to the available columns; never wrap it across labels or outside the viewport. Below the supported viewport, show only a bounded too-small status where possible, retain session state and require enlargement before menu interaction; a keyboard command still receives the same geometry error.

Right-click delivery is host-dependent: Apple Terminal or an application running in the pane may consume the event. The first host is Apple Terminal, but this interaction is not yet verified there. Provide the keyboard menu command as an equivalent route and document any host setup needed after actual testing. Do not infer right-click compatibility from the current one-shot probe.

Menu flow:

1. Hit-test the clicked pane, or capture the focused pane for keyboard invocation. Read its full `TargetRef` and the current layout revision from one state snapshot.
2. Calculate feasible directions from current geometry. Render all four menu items and any disabled reasons.
3. On activation, submit the original `TargetRef`, the captured `expected_layout_revision`, direction, and explicit launch request. Do not resolve a name or current focus again. If no shell launcher is configured, open a launcher chooser first; do not commit an empty split and infer a daemon shell.
4. The single state owner first validates the authenticated caller/key/generation/lease and checks any retained same-key payload duplicate/conflict. A duplicate returns its original IDs/status before reevaluating layout revisions, since the first split itself changes that revision. Only a new operation revalidates target, revision, geometry, access and capacity atomically. If anything changed, return `target_stale`, `layout_stale`, or the specific capacity/geometry error without creating a pane or child.
5. On success, update the local TUI's selected pane to the new pane after the split result is committed. This is a presentation choice for the initiating TUI only. It does not claim or transfer the new pane's agent input lease.

If focus changes while the menu is open, the request still names the pane captured when the menu opened. If the target is replaced, moved into another layout root, or its layout revision changes, reject the stale request and ask the user to reopen the menu. Never retarget automatically. An accepted retry uses the identical operation key, target, revisions, direction, and launch payload; it returns the original assigned IDs or status without creating another child.

## Split launch and operation contract

Propose one durable `pane.split` operation shared by the TUI and the R-15 CLI/API. It creates the sibling terminal pane and launches the explicitly selected launcher for it. The first implementation should support the configured shell launcher; agent or custom-command launchers can use the same launch contract when their separate support gates are met. A shell profile is a recommendation for the split action, not a decision about server lifetime or the global launch UX.

Every split includes a launch context from the requesting client, an explicit working directory, the selected launcher/profile, and any explicit environment overrides. A TUI uses its attached client's launch context; a CLI uses that invocation's context or an explicitly selected profile. The background server's ambient environment and the existing target process's current environment are never substituted. Resolve the executable and working directory under the existing launch rules before spawn. Use argv, not shell evaluation or command text copied from terminal output. Environment values and sensitive arguments stay volatile and out of metadata, journals, and diagnostics. The R-15 open decision about fitting or staging a launch environment across the 65,536-byte request cap remains a protocol gate; this document does not choose that encoding.

Proposed request fields:

```text
PaneSplitRequest {
  target_ref: TargetRef,                 // complete R-15 reference
  expected_layout_revision: u64,
  direction: SplitDirection,              // Left | Right | Up | Down
  launcher: ExplicitLaunchRequest,        // profile/argv, cwd, client context, volatile env
  ratio: Equal,                           // fixed 1:1 in the first split task
}
```

The layout revision and full target are required preconditions. `pane.split` is a `DurableMutation`; reserve the exact operation key and all needed capacity before effects. A same-key/same-payload retry within the valid generation returns the same result. Reusing the key with a changed direction, target, revision, launcher, cwd, or environment payload returns `operation_conflict`. After an uncertain acknowledgment, query `operation status`; never mint a new key to retry the split and never replay child input.

Example CLI surface, using an exact reference file and explicit launch details:

```sh
panelink pane split --target-ref-file target.json --expected-layout-revision 37 \
  --direction left --launcher shell --cwd /repo --env-file launch.env --json
panelink pane split --target-ref-file target.json --expected-layout-revision 37 \
  --direction right --launcher shell --cwd /repo --env-file launch.env --json
panelink pane split --target-ref-file target.json --expected-layout-revision 37 \
  --direction up --launcher shell --cwd /repo --env-file launch.env --json
panelink pane split --target-ref-file target.json --expected-layout-revision 37 \
  --direction down --launcher shell --cwd /repo --env-file launch.env --json
```

`--env-file` is a bounded volatile input; it is not saved as a launch record. The command's application schema, size validation, and file/standard-input behavior must be reviewed with R-15 before CLI routes or typed payloads are frozen.

The response should use the existing R-15 JSON envelope and include:

```json
{
  "result": {
    "type": "pane_split",
    "operation_status": "applied",
    "layout_revision": 38,
    "direction": "left",
    "existing_pane_target_ref": { "server_generation": 12, "workspace_id": "ws_7d2", "pane_id": "pn_31a", "session_id": "ses_9f6", "occupant_revision": 4, "token": "tr_..." },
    "new_pane_target_ref": { "server_generation": 12, "workspace_id": "ws_7d2", "pane_id": "pn_42b", "session_id": "ses_a14", "occupant_revision": 1, "token": "tr_..." },
    "new_session_launch_status": "running"
  }
}
```

Return fresh `TargetRef` values for both the existing and new panes so callers can continue using validated references after the layout revision changes. The existing reference retains the same pane, session, and occupant revision; a fresh token may be issued. The new reference identifies the newly allocated pane and session. Report the assigned IDs and layout revision in status results even when a reply is recovered after a lost acknowledgment.

## Admission, effects, and failures

The state owner serializes split requests with layout updates. After caller/key validation, it resolves a retained same-key/same-payload admission before current-state preconditions; otherwise a successful split would make its own retry stale. Conflicting payloads fail even when capacity is full. For a new key it validates the complete target/layout precondition and reserves every resource before committing admission or ID/revision watermarks. Before dispatch it reserves:

- one pane leaf and one managed-session identity;
- the updated layout tree and journal space;
- one PTY/process launch slot and its bounded launch/effect queue entry;
- the existing session's absolute resize slot;
- the global pane/PTY and estimated terminal-memory budget;
- the exact operation-ledger payload bytes and result record.

No reservation means no tree mutation, resize, or process spawn. Persist a launch intent before spawning and keep secret-bearing environment values out of that record. Commit the new layout revision with a visible `starting` leaf before process effects. Then apply the existing pane's absolute target size and spawn the new session at its assigned size. `Applied` means the layout is committed and the requested launch and resize completed. A proven pre-effect failure is `Failed`; a committed split with a failed resize or proven launch failure is `Partial`; a spawn or durable-result boundary whose outcome cannot be established is `Unknown`.

Recommended launch-failure behavior: keep the committed split and show a visible `launch_failed` empty pane with a sanitized reason and an explicit retry/choose-launcher action. Do not roll back the layout or tear down the existing session. Retry is a new user action with a new operation key after the cause is addressed; the old accepted operation is never dispatched again. If spawn succeeded but recording its result failed, keep the child server-owned when possible, mark the pane/session `unknown`, and reconcile it; do not relaunch or kill it as an implicit rollback. A server crash cannot make filesystem layout updates and OS process creation one atomic transaction, so no cross-crash exactly-once guarantee is claimed.

If the existing PTY resize fails after layout commit, retain the committed tree and display a `resize_pending`/`Partial` result. Reconcile with an absolute desired-size operation. Do not send a relative resize delta or silently revert the other session. The TUI and agent API report the same operation outcome and error codes. Until a resize succeeds, the leaf must report both actual engine/PTY dimensions and desired layout dimensions. Its display shows a bounded resize-pending placeholder (or a separately reviewed clipped cell view), never feeds a larger snapshot to a smaller host rectangle or paints outside its leaf; the existing engine/session continues without relaunch. When the entire host becomes too small, retain the last accepted tree/PTY sizes and show a viewport-bounded too-small overlay until a valid layout can be accepted. Do not apply a partially invalid host resize.

Proposed errors for a schema review are `layout_stale` (revision precondition failed), `layout_too_small` (one or more resulting leaves fall below the minimum), `pane_limit_exceeded`, `split_depth_exceeded`, `memory_budget_exceeded`, and `unsupported_layout_resize`. Reuse the R-15 `target_stale`, `generation_mismatch`, `capacity_exceeded`, `operation_conflict`, and launch-policy errors where their meanings fit. A rejected precondition or admission returns no pane/session IDs and causes no tree, resize, or process effect. A committed operation that later has a launch/resize problem returns the assigned references with `Partial` or `Unknown`, not a rejected-command error. Layout-change events must use the reviewed R-15/P3 event framing and cursor contract; this proposal does not assume the current envelope has an Event kind.

Focus remains separate from input ownership. After a successful TUI split, the initiating TUI may select the new pane as a local presentation update. An API/agent split does not alter human UI focus. Neither focus action changes `input_owner_revision`, authorizes agent input, nor takes control of the new pane. Input ownership follows the separate R-15 claim/CAS/grant contract. Typed focus controls, if later needed, must identify an attached UI and remain separate from `pane.split`'s process mutation.

Removing a pane from a view, hiding it, or zooming another pane must not by itself stop or relaunch its managed session. Any explicit session stop uses the distinct lifecycle operation and capability. This layout plan does not settle detach, client-close, or server-stop effects on children.

## Resize integration with R-15

The current R-15 proposal includes absolute `pane.resize`. Once a pane belongs to a tiled tree, an agent must not set that session's PTY dimensions independently of the pane rectangle. Route every resize through the layout state owner with the full `TargetRef` and expected `layout_revision`. A request to resize the attached root viewport recomputes the tree; a request to change a leaf's size may adjust only the relevant split ratio(s), and only when the chosen layout policy yields a valid constrained solution. Recompute and validate every affected leaf against the 5x20 minimum and 80x240 maximum before committing a new revision.

If a requested leaf dimension cannot be expressed by the current tree/constraints, reject it with a stable `layout_constraint_error` or `unsupported_layout_resize` and perform no PTY resize. The layout owner reserves all needed absolute-resize work before commit. `TerminalSession` receives only the cell rectangle computed from the accepted layout revision; it never receives caller-supplied dimensions that bypass the tree. A successful resize returns the new layout revision and fresh target references. This amendment should be incorporated into the parent-owned R-15 plan before typed `pane.resize` schemas are frozen.

## Provisional prototype bounds

These values are review proposals for pure layout and admission work. They are not release budgets, measured RSS, or a claim that the 10-agent workload is affordable.

| Resource | Proposed prototype starting bound | Status and guard |
| --- | ---: | --- |
| Pane leaves per server | 16 | Allows a trial of up to 10 agent panes plus 6 ordinary panes; R-03 still requires a measured representative workload. |
| Split-tree depth | 8 | Bounds recursive validation/layout traversal; reject before mutation when exceeded. |
| Nodes in a full tree | 31 | Derived from 16 leaves for allocation and validation accounting. |
| Terminal viewport per pane | 5–80 rows by 20–240 columns | Current tested core limit; chrome and divider cells are outside the PTY viewport. |
| Per-PTY core reservation | 64 MiB | Rounded up from the conditional ~60.17 MiB core allocation estimate in the P3 plan; not an RSS guarantee. |
| Aggregate core reservation | 1 GiB | A provisional admission ceiling for these core reservations only. It does not include the rest of the server/UI budget or prove 10-PTY suitability. |

Before enabling these values in a runtime, parent review must reconcile them with the selected source/toolchain allocation model, terminal/render queues, history policy, and a measured workload on the specified Apple Silicon hardware. Do not infer the R-03 10-session target by multiplying a prototype estimate. Until a reviewed global memory bound exists, reject a split before effects when the applicable resource budget is unavailable or uncertain.

## Proposed acceptance matrix

These are future acceptance scenarios. None were run for this plan.

| Area | Scenario | Required result |
| --- | --- | --- |
| Direction geometry | Split one leaf left, right, up, and down in a known rectangle. | New/existing panes occupy the requested physical sides; divider is one cell; odd rounding is deterministic. |
| Tree preservation | Split a nested leaf while unrelated panes run distinct commands. | Only the selected leaf becomes a split; every existing session ID and process continues without relaunch. |
| Reference integrity | Open menu on pane A, move keyboard focus to pane B, then activate Split Right. | Operation uses A's captured `TargetRef`; response returns fresh refs for old and new panes. |
| Stale target/layout | Replace the occupant or commit a layout/viewport change while the menu is open. | `target_stale` or `layout_stale`, no tree, resize, PTY, or launch side effect; no automatic retarget. |
| Minimum sizes | Try left/right at 40 columns and 41 columns; up/down at 10 rows and 11 rows. Include chrome and divider accounting. | Direction is disabled/rejected below its exact threshold and accepted at the threshold when all child/core bounds hold. |
| Maximum/core bounds | Exercise 80x240 pane limits, oversized host rectangles, and a host resize below the minimum. | No unsupported PTY dimensions; oversized root is capped per proposal; too-small resize reports an error without killing or silently resizing a child. |
| Layout-owned resize | Ask an agent to resize one leaf to dimensions the current split ratios cannot express; then resize through a valid divider constraint. | Unsupported arbitrary PTY dimensions are rejected without effects; accepted requests commit a new layout revision and resize sessions only to computed leaf rectangles. |
| Menu behavior | Right-click near each terminal corner and in pane content; open via the keyboard command; test host mouse reporting and a child mouse mode. | Menu stays in bounds, all four actions remain visible with disabled reasons, keyboard navigation is captured, and host-consumed right-click has a discoverable fallback. |
| Admission and retry | Exhaust pane, depth, PTY, journal, queue, or memory capacity; then duplicate and conflict an admitted split key. | Capacity rejection leaves counters/tree/effects unchanged. Same key/payload returns the same pane/session IDs despite its now-stale original layout revision; changed payload conflicts even at capacity; rejected reservations do not consume ID/revision watermarks; no second child starts. |
| Launch inputs | Launch with two distinct client contexts and explicit cwd/environment; test unavailable profile, cwd, and executable. | Child gets only the selected request context and overrides; daemon/target environments are not inherited; secret values never enter diagnostics or persistent metadata. |
| Partial effects | Inject proven spawn failure, resize failure, lost reply, result-journal failure, and crash around the intent/spawn/result boundary. | Show the committed split state truthfully; `Partial`/`Unknown` where needed; no automatic relaunch, guessed rollback, or input replay. |
| Focus and ownership | Split from TUI and from agent CLI while an agent owns another pane's input. | Local TUI may focus the new pane; API does not steal UI focus; focus grants no input ownership and owner revisions remain unchanged. |
| Lifecycle | Hide/zoom/remove a pane view while its session remains live. | View changes alone do not stop or relaunch its child; stopping remains an explicit lifecycle operation. |

## Integration sequence and open decisions

1. Parent reviews this proposal with the R-15 operation and target contract, including the layout-owned `pane.resize` amendment. Decide which split/menu defaults become the typed schema; update `docs/AGENT-CONTROL.md`, `docs/SPEC.md`, and the roadmap under parent ownership.
2. Finish the typed R-15 schema, capability, event, and launch-environment-size decisions before freezing `pane.split`. Keep `TargetRef` plus `expected_layout_revision` mandatory; return fresh old/new references and the committed revision.
3. After the documented Phase 2 Apple Terminal host gate, implement and test pure integer geometry/tree replacement with the above provisional bounds. These tests do not establish mouse or process compatibility.
4. Add the state-owner split admission with fake launch/resize effects and injected failures. Preserve the admission-before-effects and lost-ack contracts from the spec and P3 plan.
5. Integrate shell launch, absolute PTY resize, and the local TUI context menu. Test physical mouse delivery, clipping, keyboard fallback, and operation outcomes on the recorded Apple Terminal version.
6. Measure the R-03 session workload and update resource budgets from observed source/runtime behavior before claiming support.

Open product or implementation choices remain the shell-launcher defaults, launch-context staging over the request cap, pane/tab hierarchy, persistent layout storage and migration, divider-drag/keyboard-resize interaction, exact menu shortcut, and same-user/collaboration policy. This proposal recommends an equal split, one-cell divider, left/top-first integer rounding, a configured shell launcher when available, local TUI focus on the new leaf after commit, and no agent input-ownership change. Each is a scoped starting point for parent review, not a newly confirmed global product decision.

## Evidence status

Completed for this document: required product, roadmap, R-15, and Phase 3 planning documents were read; the system-design framework was applied; this independent split proposal was drafted. No source, manifest, dependency, canonical shared plan, or protocol schema was changed. No tests, Apple Terminal interaction, memory benchmark, or runtime compatibility check was run. The acceptance matrix above is proposed work, not completed evidence.

References: [working specification](SPEC.md), [agent-control proposal](AGENT-CONTROL.md), [Phase 3 protocol plan](progress/P3-PROTOCOL-PLAN.md), [roadmap](../ROADMAP.md).

## Parent design review

Parent clarified duplicate admission before layout preconditions, tentative-ID reservation before committed watermarks, bounded five-row menu status presentation, and actual-versus-desired geometry while a PTY resize is pending. These are proposed implementation contracts, not executed tests. This document is now linked as the detailed R-16 plan; exact shortcut/launcher/hierarchy/live-memory choices remain open and must be settled in the relevant bounded implementation task. No host or runtime compatibility claim follows from document review.
