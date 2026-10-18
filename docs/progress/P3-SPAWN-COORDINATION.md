# P3 spawn coordination contract — 2026-10-18

**Recommendation: adopt a small, cooperative in-process spawn gate for
Panelink-owned descriptor creation and child launch. Keep API launch disabled
until the gate is shared by the PTY spawn path and transport creation paths,
then pass the focused race tests below.** This is a bounded binary-level
contract, not a process-wide fence against code that does not use the gate.

The parent must adopt the following before implementation: one crate-private
non-reentrant gate; the acquisition windows and error rules below; and support
limited to descriptor creators and child-spawn paths owned by this Panelink
binary. No actual concurrent native `fork`/`spawn` caller was found in the
current application sources. An uncooperative library or injected code can
still create descriptors or spawn outside the gate, so Panelink makes no
global inheritance guarantee for that case.

## Source-established runtime

The refreshed `Users-sahillamba-panelink` graph was ready for the current
checkout (3,020 nodes / 12,160 edges). `detect_changes` reported only the three
tracked planning files; it cannot prove source freshness on its own. Graph
search identified `run_server_probe`, `TerminalSession::spawn`, transport
binding, and worker entry points. Direct source reads and the
[descriptor gate audit](P3-SPAWN-DESCRIPTOR-GATE.md) establish behavior.

`run_server_probe` is the live server loop and owner thread. It binds the
listener, registers stop flags, starts the singleton child, then repeatedly
ticks the driver and performs one bounded listener accept. The accept slice is
1 ms. A future launch routed through `ServerDriver::tick` therefore shares
that thread with accept; it must not be dispatched by a connection worker.
`ServerDriver` owns the `StateOwner` and `WorkerHub`, but it does not start a
separate owner thread.

| Live path | Current behavior and relevance |
| --- | --- |
| Server state owner | `ServerDriver::tick` drains at most eight queued envelopes and applies state effects synchronously on the `run_server_probe` thread. Future launch dispatch belongs here. |
| Connection workers | Zero at startup; then at most eight `thread::Builder::spawn` threads. They read/write already-authenticated streams and send bounded owner requests. They do not create child processes or descriptors in their request/error paths. |
| Worker errors and stop | Early returns drop the owned stream/frame; `ActiveGuard` releases the count. Stop uses `AtomicBool`; workers are joined before the process-owning state drops. |
| Stop signals | `signal_hook::flag::register` sets atomic flags for INT/TERM/HUP. This selected API does not create the signal-hook self-pipe. |
| Queue and synchronization | `mpsc::sync_channel` and atomics are in-memory; they are not descriptor creators. |

The current one-child bootstrap is a different path from future API launch:
`StartedServer::start` journals intent and invokes `TerminalSession::spawn`
before the driver accepts connections or starts workers. `StartedServer`
stores the driver before storage, so driver workers and child cleanup finish
before the singleton storage lock is released. Keep that ordering. Do not
describe startup as worker-time or concurrent API launch.

## Descriptor and process inventory

| Creator or operation | Current evidence | Required coordination |
| --- | --- | --- |
| Production child process | One `Command::spawn` call site, in `TerminalSession::spawn`; used by singleton startup and the one-shot PTY probe. Its existing `SPAWN_LOCK` currently covers PTY setup through `Command::spawn`. | Replace that private mutex with the shared gate after the atomic PTY helper lands. Hold through PTY creation/cloning and the return from `Command::spawn`; the Rust standard-library macOS status pipe is created inside that call. |
| macOS PTY master/slave | The old `rustix-openpty` path used `openpty` then `fcntl(FD_CLOEXEC)`, leaving an inheritance interval. A production atomic opener is being integrated separately; its own review and minimum-OS limits remain authoritative. | Keep the shared gate from before PTY creation through spawn. Require both newly opened PTY descriptors to be CLOEXEC at creation in the selected helper; use `F_DUPFD_CLOEXEC` for clones and map only intended slave clones to 0/1/2. |
| Listener bind | `UnixListener::bind` is called once before startup spawn. The installed macOS standard-library source uses `socket()` then sets CLOEXEC. Bind failure returns before the child starts. | Gate the bind call only, through its CLOEXEC setup. Directory/path validation and later chmod/identity work stay outside. |
| Accepted connection | `PrivateListener::accept` uses the standard-library post-hoc CLOEXEC path, then authenticates the peer. It runs on the server owner loop after each tick. | Gate one nonblocking `accept()` syscall and its return/error path. Drop the gate before peer authentication, readiness waiting, frame I/O, or worker start. |
| Client socket | `AuthenticatedStream::connect` uses rustix `socket()`, then sets `NONBLOCK` and `CLOEXEC`; current callers are the separate control CLI process. | Gate socket creation through successful CLOEXEC setup. Set CLOEXEC first; release before connect, readiness waits, authentication, and protocol I/O. Honor the connect call's original deadline while acquiring the gate. |
| Storage, context, terminal files | Rustix `open`/`openat` use `OFlags::CLOEXEC`; standard `File::open` is creation-time CLOEXEC in the reviewed toolchain. Directory enumeration during storage initialization closes before listener bind and child startup. | No gate needed for these audited opens. Preserve creation-time flags and keep any transient initialization descriptors closed before child startup. |
| Probe terminal descriptors and PTY clones | `/dev/tty` opens use `O_CLOEXEC`; owned-fd clones use `F_DUPFD_CLOEXEC`. | No separate gate window; retain those flags. |
| Socket pairs and test subprocesses | `UnixStream::pair`, `Command::output`, and test fixture `Command::spawn` calls are test-only. No production socketpair path or additional application process-spawn call was found. | Keep test coverage separate from production claims. A socketpair test does not exercise `accept()`'s macOS CLOEXEC window. |

