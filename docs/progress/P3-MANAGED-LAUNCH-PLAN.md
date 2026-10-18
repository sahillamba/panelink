# Managed launch and two-session prototype plan — 2026-10-18

Status: implementation proposal for parent review. This file does not authorize
source changes, select a workspace hierarchy, or complete Phase 3.

## Recommended next gate

The integrated live-watch source has parent verification at 302 passed / 13
ignored, with strict Clippy and build passing; expanded pathname CLI acceptance
is still external. The next bounded task should be a source-gate review of
descriptor creation and every process spawn path. Do not enable API launch
until that review closes the macOS CLOEXEC/inheritance race. After the parent
accepts that gate, take one bounded
managed-launch prototype through a two-session proof: two explicitly requested
sessions may coexist in one server generation, each with independent PTY,
screen, input owner, target and watch state. The cap is for this experiment; it
does not choose the product's workspace, tab, pane, right-click or session
lifecycle model. No autonomous scheduling, process replacement, provider
identity or guaranteed restart recovery is included.

Use one generation-wide operation ledger, lease/grant table and operation
sequence authority around a bounded session registry. Each registry entry owns
its session effect state, input owner and event journal. Do not create a
separate ledger or independent lease namespace per session: the same operation
key and exact payload must resolve globally, and reusing a key with different
session or launch fields must conflict before target routing. Event sequence
cursors are a separate namespace; per-session event counters are safe only with
the reviewed session-bound cursor contract below.

Before any API launch is enabled, close the descriptor-inheritance gate below.
The current macOS PTY open path sets CLOEXEC after libc openpty returns; source
does not prove atomic descriptor creation. A per-PTY spawn mutex alone does not
serialize unrelated Command::spawn calls.

## Evidence and safe reuse

The graph index was checked before source reading. It found the indexed
Panelink project ready; graph searches located StateOwner, PtyEffects,
StartedServer, LaunchJournal, WorkerHub and ServerDriver. Source inspection
then confirmed the single-target shape and the startup/storage/process call
sites. The graph connector was available; source and current progress records
remain the evidence for behavior.

- [Single-child startup](../../src/server_startup.rs) builds a volatile launch
  identity, admits an internal launch key, writes Intent, spawns, records
  Spawned, constructs the one StateOwner and records Applied. A result-write
  failure retains the child and reports Unknown. The target is currently fixed
  to workspace 1, pane 1, session 1, occupant revision 1. This is foreground
  bootstrap, not a request handler or multi-session registry.
- [Startup storage](../../src/server_storage.rs) holds the singleton lock for
  the server lifetime. LaunchJournal borrows ServerStorage, is opened for a
  mutable operation, appends fixed 56-byte metadata records, checks a 1 MiB
  file and 4,096 distinct-key caps, fully flushes each append and poisons the
  handle on write failure. Recovered Intent or Spawned is reported as Unknown;
  it never authorizes a spawn. The record contains only operation key, session
  ID and state.
- [Launch resolution](../../src/launch.rs) already constructs an explicit
  environment from account defaults, LaunchContext, override layers and owned
  values; it never reads the process-global environment. It resolves cwd from
  the client's absolute cwd and executable through an explicit effective PATH.
  It canonicalizes the executable before spawn but documents the remaining
  replacement race.
- [PTY creation](../../src/terminal/mod.rs) validates the resolved target,
  preallocates its parser, snapshot and input queue before child creation, then
  holds SPAWN_LOCK across openpty and Command::spawn. The resolved launch uses
  env_clear and explicit values. Current ServerDriver workers only pass
  requests to the single state owner; they must remain process-effect-free.
- [Operation admission](../../src/protocol.rs) keeps exact request bytes in
  volatile memory for byte-for-byte duplicate checks. One generation has a
  65,536-byte payload maximum, 8 MiB aggregate retained payload budget and
  4,096-operation cap. Exact duplicates are observed before mutable target
  checks; a changed payload for the same key conflicts. Receipts and explicit
  lease maintenance reclaim terminal records/payloads. This memory can include
  launch environment values or input text until receipt/expiry; it is not
  persistent storage or zeroization.
