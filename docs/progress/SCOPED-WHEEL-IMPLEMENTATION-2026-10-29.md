# Single-event scoped wheel: implementation contract

Design only, 2026-10-29. Checkout `t3-ca0a6ac8`, branch
`t3/improve-codex-mouse-scroll-speed`, base `daf28a63f062`; preserve all existing
dirty v0.1.7 work. This document is the only file owned by this consultation.
No production edits, builds, installation, restart, live input, provider calls,
or delegation were performed. Proposed checks below are not completed checks.

## Decision and boundary

Implement one capability-negotiated `ScopedMouseWheelInput` request per captured
event. It atomically scopes ownership around one synchronous PTY delivery in the
existing serialized owner. Healthy execution uses one `ContextFile::reserve`,
one request/reply, and one `ContextFile::ack`: two full durable flushes instead
of six. Existing occasional cumulative Receipt traffic remains; connection,
recovery, mode-rejection queries and screen observation are outside that count.
No batch, gesture lease, changed FIFO/TTL, disabled flush, owner wakeup change,
parallel input execution, or polling rewrite is included.

[Actual context measurements](CODEX-WHEEL-TRANSACTION-COST-2026-10-29.md)
show roughly 18.5 ms per three-pair logical report and roughly 54 reports/s.
The parent supplied independent 20/100-report reruns at 52.49/52.31 reports/s.
These justify removing two operations, but establish neither the fraction of
end-to-end delay nor a threefold visible speedup. The
[source diagnosis](CODEX-WHEEL-ASTRA-DIAGNOSIS-2026-10-29.md) remains relevant to
later observation/polling work; that work is not a prerequisite for this fix.

Use existing schema-1 status replies. For this new request only,
`Completed(Applied)` means **all wheel bytes written and the scope's ownership
released before the owner handles another message**. It does not mean the child
rendered, or that the pane is still vacant when the client reads the reply.
No owner revision is needed for the next scoped event: it requires vacancy,
not client-supplied CAS. Recovery requiring current ownership uses Describe.
Do not invent a custom result store or overload an existing wire field.

## Wire and capability allocation: fixed choices

Current `src/ui_protocol.rs` allocates request kinds 1 through 7 (5 is the
Hello attachment trailer), `UI_SCHEMA = 8`, and independent layout reply schema
9. Current handshake bits occupy 1 through 512. Add:

- `KIND_SCOPED_MOUSE_WHEEL_INPUT: u16 = 8` (private, alongside other kinds).
- `SCOPED_MOUSE_WHEEL_INPUT_BYTES: usize = 80`.
- Public `ScopedMouseWheelInput { key: OperationKey, target: TargetRef,
  column: u16, row: u16, up: bool }`, with the same redacted Debug policy as
  `MouseWheelInput` and `operation_class() -> OperationClass::Input`.
- `encode_scoped_mouse_wheel_input(&ScopedMouseWheelInput) ->
  Result<[u8; SCOPED_MOUSE_WHEEL_INPUT_BYTES], UiCodecError>`;
  private `decode_scoped_mouse_wheel_input(&[u8])`; new
  `UiRequest::ScopedMouseWheelInput` variant.
- `CAPABILITY_SCOPED_MOUSE_WHEEL: u64 = 1024` (`1 << 10`), capability name
  `scoped_mouse_wheel`; add to `CAPABILITY_NAMES` and `KNOWN_CAPABILITIES`.
  This is optional and requires separately selected `CAPABILITY_INPUT`.

All integers big-endian, offsets from payload start; outer envelope unchanged:

| Offset | Bytes | Value |
| ---: | ---: | --- |
| 0 | 2 | UI schema 8 |
| 2 | 2 | kind 8 |
| 4, 12, 20 | 8 each | key generation, lease, sequence |
| 28, 36, 44, 52, 60 | 8 each | target generation, workspace, pane, session, occupant revision |
| 68 | 2 | zero-based content column |
| 70 | 2 | zero-based content row |
| 72 | 2 | direction: 1 up, 2 down |
| 74 | 6 | zero reserved bytes |

