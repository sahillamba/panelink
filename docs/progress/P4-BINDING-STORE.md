# P4 durable provider-binding store — 2026-10-24

Status: a bounded last-confirmed binding store is implemented as a standalone
component over `provider_binding_codec`. It is not connected to application
startup, the CLI, or a provider bridge. The parent-owned component integration and
crash/acknowledgment tests pass; see
[P4-BRIDGE-STORE-RESULT.md](P4-BRIDGE-STORE-RESULT.md).

## Scope and API

`ProviderBindingStore::open(path, target, project_bytes, supported_version)`
opens one caller-dedicated canonical absolute directory for one complete
`TargetRef`, exact opaque project identity, and exact supported-version token.
The caller chooses the per-target location; this slice establishes no product
directory hierarchy. The store accepts only records matching that entire
scope. `load` returns a `LastConfirmedRecord` only after decoding the committed
slot and checking the configured scope. The provider-binding core still
restores that record as stale.

`save(&mut self, binding)` requires an exact scope match. Revisions increase
across bridge instances; lower revisions and same-revision changed records are
refused. Exact byte-identical duplicates sync the directory before returning
`AlreadyDurable`. A new record is encoded within the codec's 4,384-byte bound,
written to the fixed exclusive temp, file-synced, atomically renamed over
`last-confirmed.binding`, then directory-synced before `Written` is returned.
The separate `discard_pending` method removes only the recognized fixed temp
after checking its type, owner, mode, link count, and size. `open` never
promotes or silently removes an interrupted temp.

The store holds a nonblocking exclusive `flock` for its lifetime. Its lock file
is empty, private, regular, single-link, and checked against the named inode on
each operation. Directory identity and private parent constraints are checked
before use. Committed and temporary entries are opened relative to the pinned
directory with no-follow, nonblocking flags; unsafe file types, malformed
records, and oversized files are refused and preserved. Debug output and
application-defined errors are fixed and redacted. Same-UID hostile processes
are outside this prototype's isolation model.

An injected failure before rename leaves the old committed slot authoritative
and cleans only a temp created by that save call. An error after rename during
directory sync is ambiguous: the save returns an error, latches that store
instance read/write-disabled, and requires an explicit close and reopen to
inspect the slot. Storage sync calls provide filesystem-ordering evidence only;
they do not establish power-loss, reboot, or current-at-crash guarantees.
There is no cryptographic integrity check or provider authenticity proof.

## Verification

Focused private-unit tests cover initial write/load/reopen, exact duplicate and
revision conflict behavior, wrong scope, corrupt-slot preservation, lock
contention, directory replacement, unsafe slot type, interrupted-temp
nonpromotion and explicit discard, and injected failures before write, after
file sync, before rename, and after rename at directory sync. Parent-owned
integration tests separately prove that store failure prevents core confirmation
and exercise process termination before/after storage and acknowledgment; see
the linked combined result.

Requested command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --lib provider_binding_store -- --test-threads=1
```

The first attempt was blocked before compilation because the configured offline
Cargo cache could not resolve locked `libc` from its local index. The parent
prepared an isolated Cargo cache from the installed registry, and the command
above passed: 9 focused store tests passed, 0 failed, 0 ignored.
`rustfmt --edition 2021 src/provider_binding_store.rs` completed
successfully. The separate target directory preserves the active
`target/debug/panelink` binary.

No dependencies, provider integration, CLI behavior, or startup behavior were
added.
