# P5 bounded history end intent — design and results

## Design (before implementation)

- `history_capture.rs:868-929` retries metadata-only work through ordinary
  `try_submit`; if count capacity or its accounting lock is unavailable until
  shutdown, `note_drain_deadline` only reports `final_metadata_unsubmitted`.
  `history_writer.rs:999-1069` finalizes only accepted canceled batches, so a
  never-submitted end has no worker-side recovery path.
- `history_capture.rs:543-555` keeps unknown time as `None`, but a successful
  metadata-only batch without an end timestamp only marks a gap; it cannot
  represent a recorded end in the existing on-disk format. Retain explicit
  `unknown_end_time` and a conservative gap; do not invent an end timestamp.
- `history_capture.rs:738-814` drops final payload when byte admission fails;
  the accepted metadata-only fallback marks a gap. Never claim that a drained
  metadata queue means the missing payload was written.

Policy: use a separately bounded, metadata-only end-intent reservation for at
most eight registered targets, owned by the same guarded writer. It does not
consume the 16-batch/32-MiB payload quota and cannot carry rows or screens.
Process reservations after accepted FIFO work, both on normal channel closure
and after timeout cancellation, using conservative gap/end metadata; suppress
them when the matching end batch commits. Unknown-time reservations record a
gap only and remain explicitly unknown in diagnostics. Never enlarge the
250-ms caller wait or on-disk format. Reject an already-ended or mismatched
epoch rather than rewrite it. Alternative of increasing batch/byte quotas or
waiting for space is rejected; treating a synthetic timestamp as a real exit
is rejected; making every successful final payload into a gap is rejected.

Under byte pressure, when candidate screens alone exceed the available budget,
one existing nonblocking retry drops those screens and admits a newest complete
row suffix if at least one row fits. Dropped screens/prefix set a sticky failure
and gap. If no row fits, the existing metadata-only fallback applies. An
accepted final row batch still requires its durable receipt; cancellation
preserves the separate gap/end reservation, not a claim of payload completeness.

## Implementation and checks

The reservation is fixed at eight target identities and retains only target,
epoch, first known time and a gap bit. It is written by the guarded worker
after FIFO batches are terminal, including after an explicit timeout. A plain
writer Drop suppresses this reservation. An unknown-time reservation writes
only a conservative gap with `ended_at_unix_ms=0`; existing file semantics
cannot distinguish that from a live target after process loss, so the sticky
`unknown_end_time` reason is the observable uncertainty for the current driver
generation, not a durable unknown-ended flag. An exited target with no known
wall time cannot receive 30-day ended retention in this format. Accepted
payload that commits first resolves the reservation, while an accepted canceled
known-end batch may use the preexisting timeout helper before the reservation.
The caller's timeout stays timed out after any later metadata success.

`src/multi_runtime.rs` needs no driver edit for this policy: existing Drop
already calls `begin_shutdown`, `take_writer_for_shutdown` and the single
bounded writer shutdown. If the parent chooses to attribute metadata completion
to the final Drop summary, sample the retained diagnostic handle after the
writer guard settles rather than changing the shared 250-ms deadline.

Checks run with `CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-s2`:

| Actual command | Output/result |
| --- | --- |
| `cargo test --locked --offline --lib history -- --test-threads=1` | `test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 580 filtered out; finished in 7.67s` |
| `cargo test --locked --offline --test p5_history_end_intent --test p5_history_writer --test p5_live_history --test p5_live_history_driver --test p5_history_diagnostics -- --test-threads=1` | Five completed targets: 2 + 1 + 6 + 1 + 2 = 12 passed, 0 failed, 0 ignored. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | `Finished dev profile ... in 0.09s`; exit 0. |
| `cargo fmt --check` | No output; exit 0. |

The added deterministic fake-backend checks cover a full 16-batch queue with
unsubmitted end, known-time timeout/late metadata, unknown time with no
fabricated timestamp, and final-row salvage when large screens cannot fit.
One library test uses the actual store for known and unknown reservations; the
new integration test opens the real store again after a known-end write. An
intermediate fixture used 650 maximum rows when fewer could fit and failed
its assertion; after calculating the available row count it exposed a gated
backend needing three releases, which was corrected before the final checks.
Those failures are not counted as passes. Physical power loss, a permanently
blocked filesystem call and durable unknown-ended labeling remain unproved.

## Parent review — 2026-10-21

Worker: GPT-6-sol (medium), parent-monitored. Design accepted: separate
fixed metadata-only end reservations (8), no quota/deadline/format change,
unknown time stays explicit (gap, no fabricated timestamp), screens dropped
before rows under byte pressure with sticky gap.

Parent fix: reservations were never freed, so a long-lived generation with
repeated split/close (each new pane registers a target) would permanently
fill the 8-slot table and refuse capture for later panes. Resolved slots are
now reused; regression `resolved_end_reservations_are_reused_beyond_the_fixed_table`.

Parent also diagnosed a PRE-EXISTING intermittent failure (reproduced 2/12 at
committed d864eb8 before this slice): `HistoryStore` read paths reported
`unsafe history storage` when a concurrent atomic rename by the writer left the
reader's just-opened descriptor on the replaced inode (`nlink == 0`).
Instrumented runs showed `mode=600 nlink=0`. Committed-file reads (rows,
screens, layout slots) now reopen the current name up to 4 times when
`nlink == 0`; real hard links/unsafe files still fail closed. After the fix
`p5_live_history_driver` passed 20/20 consecutive runs.

Parent checks: `--lib history` 55 passed/0 failed; full verifier 8/8 with
927 passed/0 failed/15 ignored across all Rust targets in one run. A
subsequent verifier run hit the historical unconfirmed parallel
`default_open::tests::lock_probe_detects_a_held_storage_lock` assertion once
(1 failure); the binary then passed 15/15 parallel and 10/10 serial runs.
Cause still unconfirmed (suspected transient lock inheritance by a concurrent
child spawn in another test); it is not counted as a pass and is unrelated
to this slice's files.
