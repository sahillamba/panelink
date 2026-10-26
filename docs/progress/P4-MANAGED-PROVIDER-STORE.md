# Phase 4 managed provider record store

## Scope

This slice adds one atomic historical record containing both managed-session
launch metadata and the provider binding. It extracts the existing hardened
private byte-slot publication path into `src/private_record_store.rs` and
keeps the component-only `ProviderBindingStore` API and filenames intact.

`ManagedProviderStore` is scoped to one complete target, canonical project,
provider version, launch `OperationKey`, pinned SHA-256 executable fingerprint,
and optional already-canonical absolute executable path. Its single bounded
record contains the complete launch key, fingerprint, optional path, lifecycle
snapshot, launch outcome, and the existing validated binding codec payload.
The binding payload remains the source of target/project/version identity;
the store cross-validates those fields against its configured scope before
publish and load. The maximum encoded record is 8,552 bytes.

The persisted `Running` lifecycle is historical. Loading it does not establish
that a process remains live, authorize a respawn, prove the provider identity
at crash time, or resume a conversation. `Unknown` is retained as uncertain
launch history. A record is a combined atomic slot, so a binding is never
published separately from the managed launch metadata it describes.

The generic slot retains the existing private-directory/file checks, lifetime
exclusive lock, no-follow opens, bounded reads, inode identity checks, synced
temporary file, atomic rename, directory sync, preserved interrupted temp,
explicit safe temp discard, and fail-closed behavior after ambiguous
post-rename directory-sync errors. Typed stores own their own codecs, scopes,
revision rules, and filenames.

## Verification

Focused verification passed: the existing `ProviderBindingStore` suite passed
9/9 tests after the shared byte-slot extraction; the managed record suite passed
9/9 tests, including exact historical inspection and rejection of wrong
target/project/executable-pin scope. `rustfmt` completed for the three owned
Rust files, and strict library Clippy passed with
`cargo clippy --locked --offline --lib -- -D warnings`.

The full serial library run completed with 800 passed, 2 failed, and 4 ignored.
Both failures were existing pathname-listener tests in
`provider_bridge_launch::tests` (`Operation not permitted` at line 432); they
are outside this storage slice and remain an environment limitation. No
provider runtime wiring, current-at-crash claim, automatic launch retry,
automatic provider resume, or atomic-managed-linkage acceptance is claimed by
this component alone.