There is deliberately no expected owner revision, count, repeat multiplier,
raw escape string or caller TTL. The request means acquire only if vacant now.
Use `exact_size(bytes, 80)` before field reads: shorter is Truncated, longer
is TrailingBytes. Reject zero key/target components, generation mismatch,
direction outside 1/2, nonzero reserved bytes, column >= `MAX_PANE_COLUMNS`,
row >= `MAX_PANE_ROWS`, unsupported schema and unknown kind. Actual PTY
dimensions are checked again by the owner. Encoder validates before emitting
bytes. Update `decode_ui_request`, `UiRequest::operation_class` and Debug;
leave all old kinds and encodings unchanged. No outer-frame limit increase.

The existing status reply is exactly 37 payload bytes: schema 1 at 0, status
kind 1 at 2, key at 4/12/20, body length 1 at 28, reserved u32 zero at 32,
status byte at 36. Existing codes are 0 NotAdmitted, 1 InProgress, 2 Applied,
3 Failed, 4 Partial, 5 Unknown. Existing rejection reply is 38 bytes, kind 3,
body length 2, existing big-endian `RejectionCode` at 36. No response codec
change or new outcome is required. `Accepted` is not a legal terminal result.

## Negotiation and dispatch: all paths

1. Add the optional bit to `ui_client::conn::handshake`. Store selected support
   in `PaneConn::scoped_mouse_wheel: bool`, initialize it on connect and replace
   it from each reconnect Welcome. Never infer support from version strings.
2. Add the bit to `MultiSessionOwner::connect`'s capability mask. Also update
   `GlobalAuthority::connect_with_capability_mask` in **src/server.rs**: its
   explicit available-bit construction otherwise drops the new bit. Include
   it only when the passed mask contains it and the session's available input
   capability is present. Resume continues intersecting the saved grant with
   availability; never widen an old grant. A fresh lease can negotiate support.
3. Keep `GlobalAuthority::connect`'s single-session mask READ|INPUT|EVENTS.
   `server_runtime::dispatch` does not implement UI schema 8; it must not
   advertise this operation. No single-session implementation is added.
4. Add `RequestSchema::UiScopedMouseWheel(OperationKey)` in `multi_runtime.rs`.
   `classify` maps the new decoded variant to it; `response_matches` accepts
   only matching-key schema-1 Status or Rejected bodies, not Read bodies.
   It is not a snapshot or watch request. Route it through the existing UI
   dispatch branch in `dispatch_ui`, invoking the new handler below with
   `PtyEffects::validate_mouse_wheel` and `PtyEffects::mouse_wheel` closures.
5. Enforce both capabilities in the owner, regardless of worker checks.
   An optional early worker check in `run_worker` is unnecessary: use the
   owner's keyed Unauthorized rejection. Do not let broad UI decoding bypass
   authorization. Keep the usual bounded queue/deadline and Error framing.

Old-server fallback is selected **before sending**, based on negotiation.
Git source `10d81c24cdc2` already has schema-8 HistoryQuery kind 6 and owned
MouseWheelInput kind 7 and ignores unknown optional Hello bits. It therefore
supports the fallback below without a schema bump. Its committed capability
vocabulary differs from the recorded dirty running 0.1.4 mask 511: this is
source compatibility evidence, not proof of that running binary's exact code.
Old clients against a new server keep old behavior and bytes. A single-session
server does not become UI-capable through this change; its existing unsupported
UI behavior remains. Never probe kind 8 speculatively, and never resend kind 7
after a kind-8 timeout/error. The speed improvement requires an updated server;
this document does not authorize restarting or replacing the user's server.

## Owner implementation and prepared state transitions

Add `MultiSessionOwner::handle_scoped_mouse_wheel<V,A>` with the same arguments
and closure bounds as `handle_mouse_wheel`, replacing the request type with
`ScopedMouseWheelInput`. Keep the old handler intact. Use the existing
`SessionHandler`, `known_reply`, `rejected_reply`, `status_reply`,
`terminal_outcome`, `is_capacity_error` and metadata synchronization.

No new `InputOwner` state or production API is necessary. Its existing Clone,
snapshot, claim, begin_input, finish_input and release methods allow all three
possible owner states to be prepared without touching authoritative state.
This avoids a fallible release after the global ledger says Applied.
Use a fixed scope TTL of `MAX_OWNER_TTL_SECS` (30, matching the current client).
This is a failure-retention expiry, not an idle performance lease.