- [Target identity](../../src/control_protocol.rs) is the full five-part
  TargetRef: generation, workspace, pane, session and occupant revision.
  [Watch protocol](../../src/watch_protocol.rs) currently uses schema 4 and a
  generation-plus-sequence EventCursor. The [watch owner](../../src/server.rs)
  and [watch delivery path](../../src/server_runtime.rs) are authoritative for
  reconciliation, cursor reads and bounded waits today, but each StateOwner
  owns one target.
- Watch wave evidence: [codec](P3-WATCH-CODEC.md),
  [owner](P3-WATCH-OWNER.md) and [delivery](P3-WATCH-DELIVERY.md) cover the
  current wire codec, owner and finite delivery pieces.
  Parent reports the integrated live-watch suite at 302 passed / 13 ignored
  with strict Clippy and build passing. This does not verify managed launch.

The CLOEXEC source gate is specific. The resolved rustix-openpty 0.2.0 source
at src/lib.rs lines 59–107 says non-Linux openpty uses libc::openpty and then
sets CLOEXEC with fcntl; its own comment calls this non-atomic. The Linux path
uses atomic CLOEXEC, but Linux is outside this prototype. Keep the current
spawn serialization while auditing, but do not treat it as closing the race:
it only serializes Panelink TerminalSession::spawn calls. Before API launch,
review an Apple Silicon macOS-safe creation path or enforce and prove one
process-wide spawn coordinator covering every in-process exec path and every
period where a newly created descriptor is not CLOEXEC. Audit standard-stream
duplication separately from extra PTY descriptors. If the race cannot be
closed, API launch stays disabled.

The source-gate deliverable should enumerate every process `Command::spawn`
callsite and distinguish it from Rust worker-thread `spawn` calls, inspect all
PTY open/clone/duplication and CLOEXEC paths, and identify the interval in which
any descriptor can be inherited. The current application child-spawn path is
`TerminalSession::spawn`; source search also finds test-only self-child helpers,
while runtime/worker `.spawn` calls start threads. Recheck the complete tree at
that gate. The review must choose an atomic macOS descriptor-creation path or a
process-wide coordinator that every child exec path obeys. Its fault test should
hold a newly opened PTY descriptor at the vulnerable boundary while an unrelated
child spawn is attempted, then inspect the child's inherited descriptor set;
also test concurrent managed spawns and standard-stream duplication. Until the
source and race gate pass, keep launch API and worker-time PTY creation disabled.

## Proposed bounded API and operation order

Provisional Rust request shape:

- ManagedLaunchRequest carries an OperationKey, explicit LaunchContext, requested
  executable, requested working directory, argv, explicit environment override
  layers and initial TerminalSize.
- LaunchContext carries the caller's absolute cwd and captured caller
  environment. Resolve a relative requested cwd against that context. Build the
  effective environment from an explicitly selected defaults map, the client
  context, explicit overrides and Panelink-owned values; use no inherited
  daemon environment. For the smallest experiment, use no profile lookup unless
  the parent first chooses an explicit profile source.
- ManagedLaunchResult carries the exact OperationKey, Outcome and an optional
  server-issued full TargetRef. A target is present once a session ID has been
  reserved; Unknown after a successful spawn still returns that identity.
  Applied means the child was registered and the result committed, not that
  its application completed successfully.
- Stop remains blocked until the parent selects explicit stop semantics. If
  approved, its typed request must carry an OperationKey, the exact full
  TargetRef, a bounded graceful deadline and an explicit escalation policy.
  Do not expose a generic stop that silently inherits TerminalSession's current
  HUP-then-KILL cleanup. Stop is separate from client Detach; no replacement
  launch follows.

Treat these as proposal types, not an existing API. The wire schema number,
capability bit and target mapping require parent review before coding. A launch
request needs an explicit session-management capability; current Read, Input
and Events grants do not imply permission to create or stop children. Same-UID
socket authentication does not isolate local agents from each other.

Apply this order for a new launch:

1. Decode and validate the complete encoded request within the outer 65,536-byte
   operation-payload limit. Count schema, key, request metadata, requested
   executable and working-directory bytes, caller cwd/environment, argv,
   environment names and values, override metadata, initial dimensions and all
   count/length prefixes in that budget. This is the original encoded body
   passed to the operation ledger, including the caller's declared absolute cwd
   and requested executable/cwd bytes; it does not include server-resolved
   canonical paths.
   Keep current maxima of 256 argv entries and 4,096 environment entries only
   as subordinate count bounds; the complete encoded body is authoritative.
   Reject arithmetic overflow before building additional copies.
