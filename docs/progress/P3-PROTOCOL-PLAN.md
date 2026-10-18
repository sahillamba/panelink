# Phase 3 protocol and reconnect plan

Status: design contract dated 2026-10-17, superseded where present-tense implementation status conflicts with the 2026-10-18 checkpoint below. Bounded Phase 3 prototype work is now integrated; this plan does not close any Phase 2 host acceptance gate or settle release policy.

## Purpose and boundaries

This plan defines the first persistent-server boundary: a bounded local protocol, same-user socket authentication, generation and lease issuance, operation admission, reconnect state transfer, and a later PTY-owning server. A separately authorized bounded one-child PTY prototype now exists; it does not change the one-shot probe or settle the product lifecycle contract.

The contract builds on the existing types in [src/protocol.rs](../../src/protocol.rs): OperationKey contains generation, lease, and monotonically increasing sequence; OperationClass separates queries, durable mutations, terminal input, signals, clipboard operations, and absolute resize; Ledger checks payload identity, per-lease operation count, TTL, receipts, and generation. The bounded prototype adds server-wide limits, server-issued leases/generations, and durable lifecycle metadata; the operation ledger itself remains volatile. These prototype limits are not final product budgets.

The documented prototype assumptions are local execution, a persistent server, explicit stop, and launch/monitor/resume without autonomous scheduling. They are not confirmed product choices. Workspace hierarchy, socket discovery and path, auto-start, multi-window and multi-controller policy, persistent storage location and retention, and server-stop effects on children remain unresolved. For an initial socket proof, a second concurrent controller may be rejected as a test-scope limit; that must not be recorded as the eventual multiple-client policy.

## Current evidence and pending gates

Current implementation and verification evidence is maintained in [INTEGRATION.md](INTEGRATION.md); use that record for actual commands and results instead of copying test counts into this design plan. The private one-child PTY proof is separately authorized while Phase 2 manual host gates remain pending. Its socketpair reconnect/lost-ack results do not establish pathname CLI acceptance: the pathname fixture returned EPERM. They also do not establish Apple Terminal compatibility, provider recovery, or full agent-control behavior.

The plan-document checks recorded below are historical checks from 2026-10-17. The rest of this document preserves proposed product and future-runtime contracts unless a current checkpoint below explicitly records bounded implementation evidence.

## Component ownership and request path

Use a single server state owner to serialize lease state, operation admission, status, and bounded effect reservations. Socket readers decode frames into a bounded request queue. They never call child-process or PTY methods. Each connection has exactly one writer that serializes its outbound frames. A later PTY adapter receives only operations admitted by the state owner.

~~~text
terminal client
    │ versioned local socket frames
bounded socket reader ── peer UID check
    │ bounded request queue
single server state owner
    ├── generation / lease / global operation ledger
    ├── launch journal and session status
    ├── bounded session command queues (later PTY integration)
    └── per-connection bounded outbound queues
             │
        one writer per connection
~~~

Use standard-library threads and bounded channels for the first prototype. Do not use an unbounded channel for requests, PTY input, replies, event delivery, or frames waiting on a slow client. If an implementation uses a different ownership arrangement, it must retain the same admission ordering, exclusive writer rule, and measurable queue bounds.

## Wire framing and versioning

The initial transport is a length-prefixed binary protocol on a local Unix-domain stream socket. The codec is hand-written Panelink code over fixed-width integers and bounded byte strings; no serialization dependency is selected. Integers have one specified byte order. A frame has a fixed header containing a Panelink magic value, major/minor protocol version, message kind, zero-reserved flags, request identifier, and the payload whose size is declared by the length prefix. Requests with malformed lengths, unsupported major versions, unknown required message kinds, or nonzero reserved flags are rejected before dispatch.

The first exchange is Hello/Welcome version negotiation. Hello carries a bounded supported-version range and a client nonce for matching the reply; the nonce is not authority. Welcome returns the selected version, server generation, issued lease, and negotiated frame and capability limits. Every later request carries the generation, lease, and sequence fields where the operation contract requires them. The kernel-authenticated peer UID, not a UID in the message, is the local identity.

Wire limits for the first implementation:

- Handshake payload: at most 4 KiB.
- Operation request payload: at most the existing 65,536-byte limit; the frame decoder also checks framing overhead before allocation.
- Ordinary control, event, and history-page payload: at most 65,536 bytes.
- One encoded screen snapshot: at most 2 MiB, matching the current 80-by-240 maximum viewport and the existing fixed-cell snapshot model with room for envelope metadata.
- No compression in the first protocol version.
- Counted arrays, strings, launch arguments, environment entries, and history pages have explicit count and byte limits before allocation. Display text is UTF-8; Unix paths, arguments, and environment bytes use a documented byte-string representation. Credentials and environment values are never formatted for diagnostics.

The decoder first reads only the fixed header, validates the advertised length against the message-specific maximum, then allocates at most that amount. Inbound operation requests remain capped at the existing operation payload limit even though an outbound snapshot is larger. Incompatible major versions fail closed with a small bounded error. Minor-version additions require explicit unknown-field and unknown-message behavior; silently ignoring a side-effecting request is forbidden.

Exact header layout, operation schemas, enum numeric values, and byte-string encoding need parent approval before a client implementation relies on them. The values above are proposed wire limits, not user-facing compatibility or performance promises.

## Local socket and peer identity

Place the socket under a per-user directory inaccessible to other users, with directory permissions equivalent to 0700 and socket permissions equivalent to 0600. The exact runtime directory and CLI discovery method remain unresolved. The listener accepts only peers whose kernel-reported effective user identity equals the server's. Same-user access is the current trust boundary; it does not distinguish applications belonging to that user.

The bounded prototype implements a protected private-directory pathname listener, existing-endpoint refusal, and kernel same-UID authentication before handshake processing. See [P3-SOCKET-GATE.md](P3-SOCKET-GATE.md) and [P3-PEER-REVIEW.md](P3-PEER-REVIEW.md) for the selected path and source review. The current environment denied the pathname acceptance fixture with EPERM, so socketpair test evidence does not establish pathname CLI acceptance. Same-UID authentication does not isolate processes or agents belonging to that user.

Keep server startup single-instance. The prototype refuses an existing endpoint instead of removing it automatically. Symlinks and unrelated files must never be followed or removed. Broader stale-endpoint recovery remains unresolved.

## Generation and lease issuance

Preserve OperationKey's existing u64 generation, lease, and sequence fields.

- Issue a new nonzero generation before accepting a connection. The prototype supports explicit `--initialize` only for an empty private directory, writes a random nonzero generation seed, then advances a persisted monotonic counter before normal use. Ordinary open requires existing lock/counter/journal metadata and fails closed when it is missing or corrupt. This does not claim rollback resistance across restored backups or collision-free identity across independent storage domains. See [P3-STORAGE.md](P3-STORAGE.md).
- The server, not the client, issues lease IDs. Within one generation, issue increasing nonzero u64 IDs and never reopen or reuse an ID. A single issued-through watermark is the tombstone for all lower IDs, allowing expired lease details and payloads to be reclaimed without making an ID reusable.
- A client may resume its still-valid lease after reconnecting to the same server generation. The server authenticates the peer again, checks the lease and TTL, then renews it. A new generation invalidates every old key. The initial TTL remains the existing 1,800 seconds unless a measured prototype result justifies a parent-reviewed change.
- The client chooses strictly increasing per-lease operation sequence values as in the existing ledger. Receipt advancement retains retired_through/highest_sequence watermarks while the lease remains live; retired keys never become executable again.

The current issuance behavior is bounded prototype evidence, not a production identity guarantee. Cross-domain collision and restored-backup rollback remain outside the guarantee.

## Bounded global ledger and expiry

Keep the existing per-operation payload cap, per-lease unreceipted-operation cap, TTL, exact-payload duplicate check, and retry classification. Add provisional server-wide prototype limits before any multi-session process effects:

| Resource | Proposed initial cap | Rejection behavior |
| --- | ---: | --- |
| Concurrent local connections | 8 | Refuse the connection before allocating a worker or queue. |
| Live or resumable leases | 64 | Refuse a new lease; do not evict an unexpired lease. |
| Ever-opened lease records per generation | 4,096 | Refuse new IDs at exhaustion; retain expired tombstones and do not automatically roll generations. |
| Unreceipted operations across the server | 4,096 | Reject admission without changing sequence, ledger, or effect state. |
| Retained exact operation payload bytes | 8 MiB total | Reject admission before effect; do not evict another operation's deduplication bytes. |
| In-flight requests per connection | 64 | Apply read backpressure or reject before admission. |