Execute in this exact order, on the serialized owner with no yielding:

1. `global.require_live_connection(connection, now_secs)`. Check key generation
   and lease match server/connection and both Input and ScopedMouseWheel are
   selected. Failure: existing Unauthorized rejection (or existing live-context
   error), no effects. Codec validation has already rejected malformed fields.
2. Call `known_reply(&global, key, payload, now_secs)` BEFORE target, owner or
   mode revalidation. Existing identical key/payload returns retained status;
   changed payload returns the existing protocol conflict error. Neither
   path invokes effects, claim or release. Current capability checks still
   precede this lookup as in the existing handler.
3. `require_target_scope` (Unauthorized) and `launches.session_mut(target)`
   (StaleTarget for InvalidTarget/EffectsUnavailable). Never resolve a fresh
   target from pane number or replace occupant identity.
4. Run `validate(&session.effects)` first. Runtime closure checks live/nonclosing
   PTY, active alternate screen, enabled mouse reporting, actual coordinates,
   empty input queue and capacity, including the selected mouse encoder's
   coordinate limits. Return its Unsupported/InvalidRequest/Capacity code.
   No owner mutation or ledger admission occurs. In particular stable no-mouse
   returns Unsupported even if an unrelated owner is holding the pane.
5. Inspect `input_owner.snapshot()`. Reject `InputBusy` if **either holder or
   pending is Some**, even for this same client lease and even if its TTL is
   due. Do not call expire here to manufacture vacancy. Ordinary server polling
   continues its existing expiry policy; the request itself steals nothing.
6. Prepare three local clones using only existing APIs:
   `pinned = input_owner.clone()`; call `pinned.claim(Owner::Client(key.lease),
   initial.revision, MAX_OWNER_TTL_SECS, now_secs)`; then `pinned.begin_input`
   with that claimed revision and key. Set `retained = pinned.clone()` and call
   `retained.finish_input(key)`. Set `released = retained.clone()` and call
   `released.release(Owner::Client(key.lease), claimed.revision)`.
   Map preparation errors using existing `owner_rejection`, except vacancy
   rejection above is explicitly InputBusy. All are pre-admission failures.
   Revision r+1 and r+2 and `now+30` are thus proven representable. At
   r=MAX-1 or MAX reject without admission; r=MAX-2 may finish at MAX.
   The clones are not observable owner mutations.
7. Pre-encode four terminal status replies (Applied/Failed/Partial/Unknown)
   before effects, so result encoding/allocation is not a post-release step.
   Then `global.ledger.admit(key,payload,now_secs)` reserves its bounded payload
   and operation record. Execute only on `Admission::Execute`. Existing other
   admission results return their status without swapping any owner state;
   capacity errors return Capacity; other errors preserve existing mapping.
   No independent new result cache is needed: `Ledger::complete` changes an
   already reserved record and allocates no completion slot.
8. Assign `session.input_owner = pinned`. Invoke `apply` exactly once, normalize
   Accepted to Unknown with `terminal_outcome`. `PtyEffects::mouse_wheel` already
   rechecks mode, encodes once, uses bounded `drain_request`, and discards all
   unwritten suffixes. Do not replace it with raw PTY writes or another queue.
9. Call `ledger.complete(key,outcome)` while the authoritative owner is still
   pinned. If it fails, return the existing error, leave that pin/owner intact,
   and return no terminal success. Do not manufacture a pre-admission rejection,
   clear the pin, or retry effects. In the normal serialized path completion
   cannot fail: this exact key was just admitted and no intervening ledger
   mutation/receipt/expiry occurs. Test this invariant.
10. Immediately after successful completion, perform only an infallible owner
    assignment: `released` for Applied; `retained` for every other outcome.
    No fallible method, callback, await, journal write or reply encoding may be
    inserted between completion and this assignment. Return the pre-encoded
    status. Run `handler.synchronize_metadata()` after the transaction, following
    existing handler structure. Metadata notification failure cannot undo it.

This is logically atomic to other owner messages, not durable across a process
crash. A fatal interruption in step 9/10 loses the volatile ledger with the
server generation; it cannot expose Applied in a surviving generation before
release. If future work permits owner reentrancy, this invariant must be revisited.