2. Validate the live connection and launch capability, then check the one
   generation-wide ledger for the exact key and original encoded request bytes.
   Exact duplicates return the same reserved session/result without
   canonicalizing or resolving paths again and without spawning again; changed
   payload or target fields conflict. This lookup precedes path re-resolution
   and mutable registry capacity checks so a retry can retrieve its result
   while the registry is full.
3. For a new key, resolve the explicit environment, cwd and executable; validate
   initial size and all caller-controlled fields. Bound each canonical resolved
   executable and working-directory path separately (proposed maximum: 4,096
   raw bytes each), reject overflow/oversize without truncation, and confirm
   that bound against the supported OS path APIs before implementation. Check
   active/record capacity, session ID availability, ledger capacity, journal
   key/byte headroom, event state allocation and registry insertion capacity
   before reserving an operation or touching a child process. For a new launch,
   require one unused journal key and at least 168 bytes for the normal
   Intent/Spawned/Applied record sequence before writing Intent; spawn failure
   instead needs Intent/Failed (112 bytes). A write fault after spawn still
   yields Unknown, never a retry.
4. Reserve the session ID and in-memory registry slot. IDs are nonzero,
   monotonically issued within the generation, never recycled, and returned
   only by the server. Gaps after a failed attempt are acceptable. Use checked
   overflow and fail before Intent or spawn.
5. Admit the exact launch payload in the generation-wide ledger. Append and
   fully flush a metadata-only Intent containing the key and reserved session
   ID. A known intent failure prevents spawn and completes the in-memory
   operation as Failed; the journal handle stays poisoned.
6. Spawn only through the reviewed, serialized process path. Keep the spawned
   TerminalSession owned by the reserved registry slot immediately. Record
   Spawned, commit the exact target/result mapping, then record Applied and
   complete the ledger before replying Applied. Do not send success before all
   those steps finish.
7. If any post-spawn commit fails, keep the child handle and reserved target in
   an Unknown registry state, return Unknown when possible and never retry
   spawn. If the response receiver vanished, finish admission/registration and
   keep the same child. Incomplete journal facts on a later generation remain
   Unknown observations and are never executable recovery instructions.

The operation payload is retained only in volatile ledger memory for exact
duplicate comparison. It may contain secrets. Redact request Debug/errors,
never log the payload, never persist argv/environment/input, and release any
temporary decoded copies as soon as the operation no longer needs them. The
current OsString/allocator path offers no zeroization guarantee; if memory
residue is unacceptable, that is a product/security decision before launch
acceptance, not a claim this prototype already solves.

For identity, one prototype-only mapping to review is a synthetic workspace
token 1, a fresh opaque pane token equal to the server-issued session ID, the
same session ID, and occupant revision 1. It exists only to exercise current
full-target control/watch routing. It does not define user-visible workspaces,
tabs, panes, hierarchy, right-click actions or future reuse rules. The parent
must approve a different mapping before implementation if this test adapter
would constrain the eventual model.

The future registry must centralize admission, grants and operation-sequence
validation once per generation. A per-session `StateOwner` must not clone its
own `Ledger` or lease table. A concrete shape to review is one registry owner
holding `Ledger`, the generation's lease/grant table, a checked session-ID
counter and a map of session ID to `{full TargetRef, PtyEffects, InputOwner,
EventJournal, revision/exit metadata}`. Every request first passes global
connection/grant/operation admission, then exact target validation and routes
to that session's effect/input owner. Reconciliation reads and commits the
screen revision, owner/exit metadata and that session's cursor within one owner
turn. Whether a lease grant can name one session or all sessions is an explicit
authorization decision; do not copy current grants into isolated per-session
owners or assume same-UID peers are mutually trusted.

The current storage lock and journal shape also need an explicit integration
seam. Do not store a borrowed LaunchJournal inside StateOwner or ServerDriver:
it borrows ServerStorage, and StartedServer currently owns driver before storage
so the driver and all children drop before the singleton lock. Keep storage
outside worker threads. A parent-owned StartedServer tick/orchestrator can open
a short mutable journal borrow for one launch while preserving drop order; any
alternative must show that worker shutdown and every child cleanup finish
before storage releases its lock. Extending persistent records needs a schema
review; do not silently add paths, argv, environment, PID or terminal data.

