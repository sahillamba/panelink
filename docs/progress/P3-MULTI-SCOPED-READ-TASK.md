# Multi-session scoped read API — parent adoption contract

## Parent adoption — 2026-10-18

Adopt after session dispatch release/review. Exclusive files: server.rs for
the private preflighted-body extraction only, server/multi_session.rs for
the two scoped APIs/types and Events capability change, and new
P3-MULTI-SCOPED-READ.md. No codec/runtime/CLI/registry/manifest edits. Both
prefixes are128 bytes; do not subtract eight bytes or change wire offsets.
Preserve the old StateOwner authorization/error order and tests. The new
owner's earlier Events-withholding test is superseded by tests for actual
scoped-method support: advertise Events only with healthy metadata, omit it
otherwise, and reject required Events without consuming a lease when metadata
is unavailable. Newly negotiated scoped-read clients must use fresh leases;
existing grants never gain new capabilities merely because support exists.
No public concrete state borrow, pending-watch scheduler or stop authority.

**Status: adopted with the amendments above; implementation follows dispatch review.** This follows the
multi-session owner/dispatch slices. It adds two synchronous library-core read
methods. It does not add socket/worker/CLI dispatch, pending-watch scheduling,
wait loops, stop behavior, or a runtime route. Existing one-child
`StateOwner::{reconcile,read_events}` behavior remains unchanged.

## Source boundary and prefix size

Graph-first search of `Users-sahillamba-panelink` located
`SessionHandler::{reconcile,read_events,require_watch_authority}` and the
schema-5 `scoped_watch_protocol::{SessionCursor,Reconciliation,
encode_reconciliation}`. Direct source inspection confirms the current
contracts in `src/server.rs`, `src/scoped_watch_protocol.rs`,
`src/watch_protocol.rs`, `src/event_journal.rs`, and
`src/event_protocol.rs`.

The public `SessionCursor` is `{ generation, session, sequence }`; schema-5
codec validates generation and session against the full `TargetRef`, permits
sequence zero, and redacts cursor/snapshot data from Debug. Existing
`SessionHandler::read_events` takes the older `EventCursor { generation,
sequence }`, whose per-target `EventJournal` has no session namespace. The
multi-owner API must validate `SessionCursor` against the requested full
target before converting to `EventCursor`; never accept a sequence-only
cross-session cursor.

Both current `watch_protocol::RECONCILIATION_PREFIX_BYTES` and
`scoped_watch_protocol::RECONCILIATION_PREFIX_BYTES` are **128**. Schema 5
writes its 8-byte session value into bytes 108..116, previously reserved in
schema 4; text still begins at byte 128. The scoped codec tests assert
`128 + text.len()`. There is no prefix discrepancy and no codec/spec change
in this task. Preserve the schema-5 wire format unchanged.

## Proposed public API and return types

Place these documented public types and methods on the public
`MultiSessionOwner` in `server::multi_session`. Keep `TargetState<E>` and its
borrow private. Use existing `SessionInfo`, `Event`, and schema-5 `SessionCursor`.

```rust
pub struct ScopedReconcileState {
    pub info: SessionInfo,
    pub cursor: SessionCursor,
    pub screen_revision: u64,
    pub truncated: bool,
    pub text: String,
}

pub struct ScopedEventPage {
    pub events: Vec<Event>,
    pub cursor: SessionCursor,
    pub resync_required: bool,
}

pub fn reconcile(
    &mut self,
    connection: &Connection,
    target: TargetRef,
    max_bytes: u32,
    max_lines: u16,
    now_secs: u64,
) -> Result<ScopedReconcileState, ServerError>;

pub fn read_events(
    &mut self,
    connection: &Connection,
    target: TargetRef,
    cursor: SessionCursor,
    limit: u16,
    now_secs: u64,
) -> Result<ScopedEventPage, ServerError>;
```

Implement manual `Debug` for both return types: redact `SessionInfo`, cursor,
screen revision, event contents/identities, and text; report only event count,
text length, truncation, and resync state. `Event` already redacts cursor and
target in Debug, but the container's manual implementation should not rely on
revealing embedded metadata accidentally. Do not return encoded wire bytes
from these library methods; the schema-5 codec remains the transport adapter's
pure encoder.

## Shared authorization and exact target selection

For both methods, preserve this order:

1. `global.require_live_connection(connection, now_secs)`; propagate
   `UnknownContext` for absent, expired, wrong-generation, or widened
   capability contexts.
2. Require both `CAPABILITY_READ | CAPABILITY_EVENTS`; missing either returns
   `Unauthorized`, matching `SessionHandler::require_watch_authority`.