## Status, ownership, and recovery contract

| Observation for this key | Delivery assertion | Scoped ownership assertion / client action |
| --- | --- | --- |
| Rejected | Not admitted, no wheel effect | Request changed no owner; do not release anyone |
| NotAdmitted in live generation | Not admitted at status observation | Does not reserve the key or authorize resend; drop this event |
| InProgress | Unresolved | May be pinned; retain uncertain context, no new input/replay |
| Completed Applied | Complete PTY write | This scope released at completion; clear local scope bookkeeping, no Release RPC |
| Completed Failed | No wheel bytes known delivered | Scope did not release; reconcile current owner explicitly |
| Completed Partial | Prefix delivered, suffix discarded | Scope did not release; never send suffix; reconcile explicitly |
| Completed Unknown | Delivery unknown | Scope did not release; no resend; reconcile explicitly |
| Error / invalid or lost reply | No trustworthy new assertion | Query OperationStatus, never resend the event |
| Generation changed / expired result | Old result unavailable | Unknown historical delivery/release; discard unsent work, no replay |

“Did not release” is the operation's historical disposition, not a permanent
lease guarantee. Existing terminal-input behavior clears the pending pin once
the sink returns a terminal result; the holder remains with the original
30-second expiry. `SessionHandler::poll_session` may later expire that holder.
Do not claim that failed/partial/unknown ownership survives TTL, server death,
or explicit release. This design preserves those existing semantics and does
not add an indefinitely retained claim. If ledger completion itself fails, the
pin remains and expiry returns `PendingNeedsResolution`, as today. Explicit
recovery must distinguish that unresolved internal case from a completed result.

For a retained terminal result, recovery is existing read-only OperationStatus
plus Describe of the exact target. If its current holder is the same lease and
there is no pending input, an explicitly requested Release may use that exact
current revision. Otherwise report the current disposition; never release a
different owner or renew/steal a same-caller retained claim. Status/Describe do
not clear ownership. No new reconciliation command or automatic recovery release
is added by this feature. `Completed(Applied)` retrieved after a lost reply is
enough to establish historical scoped release, but it is not permission to
release a later claim. No extra revision-bearing response is required.

Client crash before reserve: nothing sent. After reserve/before write: durable
uncertainty reconciles NotAdmitted where available; never recreate the wheel.
After write/before reply or during ack flush: retain uncertainty and query
status. A terminal Failed/Partial/Unknown may clear the durable key through
existing reconcile/ack while ownership remains; clearing a context key is not
release. Server crash at any point means a new generation and unknown old
effects, not exactly-once recovery. Duplicate Applied after later claims or
target/mode changes returns Applied without touching those claims. A duplicate
failed operation likewise cannot clear a retained owner. Conflicting same-key
bytes never reach the sink.

## Client changes and no-mouse repair

Add `PaneConn::send_scoped_mouse_wheel(column,row,up)` returning the existing
`MouseInputResult`. Require negotiated bit and Input locally before reserving;
use `keyed` with the new encoder then `settle_response`, preserving key validation,
durable ack, cumulative receipt and uncertain-status behavior. Map Applied to
Applied; Unsupported to Unsupported; InputBusy/StaleOwner/ExpiredOwner to NotOwner;
all other statuses/rejections/errors to Failed with truthful labels. For
Failed/Partial/Unknown use “wheel delivery failed/partial/unknown; ownership
release not confirmed”. InProgress stays uncertain. Never set `owner_revision`
from a scoped response and never call `release_confirmed` for scoped input.
Do not erase a preexisting local retained revision on a rejected scoped call.

`Net::alternate_wheel` selects:

- Negotiated scoped support: call the method directly, with no Describe/Claim
  beforehand. Applied yields `ScrollResult::MouseDelivered`. Unsupported proves
  pre-admission rejection: issue read-only `seek_history(target,epoch,0)` to
  obtain authoritative mode and existing `history_result` feedback; never send
  this wheel again. Stable alternate/no-mouse yields AlternateScreenNoMouse
  and the existing “use its scroll keys” message, without owner transitions.
