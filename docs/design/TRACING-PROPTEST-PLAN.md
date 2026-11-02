# Structured diagnostics and property testing adoption plan

Execution update (2026-11-02): user authorized implementation. See
[implementation evidence](../progress/TRACING-PROPTEST-RESULT.md) and
[current usage/limits](../DIAGNOSTICS.md). The original investigation and
proposed exit criteria below remain the design record, not a claim that every
performance/native acceptance gate is closed.

Date: 2026-11-02. Investigation and proposed implementation packets only.
Baseline: `5ecc22a0c7674e5d2a9f60959b779fa8111f2317`, branch
`t3/investigate-two-suggestions`. No dependency adoption, application change,
installation, server restart, or product acceptance is implied.

## Recommendation

Investigate `tracing` first, then implement the smaller `proptest` pilot before
shipping diagnostics. Both address concrete Panelink needs. Property tests can
exercise pure boundaries without changing the released program. Useful tracing
requires deliberate correlation, privacy, bounded output, and performance work.

Use exact reviewed versions and minimal features. Treat the full resolved graph,
build scripts, licenses, advisories, compile time and binary size as adoption
criteria under [the security requirements](../SPEC.md#proposed-security-review-requirements).
Firm approval remains a separate unresolved gate.

## Current code findings

- [Client host](../../src/ui_client/host.rs) owns terminal decoding, frame output,
  and restoration. It has a 5 ms input-work budget, 4 ms key-flush delay and
  16 ms render interval. These are scheduling constants, not measured latency.
- [Client worker](../../src/ui_client/worker.rs) moves owned commands through a
  bounded queue (32 commands, 4 MiB owned-byte budget). Its `Envelope` has no
  diagnostic correlation or enqueue time. Instrument queue delay separately
  from backend execution and result application.
- [Pane connections](../../src/ui_client/conn.rs) have request identifiers and
  fixed diagnostic labels. `request` poisons broken/timed-out streams. Preserve
  that behavior and nonreplayable input; tracing must never reconnect or retry.
- [Multi-session runtime](../../src/multi_runtime.rs) has its own workers,
  `MultiEnvelope`, owner tick, deferred close/watch/split processing, and launch
  worker. Instrument this active path first. The separate single-child
  [server runtime](../../src/server_runtime.rs) is insufficient coverage.
- [Default launcher](../../src/default_open.rs), `open_log`, creates a private
  append log and rotates above 1 MiB **at startup**. That is not a runtime size
  cap. Simply sending every tracing event to existing stderr can grow the log
  throughout a long server session; client stdout/stderr also risk TUI or JSON
  output contamination.
- [Wire framing](../../src/protocol_wire.rs) and
  [typed control requests](../../src/control_protocol.rs) are pure, bounded
  codecs with existing example tests. Random bytes alone mostly exercise early
  rejection; useful strategies must also generate valid and near-valid frames.
- [Pane layout](../../src/pane_layout.rs) exposes bounded geometry and mutation
  rules. [UI protocol](../../src/ui_protocol.rs) predicts split sizes using layout
  geometry; compare predictions with resulting layout, alongside independent
  partition invariants.
- [Input decoder](../../vendor/crossterm-input/src/decoder.rs) exposes `advance`,
  `finish_escape`, `pending_bytes`, and `is_failed`. Its ordinary and paste
  sequence bounds differ, and parse failure permanently poisons it. ESC handling
  depends on availability/timeouts, so arbitrary chunking is not universally
  equivalent.

## Dependency investigation

Read-only upstream research and isolated `cargo metadata` resolution succeeded;
no candidate code was compiled or run. Temporary manifests did not alter the
repository manifest or lockfile. Candidate versions/features:

```toml
# Proposed only; place runtime entries under the appropriate target cfg.
tracing = { version = "=0.1.44", default-features = false, features = ["std"] }
tracing-subscriber = { version = "=0.3.23", default-features = false, features = ["fmt"] }

[dev-dependencies]
proptest = { version = "=1.12.0", default-features = false, features = ["std"] }
```

The tracing pair declares Rust 1.65 and MIT licensing; proptest declares Rust
1.88 and MIT OR Apache-2.0. Both are below Panelink's pinned Rust 1.99.1.
This is metadata compatibility, not build verification. Sources:
[tracing manifest](https://docs.rs/crate/tracing/0.1.44/source/Cargo.toml),
[subscriber manifest](https://docs.rs/crate/tracing-subscriber/0.3.23/source/Cargo.toml),
[proptest release manifest](https://raw.githubusercontent.com/proptest-rs/proptest/v1.12.0/proptest/Cargo.toml).

The standalone tracing configuration resolved these nine packages (including
the two candidates): `cfg-if 1.0.5`, `lazy_static 1.5.1`, `once_cell 1.21.4`,
`pin-project-lite 0.2.17`, `sharded-slab 0.1.7`, `thread_local 1.1.10`,
`tracing 0.1.44`, `tracing-core 0.1.36`, `tracing-subscriber 0.3.23`.

The standalone proptest configuration resolved sixteen packages (including
proptest): `autocfg 1.5.1`, `bitflags 2.13.2`, `cfg-if 1.0.5`,
`chacha20 0.10.2`, `core_detect 1.0.0`, `cpufeatures 0.3.1`,
`getrandom 0.4.3`, `libc 0.2.191`, `num-traits 0.2.19`, `proptest 1.12.0`,
`r-efi 6.0.0`, `rand 0.10.3`, `rand_core 0.10.1`, `rand_xorshift 0.5.0`,
`regex-syntax 0.8.11`, `unarray 0.1.4`.

These are resolver inventories across targets, not incremental application or
Apple Silicon build counts. Some packages already occur in Panelink's graph;
target-specific packages may not compile on macOS. Metadata reports permissive
MIT/Apache alternatives throughout, with `r-efi` also offering LGPL as an
alternative. Source/license-file verification and advisory review remain pending.

Avoid `tracing`'s default attribute macro and subscriber ANSI/log/env-filter/JSON
features initially. Add structured text fields with explicit macros; avoid
automatic argument Debug capture. Proptest's `std` profile avoids its default
fork/timeout/bit-set features but still brings regex-syntax and RNG dependencies.
Do not introduce derive or state-machine extension crates for the pilot.

## Packet 1: dependency and baseline evidence

Owner: manifest, lockfile, dependency evidence and notices; serialize shared edits.

1. Resolve each proposed graph against the actual Panelink manifest in a
   disposable directory. Record normal versus dev and target-specific deltas,
   feature unification, source hashes, licenses, build scripts and macros.
2. Run dated advisory checks using the existing release/security workflow.
3. Measure clean/incremental test compilation, release size and baseline serial
   test duration. The current handoff reports near-420-second Rust checks and
   pre-existing Python bridge errors; re-establish their current state rather
   than assume either a clean baseline or the same failure.
4. Pin reviewed candidates, update notices/inventory with runtime/dev scope,
   and record any unresolved findings before adopting them.

Exit: reviewed dependency deltas and measured build/test costs. Split the two
adoptions so either can be reverted independently.

## Packet 2: proptest pilot

Owner: new root integration tests and synthetic strategies; leave vendored
component sources and fingerprints unchanged. Root tests can call the existing
decoder dependency. Keep named deterministic regressions and PTY tests.

1. **Wire/control:** generate valid bounded frames/requests; check round trips,
   borrowed payload preservation and exact lengths. Mutate magic, version,
   reserved bytes, declared lengths and identifier rules; check specified
   rejection classes. Exercise truncation and trailing data. Include random
   bounded byte vectors with a no-panic property, but do not stop there.
2. **Layout:** generate bounded sequences of split/resize/close/swap operations
   with valid and stale identities. Independently check root containment,
   disjoint pane content, required divider cells, pane limits, identity
   uniqueness, valid minimum sizes and revision rules. Rejected operations must
   preserve the entire observable state. Shrinking must preserve prerequisites
   or treat newly invalid operations as explicit rejection steps.
3. **Terminal input:** generate supported UTF-8/key/mouse/paste sequences and
   malformed prefixes. Check fixed pending bounds, sticky poison, no output
   after poison, and documented paste handling. Compare delivery schedules only
   when availability flags and ESC timeout semantics are equivalent; explicitly
   test intended ESC/Alt ambiguities separately. Apply timeouts with a synthetic
   clock/driver where needed, without sleeping or creating terminals.

Start with 256 cases/property, bounded vectors and operation sequences, plus
explicit shrink limits. Rare maximum-size boundaries should be deterministic
cases rather than allocate 2 MiB per random trial. A proposed larger opt-in
local run uses 4,096 cases; measure cost before expanding CI.

Use generated synthetic data only. Capture seeds and retain reviewed
`proptest-regressions` files in Git; additionally promote important minimized
cases to named tests because strategy changes can change seed interpretation.
Keep some changing seeds for exploration and record an exact replay procedure.
Do not retry failed properties until green. Without the timeout feature,
case/shrink bounds are not a wall-clock timeout; retain the verifier's process
timeout. Sources: [configuration](https://proptest-rs.github.io/proptest/proptest/tutorial/config.html)
and [failure persistence](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html).

Exit: demonstrate that selected deliberate codec/layout mutations fail the
properties, replay a minimized synthetic failure, and pass the canonical gate
within its existing budget. Property tests supplement race/PTY/native evidence.

## Packet 3: tracing foundation and one vertical path

Owner: proposed diagnostics module/bootstrap, client worker/connection, multi
runtime envelopes, and focused diagnostic tests. Shared files need one owner.

1. Keep tracing off by default and local only. Initialize the subscriber at the
   executable boundary; libraries only emit events. Preserve existing CLI
   diagnostic and JSON contracts. Use target/level filtering initially.
2. Define a field allowlist: fixed operation/stage/outcome labels, durations,
   byte/count totals and approved numeric correlation identifiers. Exclude all
   input/paste/screen bytes, command arguments, paths, credentials, environment
   values, provider conversation IDs and raw Debug/error strings. Do not hash
   sensitive content as a substitute. Assert exclusion with synthetic canaries.
3. Trace one attach and one input batch through host submission, worker queue,
   durable context reservation, socket exchange, server queue, owner dispatch,
   PTY acceptance, reply and result application. Later add snapshot composition
   and host write stages. Avoid per-byte/per-cell/per-poll event floods.
4. Propagate spans explicitly through in-process envelopes and deferred work;
   enter only while processing each request. A shared global subscriber does
   not propagate current spans between threads. Account for retained span and
   envelope size in bounded queues; release on cancellation/shutdown.
5. Across processes, join on an approved tuple of generation, connection/lease
   scope and wire request ID. Request ID alone is connection-local. Reuse
   existing wire data; do not add a protocol field merely for tracing. Keep
   diagnostic identity separate from deduplication/ownership authority. Label
   any host-to-wire relationship that the initial pilot cannot join.
6. Record monotonic durations inside each process; do not subtract independently
   based process timestamps. Keep queue wait, service time, round trip and host
   output acceptance distinct. PTY acceptance does not prove child processing,
   provider success or visible display latency.

Spans need an explicit timing configuration. `FmtSpan::CLOSE` with timestamps
enabled reports busy/idle time; those are span-entered/lifetime measurements,
not CPU profiling. Explicit stage duration fields can describe queue waits more
clearly. See [subscriber timing](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.SubscriberBuilder.html#method.with_span_events).

Exit: deterministic capture tests prove correlation, correct stage attribution,
redaction, and unchanged no-replay behavior. No claim of latency improvement.

## Packet 4: bounded diagnostic output and performance gate

Compare a small bounded typed-event writer with the additional reviewed
`tracing-appender` graph. Prefer the established component if it meets the
requirements with lower total maintenance cost; do not assume a custom writer
is cheaper. Keep formatting/field costs in the producer budget either way.

Production prerequisites: dedicated private client/server files, no terminal or
JSON writes, bounded queue **and record size**, runtime byte-capped rotation and
retention, nonblocking producer overflow with visible dropped-record counts,
disk/permission failure behavior, and bounded shutdown after terminal restoration.
Do not allow sink failures to cancel admitted input or hold the PTY owner.
No automatic uploads or diagnostic exports.

Upstream appender provides a bounded queue; lossy mode drops at capacity and
non-lossy mode backpressures producers. Retain its guard if selected, but verify
shutdown and rotation against Panelink's requirements. Time-based rotation alone
does not bound disk usage. See [nonblocking writer](https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/).

Measure identical release workloads with no instrumentation, instrumented but
disabled, and enabled: attach, paced/burst input, active output, scrolling,
resize, detach, and slow/full/broken sinks. Use an actual attach/server/PTY
topology and report median/tail latency, idle CPU, RSS, event bytes and drops.
Set explicit tolerances from repeated baseline variability before accepting
results; upstream filtering is not a zero-overhead promise. Preserve separate
native host/provider gates. See [compile-time filtering](https://docs.rs/tracing/latest/tracing/level_filters/).

## Completion and evidence

For implementation, run focused checks, then the canonical serial command once:
`RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py`. Investigate failures and
record environmental blockers separately. Do not silently increase timeouts or
count skipped checks as passes. See [CI](../CI.md).

Before exposing diagnostics, update CLI help, [usage skill](../../.agents/skills/panelink/SKILL.md),
security/storage contract, notices and dependency inventory, setup/CI docs and
roadmap/session evidence. Document defaults, files/retention, enabling both
client and server, losses, failure behavior and client/server version identity.
Changing the client does not activate instrumentation in an existing server.

Investigation performed: current source inspection with a fresh checkout index,
primary-source research and two successful disposable metadata resolutions.
No candidate build, property run, latency measurement, advisory scan or full
source/license audit was performed. Only this planning document was added.
