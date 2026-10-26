# ADR-P3-LAYOUT-01: Workspace layout notifications

Status: accepted bounded engineering task, 2026-10-23. Decider: parent under
the user's Phase 3 continuation authorization. This refines the separate
[proposal](P3-LAYOUT-EVENTS-PROPOSAL.md); it does not complete Phase 3.

## Context and decision

Pane journals are target-scoped and cannot correctly describe a workspace
change. Add a separately negotiated workspace notification capability (bit 8),
a volatile 64-record workspace journal and finite watches. Keep existing pane
events unchanged. Prefer small revision invalidations over retained full layout
snapshots: clients reconcile current topology after notification. Intermediate
pane open/close observations may be coalesced; do not claim a complete lifecycle
transcript. Polling remains the explicit compatibility fallback for servers or
existing leases without the new capability.

The owner records each committed layout revision and the terminal-empty
transition (which need not advance layout revision). Snapshot and cursor are
copied in one owner turn. Events never execute or retry mutations. A missed
event requires explicit reconciliation; a cursor from another generation or
workspace is refused. Future cursor and malformed requests fail closed.
Sequence exhaustion disables notifications, preserving processes and ordinary
layout queries. Empty means no positively live layout remains; no historical
target is synthesized. Before layout establishment, workspace reconciliation
is unavailable. The current workspace identity is the first layout target's
workspace and every occupied target must match it.

Read plus bit 8 is required. The connection's existing grant must name a target
in that workspace, but its target may since have retired; the grant and lease
must remain live. A pane Events grant alone is insufficient. Resume never
widens a grant. The one-child legacy server never offers bit 8. An unsupported
optional bit is ignored; required bit 8 is refused by old servers.

## Exact component contract

New `workspace_watch_protocol` (unix), schema 11, big endian. Existing Request
and Response frame kinds, no Event frames. Fixed reserved bytes must be zero.
`WorkspaceCursor { generation, workspace, sequence }`, all IDs nonzero, sequence
zero allowed. Debug/error text redacts identities and revisions.

- Request: 40 bytes. schema u16 at 0; kind u16 at 2 (1 reconcile, 2 watch);
  reserved 4..8; cursor at 8/16/24; limit u16 at 32; wait_ms u16 at 34;
  reserved 36..40. Reconcile requires sequence/limit/wait zero. Watch limit
  1..64 and wait 0..1000 ms.
- Reconciliation: 472 bytes. schema/kind (0x8001) at 0/2; flag at 4
  (0 empty, 1 layout present); reserved 5..8; cursor at 8/16/24;
  revision u64 at 32; reserved 40..48; validated existing 424-byte LayoutReply
  at 48. Empty has zero layout bytes and nonzero last revision. Nonempty
  layout revision and every target's namespace must match the prefix.
- Watch reply: 40-byte header plus 24 bytes/event, maximum 1576 bytes.
  schema/kind (0x8002) at 0/2; status u16 at 4 (1 Changed, 2 TimedOut,
  3 ResyncRequired); count u16 at 6; cursor at 8/16/24; reserved 32..40.
  Event: sequence u64 at 0, revision u64 at 8, empty u8 at 16 (0/1),
  reserved 17..24. Events are positive consecutive sequences and positive
  revisions. Changed requires 1..64 events ending at header cursor sequence;
  other statuses require zero events. Decoder validates the whole response.

Pure API: `Request::{Reconcile { generation, workspace }, Watch { cursor,
limit, wait_ms }}`, `Reconciliation { cursor, revision, layout: Option<LayoutReply> }`,
`LayoutEvent { sequence, revision, empty }`, `WatchReply { status: WatchStatus,
cursor, events: Vec<LayoutEvent> }`; `encode_request/decode_request`,
`encode_reconciliation/decode_reconciliation`, `encode_watch_reply/decode_watch_reply`.
Encoders return Result<Vec<u8>, CodecError>; decoders own bounded values.

New `workspace_event_journal` (unix): `WorkspaceEventJournal::new(generation,
workspace)`, `current_cursor()`, `push(revision, empty)`, `read_after(cursor,
limit)`. Errors distinguish scope mismatch, expired, future, invalid limit,
exhausted and invalid revision. 64 retained events, pop-before-push, checked
sequence increment; expiry boundary equals last evicted sequence and remains
valid. Logical retained event bytes <=1536. No IO or dependency additions.

Owner APIs: `reconcile_workspace(connection, generation, workspace, now)` and
`read_workspace_events(connection, cursor, limit, now)` returning codec types.
An empty immediate watch returns TimedOut; the driver retains positive waits
and rechecks on ticks. Workspace and pane watches share the existing eight
pending-watch budget. Stop/disconnect semantics match existing watches.
Worker validates request scope, page consecutiveness after requested cursor,
limit/status/count and complete codec before any response bytes. One bounded
Response frame uses the existing write deadline.

CLI: request bit 8 optionally only on new dedicated events context; keep old
leases unchanged. Use a separate persistent connection for workspace watches
so pane watch connections retain their target scope. On Changed, emit JSON
`layout_changed` records with generation/workspace/sequence/revision/empty and
reconcile current topology for pane observations. On ResyncRequired, emit
`layout_resync`, reconcile and reset cursor; do not fabricate missing events.
Pin `--pane N` to the initial full target, never a replacement at index N.
Validate scope and entire watch reply before advancing cursor or output.
Watch/read retries never reserve keys or replay input.

## Verification and ownership

Parent owns owner/global capability plumbing, exports, spec/roadmap/session and
integration. Worker A owns only two new codec/journal source files and a result
note. Worker B owns multi_runtime.rs and workspace runtime acceptance tests.
Worker C owns p3_cli.rs, agent_cli.rs and CLI acceptance tests. Assign after
interfaces above are available; no worker commits or edits shared docs.

Required checks: malformed/golden codec and bounds; journal rollover and
overflow; grant/cursor denial; no duplicate event for rejected/no-op/retried
mutations; ordered split/resize/collapse and terminal empty; atomic snapshot/
cursor; actual socketpair pending wait and stop; CLI gap recovery and pinned
target filtering; existing four-pane attach/relaunch and input nonreplay.
Run serial full suite and verifier with permitted local sockets, recording
ignored host cases separately. No Apple Terminal, physical reboot or provider
identity acceptance follows from private-PTY tests.