These are bounded implementation starting points for the 5–10-agent workload, not settled product budgets. Parent approved only the ledger-specific subset in [P3-LEDGER-TASK.md](P3-LEDGER-TASK.md); connection/queue/effect-copy limits remain proposed. The task also fixes exact counter/expiry/issuer behavior, which takes precedence over this earlier broad plan. Capacity accounting must include the bytes actually retained by the ledger and pending effect work, including any copies, rather than counting only payload length once on paper. The current P2 terminal-core estimate is about 60.17 MiB per PTY under its inspected Rust 1.99.1 allocation model; it is not an RSS guarantee. Multi-PTY memory/history limits still need a separate budget before enabling the 10-session target.

On receipt, remove completed operation records through the receipt and advance the sequence tombstone. Expiry reclamation is explicit `expire_leases(now)`, not a side effect of read-only status; the current runtime's bounded tick schedules maintenance. It marks due leases irreversibly expired, releases their payload allocations and retires completed metadata; pending identity/state remains under the global operation cap and can complete. A later maintenance call retires newly completed expired records. Public status for expired leases remains `LeaseExpired`; it never becomes `NotAdmitted` or dispatch authorization. Completion stays idempotent while fixed metadata remains, then returns `ResultExpired` once maintenance retires it. The permanent, bounded lease record and high-water marks prevent reuse; exhausting all 4,096 ever-opened records fails closed. This changes no server lifecycle policy and does not implement generation rollover. Logical byte reclamation does not promise allocator-memory zeroization. Timestamps are caller-supplied monotonic logical time.

All capacity checks happen before mutating the ledger or scheduling an effect. A failed capacity check leaves counters, sequence watermarks, queue contents, and process state unchanged. Expiry maintenance itself is bounded and must not scan or copy an unbounded history.

## Admission, launch journal, and status

For an admitted request, the state owner must reserve ledger bytes, a bounded effect slot, and any required journal capacity before it returns dispatch authorization. A process effect can happen only after:

1. The frame, version, peer, generation, lease, sequence, operation class, payload, paths, environment map, and launch policy validate.
2. Global and per-lease capacity is reserved.
3. The existing Ledger admits the exact key and payload. Only Admission::Execute can create the first effect request. InProgress/Completed are replies to duplicates and do not dispatch.
4. For launch, a versioned, bounded, secret-free LaunchIntent has been durably recorded before PTY/process spawn. If journal write/sync fails, complete the admitted operation as Failed and do not spawn; a retry needs a new explicit operation after the storage problem is resolved.
5. The single bounded effect queue accepts the command. Queue capacity is reserved before admission so an accepted record cannot be stranded solely because the queue was already full. If a supposedly reserved enqueue fails, mark the operation Failed and do not dispatch.

Same-key, same-payload retries on a still-live lease return the current status or continue the original admitted work. A changed payload conflicts. A status query is observational and never authorizes dispatch. Durable mutations can retransmit only the identical key and payload to the same live lease. Queries can be reissued as new observations. Keystrokes, paste, prompts, signals, and clipboard operations are never automatically retransmitted. Absolute resize is reconciled against the server's accepted current size.

The launch journal is lifecycle metadata, not an operation-payload store. It records a launch/session ID, generation and operation key, safe launch-spec reference, intent/spawn/result state, bounded timestamps, and sanitized outcome. It omits inherited environment values, credentials, auth tokens, SSH_AUTH_SOCK, sensitive override values or arguments, terminal input/paste, PTY output, and provider conversation contents. Sensitive values remain transient in memory for the explicit launch and must be cleared when no longer needed. Existing launch validation and Debug redaction remain in force.

The prototype startup journal is durable metadata written before child spawn and records only the operation key, managed session ID, and lifecycle state; it does not persist launch arguments, environment values, or process identity. Its narrow format and recovery behavior are documented in [P3-STORAGE.md](P3-STORAGE.md). The broader append-only launch journal described in this plan remains a proposal. Across process crashes, uncertain launch outcomes are not auto-replayed; no exactly-once execution guarantee is made.

Within a live generation, use existing OperationStatus::{NotAdmitted, InProgress, Completed(outcome)}. NotAdmitted is only an observation. In the prototype, `Applied` for input means bytes were accepted by the PTY write interface; for interrupt it means the signal call succeeded. Neither means the child application interpreted input or completed a task. Partial or uncertain writes report Partial/Unknown without exposing content and without automatic replay. The nominal input drain window is 100 ms, but core hard-error cleanup may add up to two seconds; it is not a hard 100 ms call deadline.

