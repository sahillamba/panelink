# Wave 2 client responsiveness design

Status: parent-approved scoped engineering design on2026-10-22, implemented
under WAVE2-TASKS.md. The baseline and API sketches below record the original
proposal; final limits, behavior and test boundaries are authoritative in
W2-CLIENT-RESULT.md and WAVE2-RESULT.md.

At design time, the host owned `App` and synchronously called `Net`
through `Effects`; socket exchanges and client-context durability therefore
ran on the same thread that read input, flushed frames, and restored the host
terminal.

## Current call paths and constraints

- `ui_client::host::run_inner` calls `Net::connect` before raw mode. This
  discovers/resumes contexts, handshakes, fetches layout and attaches panes.
  Keep initial connection before `HostGuard::enter` for this slice; otherwise
  startup needs a separate loading/error state and terminal ownership change.
- `host::event_loop` calls `App::handle_event` with `&mut Net` as `Effects`,
  `App::flush` after the key debounce, then `Net::poll` before every render.
  Input polling, layout/resize, watch, screen fetch, renew, and claim are
  sequential network work in `Net::poll`.
- `Net::refresh_layout`, `maybe_resize`, `fetch_screen`, `maintain` and its
  attach/reconnect paths call `PaneConn::request` or other synchronous
  `PaneConn` methods. Each exchange has a deadline, but multiple exchanges can
  accumulate in one host-loop iteration.
- `PaneConn::keyed` calls `settle_uncertain`, builds the operation payload,
  durably reserves the next context key, sends once, then classifies the reply.
  `send_keys`/`send_paste` eventually use this path. The reserve must remain
  before the socket send. Input/paste payloads must remain volatile and must
  never be resent after an uncertain acknowledgement.
- `Effects::split` and `Effects::close` synchronously wait for server replies
  (up to their longer deadlines). `App::activate` and `close_pane` also apply
  focus/status changes in the same call. Split and close plans already capture
  their target and layout revision; this snapshot must be preserved.
- `App::flush` currently empties the key buffer before invoking `send_keys`.
  Moving work behind a queue must make queue rejection or worker failure
  visible; it must not silently discard the batch or retarget it after focus
  moves.
- `Net::detach` performs bounded synchronous settle/release/receipt work, and
  `HostGuard` restores the terminal before detach. Preserve that ordering.

The known user-visible path is host loop → `App` → `Effects for Net` →
`PaneConn` → socket/context. The independent U3 server cleanup finding is
outside this assignment and remains open. Code graph discovery was refreshed
before source inspection; exact source call paths above are authoritative.

## Proposed ownership boundary

Introduce one private client IO worker, owned by the attach session. It is the
sole owner of `Net`, every `PaneConn`, the connected streams, and all context
file operations. The host thread remains the sole owner of `App`, decoder,
compositor, output buffer, and `HostGuard`. Use `std::sync::mpsc::sync_channel`
with fixed capacities; do not add an async runtime or dependency.

The worker receives typed commands and returns typed results. Keep the
protocol/layout/render models already used by the client; do not expose
`PaneConn`, `Net`, streams, context guards, or arbitrary closures across the
thread boundary.

Commands are FIFO and include:

- `Keys { request_id, target, keys: bounded owned batch }`
- `Paste { request_id, target, text: bounded owned payload }`
- `Split { request_id, plan: SplitPlan }`
- `Close { request_id, plan: ClosePlan }`
- `SetHostSize { rows, columns }` (replaceable/coalesced latest value)
- `Poll` (at most one outstanding poll command)
- `Detach` (only after host restoration; worker performs existing bounded
  detach sequence and exits)

The worker returns fixed typed outcomes keyed by `request_id`: input applied,
input not sent/not owned, input unknown, split result/failure, close
result/failure, or a poll update containing the validated layout, screen
checkpoints/revisions, and fixed status labels. The host applies poll results
only on its thread. A split result may request focus only if its `request_id`
is still the active split and its returned target is present in the current
layout; a later user focus choice wins. A close result affects focus/pending
keys only for the captured target. Poll updates older than the latest applied
layout revision are ignored; screen updates must match their target and retain
the existing revision checks.

Do not let a delayed split completion steal focus after the user has navigated
elsewhere. Do not let stale close completion clear keys for a different pane.
The host keeps rendering local help/menu/navigation, handling the detach key,
checking signals and host size, and writing current frames while the worker is
blocked on socket IO or a context durability call.

## Queue and key-batch contract

