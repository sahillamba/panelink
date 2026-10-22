# Bounded layout storage completion — 2026-10-22

Historical two-pane completion evidence follows. The incoming four-pane
implementation writes280-byte current records and supports legacy160-byte
reads/migration. Combined-tree evidence is in
[STASH-MERGE-RESULT.md](STASH-MERGE-RESULT.md).

Scope: finish and verify the integrated two-pane saved-layout storage slice
after combining the earlier persistence implementation with Wave2 phased close.
Full Phase 5, provider recovery and physical reboot acceptance remain open.

## Delivered and reviewed

The existing implementation supplies a strict 160-byte historical codec,
`current.layout` and `previous.layout`, archive-before-current rotation,
private-directory/file validation, bounded temporary cleanup, a separately
guarded background writer, owner observations, freeze-before-stop, and
startup-free `panelink layout show [--previous]`. See the original
[implementation result](P5-LAYOUT-SLICE.md) and
[design contract](P5-SAVED-LAYOUT.md).

Two GPT-6-Luna high workers independently reviewed storage/writer and
lifecycle/CLI paths. Neither found a material store rotation, bounds or CLI
defect. The writer review identified a shutdown ownership gap: abandoning
`PendingLayoutShutdown` left queued saves and retries running. The parent
added cancellation on handle Drop and a blocked-backend regression proving
queued saves do not start and singleton exclusion lasts until active IO ends.

Additional acceptance covers an uncertain last-pane close retaining its
nonempty topology until a positive exit; explicit last-pane close publishing
an empty tombstone; driver shutdown during an active close preserving the
pre-stop layout; and both metadata workers blocked after the child has exited.
The shared-budget test isolates metadata waiting, checks driver Drop below
425 ms to distinguish the single 250 ms deadline from sequential budgets,
and verifies neither blocked save is called durable and the singleton remains
held until both gates are released. This is an automated software observation,
not a physical disk-latency or power-loss guarantee.

## Verification

Parent independently ran the final combined source tree:

```text
cargo test --locked --offline --test p5_layout_store --test p5_layout_writer --test p5_layout_lifecycle --test p5_layout_cli --test saved_layout_contract --test w2_server_close --test w2_server_shutdown --test p5_live_history_driver -- --test-threads=1
66 passed; 0 failed; 0 ignored
  store 32, writer 5, lifecycle 12, CLI 11, codec contract 2,
  phased close 1, shared child shutdown 1, live history driver 2

library suite (serial, through the verifier)
625 passed; 0 failed; 4 ignored

combined verifier: 7/8 checks passed
binary suite: 115 passed; 1 failed at pathname socket bind (EPERM)
decoder: 39 passed; Python: 60 passed
strict all-target Clippy, fmt, build, component fingerprints: passed
```

The failing binary case is
`default_open::tests::endpoint_state_classifies_files_and_sockets`; socket bind
returns `PermissionDenied: Operation not permitted`. Cargo stops before the
integration targets, so the 66 checks above were run separately. Unrelated
integration targets were not rerun in this completion pass. The 4 ignored
library tests are not acceptance passes. Final documentation validation:
214 Markdown files, 0 findings.

The first direct verifier invocation selected an empty temporary Cargo cache
and failed offline dependency resolution. The rerun executes the same eight
fixed commands through `run_checks`, with a process-runner wrapper removing
the injected `CARGO_HOME` so Cargo uses the populated default cache. It also
sets `RUST_TEST_THREADS=1`. This changes verification environment selection,
not repository tooling or application behavior.

Raw final logs are in `/private/tmp/panelink-layout-focused-final.log` and
`/private/tmp/panelink-layout-verifier-final.log`; these temporary logs are
not required runtime data or permanent repository artifacts.

## Remaining architecture issue

Subsequent work completed the live-split worker described in
[ASYNC-LAUNCH-RESULT.md](ASYNC-LAUNCH-RESULT.md). The paragraph below records
the open issue at the layout-storage checkpoint; initial startup and legacy
direct-core calls remain synchronous.

Launch preparation and launch-journal sync still run synchronously on the
server owner. Completing layout storage does not resolve their latency.
A later design must preserve durable-before-effects ordering, operation
deduplication, ownership through uncertain results, and the existing spawn
descriptor gate while moving slow work off the owner. No async launch worker
or concurrent-spawn policy is adopted by this storage slice.

The two layout slots are not a transaction with saved history. Coalesced or
unsubmitted changes can be absent from the latest durable layout; ongoing
storage calls cannot be forcibly interrupted. Corrupt records remain refused
and preserved. No checksum, machine-reboot acceptance or provider resume is
claimed. No dependencies, manifests, lockfiles or vendor sources changed.
