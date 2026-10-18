# Bounded live watch/reconciliation implementation — 2026-10-18

Explicit user request: assign parallel agents to live event delivery/watch and
snapshot reconciliation. Three GPT-6-Luna xhigh owners; parent owns CLI, PTY
adapter, exports, integration tests, canonical docs and roadmap. Implement this
bounded watch request, not an indefinite subscription or provider wait.

FIRST read ROADMAP.md, AGENTS.md and product prerequisites, latest
INTEGRATION.md, P3-NOTIFICATION-RUNTIME.md and this file. Graph first; report
connector failure once then use source. Preserve changes, no dependencies,
ms-rust installation claims, commits or pushes. Match existing style. Edit ONLY
owned files. Parent exclusively owns ROADMAP (exception to append-only rule).
No completed phase/host acceptance or provider recovery claims.

## Shared protocol and API (exact)

New pure `watch_protocol` module, schema4. All integers big-endian. Full target
is five u64 in existing order. EventCursor generation nonzero, sequence0 valid.
WATCH_SCHEMA_VERSION:u16=4, RECONCILIATION_PREFIX_BYTES:usize=128,
WATCH_END_BYTES:usize=32, MAX_WATCH_EVENTS:u16=64, MAX_WATCH_WAIT_MS:u16=1000.

Public Copy/Eq `WatchRequest` enum:
`Reconcile {target:TargetRef,max_bytes:u32,max_lines:u16}` and
`Watch {target:TargetRef,cursor:EventCursor,limit:u16,wait_ms:u16}`.
Reconcile exact56byte request: schema u16 at0,kind1 u16 at2,target40 at4,
max_bytes u32 at44,max_lines u16 at48,reserved6zero at50..56.
Watch exact72bytes: schema/ kind2 at0/2,target at4,cursor.generation u64 at44,
cursor.sequence u64 at52,limit u16 at60,wait_ms u16 at62,reserved8zero at64.
All target fields nonzero; watch cursor generation must equal target generation.
Reconcile max_bytes128..65536 (ENTIRE response),max_lines1..1024;
watch limit1..64,wait_ms0..1000. Zero wait is an immediate poll.
`encode_watch_request(&WatchRequest)->Result<Vec<u8>,WatchError>` and
`decode_watch_request(&[u8])->Result<WatchRequest,WatchError>`.

Public borrowed `Reconciliation<'a> {info:SessionInfo,cursor:EventCursor,
screen_revision:u64,truncated:bool,text:&'a str}`. Response schema4kind1:
schema/kind4bytes,existing encoded SessionInfo72bytes at4..76,
cursor.generation u64 at76, cursor.sequence u64 at84,
screen_revision u64 at92,truncated u8 at100,reserved3zero at101,
text_len u32 at104,reserved20zero at108..128,text at128.
Complete payload<=65536; exact length; nonzero screen revision; cursor gen must
equal info.target.generation. UTF8 sanitized display text permits LF only among
controls and <=1024 lines (empty0, otherwise1+LFcount). No clipping in codec.
Use existing session-info codec for its validity, no duplicate looser schema.
`encode_reconciliation(&Reconciliation)->Result<Vec<u8>,WatchError>`,
`decode_reconciliation(&[u8])->Result<Reconciliation<'_>,WatchError>`.

Public Copy/Eq `WatchStatus::{Changed,TimedOut,ResyncRequired}` and
`WatchEnd {status:WatchStatus,cursor:EventCursor,event_count:u16}`.
Exact32byte response schema4kind2: schema/kind at0/2,status u16 at4 codes1/2/3,
event_count u16 at6,cursor.gen u64 at8,cursor.sequence u64 at16,reserved8zero at24.
Changed count1..64,cursor.sequence nonzero; others count0; cursor.gen nonzero.
Changed cursor means LAST DELIVERED fact, not necessarily latest retained fact.
Timeout cursor is current empty-read cursor; Resync cursor is a hint only and
requires Reconcile before treating state as current. No partial event batch is
valid on Resync. `encode_watch_end(&WatchEnd)->Result<[u8;32],WatchError>`,
`decode_watch_end(&[u8])->Result<WatchEnd,WatchError>`.
All types redacted custom Debug, fixed data-free errors. No unknown-field
leniency, no trailing bytes, validate before encoding. No Event wire payload
changes. Notifications still use exact72byte schema3 Event/requestID0 frames.