3. Borrow the requested exact five-field identity using the consumed
   `launches.session_mut(target)` seam. Map `InvalidTarget` or
   `EffectsUnavailable` to existing `TargetStale`; map an impossible registry
   invariant to `Internal`. Do not infer a target from registry order.
4. Preserve current `require_watch_authority` order after selection: compare
   the full target first (`TargetStale`), then
   `global.require_target_scope` (`Unauthorized`). Do not broaden a grant.
   A stale target and a wrong-scope grant therefore retain distinct existing
   mappings. Extract this target/scope portion as private
   `SessionHandler::require_watch_target_scope(connection, target)`;
   `require_watch_authority` keeps its live/capability checks and delegates to
   it for the existing one-target path. The multi-owner entry point performs
   live/capability checks before lookup, then calls the extracted helper on
   the exact borrowed target. These checks precede cursor/budget validation
   and all metadata synchronization or sink reads.
5. Construct `SessionHandler` over the selected target. Do not call a public
   owner wrapper that repeats target selection. Extract the already-authorized
   bodies as private methods so this module can avoid repeating authority
   checks:

   ```rust
   fn reconcile_preflighted(
       &mut self, max_bytes: u32, max_lines: u16,
   ) -> Result<ReconcileState, ServerError>;
   fn read_events_preflighted(
       &mut self, cursor: EventCursor, limit: u16,
   ) -> Result<EventPage, ServerError>;
   ```

   Existing `SessionHandler::{reconcile,read_events}` and the `StateOwner`
   APIs keep their current validation order and delegate to these bodies.
   The multi-owner methods perform the same checks after exact selection,
   validate scoped cursors/budgets, then call the private bodies directly.
   Once the body begins, preserve its per-target metadata synchronization
   points and error behavior below. Denials before the authorized body do not
   call the sink or synchronize any session.

This makes full target identity (generation, workspace, pane, session,
occupant revision) part of authorization and selection. No default/first
registry target exists. No new `EventJournal`, event queue, or session cursor
map is allocated: each retained `TargetState` already owns its own bounded
`EventJournal`.

## Reconciliation order and exact byte/line bounds

After the shared checks and exact target borrow/authorization:

1. Validate `max_bytes` against
   `RECONCILIATION_PREFIX_BYTES..=MAX_RECONCILIATION_BYTES` (128..=65,536)
   and `max_lines` against `1..=MAX_RECONCILIATION_LINES` (1..=1,024); return
   `InvalidWatchRequest` otherwise. Use the codec constants rather than
   duplicating their numeric values.
2. Pass `max_bytes` unchanged to the existing bounded handler body. Its
   checked subtraction computes `max_text = max_bytes -
   RECONCILIATION_PREFIX_BYTES`, so the sink receives at most `max_bytes -
   128` text bytes and the caller's exact `max_lines`. Any extracted helper
   must keep this checked subtraction before the sink call.
3. Preserve `SessionHandler::reconcile`'s synchronization and read sequence:
   synchronize metadata; fail `NotificationsUnavailable` if unavailable;
   perform one bounded `ReadSource::Screen` read; synchronize again; reject a
   stale/zero screen revision, read error, excess line count, or changed
   revision with the existing `ServerError` mapping. The journal cursor and
   info are sampled only after that check.
4. Convert the local `EventCursor` to `SessionCursor` using this target's
   generation and session, then construct the scoped typed result. Validate
   the actual schema-5 representation using
   `scoped_watch_protocol::encode_reconciliation` and reject an encoded length
   above the caller's `max_bytes` as `InvalidSnapshot`. The maximum text and
   final encoding are both bounded before returning.

Do not duplicate reconciliation's metadata/revision algorithm in a second
copy. Prefer a small private handler extraction that accepts the validated
full budget or returns the existing typed result to the owner; leave
`StateOwner::reconcile` calling the same logic with its original 128-byte
minimum. The final check must use schema-5 encoding, not only the schema-4
validation encoding currently performed by the handler.

## Event page order and cursor mapping

After the shared checks and exact target borrow/authorization:

1. Compare cursor generation to the owner's generation; mismatch returns
   existing `WrongGeneration`.
2. Compare cursor session to `target.session`; mismatch returns existing
   `TargetStale`. The schema-5 codec's `SessionMismatch` is the codec-level
   equivalent, but the server API adds no new error variant.
3. Validate `limit` in `1..=MAX_WATCH_EVENTS`; otherwise return
   `InvalidWatchRequest`. This API is an immediate bounded page only; it does
   not accept or schedule `wait_ms`.