- No scoped support: run the existing `scroll_inner(target,epoch,0,
  Some((column,row,up)))` query-first path. Its HistoryQuery validates exact
  target/checkpoint and exposes both alternate and mouse flags before a claim.
  This deliberately restores one read RPC on the legacy direct path. Do not
  infer mouse reporting from the cached alternate-screen bit or from ScreenReply.
  Use fresh query results, not a positive mode cache: no extra cache is needed.

Inside `scroll_inner`, after the read proves alternate+mouse, choose the scoped
method if negotiated (for callers arriving through the normal-scroll path),
otherwise the owned legacy method. Extract the existing legacy sequence into
`PaneConn::send_legacy_scoped_mouse_wheel` if needed for one shared implementation.
For that sequence add `PaneConn::claim_vacant` by factoring the current claim
body: initial Describe must match the exact target and require owner_lease==0,
including refusing this caller's retained claim; then retain existing keyed
Claim CAS, post-claim Describe, send and confirmed release. Do not change ordinary
typing/paste's existing `claim` semantics in this packet. Use `claim_vacant`
only for these wheel sequences. No double Describe before Claim is needed.
Only Applied or an explicit pre-admission Unsupported rejection can release the
claim acquired by this legacy sequence; failure/partial/unknown do not release.
Unsupported then performs the existing fresh READ-only fallback.

Mode can race the legacy READ: the server's kind-7 revalidation remains final.
Stable no-mouse must cause zero claim operations; a transition after a positive
read can still cause a safely rejected legacy claim/send sequence. Missing Input
must refuse before Claim or any wheel reservation. A new scoped Unsupported
must never downgrade to a legacy send. Server Error is not Unsupported.

Keep the existing FIFO32, capture-age expiry, epoch/cancellation logic and worker
single-flight behavior. Errors clear unsent wheel intents through existing state
handling. Do not claim a capture-to-PTY deadline: admitted worker commands can
still wait behind a poll. No production edits to state.rs/worker.rs/host.rs are
needed for this operation. Their existing tests must continue to pass.

## Exclusive implementation packets, in dependency order

These are assignments for the parent to launch, not active agents. Workers make
no commits, installs, restarts or live inputs; parent reviews and integrates each
packet before the next. Do not overlap file ownership. Current dirty files must
be read and preserved. No dependency/manifest changes.

| Packet | Exclusive writable files | Deliverable and dependency |
| --- | --- | --- |
| W1 codec | `src/ui_protocol.rs`, `src/handshake_protocol.rs` | Exact allocation/codec above, inline codec and negotiation tests. First. |
| W2 server | `src/server.rs`, `src/server/multi_session.rs`, `src/multi_runtime.rs`, `src/input_owner.rs` (tests only) | Available mask, prepared-clone transaction, dispatch/classification tests. Depends W1. No production InputOwner API change. |
| W3 client | `src/ui_client/conn.rs`, `src/ui_client/net.rs` | Optional negotiation, one-operation path, query-first fallback and vacancy-only wheel claim; inline tests/fake peer. Depends W2. |
| W4 fixtures | `tests/wheel_transaction_cost.rs`, `tests/scoped_wheel.rs` (new), `tests/scoped_wheel_latency.rs` (new) | Deterministic private protocol tests and opt-in service/receiver measurements. Depends W3. Preserve existing `tests/codex_wheel_latency.rs` as coarse no-loss baseline. |
| W5 parent evidence/docs | `docs/SPEC.md`, `README.md`, `ROADMAP.md`, `docs/progress/SESSION.md`, `.agents/skills/panelink/SKILL.md`, `.agents/skills/panelink/references/interactive.md`, `docs/progress/SCOPED-WHEEL-RESULT-2026-10-29.md` (new) | Describe actual support, compatibility, metrics and open acceptance; validate skill and links. After verified integration. |

W4 must not add timing hooks across W2/W3-owned production files. Measure those
components through existing public APIs/private peer endpoints. If later full
UI stage instrumentation is desired, assign a separate bounded task after these
results; do not make speculative instrumentation a hidden dependency here.

## Required deterministic tests and review checks

- W1: literal golden 80-byte request; round trips both directions; every truncated
  prefix, 81-byte/large suffix, nonzero each reserved byte, invalid directions,
  all zero identifiers, generation mismatch, max-1/max coordinates. Assert Input
  class and Never retry. Old kind golden vectors unchanged. Unknown optional
  bit tolerated by a legacy mask; new bit not implicitly required.