`read_dir` in storage initialization is a transient standard-library directory
descriptor whose internal flags were not established by this source review. Its
iterator is dropped before listener bind and startup spawn, so it does not
overlap the managed child in the current ordering. Recheck if initialization
moves to a concurrent runtime path. Path canonicalization and metadata checks
run synchronously before spawn. The test-only `Command::output` helper and
test fixture process launches are outside the supported server path.

## Smallest gate API and critical sections

Add a private `src/spawn_gate.rs` with one static mutex and these APIs:

```rust
pub(crate) fn acquire() -> io::Result<MutexGuard<'static, ()>>;
pub(crate) fn acquire_until(deadline: Instant) -> io::Result<MutexGuard<'static, ()>>;
```

`acquire` is for the synchronous child-spawn and startup bind paths. It blocks
only while another short descriptor-creation section or `Command::spawn` is in
progress. `acquire_until` is for accept/connect paths that already have a
deadline: use `try_lock`, check that same absolute deadline before each retry
and after acquisition, and sleep for at most the existing 1 ms poll interval
or remaining time. Expiry returns `TimedOut`; it never resets or extends the
caller's deadline. A poisoned mutex returns a fixed error and fails closed;
do not recover its inner guard.

Use lexical RAII scopes so every success and error drops the guard. Keep the
gate non-reentrant: a path holding it must not call another gate-acquiring
helper. The PTY helper itself does not acquire the gate; its caller owns the
scope. Never hold the gate across readiness sleeps, authentication, socket
frame I/O, worker joins, journal flushes, owner replies, PTY reads/writes, or
child cleanup.

| Path | Gate scope |
| --- | --- |
| `TerminalSession::spawn` | After validation and retained-state allocation; before opening either PTY endpoint; through descriptor setup/cloning, `Command` setup, and `command.spawn()` return. Drop before wrapping/servicing the child. This protects the standard-library status-pipe window and serializes PTY launch. |
| `PrivateListener::bind` | Around `UnixListener::bind` alone, including the standard-library CLOEXEC work. Drop before pathname identity checks, chmod, or other filesystem work. |
| `PrivateListener::accept` | Around one `listener.accept()` call only. On `WouldBlock`, drop before `wait_for_readiness`; on `Interrupted`, retry after dropping; authenticate the returned stream after dropping. Reuse the original absolute deadline. |
| `AuthenticatedStream::connect` | Around `socket()` and the first CLOEXEC `fcntl`. If flag setup fails, return while the owned descriptor drops under RAII. Drop before nonblocking setup, `connect`, readiness polling, or peer checks. |

The server loop already serializes bind/startup and accept/owner ticks. Keep
future launch requests on that owner path; do not let workers call the PTY
spawner. The gate is the smallest additional protection for the actual
post-hoc descriptor creators if those paths ever run from separate Panelink
threads. Existing `SPAWN_LOCK` is private to `terminal/mod.rs`, so it does not
protect transport creation. Until `TerminalSession::spawn` uses this same gate,
adding the transport guard alone does not close the launch gate.

### Lock order and ownership

The gate is independent of the lifetime `server.lock` file lock; it must not
acquire storage, state-owner, worker-list, or response locks. Flush a launch
Intent before entering the PTY spawn scope; let `Command::spawn()` return and
drop the gate before recording Spawned/Applied or replying. The current
`LaunchJournal` mutable borrow may remain in the single owner thread across
spawn, but no journal I/O should occur while the gate is held. No caller may
hold the gate while waiting for a worker or a future owner reply.

Parent owns the `lib.rs` module declaration. The smallest follow-up ownership
split is:

| Step | Exclusive implementation files | Dependency |
| --- | --- | --- |
| A. Transport gate | New `src/spawn_gate.rs`, `src/transport.rs`, focused tests there, and a separate result note such as `P3-SPAWN-GATE-RESULT.md`. Parent adds the crate-private module declaration in `src/lib.rs`. | This contract; no overlap with the active terminal helper owner. |
| B. PTY gate adoption | `src/terminal/mod.rs` and its focused PTY tests, owned by the parent after the current terminal helper owner releases those files. Replace `SPAWN_LOCK` with `spawn_gate::acquire`; do not add a nested gate in the helper. | Step A reviewed; atomic PTY helper integrated and source-reviewed. |
| C. API launch reconsideration | Parent-owned server/registry/runtime files only after A/B and race tests pass. | Explicit parent review of managed-launch API, identity, capability, storage, and cursor decisions. |

Do not change `lib.rs`, the roadmap, wire schemas, API capabilities, storage,
or managed-launch behavior as part of Steps A/B unless the parent explicitly
assigns them.

## Required verification before the launch gate can reopen

- **Gate unit tests:** use barriers, not timing guesses, to hold the gate in a
  fake descriptor-creation closure while a second caller attempts acquisition.
  Prove mutual exclusion and that the second caller enters only after release.
  Hold a test gate beyond an `acquire_until` deadline and require `TimedOut`
  even if the holder releases afterward; verify no descriptor or child effect
  occurred. Poison an isolated gate instance and require every later acquire
  to fail closed. Keep the test-only seam scoped; do not add production hooks.
- **Error and wait paths:** inject bind, socket, accept, CLOEXEC, and PTY-open
  failures. Verify owned descriptors close and guards release. Force
  `WouldBlock` and prove the readiness wait runs with the gate available; an
  accept deadline must bound both gate acquisition and socket waiting.
- **Private PTY exec test:** after the atomic opener is adopted, run a managed
  child that scans its full descriptor table by identity. It must see the
  expected slave only as stdin/stdout/stderr, and no PTY master or unexpected
  slave descriptor. Include a deliberately inheritable duplicate as a
  positive control so the scanner proves it can detect a leak. Run two
  concurrent managed PTY launches and require independent children, one
  marker each, and no sibling PTY descriptor crossing.
- **Accepted-socket race test:** use a private pathname listener and client.
  Add a test-only barrier after acquiring the gate and immediately before one
  accept syscall. Start a managed launch contender and prove it cannot enter
  until the accept syscall returns with CLOEXEC established. Keep the accepted
  stream open; have the exec child scan the complete descriptor table and
  assert the accepted socket identity is absent. Also prove `WouldBlock`
  releases the gate before readiness waiting. A `UnixStream::pair` test alone
  does not cover this accept path.
- **Socketpair/worker test:** exercise an authenticated socketpair request while
  two clients submit or abandon bounded requests. Confirm workers only relay
  envelopes, the owner thread performs the process effect, a lost reply does
  not repeat a launch, and neither worker stream identity appears in the
  managed child's descriptor table. This is future API integration evidence,
  not a test of pathname accept or an adopted API schema.
- **Source/build review:** enumerate process-spawn and descriptor-creation
  callsites again after integration; check every production callsite uses the
  shared gate or an already atomic CLOEXEC creator; run focused tests, standard
  `cargo fmt --check`, strict Clippy, and the integrated suite. No test result
  can cover a native caller outside the audited Panelink sources.

The current restricted environment has denied pathname socket bind with
`EPERM`. A permission-denied fixture is not a pass: the accepted-socket race
and pathname CLI cases require a host run where private bind/accept succeeds.
An ignored result or early `listener_unavailable` exit leaves that evidence
pending.

## Decisions still open

This note adopts no managed-launch implementation, request schema, capability
bit, target/session mapping, session-bound cursor schema, migration rule,
minimum macOS version, or product-level native embedding guarantee. The
atomic PTY spike is not production integration evidence; consult
[P3-ATOMIC-PTY-SPIKE.md](P3-ATOMIC-PTY-SPIKE.md) and its eventual production
integration review. The broader proposed API and storage sequence remain in
[P3-MANAGED-LAUNCH-PLAN.md](P3-MANAGED-LAUNCH-PLAN.md). Launch remains disabled
until the parent reviews the exact gate implementation and successful race
evidence.

## Handoff and checks

This task changes only this planning note. It does not implement worker-time
launch or alter startup. The current source and report links are in
[P3-SPAWN-DESCRIPTOR-GATE.md](P3-SPAWN-DESCRIPTOR-GATE.md); the durable status
is [ROADMAP.md](../../ROADMAP.md) and the latest integration evidence is
[INTEGRATION.md](INTEGRATION.md). Run `python3 scripts/check_docs.py` and
`git diff --check` for this document. No application tests are required for
this documentation-only contract.
