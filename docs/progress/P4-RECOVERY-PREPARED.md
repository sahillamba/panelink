# Phase 4 prepared manual recovery record

## Contract

`PreparedResumeStore` persists a typed, one-slot, one-shot prepared choice in
the exact managed target directory, separately from the atomic managed-session
and provider-binding record. The bounded record retains the full
`ManagedProviderScope` (target, project, provider version, launch operation,
pinned executable digest and optional canonical executable path), explicit
session ID, prospective binding revision, source bridge instance, caller
validation time, a nonzero OS-random 16-byte operation ticket, and either
`Prepared` or `Spent` state. It is bounded to 8,537 bytes and contains no
credential, environment, prompt, input, or fabricated `Binding`.

The first `prepare` atomically publishes a `Prepared` record and returns its
receipt only after the file and containing directory sync. Any existing
prepared or spent record blocks another prepare for that exact target. A
matching `mark_spent(ticket, revision)` transitions to `Spent` atomically and
returns only after sync; the caller may issue navigation only after that
receipt. `Spent` is terminal. The API does not perform fresh provider
validation itself: the runtime must repeat supported exact-ID/project/source
validation and require explicit user intent immediately before navigation.

On reopen, `Prepared` remains a historical prepared choice and `Spent` remains
consumed. Neither state proves current selection, provider acceptance, a
successful resume, or an authoritative `Binding`. A failed or ambiguous sync
returns no receipt; the exact slot is then inspected and the one-shot attempt
is not retried automatically. Interrupted temporary files are never promoted.
The store reuses the shared hardened atomic byte slot but has distinct current,
temp and lock filenames from the managed binding record.

## Verification

The focused prepared-recovery suite passed **11 tests, 0 failures, 0 ignored**
with locked/offline Cargo in the isolated cache/target directories. Coverage
includes prepared and spent reopen behavior, duplicate prepare/spend refusal,
wrong target/project/version/launch-key/executable fingerprint, corruption,
post-rename ambiguity for both transitions, asynchronous token matching and
busy admission, startup failure, cancellation of queued work, and bounded
finish while running IO retains the lifetime lock. Strict all-target Clippy
with `-D warnings`, `rustfmt --check`, and scoped `git diff --check` passed.

Commands:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --lib provider_recovery_prepared -- --test-threads=1

CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo clippy --locked --offline --all-targets -- -D warnings
```

The owner-facing worker uses capacity-one channels, accepts at most one
unconsumed operation, and performs open/load/prepare/spend off-owner. Completion
fields and receipt constructors are private; callers must match the completion
token before adopting its result. Drop cancels queued work without joining, and
bounded `finish` detaches on timeout while the thread keeps its store lock
until IO returns. No automatic retry is performed after uncertain publication.

This component does not wire a provider picker or actual resume command,
confirm a selected binding, or satisfy provider-interface and post-navigation
verification gates. Runtime callers must obtain fresh provider validation and
explicit user intent immediately before navigation; the `Spent` sync must
complete first.