## Two-session bounds and cursor contract

Recommended first gate: at most two session identities per generation, counting
retained terminal records. This permits the two-session isolation test without
inventing eviction, ID reuse, pane replacement or a tombstone cleanup API. Once
two IDs have been spent, new launch returns a bounded capacity error until a
normal server restart advances the existing stored generation counter. Never
fresh-initialize or reset that counter to clear the cap. Restart never replays
a launch. The append-only journal's 1 MiB and 4,096-key caps remain cumulative
across generations, so restart does not reclaim journal space. This is an
experiment limit, not a proposed product cap.

Each live session receives one 60.17 MiB conservative core allocation budget
from [P2-CORE-RESULT.md](P2-CORE-RESULT.md). Two cores therefore budget
126,177,280 bytes (120.34 MiB) before process/runtime overhead. The source
model is toolchain-conditional, not an RSS guarantee. Recheck it on any engine,
toolchain, history or snapshot change. Current additional logical bounds:

| Resource | Proposed two-session bound | Source of bound |
| --- | ---: | --- |
| Terminal core model | 126,177,280 bytes for two sessions | P2-CORE-RESULT.md |
| Event journals | 12,288 retained encoded bytes total, 6,144 per target | event_journal.rs |
| Complete request payloads retained for exact dedupe | 8 MiB across the generation | protocol.rs |
| One launch payload | 65,536 bytes including all typed metadata | protocol.rs and protocol_wire.rs |
| Current connection workers and owner queue | 8 workers and queue capacity 8 | server_io.rs |
| Pending finite watches | 8 server-wide, not per session | server_runtime.rs |
| Event response | 6,200 encoded bytes per complete batch | P3-WATCH-DELIVERY.md |
| Launch journal | 1 MiB and 4,096 distinct operation keys | server_storage.rs |

Treat the 8 MiB ledger allowance as logical retained bytes, not a process RSS
limit. During peak launch handling, request frame, decoded environment, exact
ledger copy, resolved Command environment and spawn buffers may coexist. Eight
simultaneous worker requests can hold up to 512 KiB of request payloads before
the single owner handles them; the owner queue remains bounded and launches
serialize there. Reconciliation can produce a complete 65,536-byte response;
multiple authorized readers may hold independent responses. Keep the existing
eight-worker cap and measure peak RSS plus allocator behavior with two maximum
budget sessions and concurrent bounded reads before proposing a higher count.
Do not multiply 60 MiB by an unreviewed ten-session default.

The current schema-4 EventCursor contains only generation and sequence. Two
independently numbered per-session journals in one generation would accept the
same cursor for different targets. A generation-wide journal filtered by target
would instead create sequence gaps, while current delivery validates consecutive
events and ties an empty timeout cursor to the requested cursor. Do not silently
pick either layout.

Recommended cursor migration for parent review: make the namespace explicit in
a new watch schema 5, with EventCursor { generation, session, sequence }, where
session is the never-reused server-issued session ID. Each per-session journal
starts sequence zero, validates all three cursor fields and the full target,
and rejects a cursor whose session differs from `target.session`. Keep the
schema-3 Event payload at 72 bytes: each Event already carries a full TargetRef
including session plus its event sequence, so clients can bind the sequence to
that session. Schema-5 watch/control cursors themselves carry the explicit
session namespace.

The source-backed schema-5 offset proposal preserves current payload sizes:

| Payload | Proposed session field | Remaining reserved bytes |
| --- | --- | --- |
| 72-byte Watch request | cursor.session at bytes 64..72, consuming the old reserved eight bytes | none in this payload |
| 128-byte Reconciliation prefix | cursor.session at bytes 108..116 | bytes 116..128 stay zero (12 bytes) |
| 32-byte WatchEnd | cursor.session at bytes 24..32, consuming the old reserved eight bytes | none in this payload |

The Watch request must require cursor.session == target.session; a
Reconciliation must require cursor.session == info.target.session; the client
must require WatchEnd.session == the requested target session. Every other
reserved field stays zero, including the Watch request's other fields and the
Reconciliation prefix's bytes 101..104. The existing reconciliation request
has no cursor and remains target-bound. Change the CLI cursor spelling to
`GENERATION:SESSION:SEQUENCE`. Parent must approve exact offsets, schema
negotiation and compatibility before implementation; this is not an adopted
wire contract.

