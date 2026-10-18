# Managed launch transaction core — 2026-10-18

## Delivered scope

Added `src/managed_launch.rs` as a bounded trusted transaction prerequisite.
`ManagedLaunchCoordinator<E>` owns the existing two-slot `SessionRegistry<E>`,
two fixed operation/result entries and one emergency handle slot. Each operation
borrows the caller's one generation-wide `Ledger`; there is no ledger, grant
table, launcher, server command, socket operation or automatic recovery in this
module. Its constructor rejects generation zero. The existing synthetic target
mapping remains prototype scaffolding and does not establish a pane hierarchy.

The coordinator validates the complete schema-6 launch payload and checks
generation equality before consulting `Ledger::existing_admission`. Exact
duplicates return only a result stored under that key; changed bytes and
cross-kind key reuse remain global ledger conflicts. For new keys it runs the
read-only ledger capacity preflight before any preparation, registry or journal
callback; then it preflights the next registry target, invokes pure caller
preparation, checks three-record journal headroom, admits the exact bytes,
reserves the target and stores the initial result before writing Intent.
Intent is durable before the spawn callback. A spawned `E` moves immediately
into registry ownership. Post-spawn journal failures produce `Unknown` while
retaining the handle. A rejected attachment is retained in the emergency slot
and permanently latches new operations off; exact retained results and status
remain observable. Concrete handles are not exposed mutably: `effects_mut`
returns only `&mut dyn EffectSink`. Trusted effect implementations can still
shut down or mutate their own resources; this is an ownership invariant, not a
security boundary.

Any lifecycle `record` failure also latches this coordinator unhealthy,
including an Intent failure, a failed-spawn terminal record, or a post-spawn
record. This matters because the real journal's poison bit belongs to one
borrowed journal handle and a later call can reopen it. The latch permits exact
duplicates and status reads but rejects every new key before preparation,
journal preflight, ledger admission or spawn. It does not stop already-owned
children and is volatile to this coordinator; it is not a persisted storage
health record or a cross-generation guarantee. A known preflight rejection
does not latch health.

`note_exited` accepts an exit observation only when the retained effect handle
reports the child exited, then retains that handle as the identity tombstone.
Failed and exited identities continue to consume both session slots. Journal
callback and caller callback diagnostics are discarded; the coordinator's
Debug/Display output does not render launch bytes, targets, environment values,
or the owned effect handle.

No dependencies or manifest changes were made. The real `LaunchJournal` adapter
delegates its existing preflight and record methods. Fake journals are used
only for deterministic failure injection. This code does not authorize a
caller, establish a global owner, or enable a CLI/socket launch capability.

## Automated evidence

The focused unit tests cover generation validation, call ordering, duplicate
result replay, changed-byte and cross-kind conflicts, foreign admitted keys,
global ledger capacity preflight, preparation and journal-preflight failures,
Intent write failure, known spawn failure, Spawned/Applied journal faults,
emergency attachment retention, diagnostic redaction, and capacity consumed by
Failed and Exited identities. Drop counters verify retained handles stay owned
through Unknown/Exited and are dropped only when the coordinator is dropped.

The macOS/aarch64 integration test uses the real `TerminalSession::spawn`,
`PtyEffects` and private on-disk journal with explicit cwd/environment. Two
children print distinct markers and their shell PIDs; exact retry returns the
same target and PID without printing a second marker, changed payload conflicts,
and a third identity is rejected before admission. The second integration
scenario injects both Spawned- and Applied-record failures in isolated fixtures
through a wrapper around the real journal. Each child first prints a ready
marker, then accepts a new explicit Submit token after the journal fault; the
test requires its same PID to echo that token through the retained PTY. It
verifies bounded cleanup on coordinator drop. These fixtures use no
pathname socket or outer `/dev/tty`.

Executed checks:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib managed_launch::tests::
15 coordinator tests passed; 0 failed.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib managed_launch
26 total tests passed (15 coordinator and 11 schema-6 codec); 0 failed.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_managed_launch
2 passed; 0 failed.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
passed.

cargo fmt --check
passed.

python3 scripts/check_docs.py
Checked Markdown files; 0 findings.
```

This is component evidence, not completion of the managed-launch milestone or
Phase 3. The pathname accept/exec acceptance remains separately gated by the
recorded bind permission failure. The parent owns integrated review, broader
verification, exports/shared status and any release decision. No commit was
made; implementation ownership is released to the parent for review.
