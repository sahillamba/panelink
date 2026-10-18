# One-child startup storage and runtime integration — 2026-10-18

Implemented by the parent in `src/server_storage.rs`, `src/server_startup.rs`
and `src/server_runtime.rs`, reviewed alongside the parallel transport,
state-owner, context and PTY work. This is an experimental foreground server,
not general crash recovery or an API for launching multiple sessions.

## Startup and identity

`server-probe --initialize` explicitly initializes an empty private directory.
It draws a nonzero, nonmaximum 64-bit generation seed from the OS random
device. Established startup opens existing state, takes the singleton lock,
reopens the counter after locking, increments it with checked arithmetic and
flushes the new generation before process effects. The lock remains held until
connection workers and the owned child have been shut down.

Normal startup never recreates a missing lock, counter or journal. Private
directories and metadata are checked for ownership, permissions, symlinks and
inode identity; files require mode0600 and one link. The lock record binds its
device/inode. The foreground loop checks the protected directory and named
lock still match the held inodes. Counter publication uses exclusive temporary
creation, full file flush, rename, directory sync and a further full flush.
Leftover temporary files, malformed records, exhaustion and uncertain sync
fail closed. No automatic stale-socket removal or metadata repair is provided.

Generations advance within an established storage domain. Random initial seeds
reduce accidental reuse across explicitly created domains; they do not prove
collision-free identity across domains or protect against restoring an older
backup. Lost, deleted or restored metadata requires explicit recovery and
context reconciliation. Nothing silently resumes an old lease in a new domain.

## Admission and launch effects

The complete explicit launch body must fit65536bytes before admission. It
includes resolved paths, at most256 arguments and at most4096 environment
entries with length boundaries. It remains volatile. The startup operation is
admitted to the same ledger later owned by `StateOwner`; a metadata-only
Intent is appended and fully flushed before spawning the child. Spawned and
Applied facts follow process creation and registry commitment. Failed intent
storage prevents spawn. Result-storage failure retains the already-owned child
and exposes Unknown; it never authorizes another spawn.

The fixed56byte journal persists only operation key, session ID and lifecycle
state. It excludes arguments, environment values, terminal input/output and
PID. It is capped at1MiB and4096 distinct keys. Incomplete Intent/Spawned rows
recover as Unknown observations and are never executable recovery instructions.
A failed write poisons the journal handle. This is not exactly-once execution
or a power-loss recovery proof.

The runtime connects at most eight bounded connection workers to one state
owner. A lost reply receiver does not cancel or resend an admitted effect.
Even if PTY polling fails, a tick still settles queued owner requests and reaps
workers before returning that error. Workers stop before the state drops its
child, and storage releases its singleton lock last.

## Review and verification

An independent Luna review identified missing-state reset/recreation risks;
the parent corrected them and the worker rechecked the final source without
finding another concrete defect in this scope. Parent added explicit missing
file, random initialization, lock identity, context-slot rollback and poll-error
regressions. Startup tests exercise real PTY creation, singleton/generation
behavior, secret-canary absence, injected result commitment failure, full
journal denial before spawn and complete launch-body boundaries.

One parallel test fixture collided when two invocations used only PID/time for
its directory name. Adding a per-process atomic suffix corrected the fixture;
the focused rerun passed15tests. Final combined results are recorded in
[INTEGRATION.md](INTEGRATION.md). These checks do not substitute for the actual
pathname CLI run, a second OS user, Apple Terminal acceptance, hostile backup
rollback or process/power-loss fault injection.