For migration, the safest proposal is: schema 4 stays valid only while the
generation has a single registered session; after a second session ID is spent,
schema-4 reconcile/watch requests fail with a fixed upgrade-required error.
Schema 5 is required for any multi-session observation. The two-session cap
resets only after a normal restart advances the existing stored generation; do
not call fresh initialization or reset the counter to clear the cap. If parent prefers a
generation-global cursor, it must instead specify how target filtering, gaps,
authorization and cursor advancement work, then change worker/client validation
and tests together. Test same-generation sessions A and B both at sequence 1;
A's cursor must be rejected for B, and neither a timeout nor resync may move
the other target's cursor.

Keep the 64-event retention and eight pending-watch cap bounded. Atomic
reconcile still captures target info, screen revision, owner/exit metadata and
the scoped cursor in one owner turn. A stale full TargetRef cannot read a
replacement occupant. Events remain metadata-only. No terminal bytes, input,
clipboard content, environment or provider identity enters a journal.

## Proposed owner sets and dependency order

These are future exclusive assignments after the live-watch owners release
their paths. The parent reviews interfaces between each stage; no worker edits
the roadmap, manifest, lockfile or another owner's files.

| Stage | Exclusive owner set | Depends on |
| --- | --- | --- |
| A. Parent preflight | P3-MANAGED-LAUNCH-PLAN.md; choose capability, ID mapping, cursor schema/migration, abrupt-exit and peak-memory gate | Current watch integration reviewed |
| B. Request codec | New managed_launch.rs and P3-MANAGED-LAUNCH-PROTOCOL.md | Stage A API; pure type/byte-boundary tests only |
| C. Storage contract | server_storage.rs and P3-MANAGED-LAUNCH-STORAGE.md, parent-owned due journal lifetime/schema risk | Stage A record and recovery policy |
| D. PTY spawn gate | terminal/mod.rs, server_pty.rs and P3-MANAGED-LAUNCH-PTY.md | Stage A descriptor audit, launch codec; no worker-time spawn before gate passes |
| E. Registry owner | server.rs, new session_registry.rs and P3-MANAGED-LAUNCH-REGISTRY.md | B–D exact APIs; one global ledger/grant authority |
| F. Runtime and CLI integration | Parent owns server_startup.rs, server_io.rs, server_runtime.rs, transport.rs, p3_cli.rs, lib.rs and integration tests | B–E; parent review of cursor migration and global dedup |

Implement sequentially where one interface is a prerequisite; parallelize only
the disjoint B/D review work after Stage A. Stage C remains parent-owned even if
other code proceeds. Parent owns full-source review, fault-injection seams,
integrated checks, current ROADMAP and evidence integration. No stage by itself
is a phase-completion claim.

## Required acceptance and fault matrix

| Scenario | Level and owner | Required result |
| --- | --- | --- |
| Exact same launch key/payload before and after lost ACK | Unit, then authenticated socketpair; registry/runtime owners | One spawn marker and one PID; retry returns same target/result |
| Same key with changed cwd, argv, environment, size or session-related metadata | Codec/owner unit and socketpair | Global conflict; zero second spawn |
| Invalid lease, wrong generation, missing capability and stale target | Owner unit/socketpair | Fixed denial before spawn; no target disclosure or ID reuse |
| Registry, journal-key, journal-byte, operation-count or payload-byte capacity full | Storage/owner tests with marker child | Capacity fails before intent/spawn; no late append overflow |
| Intent write fails or is partial | Storage injection + startup/owner test | No spawn; poisoned journal; no later launch authorized |
| Spawn fails after durable Intent | Private PTY with invalid executable or injected spawn failure | Failed/Unknown distinction follows observed boundary; no automatic retry |
| Spawn succeeds, Spawned/Applied/result commit fails | Storage injection + real child marker | Keep child owned in Unknown; duplicate never spawns; no success response |
| Crash/reopen at Intent, Spawned and Applied boundaries | Journal unit fixture; isolated subprocess where feasible | Incomplete row reads Unknown; restart has new generation; no command auto-replay |
| Two sessions with simultaneous output and independent input ownership | Real private PTY + authenticated socketpair | Reads, input, owner changes and exits route only by exact full target; stop remains disabled pending policy approval |
| Cursor scope and retention | Codec/owner/socketpair | Same sequence in A/B cannot alias; expired cursor requires resync; wrong generation/session fails |
| Reconciliation races with output/owner/exit and stale occupant | Owner fake sink, then real PTY | Snapshot metadata and scoped cursor are atomic; no replacement session satisfies stale target |
| Fair owner pump under launch/read/watch load | Runtime with bounded fake spawner and slow socketpair reader | Queue and work per tick remain bounded; slow writer never blocks owner; launch result is not lost or replayed |
| Explicit stop while a second session runs | Deferred until stop policy is approved; then private PTY/socketpair | The request names its grace deadline and permitted escalation; only the named child group is affected, sibling remains; repeated same key returns cached stop result |
| Capacity ceiling and memory | macOS measurement run, two maximum-size sessions plus concurrent reads | Peak allocation/RSS and allocator behavior recorded; no third session accepted |

