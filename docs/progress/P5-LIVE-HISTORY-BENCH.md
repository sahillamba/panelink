# P5 live history sustained-output observations

Status: scoped measurement slice delivered and parent-verified on 2026-10-21.
The implementation owner and parent each completed a release run. Exact stdout
is preserved in [the owner report](p5-live-history-bench.json) and
[the independent parent report](p5-live-history-bench-parent.json). The task
selected no numeric performance threshold; Phases 3/5/6 remain open.

The run used Rust 1.99.1 on macOS 26.5.2, arm64. The sandbox denied `sysctl`
queries for the host model, CPU details, and memory, so those are unknown.

## Method

Each case created a unique private mode-0700 temporary fixture with explicit
private cwd, `PATH`, `HOME`, and terminfo. A public
`StartedMultiServer`/`MultiServerDriver` started the first child at 49 rows;
two authenticated `UnixStream::pair` clients claimed input leases, and the
first client admitted the second child using a schema-7 Down split. The live
layout was checked as two 24-row panes at 80 or 240 columns. Both synthetic
children wrote readiness markers and waited on one shared start gate. Before
opening that gate, authenticated ScreenQuery responses were decoded for both
targets and their actual screen checkpoints were checked for expected 24x80
or 24x240 dimensions and target identity. The live checkpoints were also
checked for the sibling pane's synthetic row prefix. After writer settlement,
every retained saved row and checkpoint was screened for that sibling prefix.

The producers emitted known ASCII rows using the task's fixed row counts,
batch sizes, and delays. The owner used ordinary `tick` cadence; it did not
force capture per tick. Each of eight rounds submitted one control layout query
and one non-replayed `q` plus Enter input per pane, with the two pane probes
interleaved. Each probe submitted one fixed two-key synthetic input. A child
input-reader thread acknowledged each consumed input in a private counter
file. `control_rtt_us` covers the layout request/reply;
`input_reply_us` covers input submission through the server reply;
`child_ack_us` starts immediately before input submission and ends when the
client worker observes the child's counter. The two probe booleans use the
private producer completion markers observed immediately before input and at
acknowledgment. All 16 probes in each case began before producer completion
and were child-acknowledged before completion was observed.

`owner_tick_us` contains only tick calls after the start gate opened and through
the workload stop point. Readiness, socket setup, split admission, saved-file
reads, and validation are outside those tick samples. `elapsed_us` spans the
whole case from after private case-directory setup through driver Drop, lock
settlement, saved-history validation, and fixture cleanup. The raw sample
arrays are retained so percentiles can be recomputed; the table uses nearest
rank p50/p95.

After each driver Drop, the harness timed acquisition of the server maintenance
lock, then reopened the actual `HistoryStore`, decoded saved rows and both
checkpoint types, checked target identity and segment bounds, and removed the
fixture only after releasing the lock. These are post-Drop,
post-lock-settlement disk observations. A writer may finish a queued write
after driver Drop returns and before the lock is acquired. `driver_drop_us`
includes child stop/reap work; it is not a history-only drain measurement.
Queue occupancy is not exposed by the public API and was not instrumented.

## Results

Times are microseconds. Tick samples are direct `driver.tick` durations; each
control, input, and child-ack column summarizes the 16 raw per-probe samples.

| Case | Full case elapsed | Tick n / p50 / p95 / max | Control p50 / p95 / max | Input p50 / p95 / max | Child ack p50 / p95 / max | Driver Drop | Post-Drop lock wait | Sticky history failure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `paced_80` | 2,500,115 | 1,642 / 66 / 164 / 13,470 | 2,471 / 2,614 / 2,614 | 2,424 / 2,710 / 2,710 | 2,630 / 5,272 / 5,272 | 129,590 | 256 | false |
| `burst_240` | 1,780,605 | 822 / 130 / 288 / 10,741 | 2,481 / 2,543 / 2,543 | 1,260 / 2,519 / 2,519 | 3,921 / 15,470 / 15,470 | 250,139 | 5,597 | true |
| `shutdown_240` | 982,481 | 318 / 125 / 321 / 8,521 | 2,524 / 7,668 / 7,668 | 1,257 / 8,866 / 8,866 | 3,821 / 8,936 / 8,936 | 263,995 | 152 | false |

| Case | Pane | Planned producer rows | Completed producer rows | Retained rows / ordinal span | Row segment bytes | Gap | Ended | Normal / alternate checkpoint |
| --- | ---: | ---: | ---: | --- | ---: | --- | --- | --- |
| `paced_80` | 0 | 768 | 768 | 754 / 1–754 | 2,958,768 | false | true | present / absent |
| `paced_80` | 1 | 768 | 768 | 746 / 1–746 | 2,927,376 | false | true | present / absent |
| `burst_240` | 0 | 20,000 | 20,000 | 722 / 18,128–18,849 | 8,378,160 | true | true | present / absent |
| `burst_240` | 1 | 20,000 | 20,000 | 722 / 19,257–19,978 | 8,378,160 | true | true | present / absent |
| `shutdown_240` | 0 | 100,000 | incomplete | 722 / 6,775–7,496 | 8,378,160 | true | true | present / absent |
| `shutdown_240` | 1 | 100,000 | incomplete | 722 / 6,899–7,620 | 8,378,160 | true | true | present / absent |