## Reconnect, resynchronization, and retained data

After reconnect, negotiate the version, authenticate the same user, present the prior generation and lease if available, and obtain a new server inventory. A connection to a different generation cannot reuse old operation keys; it first reconciles sessions and launch-journal statuses, then issues new requests only after an explicit caller decision where the prior effect is uncertain.

Keep these three data products separate:

- A live screen snapshot is a bounded view of current terminal-engine state, returned on attach or resync. It is not scrollback and does not replay terminal bytes or side effects.
- A screen checkpoint is a separately versioned persisted read-only visible-screen record under the canonical spec, not a full emulator/process checkpoint. It needs validation, size limits, representation-version compatibility, and a recovery-view policy. The handshake must not call a snapshot a checkpoint.
- Retained history is a separately bounded output/history store with its own retention and deletion policy. History text is never parsed or replayed as terminal input, launch arguments, commands, or OpenCode identity.

The server may expose a monotonic event cursor for session and layout status changes. If the requested cursor is older than retained events or belongs to another generation, return ResyncRequired and a complete current inventory, not a partial event stream labeled current. Cursor rules for durable layout events versus transient status events require a separate storage decision. Terminal screen state is authoritative at a snapshot revision; a history cursor does not reconstruct it.

## Single-writer, backpressure, and detach behavior

Each connection has one reader and one writer. Only the writer emits bytes to that stream, preserving frame boundaries and response ordering. The state owner sends complete bounded frames through a bounded per-connection queue; it never blocks while writing to a slow client.

The selected future snapshot/event output bound is 2,162,736 encoded bytes per writer and 17,301,888 bytes across eight writers: one maximum 2,097,176-byte snapshot frame plus one maximum 65,560-byte control reply per writer. The bounded prototype has request/effect/connection limits, but no snapshot/event output queue, event subscription, or resync delivery. Coalescing snapshots and slow-subscriber behavior remain future contracts. Never drop an input acknowledgment silently. PTY input retains its existing fixed bound and atomic paste admission; it is separate from outbound backpressure.

Under the documented persistent-server prototype assumption, client socket loss closes that client connection only. It does not signal, resize, restart, or close server-owned PTYs, and it does not discard server terminal state. Drop input bytes that the server has not admitted; let already admitted input finish or report Partial/Unknown, and never replay it after reconnect. The server retains the lease until expiry to permit same-generation reconnection. An explicit detach relinquishes the client attachment and returns after the detach acknowledgment; it is not an implicit child stop. Server stop and crash behavior remain separate and unresolved product/lifecycle work. The first proof may exercise one controlling client; it must not settle simultaneous viewer or writer policy for release.

## Staged work, ownership, and acceptance evidence

Each task below is bounded to its listed files; the parent assigns owners and reviews integration. Do not combine ownership of protocol, socket, server, and PTY files in one implementation task. No manifest or lockfile edits are authorized by this plan.

| Order | Task and exclusive file ownership | Proposed observable acceptance |
| --- | --- | --- |
| P3-A | Pure bounded wire codec. Own src/protocol_wire.rs, tests within that file, and docs/progress/P3-WIRE.md. | Implemented; see [P3-WIRE.md](P3-WIRE.md). |
| P3-B | Global ledger bounds, expiry reclamation, and server-issued lease semantics. Own src/protocol.rs and docs/progress/P3-LEDGER.md. | Implemented in the bounded runtime; see [P3-LEDGER.md](P3-LEDGER.md). |
| P3-C | Same-user local socket handshake. Own src/transport.rs, socket tests, and docs/progress/P3-SOCKET.md. | Implemented for the private prototype. Pathname CLI acceptance remains pending because the fixture returned EPERM; see [P3-SOCKET-GATE.md](P3-SOCKET-GATE.md). |
| P3-D | Bounded state owner and durable lifecycle metadata. Own src/server.rs and docs/progress/P3-SERVER.md. | Implemented for the bounded prototype; operation ledger is volatile, metadata Intent is durable before spawn. See [P3-SERVER.md](P3-SERVER.md) and [P3-STORAGE.md](P3-STORAGE.md). |
| Private PTY proof | Separately authorized one-child detach/reconnect experiment; Phase 2 manual gates remain pending. | Implemented and exercised over socketpairs for logical detach/reconnect and lost-ack no-replay. Does not establish pathname CLI or Apple Terminal acceptance. |