## W1 codec owner

Own ONLY src/watch_protocol.rs and docs/progress/P3-WATCH-CODEC.md. Implement
shared protocol above with golden layouts/round trips, invariant/length/UTF8/
reserved/line/payload boundaries and Debug tests. Pure std, use current
event_journal/EventCursor, session_protocol/SessionInfo, control_protocol/TargetRef.
Coordinate exact API with W2/W3 and parent. Temporary harness until parent
exports. Record actual focused tests and owned-file rustfmt. No IO/runtime edits.

## W2 authoritative owner

Own ONLY src/server.rs and docs/progress/P3-WATCH-OWNER.md.
Add default trait `EffectSink::screen_revision(&self)->Option<u64> {None}`.
Parent updates PtyEffects to Some(revision); old fake sinks keep no Events.
StateOwner tracks a bounded EventJournal and last screen/owner/exit observations;
baseline at construction (cursor0), no fabricated initial event. Enable Events
in available capabilities only when metadata revision is Some(nonzero) and
notification tracking healthy. Resume may NEVER escalate the lease's original
selected capability grant. Existing Read/Input behavior must remain intact.

Synchronize committed metadata after public handle_control, handle_session and
poll (including owner expiry). A duplicate/rejected/preflight request without
changed metadata emits nothing. Emit order ScreenChanged then InputOwnerChanged
then ChildExited; true collected child exit only. Owner0 means vacant; if a
Human owner ever occurs, mark notifications unavailable (no fake vacant event).
Unexpected revision loss/zero, journal exhaustion or failed tracking marks
notifications unavailable without dropping child or disabling existing inputs.
Do not turn a journal failure after a process effect into a claim of rollback.
poll still returns only actual sink/owner error behavior, not notification-only
failure that would stop the foreground server. No resetting cursor/identity.

Public server-owned result structs (custom Debug redacts text/IDs):
`ReconcileState {pub info:SessionInfo,pub cursor:EventCursor,
pub screen_revision:u64,pub truncated:bool,pub text:String}` and
`EventPage {pub events:Vec<Event>,pub cursor:EventCursor,
pub resync_required:bool}`.
Methods exact signatures:
`reconcile(&mut self,connection:&Connection,target:TargetRef,max_bytes:u32,
max_lines:u16,now_secs:u64)->Result<ReconcileState,ServerError>`;
`read_events(&mut self,connection:&Connection,target:TargetRef,cursor:EventCursor,
limit:u16,now_secs:u64)->Result<EventPage,ServerError>`.
Both require live exact connection, Read|Events and exact current target before
calling sink/accessing journal; add fixed errors as needed. Wrong generation,
future cursor, stale target, unavailable tracking fail closed with fixed labels
(`wrong_generation`, `cursor_ahead`, `target_stale`, `notifications_unavailable`).
Expired cursor returns EMPTY EventPage with resync_required=true and latest
cursor hint. Successful page cursor=last returned cursor, or latest cursor when
empty. No filter skipping: this prototype has one fixed target only.

Reconcile performs one bounded sanitized Screen read max_text=max_bytes-128,
max_lines with no client input, then synchronizes metadata and verifies returned
read revision equals EffectSink current revision. Capture SessionInfo, screen
text and journal cursor under this same owner turn, no further poll between.
Validate full encoded Reconciliation against request byte/line cap using W1.
If sink read changes metadata, account for it before binding cursor. Mismatch
fails closed. Do not reserve an operation key, expose raw control bytes or alter
pending context uncertainty. Queries authorize no process effect or replay.

Focused fake-sink tests: Events negotiation/resume non-escalation, snapshot and
cursor bind to same revision/owner/exit, screen/owner/expiry/exit facts, duplicate
and rejected operation no new event, cursor old/future/wronggen/stale target,
invalid read budgets/line/control text, notification failure leaves child/input
owned, sequence exhaustion test-only fixture. Old tests must keep passing.
Use root lib filter after W1 export or temporary harness. Record actual output.