- W2: fresh multi grant selects bit only with available Input; resume old grant
  cannot widen; fresh supported grant resumes; single owner never selects bit.
  Test `classify`, correct-key response matching, wrong key/Read/snapshot rejection,
  and wire-to-PTY route, not just direct handler calls.
- Empty/vacant success: exactly one effect invocation, final holder/pending None,
  revision +2, ledger Applied. Missing capability (each separately), foreign
  scope, stale occupant, dead/closing PTY, normal mode, no mouse, invalid actual
  coordinate, nonempty input queue and full ledger: no owner change/no effect,
  and no new admission. No-mouse plus foreign holder returns Unsupported unchanged.
- Same-caller retained holder, foreign holder, pending key, due-but-not-expired
  holder: reject InputBusy, unchanged revision/expiry/pin; zero effect calls.
  Exercise before and after failed scoped input. Do not use a same-caller claim
  renewal as the success fixture setup.
- Revision MAX-1/MAX reject before ledger admission; MAX-2 can Applied-release
  at MAX. Time overflow rejects before admission. Capacity exhaustion must not
  strand a newly acquired owner. Include lease/global payload/count limits.
- Inject apply outcomes Failed, Partial, Unknown and Accepted: normalize Accepted
  to Unknown; retain holder at r+1 with original expiry, clear completed pin,
  record precise outcome; never release. Inject/simulate completion failure:
  leave pinned state and no success. Assert ordinary expiry only clears completed
  unpinned retention, and never clears unresolved pin. Explicit Release requires
  correct lease/revision and no pin; status alone never releases.
- Identical duplicate after success, failure, later foreign claim, mode change
  and target retirement: existing status, no effect/owner mutation. Changed
  payload same key conflicts. Receipt retirement / lease expiry / server generation
  change yield bounded uncertainty, never another effect.
- Drop response after execution, truncate response, wrong response key, disconnect
  before/after write, and reconnect: only OperationStatus after uncertainty, zero
  repeated kind-8 requests. Applied status confirms historical release; failed
  status does not. Verify new event cannot take retained same-caller ownership.
- PTY fixture with forced zero-write/partial-write/unknown paths: unwritten suffix
  discarded, later ticks/new owners cannot flush it. Retain existing drain tests
  as well as handler mock tests; mock Applied alone does not prove PTY behavior.
- W3 fake peer counts ordinary healthy operations: one reserve/ack pair and one
  mutation request, no Claim/Release/Describe; account separately for Receipt.
  Bad ack durability never triggers resend. Verify old-mask handshake never sees
  kind 8. Legacy stable alt/no-mouse: one HistoryQuery, zero Claim/Input/Release,
  correct ScrollResult even when held. New path no-mouse: rejected scoped request
  plus history read, zero owner transitions. Mode races remain nonreplaying.
- Existing typing/paste, history, selection, queue cancellation, expiry, screen
  polling and resize tests remain unchanged in meaning. Run permitted focused
  tests first, then parent canonical non-UI verifier, formatting, Clippy and doc
  link/skill validation. Record private-PTY tests separately from ordinary CI.

## Finite measurement specification

Extend `wheel_transaction_cost::run_case` with explicit operations_per_report
1 or 3 and collect **per-logical-report elapsed samples**, not only pooled
reserve/ack samples. Fresh private locked context per case; 1,20,100 and 300
reports, warmup separately labeled; alternate case order over five repetitions.
Report reserve and ack separately (counts, sums, p50/p95/p99/max), complete
logical report distribution and wall throughput. All durable flushes enabled.
Context creation excluded and reported separately; no concurrent shared-context
writers. Print once after each finite case. Pair count is not an RPC count.

`tests/scoped_wheel_latency.rs` compares real private same-build server paths:
one scoped operation versus the existing three keyed Claim/Input/Release
operations with their necessary Describe calls. Use exact real context reserve/
ack and real transport, not a context-only extrapolation. Run isolated panes with
one connection and no receipt boundary initially, then >=100 events with normal
receipt behavior; report receipt count/time separately, not disabled overhead.
Record each logical service interval from before reserve to final ack/release
confirmation, and each RPC write-to-reply time; reserve/ack timings measured
around actual API calls. No need to mutate production for this harness client.

