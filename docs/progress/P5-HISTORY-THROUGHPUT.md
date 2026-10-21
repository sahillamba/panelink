# P5 history throughput observation — 2026-10-21

This is the earlier synchronous storage baseline. Subsequent
[live two-pane observations](P5-LIVE-HISTORY-BENCH.md) measure the integrated
background scheduling path with a different workload; the numbers do not form
a controlled before/after latency comparison.

The new [`history_store_bench` example](../../examples/history_store_bench.rs)
measured the existing bounded history writer with synthetic terminal rows and
screen checkpoints. Its exact one-line JSON stdout is saved in
[`p5-history-throughput.json`](p5-history-throughput.json). The run completed
on macOS 26.5.2 (`Darwin 25.5.0`, arm64) with Rust/Cargo 1.99.1. The specific
Mac model was not recorded.

Run command:

```sh
cargo run --locked --offline --release --example history_store_bench
```

Each case used a unique target and private mode-0700 canonical temporary
directory. The example removed the fixture before printing the report. It
does not accept a path, read user history, start a PTY or child process, or
print row contents, fixture paths, usernames, or environment values.

For the three scrollback cases, the store was seeded with 1,024 records before
timing. The example prepared all VT100 rows and encoded the unique increasing
ordinals before timing. Its VT100 history ring was also capped at 1,024 rows;
it drained the seed and each measured batch before feeding the next chunk. It
then measured 24 `HistoryStore::append_rows` calls.
Each timed append includes store verification, reading and validating the
current segment, encoding the complete bounded segment, writing and syncing
the temporary file, `F_FULLFSYNC`, rename, and directory sync. The incoming
row encoding and seed writes are excluded. After each operation, file metadata
was read outside the timed interval to sum the exact complete segment sizes
rewritten. `new_input_record_bytes` is the input row-record payload; it does
not include per-record framing or the retained rows rewritten on every call.

The checkpoint case saved a real default blank `ScreenSnapshot` at 24x80.
Sequence and screen revision advanced from 1 through 24. Its timed operation
includes metadata construction, checkpoint encoding, `HistoryStore::save`,
reading and validating the prior checkpoint, and the same file and directory
durability operations. Percentiles use nearest-rank over the 24 elapsed-time
samples. Totals are the sum of those timed calls; metadata reads used to count
rewritten bytes are outside the samples.

| Case | New rows per call | New rows / input bytes | p50 / p95 / max per call | Timed total | Ingress | Rewritten storage bytes | Final file / retained rows |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 80 columns, batch 1 | 1 | 24 / 94,080 B | 25,038 / 45,497 / 47,514 µs | 657,743 µs | 36.488 rows/s | 96,437,952 B | 4,018,248 B / 1,024 |
| 80 columns, batch 32 | 32 | 768 / 3,010,560 B | 22,034 / 62,681 / 105,229 µs | 698,306 µs | 1,099.804 rows/s | 96,437,952 B | 4,018,248 B / 1,024 |
| 240 columns, batch 32, byte eviction | 32 | 768 / 8,908,800 B | 40,046 / 60,287 / 139,109 µs | 1,121,983 µs | 684.502 rows/s | 201,075,840 B | 8,378,160 B / 722 |
| 24x80 blank checkpoint | — | — | 9,017 / 10,017 / 10,233 µs | 217,246 µs | 110.474 saves/s | 2,214,144 B | 92,256 B / — |

Each case has 24 samples. The JSON artifact also retains each per-call
microsecond sample, p50, p95, maximum, total, and final row ordinals. The
normal cases ended at ordinal 1,048 and 1,792 respectively. The wide case
ended at ordinal 1,792, retained ordinals 1,071–1,792, and reported a gap.
Its 8,378,160-byte final segment is 10,448 bytes below the 8 MiB limit; only
722 rows remained, confirming that byte eviction bound this run before the
1,024-row limit.

The append API is synchronous and writes the complete retained segment for
each batch. A runtime that calls it inline on an owner thread will keep that
thread inside the storage call for the measured duration. Median append
storage-call time was about 22–40 ms, p95 was about 45–63 ms, and the slowest
sample was 139 ms. The 32-row cases delivered more rows per second by
amortizing each complete segment rewrite across more new rows; both 80-column
cases rewrote the same 96,437,952 bytes across their 24 calls. These results
make synchronous owner-thread wait a scheduling concern to assess in the
integrated runtime. They measure storage-call time only, not event-loop
contention, PTY handling, user-visible UI latency, ten-agent performance,
missed deadlines, or an acceptable release budget. Checkpoint timings include
the codec and full storage path for the fixed blank snapshot.

This is one synthetic storage observation on one machine. It is not a ten-agent
workload, UI latency proof, release-budget pass, or representative-hardware
claim. The OS cache and storage state were not controlled; no OS tuning or
global cache dropping was used. The row codec and row preparation were outside
the append timer, while complete segment encoding and durability syncs were
inside it. The checkpoint codec was inside its timer. No numeric acceptance
threshold was applied.

Verification recorded for this change:

- `cargo run --locked --offline --release --example history_store_bench` —
  exit 0; emitted one valid JSON object plus its trailing newline.
- `cargo fmt --check` — exit 0.
- `cargo clippy --locked --offline --example history_store_bench -- -D warnings` —
  final exit 0. An earlier concurrent run exited 101 on an unowned
  `src/history_store.rs` `clippy::question_mark` finding; that shared-source
  lint was resolved before the final check.

Parent review and an independent release rerun passed. Its exact stdout is
saved in [p5-history-throughput-parent.json](p5-history-throughput-parent.json).
The parent checked both artifacts' sample counts, nearest-rank percentiles,
totals, final ordinals, ring bound, rewritten bytes and segment quota. The
first validator referenced the ring field at the wrong JSON level; correcting
it to the per-case `vt100_preparation_ring_rows` field resolved that check
without changing the benchmark. Both example runs exited 0.

The independent run's append p50 values were 25.770 ms (80x1), 25.921 ms
(80x32) and 48.934 ms (240x32); the checkpoint p50 was 8.010 ms. Corresponding
p95 values were 75.934, 30.821, 68.460 and 13.903 ms. That rerun overlapped
component verification, and neither run controlled competing I/O or cache
state. The variation reinforces that these are observations rather than a
stable latency budget. Final integrated strict Clippy passed.

This evidence does not close Phase 5 reboot/layout, committed-record corruption
repair or representative workload gates. Interrupted temporary-file discard
is delivered separately in [P5-TEMP-CLEANUP.md](P5-TEMP-CLEANUP.md).
