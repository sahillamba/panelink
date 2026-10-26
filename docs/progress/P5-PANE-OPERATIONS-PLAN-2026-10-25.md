# Phase 5 pane move, resize, and zoom plan

Date: 2026-10-25
Status: implementation-ready proposal; parent decision required before typed schema or binding freeze.

## Decision and scope

SPEC M2's outcome remains required: multiple managed processes continue while
the user switches, moves, resizes, and zooms panes. The confirmed hierarchy is
**Project → workspace tabs → split panes**. This plan covers the remaining pane
operations inside one workspace tab: move/swap two pane locations, adjust
internal dividers interactively, and zoom/unzoom a pane. It does not claim that
existing root viewport resize satisfies interactive divider adjustment. It
does not move processes across workspaces or tabs, add arbitrary pane counts,
or redefine launch, clipboard, input ownership, or stop behavior.

Implement on the current four-live-PTY, fixed-capacity binary split tree and
the pane operations/operation-ledger authority. Root layout remains bounded by
the existing cell limits. Each topology/ratio/zoom-state change is serialized
by the server owner, validated against the current layout revision and exact
stable `TargetRef`, and returns authoritative layout/revision state. Do not
kill, relaunch, replay terminal input, or migrate a process to implement a
view change. Keep server-owned PTYs running during zoom; resize PTYs absolutely
to their accepted visible geometry, report partial/pending resize honestly,
and reconcile from desired geometry without replaying a relative action.

No dependency is needed. Before implementation, parent resolves the protocol
and focus/open-contract decisions below. Key examples are prototype
assumptions for discoverability, not final user configuration requirements.

## Current implementation boundary

The graph index for `Users-sahillamba-.codex-worktrees-7060-panelink` was
available and queried first. It identifies `PaneLayout` and split geometry in
`src/pane_layout.rs`, the single serialized connection worker in
`src/multi_runtime.rs`, multi-session owner operations in
`src/server/multi_session.rs`, typed root resize in `src/ui_protocol.rs`, and
the saved-layout encoder/decoder in `src/saved_layout.rs`. The owner already
has root-resize machinery with absolute PTY sizes, pending resize tracking,
layout revision checks and duplicate operation handling. Existing pane geometry
is a binary tree with equal integer splits and one-cell dividers. Root resize
recomputes that tree; it does not provide a user-adjustable internal divider.
The historical saved-layout record stores up to four target refs and observed
rectangles, but not a split tree/ratio or zoom restore state; it supports a
legacy 160-byte record alongside the current 280-byte form. Neither existing
record should be treated as a durable format for new ratios without an explicit
versioned design.

The source checkout is in a parent-managed source freeze with broad modified
and untracked work. This plan is read-only evidence; it does not establish that
the in-flight source has been independently verified.

## Proposed operation and state contract

### Common authority and transaction rules

Use one authenticated, workspace-scoped `PaneOperation` mutation family (or an
equivalent reviewed operation family) with a fresh `OperationKey` per user
gesture. Every request carries the full captured `TargetRef`,
`expected_layout_revision`, and operation key. Two-target move carries both
refs. For a retained key, compare exact payload and return its in-progress or
completed recorded result before checking now-stale layout/target preconditions;
same key with a different payload is `operation_conflict`. A new key must pass
workspace scope, generation, target identity/occupant revision, current
revision, capability, and resulting-geometry checks before admission or effects.
Unknown/lost acknowledgements are queried by the same key; never issue a new
key automatically for an uncertain mutation. A rejected request has no layout,
PTY, focus, or process effect. A committed geometry with a failed PTY resize is
`Partial` with actual and desired dimensions; reconcile by absolute sizes.

The `TargetRef` identifies the pane/session/occupant, not its current tree
slot. Moving a pane therefore preserves its stable pane identity, session,
occupant revision, process and input-owner revision. Layout slots/rectangles may
change while that ref remains authoritative; return fresh refs and current
revision in every successful or partial result. Do not use positional pane
indexes as mutation authority. UI menus capture a ref and revision when opened;
activation never silently retargets after focus changes.

### Move: swap locations within one tab

Implement move as a stable-identity swap of two occupied leaves' locations in
the same workspace tab. This preserves a valid full binary tree and avoids
speculative cross-workspace process migration. Payload:

