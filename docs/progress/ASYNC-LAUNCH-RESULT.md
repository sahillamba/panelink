# Serialized live-split launch worker — 2026-10-22

Post-stash merge: the incoming four-pane model and280-byte layout format are
preserved. This report records the earlier two-pane implementation run;
combined-tree checks and any integration corrections are recorded in
[STASH-MERGE-RESULT.md](STASH-MERGE-RESULT.md).

Status: implemented, independently reviewed and parent-verified within the
scope of [ADR-LAUNCH-01](ASYNC-LAUNCH-DESIGN.md). Parent phases 3/5/6 remain
open. Initial startup and legacy direct-core launch calls remain synchronous;
the live multi-pane split route now uses the worker.

## Behavior and ownership

One launch worker performs resolution/preparation, launch journal reads and
syncs, and PTY process creation. The owner admits exact split bytes, reserves
the target and preview, retains registry/ledger authority, and adopts every
returned child before the worker writes final Spawned/Applied records. Intent
is durable before spawn. A known pre-spawn failure is Failed; an uncertain
worker loss is Unknown. An acquired child survives final journal failure and
stays owned with an Unknown result. Launches are never implicitly replayed.

The worker has one command slot and one completion slot, with one active owner
split and at most eight bounded reply waiters. Payloads use the existing
65,536-byte split bound. A separate journal claim excludes direct journal
handles/another worker, duplicates singleton/directory descriptors with
atomic CLOEXEC, and never advances generation. Existing PTYs, input ownership,
layout mutation and history observation stay on the owner. The new child alone
crosses threads, with TerminalSession's Send property compile checked.

Busy topology mutation protects the reserved preview while input/read/watch
continue. Natural exits do not end the workspace prematurely; settlement
reconciles exited panes. Disconnect/reply timeout drops only the waiter. Exact
retry returns the original result or uncertainty; changed bytes conflict.
No wire protocol, dependency, manifest, lockfile or vendor source changed.

Shutdown requests cancellation before stopping owned children. Worker waiting
shares the existing250ms metadata deadline with history/layout; it joins only
an already-finished thread. A blocked call or unclaimed child keeps singleton
exclusion on the worker. Timeout prints a fixed incomplete-shutdown warning.
Neither syscall interruption nor cleanup beyond server process exit is
guaranteed; detached threads cannot survive process exit. No phase, host,
provider, physical reboot or general latency guarantee follows from this slice.

## Work and review

Three GPT-6-Luna high workers handled owner/coordinator phases, worker/storage,
and runtime acceptance/final source review with exclusive file ownership.
The parent wrote the ADR, integrated runtime dispatch/finite waiters, reviewed
all boundaries, fixed failure publication outside the completion mutex, and
added the both-children-exited-before-finalization regression.

Parent review also corrected the worker's existing-target/new-target check,
duplicate-before-waiter-capacity ordering, truthful pre-spawn failure labels,
and grace-period cleanup waiting. The first integration run exposed a debug
stack overflow caused by inline pending state. Boxing bounded pending/emergency
state and the child completion fixed it; normal-stack lifecycle tests pass.
Intermediate compile, formatting, lint and test-fixture failures were fixed
before the final runs. The final independent source review found no
substantial blocker.

## Actual verification

```text
cargo test --locked --offline --lib -- --test-threads=1
641 passed; 0 failed; 4 ignored

cargo test --locked --offline --test p3_split --test p3_multi_owner --test p3a_runtime --test p3b_close --test w2_server_close --test w2_server_shutdown --test p5_layout_lifecycle --test p5_layout_writer --test p5_layout_store --test p5_layout_cli --test saved_layout_contract --test p5_live_history_driver -- --test-threads=1
73 passed; 0 failed; 0 ignored

combined verifier: 7/8 checks passed
binary: 115 passed; 1 failed at known pathname bind EPERM
decoder: 39 passed; Python: 60 passed
strict all-target Clippy, fmt, build, component fingerprints: passed
final docs check: 216 Markdown files; 0 findings
```

New library coverage includes six worker cases, the exclusive storage claim,
six owner phase cases and three real-PTY/socketpair runtime cases. Runtime
responsiveness is gated at both BeforeIntent and BeforeFinalize: existing
input/echo and Describe/screen reads progress before release; exact retries
create one child marker. Other cases prove a disconnected waiter still
commits once, and canceled gated work retains the singleton until release
without starting a child. Worker cases cover finalization uncertainty with a
held child and cleanup of an unconsumed child on the worker. Owner cases cover
Unknown without an adopted handle, attached Unknown, topology guards and
exits during finalization. These are controlled software stage delays, not
physical disk-stall or power-loss measurements.

The verifier uses the same fixed eight commands via its `run_checks` runner,
removing its injected CARGO_HOME to use the populated default cache and setting
RUST_TEST_THREADS=1. This is the environment adaptation recorded in
[layout completion](P5-LAYOUT-COMPLETION.md), not a tooling change. Cargo's
binary failure is `default_open::tests::endpoint_state_classifies_files_and_sockets`
with `PermissionDenied: Operation not permitted`; it stops Cargo before the
integration targets, which were run separately above. Ignored tests are not
passes. Remaining unrelated integrations and host/provider acceptance were
not rerun. Raw logs are temporary `/private/tmp/panelink-async-launch-{lib,integration,verifier}.log`.

No Git mutations were performed; the user deferred Git work. All worker
ownership is released. Continue from ROADMAP.md without repeating this slice.