Use the integration-test executable itself as a dedicated receiver subprocess
(one ignored helper test selected by an explicit private fixture token/path),
launched under the managed PTY. It enters raw mode, alternate screen and SGR
mouse reporting, incrementally parses input in one process, records monotonic
arrival samples in a preallocated vector, and writes a bounded counter redraw
after each report. At a finite count/deadline it sends one binary sample block
over a fixture-private socket and restores terminal modes. No shell `dd`, command
substitution, per-report process, log append or disk flush. Distinct direction/
coordinate pairs identify sequence positions; fixture compares exact order and
count, not just identical up-report totals. Timestamp receipt of each complete
report, first output write and final output write. Parent timestamps send and
screen observation using its monotonic clock; do not subtract independent
`Instant` origins across processes. Receiver emits durations since a synchronized
start marker, with synchronization uncertainty reported, or reports interarrival
durations separately from parent service latency.

Keep bounded sample vectors (4096 max), finite 10-second per-case deadlines,
identity-safe fixture cleanup, and private files/socket paths. Receiver readiness
must be observed before timing. Required cases: 20-report burst; sustained
20/60/120 reports/s for five seconds; reversal midway; one pane and four panes
with a bounded busy sibling. A separate attach-client fixture can inject into
the private outer PTY to exercise the existing FIFO and observe final visible
count; parse the outer output incrementally instead of reparsing its entire
history. Do not attribute protocol-only harness times to full UI capture/render.
Report sent, admitted, delivered, visible final count, duplicate, overflow,
expiry/cancellation and timeout totals; a 64-report UI burst may legitimately
drop beyond the finite queue and must account for those drops.

Record build fingerprint, geometry, profile, host, server mask, sample counts,
polling configuration and direct receiver baseline. Compare distributions and
the actual one-versus-three operation service rates. Provisional goals from the
diagnosis remain acceptance candidates: sustained 60/s without growing backlog,
20-report last-visible burst <=250 ms, input-to-PTY p95 <20 ms, input-to-visible
p95 <50 ms/p99 <100 ms. Do not put hardware timing thresholds in ordinary CI
or increase TTL to pass them. Failures are results, not permission for a broader
rewrite. Real Codex/host displacement, acceleration and visible responsiveness
remain separately authorized acceptance; this synthetic receiver proves none
of those application-specific behaviors.

## Self-review and handoff

Lost-reply review: authoritative key remains queryable in the live ledger; no
request replay path is added; Applied is completed only adjacent to an already
prepared, infallible release-state assignment. No status consumer mistakes
successful bytes with a fallible later Release for scoped success. The ordinary
kind-7 semantics remain unchanged.

Same-caller review: a retained holder is refused before any authoritative owner
mutation; legacy wheel's vacancy-only Describe/CAS also refuses it. Duplicate
and status routes never release. No idle claim is used to amortize cost.

No unresolved design blocker for this bounded implementation. Retention is
explicitly the existing expiring-owner model, not indefinite retention; complete
outcomes clear pending pins, while internal unresolved completion keeps its pin.
The existing generic context reconciliation API does not expose operation kind
or current ownership: use status as historical evidence and Describe for present
ownership, never infer current vacancy or automatic release from an ack.

Read-only evidence: AGENTS/onboarding documents, current roadmap/session and
wheel reports; graph-first discovery for project
`Users-sahillamba-.t3-worktrees-panelink-t3-ca0a6ac8` reported ready, followed by
current-file verification because its index is stale. Source anchors include
`ui_protocol::{decode_ui_request,decode_mouse_wheel_input}`, handshake constants,
`GlobalAuthority::connect_with_capability_mask`,
`MultiSessionOwner::{connect,handle_mouse_wheel}`, `multi_runtime::{classify,
response_matches,dispatch_ui}`, `InputOwner`, `Ledger::complete`,
`PtyEffects::{validate_mouse_wheel,mouse_wheel,drain_request}`,
`PaneConn::{keyed,settle_response,settle_uncertain,claim,release_confirmed}` and
`Net::{alternate_wheel,scroll_inner}`. No live identity refresh was performed;
installed/running versions above come from current task reports, not this run.