```json
{
  "type": "pane.move_swap",
  "operation_key": {"generation": 12, "lease": 8, "sequence": 91},
  "expected_layout_revision": 38,
  "source_target_ref": {"...": "full TargetRef"},
  "destination_target_ref": {"...": "full TargetRef"}
}
```

The server verifies distinct current refs in the same tab, swaps their leaf
identities in the tree, validates all resulting rects, commits one revision,
then reconciles absolute PTY sizes for panes whose rect changed. It must not
exchange process/session identities. Reject same-pane, stale, cross-tab,
minimum-size-invalid, revision-overflow, or pending-conflicting topology
requests before mutation. If a PTY resize fails after the layout commit, return
the committed revision and target refs as `Partial`, preserving both processes.
Do not offer a drag operation until the chooser/keyboard path and owner results
are accepted.

### Interactive divider adjustment

Represent the split fraction as a bounded integer ratio (recommend 16-bit
fixed fraction, `first_units / 65535`) on each internal split. Each split keeps
its axis and child order; all geometry uses checked integer arithmetic and
deterministic rounding. Clamp only when the requested divider position is
inside the legal range for every affected leaf; otherwise reject with
`layout_too_small` and leave state unchanged. Do not silently clamp a user
gesture to a different location. Preserve current root min/max bounds and
current four-pane ceiling. Parent must select/confirm whether nested ratios
remain independently adjustable; proposed answer: yes.

Payload for each committed divider position:

```json
{
  "type": "pane.divider_set",
  "operation_key": {"generation": 12, "lease": 8, "sequence": 92},
  "expected_layout_revision": 38,
  "target_ref": {"...": "full TargetRef for a leaf below this split"},
  "split_path": ["first", "second"],
  "first_units": 32768
}
```

`split_path` is scoped by the captured target and revision and is only a
structural locator, not identity authority. Alternatively the server may
resolve a stable split ID, if the parent chooses to add one. Each drag update
that is sent to the server is its own keyed absolute desired ratio, never a
relative delta. Coalesce unsent pointer samples client-side; do not coalesce or
rewrite admitted operation keys. For keyboard resizing, send one absolute
ratio update per repeat step. No source event is terminal input.

### Zoom and restore

Zoom is a server-authoritative, workspace-tab-scoped presentation mode. State
contains `zoomed_target_ref` and the unchanged underlying tree/ratios; entering
zoom does not delete or collapse leaves. Zoom target must be a current occupied
leaf. The focused zoomed pane uses the full accepted root content rectangle,
and its PTY is resized absolutely to those actual cell dimensions. Other PTYs
remain alive, continue reading/writing and collecting output under the existing
bounded server policy; their last accepted PTY dimensions remain unchanged
while hidden. On unzoom, restore the exact underlying tree and reconcile every
leaf to its current rect. This is required so “zoom” means a real viewport and
PTY resize, not merely hiding sibling cells or drawing a clipped pane.

```json
{"type":"pane.zoom_set","operation_key":{"generation":12,"lease":8,"sequence":93},
 "expected_layout_revision":38,"target_ref":{"...":"full TargetRef"},"zoomed":true}
```

For unzoom, `zoomed:false`; target ref must match the currently zoomed pane.
Zoom/unzoom increments the same authoritative layout revision because visible
geometry changes. Exact duplicate retries return the same result. Failed PTY
resize after commit returns `Partial`; keep truthful desired/actual sizes and
pending state. Do not relaunch, stop, or replay hidden sessions. One active
zoom per tab is the proposed bound.

### Multiple clients and focus

Because zoom changes PTY dimensions, it is a server-global tab view, not a
per-client overlay. Clients attached to that tab observe the same zoom target,
revision, geometry and resize-pending state. A client whose viewport differs
from the authoritative root must not overwrite the shared layout implicitly;
the current attachment/resize ownership policy must authorize root-size changes.
Until that policy is confirmed, a conflicting viewport owner makes the
operation fail closed with a stable ownership/capability error. Focus is a
separate UI presentation state. A TUI-initiated move/zoom may focus the chosen
pane locally after authoritative success; an API mutation does not steal a
human client's focus or input ownership.

## Protocol, events, and saved layouts

Do not change the existing opaque envelope or reinterpret legacy operations.
Parent should choose one additive version strategy before schemas freeze:

1. Add optional negotiated `CAPABILITY_PANE_OPERATIONS` and typed mutation and
   result kinds under a new schema version; or
2. Extend the existing negotiated control schema with strictly additive kinds
   and require capability selection before dispatch.

