# Phase 3 managed-child metadata and observer renewal

Status: scoped implementation, parent review and verification complete, 2026-10-23. The user authorized a checkpoint commit; no push.

This slice extends the already-reviewed workspace layout-events work on
`codex/p3-integration-layout-events`. It does not integrate the separate
live-scrollback work in `/Users/sahillamba/panelink` or complete Phase 3.

## Delivered behavior

`pane list` now samples the current direct managed child's cwd and executable
path (the existing `command` field contains no arguments). JSON includes the
managed_child source, sample-start time relative to the server generation,
and independent field availability reasons. Shell `cd` and direct `exec` are
observable on refresh. A foreground descendant does not become the reported
managed child. Paths never fall back to stale launch data and are never
persisted in launch/layout/history metadata or diagnostics.

The separate schema12/capability16 read query binds the exact five-part target
and grant. A dedicated client context avoids widening an older grant. Paths
are bounded absolute UTF-8 with no NUL/control characters, no lossy conversion
or truncation. Native libproc calls use reviewed SDK-derived fixed buffers;
PID plus captured birth identity is compared before/after sampling. Publication
revalidates authority, full target and live captured identity. No dependencies
were added. See [accepted contract](P3-PANE-METADATA-TASK.md),
[codec result](P3-PANE-METADATA-CODEC-RESULT.md),
[native boundary](P3-PROCESS-METADATA-RESULT.md), and
[runtime result](P3-PANE-METADATA-RUNTIME-RESULT.md).

One capacity-one native worker and one driver request keep libproc off the PTY
owner. A500ms deadline returns timed_out; the driver remains busy until late
native work is drained. No kernel cancellation is claimed. Failed capture,
process exit and unavailable/unsafe paths produce explicit absent fields.

The workspace observer proactively resumes the same lease at25 minutes,
requesting Read plus workspace access only. It retains its cursor and original
grant target when that pane retires. Transport-only renewal retries are bounded
at3 attempts with15s spacing; expiry, generation change and invalid grants
fail closed. It never replaces the lease or replays mutations. See
[renewal result](P3-OBSERVER-RENEWAL-RESULT.md).

## Parent review and verification

The parent corrected a codec malformed-length test, metadata-only handshake
capabilities, unavailable identity classification, capability advertisement,
late-result deadline ordering, platform guards, spawn-gate scope, errno mapping,
and strict native path count/terminator checks. Incidental vendor formatting
was restored; no vendor source changes belong to this slice.

Initial real CLI metadata acceptance failed because capability16 was not offered;
after fixing negotiation the complete fixture passed. It checks initial cwd/
executable, a foreground sleep leaving zsh identity intact, shell cd, direct
exec, argument/environment sentinel exclusion from query JSON, and child close.
Codec5/5, sampler10/10 and renewal client4/4 focused checks pass. Intermediate
worker compile errors while APIs were being integrated were corrected, not
counted as successful executions. The gated runtime fixture also passes with actual Applied input and watch/read
progress while native sampling is blocked. Parent review is recorded in
[P3-PANE-METADATA-REVIEW.md](P3-PANE-METADATA-REVIEW.md).

Final full serial Cargo: **1,059 passed / 0 failed / 15 ignored** — library709,
binary123, integration227 across50 targets, plus empty doctests. Actual elapsed
152.92s. Command: `/usr/bin/time -p cargo test --locked --offline -- --test-threads=1`
in the permitted private-fixture context. Decoder39, Python77, strict all-target
Clippy, fmt, build, component fingerprints and docs checks pass separately.
The final seven non-Cargo checks were run using the existing verifier's command
inventory with only cargo_test omitted because the complete serial Cargo run
was already separately logged. Docs subsequently rechecked251 files/0 findings
after this final evidence update.

The initial default-parallel standard verifier was **7/8**, not8/8. Its Cargo
command stopped at the library:707passed/2failed/4ignored. The existing
history-writer `exhausted_stage_fails_writer_closed_without_later_effects` test
hit timing admission Full; the new descendant metadata test sampled before the
expected process observation was ready. The latter now waits within a2s bound;
the full serial run passes both. An earlier sampler channel test also raced a
capacity-one completion queue; the parent made the test drain completion1 before
releasing call2, matching the production one-in-flight invariant. These failures
remain recorded, not relabeled as successful runs. No Cargo deadline expired in
this worktree's final run; the separate checkout's180s failure is separate.

Final logs: `/private/tmp/panelink-metadata-final-cargo-serial.log` and
`/private/tmp/panelink-metadata-final-noncargo.log`. Initial failed verifier:
`/private/tmp/panelink-metadata-final-verifier.log`. Temporary logs are supporting
local evidence; the durable counts/commands and limits are recorded here.

## Evidence boundaries and next work

The native identity-mismatch case simulates a changed birth token; it is not
actual kernel PID-reuse injection. Private automated PTYs and authenticated
sockets do not establish human Apple Terminal acceptance, physical reboot,
provider session identity or active-agent load. Foreground-job metadata and
non-UTF-8 path encodings remain outside this contract.

Next: review and integrate the separate live-scrollback slice, preserving its
recorded verifier deadline/attach timing failures; do not claim it is already
present here. Managed `pane run`, richer owner identity and wider host/lifetime
contracts remain open, as do Phase4 identity and broader Phase5 recovery.