## W3 delivery/driver/transport owner

Own ONLY src/server_io.rs, src/server_runtime.rs, src/transport.rs and
docs/progress/P3-WATCH-DELIVERY.md. Coordinate W2 APIs above, W1 types above.
Parent owns CLI and real PTY tests. Do not edit other tests/modules/exports.

Transport: retain default denial of Event and Snapshot before enablement. Add
`AuthenticatedStream::enable_metadata_events(&mut self)->io::Result<()>` only
for caller use AFTER validated negotiated Events; reject poisoned stream. When
enabled accept/write only exact72byte valid schema3 Events (header validation
before allocation; requestID0 from envelope). Snapshot always unsupported.
Worker incoming client Event is still a forbidden REQUEST role, even enabled.
Test pre-enable denial, post-enable exact event, malformed/oversize header body
rejection and no poisoning bypass. Do not weaken current deadlines/path/auth.

Worker decode schema4 requests into RequestSchema::Watch(WatchRequest). Both
Reconcile and Watch need selected Read|Events; owner repeats authoritative check.
Enable stream events only after validated Welcome selected Events. For a
reconcile OwnerReply::Response, validate decoded full target, request max_bytes
and max_lines before writing. Existing schemas/roles unchanged.

Add OwnerReply variant `Watch {events:Vec<Event>,end:WatchEnd}`. Only a Watch
request can receive it; verify <=request.limit, count/status agreement, exact
target, consecutive cursors strictly after requested cursor and end.cursor
matching last event when Changed. Resync/Timeout emit zero events. No batch
allocation beyond64 metadata Events. Write all Event frames requestID0 and the
WatchEnd Response with original requestID using ONE absolute250ms write budget
for the entire batch. Single worker remains sole writer; no extra thread or
channel. Capability/body/role mismatch closes connection before leaking events.

Driver handles schema4: Reconcile calls W2 and encodes response; Watch calls
read_events and replies Changed immediately if events, ResyncRequired if old,
TimedOut immediately if empty and wait0. For empty positive wait, retain the
OWNED OwnerEnvelope with request connection/target/cursor/limit and absolute
Instant deadline<=1000ms. At most8 pending watches, fail full before storing.
Every tick pumps state/processes bounded owner queue and revisits at most8
pending watches without blocking/sleeping on owner thread. Revalidate live
authority/target on each attempt; return Changed/Resync/Error when appropriate,
or TimedOut at deadline. Existing worker reply wait remains2seconds. Clock used
for ledger authority is caller monotonic seconds; Instant only for finite watch
duration. Pending reads do not keep child alive separately or signal it.
Lost reply receiver drops observation; never resend/cancel effects. Stop settles
pending watchers boundedly after connection shutdown; Drop stops workers before
dropping state. Fixed eight-worker/ownerqueue/replyslot limits remain. Retained
notification batch bound64*(72+24)+56endframe=6200encoded bytes/worker, separate
from snapshot/ordinary control response bounds. No unbounded output queue.

Tests: actual socketpair negotiated event frames followed by correct terminal
Response; requested limit/page and timeout; current target/expired cursor/errors;
no Events capability denial; malformed owner batch; disconnect; slow writer /
deadline and eight pending cap/shutdown. Fake sinks may implement Some(revision)
for tests. Finite waits, no host/path acceptance claim. Run focused root tests
after W1/W2 ready, owned-file rustfmt, strict Clippy. Record actual initial
failures/corrections, gotchas, remaining limits and release ownership.

## Parent integration

Parent exports watch_protocol, updates PtyEffects metadata method and CLI:
`reconcile TARGET` and `watch TARGET GEN:SEQ LIMIT WAIT_MS`. These are read-only
queries, no context sequence mutation; require negotiated Read|Events. New init
requests Events optionally; old leases cannot escalate and fail visibly rather
than silently creating replacement contexts. CLI validates bounded complete
event batch/end and preserves stored context uncertainty. Real authenticated
socketpair/private-PTY proof must cover output during watch, atomic reconcile,
ownership/exit, no duplicate input and bounded shutdown. Canonical docs and
ROADMAP reflect actual results. Phase2 clipboard/zsh host retest remains open.