Unsupported clients must receive a clear `capability_denied`/unsupported result
before mutation, and must never parse new kind payloads optimistically. Retain
current codecs and behavior for peers that do not select the new capability.
Do not bind new operation kinds to current capability numbers without checking
the handshake's available bit assignments and compatibility table.

Responses need operation key, terminal/in-progress status, authoritative
revision, full current refs for affected panes, actual and desired per-pane
dimensions when pending, and sanitized failure classification. Layout-change
observation must use the existing event/watch cursor and gap reconciliation
contract where available. Event replay is observation only; it cannot replay a
mutation. A client reconnects by querying current layout and pending operation
status. Keep one owner serialization point for operations and resize effects.

Keep version 1 saved-layout records readable byte-for-byte. First implementation
may treat ratio/zoom as runtime-only, but then must say so clearly and ensure
saved geometry observations do not imply the ratios can be reconstructed. If
durable restore is included, define a new magic/version and explicit bounded
tree encoding (four leaves maximum, three split nodes maximum, ratios, axis,
child order, target refs, root geometry, zoom target or explicit no-zoom).
Decoder must validate full-tree shape, unique target refs, ratio bounds,
minimum/maximum leaf geometry, checked arithmetic, scope, zero/reserved bytes,
and exact record length before use. Migrate old 160-byte and current 280-byte
records as historical observed geometry only; do not fabricate ratios or zoom
state. Writer uses existing atomic layout publication rules. Never persist
process input, environment secrets, or a command to replay.

## Prototype interaction defaults (not final config decisions)

Use the existing `Ctrl+B` prefix as a parent-review prototype. Candidate keys:

- `z`: toggle zoom/unzoom current pane.
- `m`: open a bounded in-tab pane chooser; selecting another pane swaps its
  location with the captured current pane.
- `e`: enter divider-adjust mode; arrows move the nearest divider by one cell,
  Enter commits/exits, Esc exits. Existing `r/l/u/d` remain split shortcuts;
  existing arrows for focus, `c` copy, and `w/n/p/[/]` tab/project navigation
  remain reserved as recorded. Do not infer these examples are configured or
  user-approved bindings.
- A mouse grab/drag can be a later UI affordance using the same absolute-ratio
  operation; it is not required for this bounded initial interaction unless
  parent selects it.

Show a divider highlight and current axis/location while adjusting. The chooser
must report unavailable/stale targets and must not send keystrokes to the PTY
while active. Mouse hit-testing and host mouse-protocol compatibility require
separate acceptance; a keyboard path is mandatory for initial usability.

## Failure and lifetime behavior

- All validations and resource reservations precede admission and effects.
- Layout mutation is committed once at the owner. Geometry and revision never
  advance for precondition/capability/minimum-size rejection.
- PTY dimensions follow committed visible rectangles only. On failure, preserve
  the committed layout, both actual and desired sizes, and `Partial` status;
  retry as an observation-based absolute reconciliation, not the old mutation.
- Cancellation before server admission has no effect. After admission, client
  cancellation/disconnect does not cancel or repeat the mutation; operation
  status remains queryable. A lost response is `Unknown` to the client until
  queried by its original key.
- A concurrent topology/ratio/zoom mutation conflicts or fails stale revision;
  it cannot overwrite a newer tree. Owner queue timeout after admission does
  not imply rejection or authorize a new key.
- Closing a tab or explicit stop remains governed by the existing lifecycle
  operation; moving, zooming, hiding or unzooming never means stop.
- All server and client diagnostics redact target tokens and sensitive launch
  metadata under existing policy.

## Exclusive implementation sequence and ownership

Each worker owns only the listed files; workers do not commit. Parent resolves
the canonical choices and serially integrates/compiles the combined tree.

1. **Pure tree/codec worker** — `src/pane_layout.rs` and its private tests only.
   Add ratio geometry, stable-identity slot swap, divider selection and zoom
   projection/restore as pure validated state transitions. Keep four leaves,
   current min/max bounds, checked arithmetic and monotonic revisions. No
   protocol or PTY effects.
2. **Typed wire worker** — `src/ui_protocol.rs` plus the parent-approved
   handshake capability declaration file only. Add version/capability-gated
   request/result codecs, bounds and refusal paths. Do not edit the tree,
   runtime or owner.
