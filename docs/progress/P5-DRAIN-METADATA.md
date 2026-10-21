# P5 canceled end-metadata finalization — result — 2026-10-21

Status: bounded writer/coordinator implementation, parent review, independent
actual-store checks and two sequential release workloads complete. This does not complete Phase5 or establish
complete saved-history durability.

## Behavior

An explicit `HistoryWriter::shutdown` timeout now selects a cancellation mode
that permits metadata-only finalization for accepted work whose validated
batch carries a known nonzero end timestamp. The detached worker processes the
current canceled item and queued items in FIFO order, with the existing maximum
of16 accepted batches. It passes the batch's owned target, epoch and first end
timestamp to `HistoryBackend::finalize_incomplete_end`. It does not append row
payload or write screen checkpoints after cancellation. Missing, non-final and
unknown-time work does not create an end marker.

Plain `Drop`, worker disconnect and ordinary panic retain strict cancellation.
An already executing end stage that returns success after the deadline remains
`Committed`; it does not invoke the metadata helper or set its success reason.
Metadata finalization never changes a canceled receipt to `Committed`, never
advances the capture coordinator's durable row/screen cursor, and never changes
the caller's `TimedOut` drain outcome. It does not imply the helper completed by
the time shutdown returned.

The metadata helper is available through a safe `Unsupported` trait default;
the production store backend calls the parent's atomic `HistoryStore` helper.
Unsupported is terminal and records the finalization-failed reason without a
storage-error or retry-exhausted reason. Other helper errors record each actual
storage error and retry at most three total attempts with a fixed10ms delay.
Only a terminal error records retry exhaustion and finalization failure. A
helper panic records worker failure and terminal finalization failure, then
fails closed. Success is recorded only after the helper returns `Ok`. A
terminal failure prevents later accepted end metadata from being written. All
receipts are still terminalized as canceled and diagnostics are published
before the singleton guard is released.

`history_capture` continues to treat a canceled receipt as a capture failure;
the new test confirms that even successful metadata finalization leaves its
durable row cursor unchanged. The conservative gap/end metadata identifies an
uncertified capture boundary. It does not claim the saved payload is complete.

The caller still waits at most `min(wait, 250ms)`. Storage syscalls cannot be
interrupted: at most16 accepted batches can each receive three helper attempts,
but wall time is not bounded when the filesystem blocks. The detached worker
retains the singleton lock through that work, preventing maintenance or a new
writer from overlapping it.

## Verification

Commands ran offline after the owned source freeze:

- `rustfmt --edition 2021 src/history_writer.rs src/history_capture.rs` —
  passed.
- `cargo test --locked --offline --lib history_writer -- --test-threads=1` —
  passed:25 tests, 0 failed, 530 filtered out. The run includes FIFO current
  and queued metadata, first timestamp retention, unknown-time skip,
  no-payload-stage-after-cancel, transient retry recovery, retry exhaustion,
  unsupported default, panic fail-closed behavior, strict `Drop`, late
  in-progress end success, bounded shutdown and retained singleton.
- `cargo test --locked --offline --lib history_capture -- --test-threads=1` —
  passed:6 tests, 0 failed, 549 filtered out. The added timeout case observes
  successful end metadata and a canceled receipt while confirming the durable
  cursor remains at its initial value.
- `cargo clippy --locked --offline --lib -- -D warnings` — passed.

During development, the first capture-test compile missed a test-only import;
the import was added. The first version of that test expected a canceled target
to be removed, but the coordinator correctly retains its terminal-failure
state; the test now checks the canceled-receipt diagnostic and unchanged
durable cursor. The fixture also records the expected preceding history-begin
stage. These were corrected before the final passing checks.

Parent reports that its independent actual-store timeout acceptance passed2/2:
current and queued known ends were recovered after explicit timeout, receipts
remained canceled, existing rows and the normal screen were preserved, no new
alternate-screen/payload was written, the guard stayed held through a blocked
finalizer, and plain `Drop` performed no finalization. Parent also reports its
store-helper fault/refusal tests passed3/3 and strict diagnostic validator
tests passed14/14. Those parent-owned checks were not run by this worker.

## Release and remaining ownership

The only production files changed by this worker are
`src/history_writer.rs` and `src/history_capture.rs`; this note is the only
worker-owned documentation file. Parent owns the helper, fixed diagnostic
schema/validator, independent store tests, source-stable workload measurements,
canonical evidence and roadmap. The worker files are frozen and released; no
Cargo command ran alongside the parent's requested sequential release
workloads. No benchmark, dependency, manifest, vendor, storage-format or public
API change was made. No commit or push was made. Parent verification and review are recorded below; Phase5 remains open.


## Parent review and actual-store acceptance

Parent reviewed the frozen worker cancellation transitions, FIFO/receipt paths,
late end-stage commit, panic/refusal behavior and guard teardown. The added
crate-private store helper reuses the existing codec/atomic replacement without
screen writes or format changes. Independent actual-store tests passed2/2:
explicit timeout recovers current and queued known ends while receipts remain
canceled, payload calls are absent, and maintenance stays excluded through a
blocked finalizer; plain Drop performs no finalization. Three helper tests pass
for missing/live/older epochs, first-time preservation through two uncertain
directory-sync failures, pre-rename failure, and refusal of zero/newer/ended
mismatched/disabled/corrupt/identity-mismatched/symlink records. Existing row and
screen bytes remain unchanged where required. These tests are included in the
full library run below. The coordinator test checks terminal capture-failure
state and unchanged durable cursor, rather than inventing a failed receipt.

