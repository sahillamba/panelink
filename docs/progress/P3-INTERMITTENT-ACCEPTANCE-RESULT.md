# Phase 3 intermittent acceptance stabilization — 2026-10-23

## Findings and changes

The history-writer failure was a test assumption around permitted admission
contention. `try_submit` deliberately returns immediately when its bounded
accounting mutex is busy, reports `CapacityReason::Contended`, and returns the
unchanged batch. The worker briefly takes that mutex during startup. The panic
acceptance now retries only that exact reason, preserves the returned batch and
token, yields between attempts, and fails after a one-second bound. Count/byte
capacity, stopped and failed writers, stale tokens, and all other errors still
fail immediately. Production admission behavior is unchanged.

The ACK42 fixture observed pane 2's server layout, owner and prompt before the
attach client had necessarily completed its local focus transition. `push_key`
pins input to the client's current focus. When the split result preceded the
layout, the client later focused pane 2 but left its status at “waiting for
layout.” The client now reports the same positive focus status when the newer
layout completes that pending focus. The fixture waits for that exact visible
status before typing ACK42. Input routing and split policy are unchanged.

An initial barrier-before-submit experiment for the history test was reverted:
the fake backend's `Load` call is reached only after its work item is admitted,
so that barrier cannot establish worker readiness before submission.

## Verification

- History-writer panic acceptance: **20/20** isolated serial runs in **1.57s**
  total.
- Split completion after a delayed layout: **1/1** binary unit test.
- ACK42 attach-host acceptance: **1/1** in **2.97s**, then **10/10** serial
  repetitions in **25.65s**.
- Complete locked/offline serial Cargo suite with `--no-fail-fast` and
  `--test-threads=1`: **1089 passed, 0 failed, 15 ignored** in **179.27s**
  (library 712, binary 142, integration 235 across 53 targets).
- All seven supporting checks passed in the permitted environment: decoder
  39, component fingerprints, formatting, strict all-target Clippy, build,
  Python 77, and docs 272/0.

The first restricted supporting-check attempt could not resolve cached offline
`bitflags`/`libc` packages and could not bind the Python loopback fixture. In
the permitted environment, all seven checks passed. The earlier intermittent
failures remain preserved in `P3-OWNER-LIFETIME-RESULT.md` and
`P3-LIVE-SCROLLBACK-RESULT.md`; repeated passing runs do not establish a timing
guarantee. No Cargo deadline expired.

The diagnostics were independently audited by two GPT-6-Luna High read-only
tasks; the parent made and reviewed the changes. No cost telemetry is available.
No dependency, wire/storage format or input retry policy changed. Full Phase 3
remains open. Human Apple Terminal four-pane, provider/reboot, owner-pending and
death-before-admission limits remain distinct.

Raw logs are local under `/private/tmp/panelink-p3-history-admission-fixed-20x.log`,
`/private/tmp/panelink-p3-ack42-focus-fixed-10x.log`,
`/private/tmp/panelink-p3-flake-fixes-full.log` and
`/private/tmp/panelink-p3-flake-fixes-noncargo.log`.