The original P3-A through P3-D acceptance scenarios above are historical proposal text and have since been exercised in bounded implementation slices. The current evidence is linked from the table and [INTEGRATION.md](INTEGRATION.md). These checks do not prove exactly-once process execution or close the Phase 2 host gates.

## Current implementation checkpoint — 2026-10-18

The integrated prototype includes same-UID authentication before handshake, a protected private-directory socket listener with existing-endpoint refusal, bounded workers and `StateOwner`, typed response/session codecs, durable caller contexts and input ownership, startup ledger admission, and durable lifecycle-metadata Intent before PTY spawn. The operation ledger itself remains volatile. The separately authorized one-child PTY proof covers logical detach, same-generation reconnect and lost-ack no-replay over socketpairs. It is not pathname CLI acceptance: that fixture returned EPERM. Explicit `--initialize` is restricted to an empty private directory and writes a random nonzero generation seed; ordinary opens require existing lock/counter/journal, fail closed on missing/corrupt state, and advance the counter. This does not guarantee rollback resistance across restored backups or collision-free identity across independent domains. The runtime now negotiates metadata Events for bounded watches and provides atomic reconciliation; layout, multiple sessions, persisted scrollback and provider identity/recovery remain unimplemented. See [INTEGRATION.md](INTEGRATION.md) for current verification and the roadmap for pending Phase 2 manual gates.

## References

- [Specification: protocol, retry, and environment contracts](../SPEC.md)
- [Specification: storage/history separation](../SPEC.md)
- [Roadmap: phase order and prototype assumptions](../../ROADMAP.md)
- [Integration evidence and current Phase 2 status](INTEGRATION.md)
- [Existing admission ledger evidence](P1B.md)
- [Phase 2 operator/manual evidence](P2-OPERATOR-RUN.md)
- [Phase 2 restore-order evidence](P2-RESTORE-ORDER.md)

## Parent-approved first slice

[P3-WIRE-TASK.md](P3-WIRE-TASK.md) fixes the v1.0 envelope layout and per-kind limits; its implementation is reviewed in [P3-WIRE.md](P3-WIRE.md). Typed codec subsets, socket/auth, ledger, state owner and startup journal have since been implemented as bounded prototype slices; see the current checkpoint above and linked progress records. Snapshot/event delivery, full agent-control routes, and pathname CLI acceptance remain open. The short host retest is operator-confirmed except quit; Ctrl-X replaces the conflicting documented shortcut, with live exit/restoration pending. This does not claim all original full-checklist cases ran.

## P3-A2 independent implementation and boundary choices

Pure typed request payloads cover the implemented control subset; response and session payload codecs are also implemented. Event kind7/requestID0 now carries exact72byte metadata events during negotiated finite watches. The bounded server launch request uses the existing 65,536-byte complete encoded request budget and rejects larger input before effects; the one-shot probe's 1 MiB environment capture cap remains separate. No launch staging feature is adopted. Finite event batches now use one250ms write budget; large snapshot queues and indefinite subscriptions remain unimplemented. The reviewed macOS peer-identity wrapper is called before protocol processing; the protected pathname listener and one-child runtime exist; the earlier pathname fixture is operator-confirmed passing, with expanded watch CLI pathname coverage pending. Storage initialization is explicit and ordinary open fails closed on missing/corrupt metadata; it does not claim rollback protection across backups.

## Metadata notification foundation — 2026-10-18

The parallel N1/N2 slice adds pure schema3 metadata events (72byte payloads) and
a64event,6144encoded-byte generation-scoped journal with explicit expired/future
cursor rejection. No terminal output or provider status is encoded. Parent
review and wire/journal integration are recorded in INTEGRATION.md. This does
not enable runtime Events by themselves. The later schema4 owner/delivery
integration now supports bounded watches and atomic reconciliation; indefinite
subscriptions remain unimplemented. The small
journal is distinct from the earlier future large snapshot writer budget.
See [task contracts](P3-NOTIFICATIONS-TASKS.md) and
[runtime boundary](P3-NOTIFICATION-RUNTIME.md) before implementing delivery.
