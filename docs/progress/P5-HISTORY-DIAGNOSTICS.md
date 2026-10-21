# Bounded history failure/drain diagnostics

Delivered as a scoped observability slice on 2026-10-21. One GPT-6-Luna xhigh
owner implemented the writer/coordinator/PTY/driver changes and declared
production sections frozen, then hit its usage limit before finishing tests
and this report. Parent completed attribution tests, public API/CLI/benchmark
integration, source review, both measurements and verification. Prior dirty
work is preserved; no dependency, storage-format, commit or push change.
Phases 3/5/6 remain open. The [task contract](P5-HISTORY-DIAGNOSTICS-TASK.md)
defines the fixed labels and attribution rules.

## Behavior

`HistoryDiagnostics` stores nineteen fixed sticky reason bits and one actual
writer drain outcome in an Arc of fixed-width atomics. Recording neither
allocates diagnostic records nor takes a diagnostic mutex. No payloads, target
identity, paths, inputs, environment values or error text enter these records.
A handle obtained before enabling history observes setup failure, survives
driver Drop, and retains late worker events until settlement. Concurrent
snapshots are nontransactional; explicit timeout remains timeout after a late
commit. Plain writer Drop leaves `not_requested` and records only actual
canceled work.

Admission still returns `SubmitReason::Full` with unchanged batch ownership
and token semantics. Its additional `capacity_reason()` distinguishes the
actual contention, count or byte check. Ordinary rejection can coexist with
`history_failure=false`; discarded final payload records its paired rejection
cause and retains the existing sticky failure. Deadline observations distinguish
pending accepted receipts from end intent that was never submitted. A drained
writer queue does not certify complete capture.

The CLI preserves the serving failure context and adds fixed reason labels
and writer outcome. It emits a final shutdown summary even after an earlier
failure message. The benchmark retains the observer before enabling capture
and samples it after driver Drop and maintenance-lock settlement. Schema 2
adds only `history_diagnostics`; the strict validator still accepts historical
schema 1 reports without inventing reasons for them. All three prior raw
artifacts remain unchanged.

FIFO, accepted16/32MiB quotas, capture target/deferred-screen bounds, three
stage attempts with10ms worker delay, first-end-time reservation, existing
loss/cancellation policy and the single250ms history drain are preserved.

## Actual release measurements

The parent ran the unchanged three synthetic workloads sequentially twice:
`cargo run --locked --offline --release --example live_history_bench`.
Exact stdout is retained in [run 1](p5-history-diagnostics-run1.json) and
[run 2](p5-history-diagnostics-run2.json), with no edited measurement values.
The [earlier methodology](P5-LIVE-HISTORY-BENCH.md) still applies: two real
24-row PTYs, authenticated socketpair split/control/input, actual HistoryStore,
ordinary capture cadence, explicit screen geometry/content checks and reads
only after the writer lock settles. No test/build ran alongside the measured
workloads. Raw arrays permit recomputation; there is no selected performance
threshold.

| Run / case | Tick p95, us | Child ack p95, us | Sticky failure | Reasons | Writer drain |
| --- | ---: | ---: | --- | --- | --- |
| 1 paced80 | 227 | 3,151 | false | none | drained |
| 1 burst240 | 222 | 5,811 | false | none | drained |
| 1 shutdown240 | 248 | 9,798 | false | none | drained |
| 2 paced80 | 199 | 4,246 | false | none | drained |
| 2 burst240 | 202 | 5,195 | true | admission_bytes_full, final_payload_dropped_bytes | drained |
| 2 shutdown240 | 357 | 7,672 | false | none | drained |

The reproduced burst failure is specifically final-payload rejection at the
32MiB accepted encoded-byte boundary. It is not a storage-stage failure or
writer drain timeout in that run. Both end markers were present in all six
post-settlement cases. Wide cases retained gap=true; saved gap alone still
does not identify its cause. All96 input probes began and were acknowledged
before observed producer completion; both shutdown cases interrupted active
producers and honestly retain null completed-row counts.

The clean first burst run demonstrates variability rather than a fix. These
observations cannot retrospectively identify the cause of the older missing
end markers, nor prove ten-agent behavior, host UI latency, a throughput budget,
physical power-loss durability or full reboot/layout recovery. Driver Drop
includes child cleanup, and a later settled end marker does not prove it was
durable when Drop returned.

## Review and verification

Review corrected a false inference from the writer's broad failure flag:
worker panic/disconnection must not add `storage_error`. Actual failed storage
stages and worker failure now record distinct reasons before publishing the
shared failure flag. Receipts and late worker reasons remain published before
storage guard teardown. Reason recording does not change admission decisions.

Focused tests exercise exact count/byte/contended admission, recovered storage
errors versus retry exhaustion, panic attribution, cancellation versus a late
final commit, invalid versus byte-dropped final payload, harmless ordinary
pressure, pending receipt versus unsubmitted end intent, and observers retained
before both successful enable and rejected store-domain setup. A new deadline
fixture initially assumed first submission could never contend with worker
startup; it now retries only that bounded test-setup contention using the same
owned batch. No production admission retry was added. The public fixture also
initially lacked its required server directory; that test setup was corrected.

Parent integrated verification is recorded in [INTEGRATION.md](INTEGRATION.md).
The full verifier finished7/8: root library535 passed/4ignored and default
binary94passed/1failed. All29 integration targets separately ran, with78passed,
17failed and11ignored; no target was incomplete. The unchanged failure targets
are recorded there. Strict all-target Clippy/fmt/build, Python44 (13 report
tests), decoder39, component fingerprints and strict library rustdoc pass;
the doctest command ran0cases. Final docs check:164 Markdown files,0findings.
No failed, ignored, blocked or unexecuted test is counted as acceptance. Graph
transport stayed closed; no refreshed-index claim. The requested ms-rust skill
is still absent; no guideline compliance claim.

Next: review and test final-payload admission under byte pressure before
changing scheduling or quotas. Preserve the current budget and bounded memory
contract while evaluating a bounded way to retain the final tail. Older missing
end markers and representative workload acceptance remain separate questions.