Both shutdown producers were still pending when the driver was dropped after
the required active-load interval and completed probes. Their saved rows and
checkpoints are the state visible after the writer lock settled; their
`completed_rows` values are null. In this run, shutdown recorded
`shutdown_during_output=true` and no sticky history failure. The burst
producers completed; the case-level sticky failure was true, while both panes
had ended markers in the post-settlement store. The burst and shutdown row
segments each approached the 8 MiB cap and reported a gap. A gap can reflect
retention, overflow, or shutdown; these observations do not measure queue
occupancy or establish queue saturation.

The measurements cover three synthetic two-pane workloads on this host. They
do not establish ten-agent behavior, host UI latency, power-loss durability,
a release budget, or complete Phase 5/6 acceptance.

## Verification and attempt notes

- `rustfmt --edition 2021 examples/live_history_bench.rs` and
  `cargo check --locked --offline --example live_history_bench` passed for the
  final source.
- `cargo run --locked --offline --release --example live_history_bench` exited
  0 for the final recorded report; stdout was redirected directly to the raw
  JSON artifact without post-processing. Setup-time authenticated screen
  queries verified actual 24x80 and 24x240 checkpoints, and the
  post-settlement saved-row/checkpoint scans found no sibling pane synthetic
  prefixes.
- The first targeted compile found an ownership error and unused items; those
  were fixed before the check passed.
- The first release invocation failed before workload start because the
  harness created `server/history` before `ServerStorage::initialize`, which
  requires an empty server directory. The harness now creates the history
  directory immediately after server storage initialization. That failed run
  emitted no benchmark JSON.
- The layout query was corrected to expect the runtime's Response frame. Marker
  writes use atomic rename without syncing volatile files; the public history
  store retains its own persistence behavior.

The implementation owner changed only the standalone harness and its two
report files. Parent additions, review and verification follow below.

## Independent parent results

The parent ran the same three cases sequentially after the implementation
owner released the files. Initial targeted strict Clippy found four style
errors (two byte-array spellings, one redundant error match and one redundant
binding); parent cleanup preserved behavior, and the strict example check then
passed. The independent release command exited 0 and its stdout was saved
without editing measurement values. Both final artifacts pass
`python3 scripts/validate_live_history_report.py` with exact field sets,
count/ordinal/size consistency, release mode and bounded samples. The parent
independently recomputed all timing summaries from raw arrays.

The table below contains the independent parent run, in microseconds:

| Case | Tick n / p50 / p95 / max | Control p95 | Input reply p95 | Child ack p95 | Driver Drop | Post-Drop lock wait | Sticky history failure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `paced_80` | 1,649 / 86 / 228 / 659 | 2,561 | 2,536 | 4,138 | 102,299 | 228 | false |
| `burst_240` | 815 / 122 / 331 / 11,918 | 2,530 | 6,302 | 10,214 | 192,845 | 173 | true |
| `shutdown_240` | 315 / 123 / 263 / 7,873 | 2,519 | 5,016 | 5,086 | 207,626 | 137 | false |

Each case has 16 probes, so its nearest-rank p95 is also its largest probe
sample. All 96 probes across the two final runs began and were acknowledged
before observed producer completion, eight per pane per case. These are
socketpair and child-counter observations, including worker scheduling and
1 ms acknowledgment polling; they do not measure a human host input/render path.
The owner tick sample window includes the wait between paced batches and the
normal-exit tail, rather than only turns that read output.

The final parent paced case retained 754/746 rows, with no gap/failure and both
end markers. Each wide target retained 722 rows in an 8,378,160-byte segment,
with gap=true. Both final parent wide cases had end markers; burst raised the
sticky failure flag, while active shutdown did not. Normal checkpoints were
present and alternate checkpoints absent for every final target. Both shutdown
producers remained incomplete, with completed row counts honestly null.
Producer lines include viewport output and terminal echo; subtracting saved
normal-row counts from planned producer lines does not measure capture loss.
Retained tail positions differ between runs even at the same row/byte cap.

## Preliminary variability and remaining investigation

A [preliminary parent report](p5-live-history-bench-preliminary.json) is retained
as exact stdout from the earlier harness, before explicit live-screen geometry
and sibling-content validation were added. It passes structural validation but
is superseded by the two final runs for workload acceptance. That preliminary
run reported both wide cases with sticky failure=true and both end markers
absent after lock settlement. Its burst Drop took 250,476 us, followed by 25,131 us
of lock waiting; shutdown Drop took 291,792 us, followed by 1,769 us of lock waiting.
The two final runs had all end markers present, with burst failure flags still
true. No application behavior changed between these observations. Their
variability is preserved rather than selecting only the favorable outcome.

