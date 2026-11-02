# Tracing and property testing implementation evidence

Date: 2026-11-02. Checkout `t3-af2a1b87`, branch
`t3/investigate-two-suggestions`, baseline
`5ecc22a0c7674e5d2a9f60959b779fa8111f2317`. Uncommitted implementation;
no installation, user-server restart, live pane input, push or phase closure.

## Delivered scope

- Three root property targets: 24 protocol, 9 layout and 11 input tests.
  Bounded synthetic strategies, 256 default cases, 4,096 shrink iterations,
  explicit per-target seed persistence; `PROPTEST_CASES` override is honored.
- Disabled-by-default structured local tracing. Private bounded queue, atomic
  capped records, byte-capped rotation, two backups, nonblocking overflow,
  counted sink failures and bounded shutdown after terminal restoration.
  Per-role lock prevents concurrent processes rotating the same files.
- Client attach/queue/service/result and wire stages, active multi-session
  server queue/dispatch, and PTY acceptance. Existing wire IDs correlate
  requests without changing protocol or retry/ownership authority.
- Minimal pinned dependency profiles, complete dev-edge inventory, preserved
  published license texts and a fresh advisory scan. See
  [dependency review](TRACING-PROPTEST-DEPENDENCIES.md).

## Parent review corrections

The worker's production changes required parent completion. Fixed failing
capture tests (clones had isolated storage), broken test fixture visibility and
subscriber setup, sink error counting in the failure probe, incorrect literal
length/role assertions, and strict Clippy findings. Production hardening groups
formatted fragments into one bounded record, stops ignoring rotation failures,
checks descriptor ownership/mode/regular-file/single-link properties, rejects
unsafe canonical ancestors, uses nonblocking open against FIFO paths, locks
per-role output, restricts subscriber targets, skips routine layout/screen
requests, and flushes short-lived CLI commands at normal process exit.
Operation sequences are never labeled as wire request IDs.

Mutation checks used an isolated four-module crate containing current source
copies and root protocol/layout tests. Disabling wire magic validation caused
the structured-header property to fail. Excluding the maximum valid row from
root admission exposed weak uniform-u16 generation; the parent added boundary
biased generation and a deterministic min/max matrix. That matrix fails against
the mutation and passes against unchanged production source. No mutation was
applied to the working source. Throwaway shrink/persistence/replay checks were
reported by the property worker; no artificial failing seed was retained.

## Completed focused checks

- All 44 property tests passed; the strengthened layout target separately 9/9.
- Diagnostics library capture/sink tests passed (filter also included two
  existing history-diagnostics tests: 15/15 total).
- Binary diagnostic tests 4/4; strict all-target Clippy passed.
- Python dependency-inventory helpers 6/6, including a new dev-edge regression.
- Scoped document/link checks and `git diff --check` passed.
- Temporary cargo-audit 0.22.2 installation and scan: exit 0, 40 lock entries,
  zero known vulnerabilities, no warnings. Dated raw JSON is retained in
  [audit evidence](p6-raw/tracing-proptest-audit.json).

## Private runtime observation

A disposable real server, pathname socket, attach client on an outer PTY and
managed `/bin/cat` pane ran with tracing disabled and enabled. Both delivered
eight synthetic inputs, detached successfully and stopped the private server.
The enabled logs contain `exchange`, `client_service`, `host_apply`,
`owner_dispatch` and `pty_accept`; synthetic `CANARY_INPUT`/`TRACE_READY`
contents are absent. Log files are 0600 and below the runtime cap.

[Reproduction script](p6-raw/tracing-private-pty-check.py) and
[raw observations](p6-raw/tracing-private-pty-results.json) are retained. Run the
script from the repository after building `target/debug/panelink`. It creates
only temporary private fixtures. The initial fixture's raw byte matcher missed
cursor-positioned output; ANSI stripping corrected the matcher. A second issue
was session-leader exit waiting on unread terminal output; draining the outer
PTY during client exit corrected the harness without production changes.

These are debug-build, sequential, small-sample capture timings measured while
the canonical suite was running. They include terminal echo, do not prove child
or provider acceptance, and cannot establish an instrumentation overhead budget.
There is no uninstrumented release baseline or stable tail-latency acceptance.
Native host/provider/theme/clipboard gates remain unchanged. Deferred
launch/watch/close completion and exact host-to-wire attribution remain limited;
see [user contract](../DIAGNOSTICS.md).

## Canonical verifier

Serial `RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py` completed with
exit 0 and all eight checks passing: root Rust 1,693 passed / 0 failed / 20
ignored, decoder 39/0, Python 234 run / one opt-in skip, fingerprints, fmt,
strict Clippy, build and all 470 Markdown files. Earlier Python listener errors
did not recur in this run. [Full raw log](p6-raw/tracing-proptest-canonical.log).

Separate post-gate CLI probes passed: JSON output remains parseable with no
stderr contamination; short-command shutdown summary is written; a synthetic
secret environment value is absent; a FIFO log path is refused without hanging;
a competing process holding the role lock disables tracing; a wide-permission
directory is refused. All probes used temporary roots and no user workspace.

The optimized release build succeeded in a separate cold target directory:
13.30 seconds wall time in this single run (warm registry, no prior artifacts in
that directory). Release and baseline observations follow below. This is not a
stable performance or reproducibility claim.


## Release observations

The baseline revision and instrumented source both built successfully offline in
separate fresh target directories. One baseline build took 11.84 seconds; one
instrumented build took 13.30 seconds. Binary sizes were 3,951,552 and 4,277,408
bytes respectively. The archive baseline has no Git metadata, so its embedded
revision differs. These single-run observations do not isolate dependency
compile cost or establish repeatable budgets. See
[raw size/build comparison](p6-raw/tracing-release-build-comparison.json) and
[instrumented build log](p6-raw/tracing-release-build.log).

The same private server/attach/PTY probe also passed using the optimized
instrumented binary, with tracing disabled and enabled; the assertions for
input, detach, stage presence, metadata-only logs and private capped files all
passed. [Release observations](p6-raw/tracing-private-pty-release-results.json)
remain eight-input samples, not a stable tail-latency or native display claim.
No instrumented-release versus uninstrumented-release runtime comparison was
performed. Defaults stay off and performance/native acceptance stays open.
