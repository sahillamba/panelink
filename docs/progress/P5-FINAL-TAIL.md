# P5 bounded final-tail admission — result — 2026-10-21

Status: implementation and focused verification complete; production files
released to the parent for sequential release workloads. Local checks pass,
but parent-reported workload runs timed out and the matched comparison is
pending. This slice does not establish complete transcript durability or
reboot recovery.

## Behavior

After a final batch is rejected specifically for encoded-byte capacity, the
coordinator takes one nonblocking accounting snapshot and makes at most one
more admission attempt. If the complete batch now fits, it retries unchanged.
Otherwise it retains the newest complete-row suffix that fits alongside every
candidate screen. The original `HistoryBatch::new` validation runs before any
prefix can be discarded. The helper moves owned row boxes and preserves the
screen boxes, token, epoch and first end timestamp.

If a valid row prefix is removed, the capture failure and gap remain sticky,
and `FinalPayloadDroppedBytes` records partial as well as complete payload
loss. A rejected suffix does not consume its token. If the capacity snapshot
is contended/unavailable, or no row fits with the screens, the prior
metadata-only fallback remains in force. Initial Count and Contended
rejections also keep that fallback. The snapshot itself records no admission
reason. A retry that is Full records its actual paired loss reason without
clearing the initial byte-pressure cause or any earlier failure.

Submitted row cursor advances only after admission; durable cursor and screen
state still advance only after the matching committed receipt. A screen-only
retry preserves the submitted row cursor, except for the existing epoch-reset
rule that uses ordinal zero in the newer epoch. No new queue, capacity wait,
or storage call was added to the owner thread. FIFO ordering, writer quotas,
stage retries and shutdown deadline are unchanged.

## Verification

Commands were run against the local offline workspace after the source freeze:

- `rustfmt --edition 2021 src/history_writer.rs src/history_capture.rs` — passed.
- `cargo test --locked --offline --lib history_writer -- --test-threads=1` —
  passed: 20 tests, 0 failed, 524 filtered out. Coverage includes exact byte
  accounting, unchanged full-payload and screen-only cases, newest suffix,
  no-fit, invalid original prefix, snapshot contention without a synthetic
  admission diagnostic, a pressure-then-drain unchanged retry with the same
  token, writer quota ownership, and existing lifecycle tests.
- `cargo test --locked --offline --lib history_capture -- --test-threads=1` —
  passed: 5 tests, 0 failed, 539 filtered out. The byte-pressure test now
  verifies the newest submitted ordinal (`900`) while durable cursor remains
  at ordinal `0` pending its receipt; the no-fit test verifies reserved end
  metadata with two screens that exceed available payload bytes.
- `cargo clippy --locked --offline --lib -- -D warnings` — passed.
- Parent-reported actual-store gated acceptance — passed 1/1 before the final
  `CommitPlan` parameter refactor. It decoded the newest suffix from a newer
  epoch with both maximum screens and verified the first end time, gap,
  submitted-versus-durable cursor and drained outcome. This worker did not
  rerun that sibling-module test after the refactor; the refactor only bundled
  the existing plan values into one argument.

The first writer-test attempt failed because the small row fixture yielded
fewer than three rows. The replacement maximum-row fixture then exposed that
the minimum accepted terminal dimensions exceed 2×2. Both were fixture-only
issues; the final focused run passed with three maximum rows and an 8×20
screen fixture. The added pressure-then-drain retry test passed in the final
20-test writer run. No production behavior was changed in response to those
failures.

## Release-workload status

Parent reports preserved run1 burst and shutdown cases timed out with
`work_canceled`/pending receipts and one end marker missing per wide case.
Run2 reports all three cases timed out, including paced with no admission
reason, and one end marker missing per case. The results cannot yet be
attributed to suffix salvage because paced does not exercise the suffix path.
The parent-reported matched control exited 0 with the suffix branch disabled:
paced drained with no reasons, while burst and shutdown timed out with
`WorkCanceled` and `PendingReceiptsAtDeadline`, each missing one end marker;
the burst control ended at ordinals 18046 and 19982. This shows the suffix
retry is not necessary for the wide-case timeouts. One control does not rule
out an added workload or regression from the suffix path. These runs do not
support a claim that final-history reliability is fixed. Parent completion
and evidence follow below.

## Parent review and independent acceptance

