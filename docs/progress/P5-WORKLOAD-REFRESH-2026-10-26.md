# Live history workload refresh — 2026-10-26

## Harness correction

The existing release benchmark stopped before producing a report because its
49-row root and direct pane-rectangle checks predated the one-cell padding
on every pane edge. A 24-row outer pane now has only 22 content rows.
The harness now allocates a 53-row root: two 26-row outer panes plus the
divider, giving each child the intended 24-row PTY.

The narrow case retains 80 content columns with an 82-column root. The
supported root width limit is 240, so the wide cases use 238 content columns
inside a 240-column root. The harness checks the production content rectangle
and independently decodes actual live screen checkpoints at 24x80 or 24x238.
No application geometry, quotas, failure policy, or timeout changed.

Schema **3** identifies the new fixed `paced_80`, `burst_238`, and
`shutdown_238` profile. Schemas 1/2 retain their exact older 240-column case
requirements. The validator rejects a profile/schema mismatch and keeps
diagnostics mandatory for schemas 2/3. Historical measurements are preserved;
new wide results are not presented as the identical workload.

## Current release observation

Raw stdout: [p5-live-history-2026-10-26.json](p5-live-history-2026-10-26.json).
Host: macOS 26.5.2 build 25F84, arm64; pinned Rust 1.99.1. Source base
`1060668`, with the harness/validator changes in this acceptance branch.
Scheduler and background load were not controlled. This is one observation.

All timings below are microseconds. Percentiles use nearest rank on the raw
arrays. Each case has 16 probes, eight per pane; all 48 started during output
and were acknowledged before observed producer completion.

| Case | Owner tick p95 | Control reply p95 | Child acknowledgment p95 | Driver Drop | Sticky history failure |
| --- | ---: | ---: | ---: | ---: | --- |
| paced_80 | 205 | 2,588 | 3,000 | 95,728 | false |
| burst_238 | 310 | 2,555 | 7,191 | 182,516 | true |
| shutdown_238 | 338 | 2,580 | 6,315 | 220,831 | true |

The paced case retained 754/746 rows, no gap, both end markers and no failure
reason. Both wide cases retained 728 rows per pane, reported gaps and end
markers, and recorded `admission_bytes_full` plus
`final_payload_dropped_bytes`. All writers reported `drained`; draining the
queue does not recover the rejected final payload. Active-shutdown producers
remained incomplete and their completed-row counts remain null. Quota and
final-loss observations are retained as failures, not silently accepted.

These socketpair/child-counter measurements do not establish native host
input latency, ten active agents, physical reboot durability, or a numeric
performance budget. A representative combined workload and agreed
loss/lag/error/latency limits remain open.

## Verification and failed attempts

Command:

```sh
CARGO_TARGET_DIR=/private/tmp/panelink-phase-acceptance-target \
  cargo run --locked --offline --release --example live_history_bench
python3 scripts/validate_live_history_report.py \
  docs/progress/p5-live-history-2026-10-26.json
python3 -m unittest discover -s tests -p test_live_history_report.py
```

The final release run exited 0; report validation passed; 15 Python validator
tests passed, including historical reports and strict profile/schema checks.
The new artifact is also included in the recorded-report regression.
Strict all-target Clippy passed. Full repository verification is recorded in
[the continuation note](P3-P5-RESUME-2026-10-26.md).

Three preceding attempts failed before a completed report: the old root
geometry, a remaining split-target outer/content check, and an attempted
242-column root exceeding the supported 240-column limit. Logs are
`/private/tmp/panelink-live-history-2026-10-26.log`, `-r2.log`, `-r3.log`;
the completed run is `-r4.log`. No failure was converted to a pass or hidden
by increasing runtime bounds.
