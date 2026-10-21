# Live history sustained-output measurement task — 2026-10-21

Status: scoped measurement slice delivered and parent-verified; actual results
and limitations are in [P5-LIVE-HISTORY-BENCH.md](P5-LIVE-HISTORY-BENCH.md).
Parent owns contract/review/independent verification/shared docs; one explicitly
requested GPT-6-Luna xhigh owner implements the bounded harness. Preserve the
accumulated uncommitted work. Phases 3/5/6 acceptance remains open.

## Scope and ownership

Luna may edit ONLY `examples/live_history_bench.rs`,
`docs/progress/P5-LIVE-HISTORY-BENCH.md` and
`docs/progress/p5-live-history-bench.json`. Parent owns the report validator,
its focused tests, a second final raw report, preserved preliminary stdout and
shared planning/spec documentation.
No runtime, manifests, vendor changes or commits. If a runtime defect blocks
measurement, report it before expanding ownership; do not mask it in results.
Use existing public driver/startup/protocol/history APIs. No production test
switches, new telemetry surface, artificial slow backend or disabled syncs.

FIRST read ROADMAP.md, SESSION.md, AGENTS.md, README.md, docs/SPEC.md,
docs/PREBUILD.md, docs/PANELINK-DISCOVERY.md and P5-LIVE-WRITER-RESULT.md.
Graph tools first; if Transport closed, fall back to source without retry loops.
Match existing example style and private fixture/cleanup conventions.

## Harness

Create a standalone release example, `cargo run --locked --offline --release
--example live_history_bench`, printing exactly one JSON object to stdout.
It uses only its own uniquely created mode 0700 canonical temporary directory;
no user history/config/provider/host TTY or pathname listener. Actual
`MultiServerDriver::enable_history` and `HistoryStore` must be used. Create the
second child through authenticated UnixStream::pair schema 7 split, proving the
same live admission path. Use a Down split from a 49-row root to obtain two
24-row panes at the configured 80/240 columns. Explicit cwd/environment/terminfo
are private; basic synthetic children do not need a host terminfo database.

The example may invoke itself in a strictly internal child mode to generate
bounded known ASCII output and read synthetic input (no shell interpolation).
Use ready/start/finish handshakes so load starts only after both panes exist.
Own all children through the driver; bounded cleanup must also occur on error.
Wait for the retained storage lock after a timed-out driver drop BEFORE deleting
fixtures, since a detached writer may still write. Failure to settle is an error,
not permission to remove storage below the worker. No unbounded threads/sleeps.

Fixed cases, <=30 seconds each, <=100000 owner samples per case:

- `paced_80`: two 24x80 panes,768 output rows per pane, batches of 8 with a 20 ms delay;
  normal exit after probes and completed generation.
- `burst_240`: two 24x240 panes,20000 output rows per pane, batches of 64 with no
  artificial delay; normal exit after probes and completed generation.
- `shutdown_240`: two 24x240 panes,100000 planned rows each, batches of 32 with a 1 ms
  delay; drop the live driver after at least 500 ms of active load and completed
  probes, while generation is still pending. Record if load ended too early.

At least 8 input+control probes per pane. Use authenticated socketpair clients,
real input lease/owner revision and monotonically unique operation keys; do not
replay input after uncertain replies. Time request/reply separately from child
acknowledgment. A child-written private ack counter/file is acceptable since
screen text can be overwritten; do not claim transport acceptance proves child
processing. Keep readiness and parent validation/file reads outside timed owner
`tick` calls. Identify probes that began before producer completion (and whether
completion was already observed); do not call idle-only probes load evidence.
Use ordinary capture cadence during load, not a force capture on every tick.

Measure owner `tick` durations, layout/control RTTs, input reply RTTs, actual
child acknowledgment RTTs, driver Drop duration and post-Drop lock settlement.
Driver Drop includes child stop/reap; it is NOT a measurement of history-only
250 ms drain. Preserve each timing sample so percentiles can be recomputed.
No numeric release/performance pass threshold is selected by this task.

After driver drop and writer settlement, reopen actual saved rows/checkpoints
under maintenance exclusion. Validate codecs, target isolation, <=1024 rows and
<=8MiB row segment. Report row count, min/max ordinal, gap, ended marker,
checkpoint presence and sticky failure. Distinguish planned producer rows from
completed rows; `gap` may reflect intentional retention, ring overflow or
shutdown, so it alone does not establish queue saturation. Internal queue
occupancy is not exposed by the current public API; report that limitation
rather than adding production instrumentation or inventing queue measurements.

## JSON contract

Top level: `schema_version:1`, `synthetic:true`, `actual_history_store:true`,
`queue_occupancy_instrumented:false`, `build_profile` (`debug`/`release`),
`cases` array in the fixed order above. No paths/terminal contents/input values/
credentials/environment values in the report. Timing units are microseconds.

Every case includes `name`, `columns`, `rows_per_pane:24`, `panes:2`,
`planned_rows_per_pane`, `elapsed_us`, `owner_tick_us` (nonempty bounded integer
array), `probes` (>=16 records,8/pane), `driver_drop_us`,
`post_drop_lock_wait_us`, `history_failure` boolean, `shutdown_during_output`
boolean, and `targets` (two records). Each probe: `pane` (0/1),
`control_rtt_us`, `input_reply_us`, `child_ack_us`, `started_during_output`
boolean, `ack_before_output_done` boolean. Ack durations begin immediately
before input submission and end on the child-confirmed observation. A timeout
or non-Applied response is an honest harness error, never a zero duration/pass.

Each target: `pane`, `retained_rows`, `segment_bytes`, `first_ordinal` and
`last_ordinal` (null if empty), `gap`, `ended`, `normal_checkpoint`,
`alternate_checkpoint`, `producer_completed` boolean, `completed_rows`
(nonnegative integer if completed, null if interrupted/unknown). Include only
sanitized numeric/boolean data; target IDs need not be published. Optional
extra aggregate percentiles are allowed only if consistent with raw samples.
A sticky history failure is observed data, not by itself a benchmark harness
failure; a corrupt/misattributed/oversize record is a harness failure. State
limits prominently: synthetic two-pane observations, not ten-agent, host UI,
physical power loss, release budget or complete Phase5/6 acceptance.

## Required verification/report

Run the release example once, save exact single-JSON stdout to the owned raw
artifact; no hand-edited measurements. Record command, actual exit/result,
case workloads/methodology, observed timings/gaps/incomplete results and any
failed attempts/fixes in the owned Markdown note. Parent runs the same binary
independently, validates both reports, and runs repository checks. Owner may
use `cargo check --locked --offline --example live_history_bench` and format
ONLY its new Rust file; parent runs all-target strict Clippy/fmt and full
verifier after file release. Do not duplicate broad suites. No workers commit;
parent shared roadmap edits are the exclusive-file exception to the skill's
append-only worker roadmap rule. Paste actual focused output in the final
report and release ownership when source/report are stable.