3. **Owner/admission worker** — `src/server/multi_session.rs` only. Implement
   exact-payload key admission, duplicate-before-precondition behavior, target
   and revision validation, owner transactions, absolute PTY resize reconciliation,
   actual/desired result reporting, and no process lifecycle effects. Avoid
   crossing the concurrently modified multi-runtime boundary.
4. **Connection dispatch worker** — `src/multi_runtime.rs` only. Route the
   negotiated typed requests through the existing serialized owner worker,
   reject unnegotiated kinds, and preserve framing/deadline behavior.
5. **Client controller/UI worker** — `src/ui_client/keys.rs`,
   `src/ui_client/state.rs`, `src/ui_client/worker.rs`, and the smallest
   dedicated UI component file parent names before start. Add captured target/
   revision, operation polling, chooser, keyboard divider adjust and zoom state;
   ensure overlays consume keys/mouse. No server contract invention.
6. **Durable codec worker (optional, parent decision)** — `src/saved_layout.rs`
   only. Execute only if persistence is selected; add new versioned format and
   migration tests, never modify old bytes/interpretation.
7. **Parent integration** — review every diff; reconcile handoff/source-freeze
   edits; run the repository's prescribed serial checks after all bounded work
   lands. Do not count task-level tests or a successful compile as acceptance.

Avoid overlapping work on shared dispatch, owner or client state files. The
parent may combine steps if no concurrency slot is available, but ownership
remains explicit and order-dependent interfaces are agreed first.

## Proposed acceptance evidence

These are required future scenarios, not completed checks:

1. Start four distinct server-owned PTYs with identifiable periodic output;
   swap two target locations, resize a nested divider, zoom one pane, unzoom,
   switch tabs, and verify all four PIDs/session identities remain stable and
   all processes' output continues. No replayed input or relaunch occurs.
2. Assert stable `TargetRef` identity and occupant revision across move/resize/
   zoom, correct new layout revision, full fresh refs, and exact operation-key
   outcomes after lost-ack/status recovery. Same key/payload executes once;
   same key/different payload conflicts; unknown key is never retried under a
   new key.
3. Reject stale revision/ref, cross-tab move, same-pane swap, invalid split
   path, invalid ratio, divider positions below minimum, root outside bounds,
   overflow, unsupported capability, and unauthorized client with no geometry,
   PTY, focus or process effect.
4. Exercise move and resize interactions through keyboard chooser/adjust mode
   and mouse drag if included. Verify captured targets survive focus movement,
   overlay keys do not leak into a child, and server revision is authoritative.
5. Verify zoom resizes the focused PTY to the actual full root rectangle; hidden
   PTYs remain alive and continue output; unzoom restores the exact prior tree
   and absolute sizes. Inject focused resize failure and recovery; prove visible
   pending state and no relaunch.
6. Attach two clients to one tab. Verify both observe the same zoom/ratio/revision
   and layout events; conflicting viewport/ownership requests fail closed and
   neither client silently overwrites state.
7. Decode legacy 160-byte and current 280-byte saved-layout fixtures unchanged.
   If durable versioning is selected, malformed new trees/ratios/duplicate refs
   fail closed and valid old data migrates as observations only; round-trip
   persists ratios and explicit no-zoom/zoom state as chosen.

Evidence should separately identify pure geometry tests, owner fake-effect
tests, real four-PTY continuity integration, client interaction tests, wire
compatibility, and any Apple Terminal/manual host acceptance. A test plan is not
evidence of a pass; this proposal has run no tests and makes no runtime or host
acceptance claim.

## Parent decisions before implementation

Recommended smallest-scope decisions: confirm same-tab swap as “move”; use
server-global per-tab zoom because it changes the focused PTY size; keep ratios
and zoom runtime-only in the first pass unless durable restore is already a
required P5 gate; use a 16-bit fixed split ratio with exact minimum validation;
use keyboard interaction first and defer drag. Resolve the shared viewport and
input/resize-owner authority for multiple clients before schema freeze. Confirm
whether one ratio update per pointer sample is acceptable if drag is included.
Select additive capability/schema strategy and assign capability ID through
the existing parent protocol process. Accept or replace prototype bindings.

Until those choices and the acceptance evidence above are complete, the plan
does not mark the M2 operations or Phase 5 complete. Existing root viewport
resize, pane focus, split, close/reflow and tab switching remain their own
delivered scopes; none alone proves interactive divider adjustment or zoom.
