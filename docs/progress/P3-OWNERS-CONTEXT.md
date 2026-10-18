# P3 input ownership and reusable client context

Status: bounded contracts implemented in `src/input_owner.rs` and
`src/client_context.rs`. This is pure owner state plus a metadata-only file
allocator; it does not authenticate a connection, authorize workspace access,
dispatch effects, launch processes, or provide a persistent server.

## Input ownership

`InputOwner` keeps one optional `Owner`, one revision, one expiry time and at
most one pending `OperationKey` for a session. A new empty owner starts at
revision 1. Claims use compare-and-set revision checks; a vacant claim, same
owner renewal, release, handoff and expiry release each advance the revision.
The prototype TTL is 1–30 seconds, using caller-supplied monotonic seconds.
Release and handoff require the current owner. A pending key blocks claim,
renewal, release and handoff, including after expiry. `expire` reports
`PendingNeedsResolution(key)` and retains the owner until the effect owner calls
`finish_input` for that exact key. The owner state machine does not record an
input suffix or decide whether the effect finished as Partial or Unknown.

`Owner` and snapshots are copyable; `InputOwner` is cloneable so a caller can
validate a candidate state before committing it alongside a ledger admission.
The snapshot reports only one holder, revision, expiry and one pending key.
Debug output redacts owner and operation identifiers.

## Reusable client context

`ContextFile::create` is the only initializer. `ContextFile::open` requires the
expected nonzero generation and lease; missing, stale, malformed, truncated or
unsafe state fails closed. The absolute file path must have an existing parent
owned by the effective user with exact mode 0700. The parent and final file are
opened with no-follow flags; the file must be a regular file owned by that user
with exact mode 0600. Creation uses exclusive create and never replaces an
existing entry.

The record is exactly 112 bytes, below the 128-byte bound: two fixed 56-byte
schema-1 slots. Each slot stores record revision, generation, lease, next
sequence, the uncertain operation's sequence, reserved zero bytes and a
64-bit FNV-1a checksum. The generation and lease fields complete the uncertain
`OperationKey`; no request body, target, environment, output, credential or
variable-length field is persisted. The checksum detects accidental changes;
it is not an authentication mechanism. Only the untouched initial layout
(valid revision-1 slot 0 and all-zero slot 1) permits one empty slot. Other
invalid slot combinations fail closed rather than falling back to an older
high-water mark.

The context opens the existing Rustix 1.1.5 `fs::open`/`openat`, `flock`,
`fcntl_fullfsync`, `fsync` and `statat` APIs. It polls the nonblocking exclusive
flock at most once per millisecond until the caller's absolute deadline; it
does not use a blocking advisory lock call. The lock remains held by the guard
until drop. Every reservation and acknowledgment writes the alternate slot
with a checked higher revision and calls `fcntl_fullfsync`. A reservation
contains the incremented sequence and uncertain key before it can return the
key for dispatch. Acknowledgment and terminal/NotAdmitted reconciliation clear
uncertainty only after the exact key is supplied and the new slot fully flushes.
`InProgress` remains blocked. Storage failure poisons the guard and returns no
dispatchable key. Sequence and record-revision overflow fail before mutation.
Creation full-flushes the initial record and syncs the parent directory.

The two-slot/checksum scheme keeps writes bounded and detects partial or
corrupted records; it does not provide a transactional filesystem or a physical
power-loss test. If an update leaves a malformed alternate slot, opening fails
closed. A full-flush error can leave either the previous complete slot or the
new complete uncertain slot visible; neither state returns a dispatchable key
from the failed call. A surviving uncertain key must be reconciled, never
resent. `flock` coordinates cooperating processes that use the same file; it
does not isolate processes running as the same user.

## Verification

The modules were tested through a temporary Cargo harness because this worker
does not own `src/lib.rs` or the crate manifest. The harness imports the two
owned modules and the existing `protocol` module; it uses the exact existing
Rustix 1.1.5 dependency/features and changes no project manifest or lockfile.

| Actual command | Result |
| --- | --- |
| `rustfmt --edition 2021 src/input_owner.rs src/client_context.rs` | Exit 0. |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-owners-context-harness/Cargo.toml -- --test-threads=1` | Exit 0: 39 passed, 0 failed, 1 helper ignored, 0 doctests. The passing lock test launches the ignored helper in a separate test process and confirms it times out while the first process holds the lock. |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --offline --manifest-path /private/tmp/panelink-owners-context-harness/Cargo.toml --all-targets -- -D warnings` | Exit 0. |
| `rustfmt --edition 2021 --check src/input_owner.rs src/client_context.rs` | Exit 0. |

The harness covers TTL and revision checks, stale/expired/wrong owners, CAS
handoff, non-interleaving input, expiry with pending work, metadata bounds,
reopen after uncertainty, NotAdmitted/terminal reconciliation, sequence
non-reuse, two-process lock exclusion, symlink and mode rejection, stale
generation, truncation/checksum failure, injected full-flush failure, and time,
sequence and revision overflow. The full crate integration suite remains for
the parent after module exports and StateOwner wiring. No runtime process or
authorization behavior is claimed here.

## Parent integration update — 2026-10-18

Parent reviewed/exported both modules and integrated them with the state owner
and CLI. `open_existing` loads the stored identity without resetting it, and
the CLI resumes only that exact generation/lease. Parent added validation that
both slots have the same identity, consecutive revisions and a valid semantic
transition; a valid checksum alone cannot authorize rollback. Opens are
nonblocking, hardlinked context files are refused, and creation full-flushes
again after syncing the parent. The context-lock guard stays held through the
matching response or uncertainty outcome. A new malformed-slot regression and
the actual socketpair reconnect test pass in the combined suite; see
[INTEGRATION.md](INTEGRATION.md) and [P3-RECONNECT.md](P3-RECONNECT.md).