Unit and fake-sink checks can establish validation, state transitions, bounds,
cursor isolation, duplicate handling and injected journal failures. Private
PTY/socketpair tests can establish real child routing, one-time spawn/input,
detach/reconnect, event binding, slow-writer behavior and bounded explicit
cleanup. Pathname socket acceptance, Apple Terminal/zsh/clipboard behavior and
outer-terminal restoration require native operator evidence. Power-loss,
filesystem-cache durability, hostile backup rollback, second-user access and
provider identity require separate evidence; the current journal tests do not
prove them.

## Release blockers and parent decisions

Before any managed-launch implementation starts, review the macOS CLOEXEC
creation race and all process-spawn paths; this source-gate review is the
recommended next bounded task after the live-watch review. Before API launch or
multi-session wire work, approve the schema-5 cursor namespace/offsets and
schema-4 migration rule. Before a
public CLI/API is exposed, approve a launch/stop capability and exact client
environment source. Before the two-session result is called acceptable, record
the memory measurement and define abrupt server-exit cleanup/orphan handling.
The first prototype will not automatically kill or resume a child recovered
only from journal metadata.

The stop endpoint remains disabled until its signal sequence, graceful deadline,
escalation permission and cleanup result are explicit. The current
TerminalSession shutdown performs bounded HUP then KILL; this existing drop
behavior is not automatic authorization for a new managed stop command.

Still open: permanent workspace/tab/pane hierarchy, user-visible labels and
right-click menus; active-session count beyond this test; how exited session
records are forgotten; whether session IDs need durable reservation beyond one
generation; named environment profiles/defaults; executable replacement
between canonicalization and spawn; graceful child stop semantics; handling
children that outlive a crashed server; whether volatile retention of
credential-bearing request bytes for dedup is acceptable; and when provider
identity/recovery is evaluated. These are decisions, not confirmed
requirements. Saved scrollback, screen checkpoints and provider conversation
state remain separate.

## Checks recorded for this planning artifact

Graph checks: after live-watch source integration, a full `index_repository`
refresh in `full` mode completed for the Panelink project. It indexed the
current source tree; graph node/edge counts are not behavior or quality
evidence. Earlier graph searches located the startup, ledger, PTY, storage,
worker, runtime and watch symbols named above. Direct reads of current source
and the latest integration/watch records remain authoritative for the claims in
this plan. The repository's application sources are untracked relative to its
initial documentation commit, so `detect_changes` alone would not establish
graph freshness.

Documentation inputs read for this plan: ROADMAP.md, AGENTS.md, docs/SPEC.md,
docs/PANELINK-DISCOVERY.md, docs/PREBUILD.md, docs/progress/INTEGRATION.md,
P3-MANAGED-LAUNCH-TASK.md, P3-STORAGE.md, P3-LIVE-WATCH-TASKS.md and current
P3-WATCH evidence. Parent's integrated live-watch result is 302 passed / 13
ignored with strict Clippy and build passing; it does not cover managed launch.

Document checks: all 13 local Markdown links resolve; explicit scan of this
untracked plan found no conflict markers or trailing whitespace and confirmed
the final newline. `git diff --check` exited 0 for tracked work; the separate
scan covers this new untracked file.

No application tests or Rust checks were run for this documentation-only task.
Parent's latest live-watch verification is reported above as existing evidence,
not as a managed-launch result.