4. Call the selected handler's `read_events_preflighted` with a converted
   `EventCursor { generation, sequence }`; do not call `read_events`, which
   repeats authorization and legacy cursor checks. It synchronizes metadata
   once before journal read, preserves `NotificationsUnavailable`, journal
   error, and `CursorExpired` behavior, and never invokes the effect sink's
   screen read. Convert its returned latest/page cursor back to
   `SessionCursor { generation: target.generation, session: target.session,
   sequence }`.

The cursor check must precede conversion and `EventJournal::read_after`.
Reusing A's sequence for B is valid only when the cursor's session is B;
passing A's cursor to B returns `TargetStale` even when the two independent
journals happen to have the same sequence. On an expired cursor, return no
events, the current target-scoped latest cursor, and `resync_required=true`.

## Events capability advertisement gate

The existing `GlobalAuthority::available_capabilities` grants EVENTS when
the selected `TargetState::notifications_available` flag is true. Target
construction can set that flag before any multi-session read method exists.
Therefore do not let the new owner's `connect` advertise EVENTS merely
because the journal seed is available. In the same source adoption that adds
both scoped methods and their tests, gate new-owner EVENTS advertisement on
the scoped read capability being compiled and covered; then restore the
existing per-target availability condition. `StateOwner`'s current
advertisement and behavior remain unchanged. Do not add a permanent runtime
configuration switch that can claim a capability without implementation.

## Required private unit tests

Use two retained fake targets with distinct full `TargetRef`s and independent
journals/sinks. Test exact data and counters, not only success status.

1. **Independent namespace despite equal sequence:** advance both journals to
   the same sequence. Reconcile A and B; assert each returned cursor includes
   its own session. Read each page and assert only that target's events/sink
   state appears.
2. **Crossed cursors:** send A's `SessionCursor` for B and vice versa with
   equal generation/sequence; assert `TargetStale`, no event page, no sink
   call, and no metadata synchronization. Wrong generation returns
   `WrongGeneration` before journal access.
3. **Authorization/no effects:** missing or expired connection returns
   `UnknownContext`; missing READ or EVENTS returns `Unauthorized`; a live
   grant scoped to A requesting B returns `Unauthorized`. Assert all denied
   cases do zero screen reads, do not consume events, and do not synchronize
   target metadata. Correct scope uses only its selected target.
4. **Reconcile budgets:** min accepted value is 128, max is 65,536, and 127/
   65,537 are rejected before reads. Exact line boundary passes; one line
   over is `InvalidSnapshot`. Assert sink's `max_text_bytes` is exactly
   `max_bytes - RECONCILIATION_PREFIX_BYTES`, including at the minimum and
   maximum. Test checked subtraction and final schema-5 encoded length at the
   limit; no response exceeds caller budget. Use
   `MAX_RECONCILIATION_LINES` for the accepted maximum.
5. **Snapshot consistency:** zero revision/unavailable notifications,
   changed revision, invalid read, too many lines, and valid truncation retain
   existing error/output mapping. The returned `info`, screen revision, and
   cursor belong to one selected target and are captured after the second
   metadata synchronization.
6. **Event limits/resync:** limits 1 and `MAX_WATCH_EVENTS` pass; 0 and
   `MAX_WATCH_EVENTS + 1` fail before journal read. An expired per-target
   cursor returns `resync_required` and a latest cursor with the same target
   session. A current cursor returns only that target's ordered events.
7. **Debug redaction and compatibility:** Debug for both scoped result types
   omits text and numeric target/cursor identifiers. Existing
   `StateOwner::{reconcile,read_events}` tests, one-child runtime tests, and
   schema-4/schema-5 codec goldens remain unchanged.

No test hook may expose a mutable `TargetState`, registry record, or journal.
If an exact sync counter is unavailable without a production seam, parent
review should decide a test-local observation strategy before source edits.

## Adoption stages and unresolved gates

1. Parent adopts the typed APIs, exact mappings, and per-session journal
   isolation.
2. Implement the private handler extraction, owner methods/types, and focused
   tests together. Restore EVENTS advertisement for new-owner connects only
   once both methods and those tests pass.
3. Run focused scoped-watch and multi-session tests, existing `StateOwner`
   watch tests, codec tests, formatting, strict all-target Clippy, docs
   checker, and the parent verifier. No runtime scheduling, socket/CLI route,
   or child-stop behavior is adopted by this document.

The pathname/runtime acceptance and explicit child-stop policy remain
unresolved gates. This task is a source-backed library contract, not evidence
that multi-session reads or watches work today.