The current public signal cannot distinguish admission loss, recovered storage
errors, retry exhaustion and drain cancellation. A roughly 250 ms total Drop in one case
suggests investigation of the drain boundary, but the data do not identify a
cause or isolate history wait from child cleanup. Likewise a gap does not
prove queue saturation. End markers and gap/failure flags must be interpreted
together; stored end metadata does not acknowledge every terminal observation.
Missing end markers can also delay automatic ended-history expiry.

Next bounded work: diagnose the burst failure flag and variable final tail/end
results with bounded, sanitized failure/drain reasons, then review scheduling
or loss-policy changes against these same workloads. Do not increase memory
limits, extend the history wait or change failure guarantees without reviewing
the contract. Committed-record corruption, full reboot/layout, host/provider
and representative workload gates remain separate.

## Parent repository checks and provenance

| Check | Actual result |
| --- | --- |
| Final release measurements | Two completed runs, three cases each; both reports validate. |
| Preliminary stdout | Preserved separately and structurally validated; additional final geometry/content checks were not yet present. |
| New report-validation tests | 9 passed, included in 40 Python passes. |
| `python3 scripts/verify_non_ui.py` | 7/8, exit1. |
| Root library / default binary | 531 passed/4 ignored; 92 passed/2 failed. |
| Binary serialized rerun | 93 passed/1 failed; socket EPERM remains. |
| Isolated lock-probe retry | 1 passed; default parallel failure remains recorded. |
| Actual-store live integration rerun | 3 passed (`p5_live_history`1 and `p5_live_history_driver`2). |
| Strict all-target Clippy, formatting, offline build | Passed in the verifier. |
| Decoder / Python / component fingerprints | 39 / 40 passed; fingerprints match. |
| Documentation | 162 Markdown files, 0 findings; final docs checked again separately. |
| Application/manifest fingerprints | All 63 recorded files unchanged in this measurement slice. |

The default binary run failed `endpoint_state_classifies_files_and_sockets`
with OS code1/PermissionDenied and also failed the post-drop assertion in
`lock_probe_detects_a_held_storage_lock`. The lock probe passed in isolation
and in the serialized full binary suite; its parallel failure cause is not
established and is not counted as a pass. The serialized run retains the
socket failure. The full 28-target integration sweep from the previous slice
was not repeated because application sources are unchanged; its older
76 passed/17 failed/11 ignored result remains historical. No host/provider/reboot
acceptance is inferred.

Commands: `cargo run --locked --offline --release --example live_history_bench`;
`python3 scripts/validate_live_history_report.py` with all three report paths;
`python3 scripts/verify_non_ui.py`;
`cargo test --locked --offline --bin panelink lock_probe_detects_a_held_storage_lock -- --test-threads=1`;
`cargo test --locked --offline --bin panelink -- --test-threads=1`;
`cargo test --locked --offline --test p5_live_history --test p5_live_history_driver -- --test-threads=1`.
Temporary check logs are under `/private/tmp/panelink-live-bench-*`; this note
and the raw reports are the durable evidence. The environment was rechecked
as macOS 26.5.2 build 25F84, arm64 and Rust 1.99.1
(`58cf8f9c3`, 2026-10-13). Cache/scheduler/background load were not controlled;
the two runs are observations, not a statistically stable benchmark budget.

All ownership is released. No runtime, manifest, vendor, commit or push change;
prior uncommitted work is preserved. The graph connector returned Transport
closed; source/tests are authoritative, with no refreshed-index claim.

Final source fingerprints (parent run; owner run precedes lint-only cleanup):

| File | SHA256 |
| --- | --- |
| `examples/live_history_bench.rs` | `4c483571a5f48696829ecc8659d8fd077656437866765a1a83a6388adea728f7` |
| `src/history_capture.rs` | `8720b4b12ddb1d8b524378482dc293ce40a275d5bfc6d84cb53ce855229b7658` |
| `src/history_writer.rs` | `5a5e59711a5712232c4178970f74d29bec0ff6a6da5158de64f067ec85c4b0e3` |
| `src/multi_runtime.rs` | `932e6b3b1c6784e461988ecf99cf04c18d742f2a0887859ebc583235aa60464d` |
| `src/server_pty.rs` | `b444101daf51d86ae244e990c8b11f25ce4654fd3e86bd8b68f5799b7f33db39` |
| `Cargo.lock` | `7d281bb10eec384b2632f0d8118288051093d25be9f958b64aa05a33a9975b16` |

Git base is `0fec6a43e3b81ecc4d75550b0be1c75f5bd47c50`; this is a dirty working-tree observation, not a committed revision.