Source review confirmed that only the owned validated batch is trimmed; row
bodies are moved, capacity is sampled without waiting, and no accounting lock
spans scanning/destruction. Worker FIFO, stage retry, retained lock and quota
logic are unchanged. Review replaced a wide helper argument list with the
existing CommitPlan value; no new public API or production test switch.
The parent added only a cfg(test) library module and independent store fixture
outside worker ownership. Existing synchronous capture is unchanged.

The independent `history_tail_tests` case passed before and after the plan
refactor. It gates actual HistoryStore IO after accepting two1024-row batches,
then submits900 new-epoch rows with both80x240 screen records. The actual free
encoded budget holds exactly685 newest complete rows alongside both screens.
The test decodes every saved row and checks ordinal/content through900,
epoch2, gap, the original end777 despite a repeated999, exact screen bytes,
submitted versus durable cursor separation and a drained writer result. This
proves that bounded suffix admission reaches the real store; the fixture waits
for settlement and does not prove the250ms live deadline.

An initial default full-suite run exposed legitimate accounting contention in
the existing FIFO/epoch test's first unconditional admission. Parent fixed its
setup to retry only Contended, retaining the exact batch/token and a bounded
test deadline; all other rejection causes still fail. No production wait or
retry was added. The focused FIFO test and final full library run pass. The
earlier failed run remains recorded, not counted as acceptance.

## Exact workload artifacts and limits

Shipping-policy stdout is preserved in [run1](p5-final-tail-run1.json) and
[run2](p5-final-tail-run2.json). The parent then copied the same runtime/harness
to a private temporary snapshot and disabled only the suffix conditional,
using the original complete-loss/metadata fallback. Its exact stdout is
[the control](p5-final-tail-control.json); [provenance](p5-final-tail-control-provenance.json)
records source/harness/manifest hashes and the one-line difference. Parent
verified the copied coordinator equals shipping source after undoing only
that condition. This is a same-runtime comparison, not an untouched historical
commit or an alternative shipped configuration. No workspace production
switch was added. No test/build ran alongside the measured workloads.

| Run / case | Tick p95, us | Child ack p95, us | Writer drain | Settled end markers |
| --- | ---: | ---: | --- | --- |
| 1 paced80 | 240 | 10,960 | drained | 2/2 |
| 1 burst240 | 344 | 15,048 | timed_out | 1/2 |
| 1 shutdown240 | 631 | 16,300 | timed_out | 1/2 |
| 2 paced80 | 255 | 8,506 | timed_out | 1/2 |
| 2 burst240 | 360 | 17,164 | timed_out | 1/2 |
| 2 shutdown240 | 585 | 14,739 | timed_out | 1/2 |
| Control paced80 | 251 | 4,072 | drained | 2/2 |
| Control burst240 | 362 | 15,017 | timed_out | 1/2 |
| Control shutdown240 | 410 | 15,269 | timed_out | 1/2 |

All wide cases in these three runs recorded admission_bytes_full,
final_payload_dropped_bytes, shutdown_timed_out, work_canceled and
pending_receipts_at_deadline. The run2 paced case recorded only the last three
reasons. There were no storage-error/retry-exhaustion reasons. Every missing
end above remains missing after maintenance-lock settlement, not merely at
driver Drop return. All144 probes began and were child-acknowledged before
observed producer completion. Each active-shutdown case retained unknown
completed-row counts. Timings retain the earlier harness methodology:16 probe
samples/case makes p95 equal max; Drop includes child stop, and saved reads
occur after lock settlement. No host UI or ten-agent performance budget passes.

Suffix preservation is proven by deterministic pressure tests. The live runs
also expose unresolved deadline/cancellation loss, with and without suffix
retry. These small sequential samples cannot quantify causality, exclude added
write cost, explain all older missing ends, or establish general reliability.
The implementation keeps failures visible and does not increase the250ms
budget to hide them. Next is a reviewed bounded shutdown/final-metadata policy;
committed-record corruption and full reboot/layout recovery remain separate.

Final parent verification is recorded in [INTEGRATION.md](INTEGRATION.md).
All seven preexisting raw reports retain identical fingerprints. Of the64
baseline application/manifest files, only writer/coordinator and the cfg(test)
library root changed; the new independent test module is test-only. No
dependency/vendor/format change, commit or push. Graph connector recovered and
parent refreshed its fast/nonpersistent index after source edits. Requested
ms-rust remains absent. All ownership is released; parent phases3/5/6 stay open.