Use fixed-size channels (32 commands, 32 results) plus one shared 4 MiB byte
budget for queued commands, in-flight owned commands, and reserved results.
This is derived from existing maxima: one paste is at most
`control_protocol::MAX_INPUT_TEXT_BYTES` (32,768 bytes); one key payload is at
most `ui_protocol::MAX_KEY_INPUT_BYTES` (600 bytes); one screen checkpoint is
at most `screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES` (921,696 bytes); one
complete screen reply is at most 921,760 bytes. A poll result holds at most
two screen replies plus one 184-byte layout and fixed labels, so reserve
1,843,832 bytes; a command's
reservation includes its owned payload plus its result maximum. The host
reserves count and the complete byte reservation before enqueueing. The
reservation returns only after the host applies or deliberately discards the
result. Thus a blocked result send retains only already-accounted memory, and
dropping the result receiver unblocks it. Polling starts only when its full
result reservation is available.

The 4 MiB number is the queued/in-flight command plus reserved-result budget,
not a whole-process memory ceiling. One synchronous transport frame is also
bounded by `protocol_wire::MAX_CONTROL_FRAME_BYTES` (65,560 bytes). During
screen validation the worker can temporarily hold a frame, a decoded borrowed
view, and a copied checkpoint, each bounded by the existing screen-reply or
checkpoint maximum; these transient decode/transport buffers are separate
from queued payload reservations and remain bounded to the one current
exchange. `App` also keeps its existing two preallocated screen snapshots on
the host thread.

Keep key/paste/split/close commands ordered in the FIFO; never coalesce them.
Coalesce only latest host-size updates and polling. Permit one outstanding
poll plus at most one replaceable pending poll. Poll updates may be coalesced
by replacing only the pending snapshot payload, never a mutation result.
Mutation results (including input outcomes) retain their own count and byte
reservation until the host consumes them. Do not use an unbounded side queue
or worker-local growing `VecDeque` to evade channel limits. If the result
budget fills, pause polling and command admission while retaining at most the
single currently executing command and its already reserved bounded result.
This can backpressure IO but cannot block the UI. Channel disconnect releases
a blocked result send.

The App retains one pending key batch with its captured target until the host
successfully enqueues it. On full/disconnected command queue, retain the batch
and show a fixed local busy/error status; do not clear it or silently retry
with a new target. Once enqueued, the worker owns it. The result identifies
that exact batch. A failed or uncertain input is terminal for that attempt:
show a fixed message and never automatically replay it. Further typing stays
in bounded host storage and is admitted in original order when capacity
returns, or is rejected visibly when the local limit is reached. Preserve
`MAX_KEYS`, the target pin, and the protocol's maximum key count. Paste uses a
separate bounded owned buffer and reports overload before ownership transfer.

For each keyed operation, only the worker calls `PaneConn::keyed`: settle a
previous uncertain key first, build payload, reserve and flush context,
then send that exact payload once. An input timeout/disconnect returns
`Unknown`; subsequent work on that pane first performs the current status
settlement flow. Never retransmit an input key or paste. Ordered durable
mutations keep their existing key/revision semantics. No key payload is added
to persistent context or diagnostics.

Queue full has an explicit local busy status, not an unbounded wait. A pinned
key batch may remain in App only while not admitted; later paste or mutations
cannot pass it. Once admitted, the worker owns it and an unknown result is
never retried.

## Async UI state transitions

Adapt the state/effects seam so `handle_event` records intent and enqueues
work, while result application is a separate host-thread method. Local actions
(help, navigation, focus, menu, close confirmation, detach) remain
synchronous and available while the worker is busy.

- On a confirmed split, enqueue the captured `SplitPlan`; display a pending
  status. On completion, update status and focus the new pane only if the
  active request and focus generation still match. On failure, clear that
  pending request and leave the layout as received from poll.
- On confirmed close, enqueue the captured `ClosePlan`; keep the close pending
  without blocking the prompt/event loop. Apply a successful stop only to the
  named target. Do not issue another close key or retry a close after timeout;
  the server's existing dedupe/status rules remain the authority.
- On detach/signal, stop accepting new work, restore the host first, then
  request the worker's existing bounded `Net::detach` path. This ordering
  continues to protect terminal restoration from a slow release. Keep at most
  one worker per attach session; never spawn a replacement because the current
  worker is slow or its result queue is full. Reconnect stays inside the same
  Net/worker owner. A new process remains subject to the existing context-file
  lock and cannot concurrently adopt a context still owned by the old process.

## Worker lifecycle and Drop

The worker owns connection cleanup. Explicit detach is the only path that
waits for `Net::detach`, after `HostGuard::restore`; retain its current bounded
budget and stop after the first stalled pane. A defensive `Drop` for the
handle must close the command sender and detach/drop the join handle without
joining or waiting on socket/context work. Dropping a `JoinHandle` detaches the
thread; it must not be confused with canceling an accepted operation. The
worker must observe channel closure between operations and exit after releasing
its owned resources. It must not hold host terminal state. In ordinary
shutdown, explicit post-restore `Detach` tries the existing bounded detach
budget and joins only after completion is reported; cap that join by the same
explicit budget. If that budget expires, drop the handle without a further
wait and allow the process to exit. There is no in-process worker restart or
retry loop that can accumulate detached threads. The context lock remains
owned by the old worker until it exits, preventing another process from
reusing that context while it is held.