## Two source-stable release observations

Commands ran sequentially, with no competing Cargo job and no source changes
between them:

```text
cargo run --locked --offline --release --example live_history_bench
python3 scripts/validate_live_history_report.py <run1> <run2>
```

Both workload commands exited0 and both reports validate. Exit0 describes a
completed measurement, not successful history capture. Raw artifacts:
[run1](p5-drain-metadata-run1.json), [run2](p5-drain-metadata-run2.json).
The existing real-store/two-PTY harness and three cases are unchanged. It waits
for post-Drop singleton-lock settlement before inspecting saved history and
reading the final diagnostic observer.

| Run/case | Drain | Driver Drop / post-Drop lock wait, us | Settled ends | Observed reasons |
| --- | --- | --- | --- | --- |
| 1 paced80 | drained | 75224 /152 | 2/2 | none |
| 1 burst240 | drained | 201416 /158 | 2/2 | admission_bytes_full, final_payload_dropped_bytes |
| 1 active shutdown240 | timed_out | 284355 /42840 | 2/2 | shutdown_timed_out, work_canceled, pending_receipts_at_deadline, canceled_end_metadata_finalized |
| 2 paced80 | drained | 90382 /337 | 2/2 | none |
| 2 burst240 | timed_out | 250459 /25540 | 2/2 | admission_bytes_full, final_payload_dropped_bytes, shutdown_timed_out, work_canceled, pending_receipts_at_deadline, canceled_end_metadata_finalized |
| 2 active shutdown240 | drained | 263142 /133 | 2/2 | none |

Every target has a settled end marker, including both timed-out cases with the
new actual-success reason. The timed-out status and canceled receipts remain
honest; neither late marker proves final payload completeness nor durability
at Drop return. Both bursts still report final-payload loss. Wide cases retain
722 rows per target, gap=true and normal checkpoints; paced cases retain
754/738 and746/754 rows respectively with gap=false. No finalizer-failed,
storage-error or retry-exhausted reason was observed in these runs. Fault
behavior is established by deterministic tests, not inferred from these logs.

All96 probes started and were child-acknowledged before observed output
completion. Owner-tick p50/p95/max microseconds for run1 were61/202/892 paced,
125/224/8632 burst,143/4221/9214 shutdown; run2 were66/202/1378,
129/601/9330,140/1636/9441. Driver Drop includes child stop/capture and is
not the writer's isolated250ms wait. No comparative performance improvement,
queue-occupancy instrumentation, numeric release budget, or representative
ten-agent workload pass is claimed. Scheduling and IO timing vary between
runs; the deterministic gated tests establish the intended timeout behavior.


## Parent full verification and remaining work

| Check | Actual result |
| --- | --- |
| Full root library /default binary | 551passed/4ignored;94passed/1failed. |
| Non-UI verifier | 7/8, exit1; socket PermissionDenied/code1 in endpoint-state binary test. |
| All29 integration targets run separately, serial within each | 78passed/17failed/11ignored; no incomplete target. |
| Strict all-target Clippy /fmt /offline build | Passed in verifier. |
| Python /strict report tests /decoder | 45 /14 /39 passed. |
| Strict library rustdoc /doctests | Passed;0 doctests. |
| Component fingerprints /final docs /diff whitespace | Match;168 Markdown files/0findings; passed. |

The four failing integration targets remain p3a_attach_host5,
p3a_default_open6, p3a_serve_pathname1 and p3b_close_runtime5: sixteen startup
socket-permission failures and one known host stderr assertion. These remain
failed/blocked acceptance; ignored tests are not passes. See current
[INTEGRATION.md](INTEGRATION.md) for log paths. No new production correction
was needed in the parent full check. The first helper test compile omitted a
qualification on test-only OpenOptions; it was fixed before the passing3/3.
Worker fixture corrections above and this compile failure are not hidden passes.

All11 prior raw/provenance JSON artifacts have unchanged SHA256 hashes. The65
source/manifest baseline files changed only in history_writer, history_capture
(test-only), history_store, history_diagnostics and history_tail_tests. Cargo
manifests/lock and vendor inputs remain unchanged. The graph connector was
available and refreshed fast/nonpersistent after source freeze. Canonical spec
revision0.11 and the writer ADR now document the explicit-timeout exception.
All ownership is released. No commit/push or full-phase completion is claimed.

Next bounded work is reviewing committed-record corruption/partial-tail policy
and restart/fault acceptance. This slice refuses invalid committed records and
does not repair them. Unsubmitted/unknown end intent, lost final payload,
filesystem stalls, complete power-loss/reboot/layout recovery, representative
workload and actual host/provider identity acceptance remain open. Plain Drop
cannot promise metadata finalization. Phases3/5/6 remain open.