If host shutdown races with a command after durable reservation, that command
may have reached the server. The context retains its uncertain key and next
attach settles status before sending a later keyed command; input is never
replayed. Accepted commands not yet reserved/sent may be abandoned only with
an explicit local shutdown outcome where observable. This is a bounded
best-effort process shutdown, not a claim that an uninterruptible filesystem
sync can be canceled. Do not block Drop waiting for such a sync.

## Deterministic and transport-level verification

Add narrow test seams for a fake client backend and an injectable context
reservation/acknowledgement writer. Keep production behavior on the existing
`Net`/`PaneConn` implementation; avoid a general executor, async trait layer,
or new dependency.

Deterministic fake backend cases:

1. Hold a poll, split, close, key reservation, and key acknowledgement
   independently behind test gates. While held, drive host input/render ticks;
   help, navigation, focus movement, detach recognition, resize observation,
   signal checks, and terminal restore remain responsive.
2. Verify a key command cannot reach the backend socket until its durable
   reservation gate releases. Verify a delayed/unknown acknowledgement causes
   no second send and the next keyed operation settles status first.
3. Fill command and result capacities. Verify FIFO key batch order, exact
   target pinning, explicit visible overload, no silent loss, no coalescing of
   keys/pastes/mutations, and bounded owned memory. Verify resize and poll
   coalescing retains the latest value.
4. Delay split/close while moving focus and changing layout. Verify captured
   target/revision is preserved, stale split completion cannot steal focus,
   stale close cannot clear another pane's pending input, and no command
   receives an automatic retry.
5. Close channels and drop the worker handle while a fake operation is
   blocked. Verify Drop returns without waiting; release the fake gate and
   verify the worker then drops owned resources and exits.

Actual socketpair/PTY cases (no pathname bind dependency):

- Use a private Unix socketpair scripted peer that stalls a pane exchange;
  host ticks, local help/navigation/focus, and command admission continue
  during the stall. The single worker serializes pane exchanges, so a slow
  exchange may delay the sibling pane until its transport deadline.
- Exercise partial frames, timeout/poisoned stream, delayed success, and
  disconnect after server admission. Check that input bytes appear exactly
  once at the peer, or remain absent before reservation; uncertain input is
  not resent on reconnect.
- Run the existing private PTY event-loop harness with a delayed fake peer:
  continuously send/render while toggling help/navigation and detach, then
  assert raw mode and alternate screen are restored before detach begins.
- Preserve the existing client/state and transport codec tests. Do not treat
  pathname socket tests blocked by sandbox `EPERM` as client-worker passes.

## Suggested staged ownership and checks

After parent approval, implement only the listed client files and a new
`src/ui_client/worker.rs` (plus its dedicated tests), then write
`docs/progress/W2-CLIENT-RESULT.md`. Keep `keys.rs`, protocol/codec files,
`lib.rs`, terminal/server/runtime code, roadmap, and shared handoff untouched.
If the result ownership shape proves too broad, stop at the first seam that
requires an unassigned module change and report that exact dependency to the
parent before expanding scope.

Suggested focused verification after implementation: `cargo fmt --check`,
`cargo test --locked --offline --lib ui_client`, the new deterministic worker
tests, and the applicable private PTY/socketpair integration target. Then run
the parent-selected integrated checks; report pathname bind restrictions and
ignored host cases separately. The ms-rust skill remains unavailable, so no
guideline-compliance claim is proposed.

## Risks and unresolved design choices for review

- The current `Net::poll` combines several exchanges and mutates `App`; its
  extraction into typed bounded view updates is the largest interface change.
  `App` remains exclusively host-owned; the worker does not keep competing
  focus/help/navigation authority.
- Blocking `SyncSender::send` for a saturated result queue is off the UI thread
  but can delay worker shutdown until the host drains or drops the receiver.
  Each mutation result has a byte/count reservation from command admission, so
  the worker needs no unbounded pending-result list; polling starts only when
  room for its bounded result is reserved.
- Context `F_FULLFSYNC` has no enforceable deadline. Worker isolation protects
  UI responsiveness but does not bound attach startup, explicit detach, or
  worker exit time.
- Initial `Net::connect` still runs before raw mode and can delay startup. This
  proposal scopes responsiveness to the active interactive session; startup
  isolation can be a follow-up after the active-session contract is proven.
- One worker serializes both panes. It keeps network IO off the host thread,
  but does not isolate sibling-pane IO from a slow exchange.
- One IO worker serializes both panes. It removes IO from the UI thread but a
  slow exchange still delays sibling IO until its deadline. Separate pane
  workers or poll budgeting is a later option only if one worker's measured
  bound is insufficient; it introduces additional ownership/order complexity.

