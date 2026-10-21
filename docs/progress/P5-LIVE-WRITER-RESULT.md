# Live history writer integration result — 2026-10-21

Status: scoped live integration delivered and parent-reviewed; integrated
results are appended below. Phase 3/5 remain open.

`MultiServerDriver` now starts one `HistoryWriter` and routes existing and
split-created `PtyEffects` through a bounded capture coordinator. PTY sampling,
screen encoding, input, and control replies remain on the driver owner thread;
history store operations run on the writer. Existing synchronous
`PtyEffects::enable_capture` remains available to component callers and its
retry semantics are unchanged.

The coordinator caps registration at eight full targets, keeps at most one
accepted ordinary batch and one final batch per target, and retains at most two
deferred screens per target. Submitted cursors/screens advance on admission;
durable state advances only for a matching committed receipt. Receipt polling
and queue admission are nonblocking, and coordinator locking is released
between shutdown polls and before writer shutdown. The driver stops and
captures children first, drains against one total 250 ms budget, and reports
timeout or writer failure through the sticky signal. A stalled writer retains
its existing lock claim.

Final intent retains the first known end timestamp. Final rows/screens that
cannot be accepted due to the 32 MiB writer payload quota are discarded with a
sticky failure and explicit gap; reserved metadata-only end work is admitted
and progresses without further effects polling. Unknown end time stays
unknown. A full eight-slot registry disables capture for a newly split target
and raises the sticky failure while allowing the split and child launch to
continue.

Focused verification completed by the implementation owner:

- `cargo test --locked --offline --lib history_writer`: 16 passed, 0 failed.
  This filter included 14 writer-component tests and two live-runtime tests.
- `cargo test --locked --offline --lib server_pty`: 2 passed, 0 failed.
- `cargo test --locked --offline --test p5_live_history`: 1 passed, 0 failed.
  The real-store test verifies retained scrollback and the distinct latest
  normal-screen checkpoint after natural child exit.
- `cargo test --locked --offline --lib history_capture::tests`: 2 passed,
  0 failed before the quota-pressure case was added; the subsequent
  `full_writer_payload_quota_preserves_reserved_final_gap_and_first_end_time`
  focused run passed 1/1.
- `blocked_history_writer_does_not_block_pty_input_or_socket_control`,
  `full_capture_registry_does_not_reject_socket_split_or_child_launch`, and
  `persistent_history_writer_failure_does_not_stop_live_shell_input` each
  passed 1/1 in focused runs. These use real PTYs and authenticated socketpairs;
  the first also verifies async capture admission for a split-created target.
- `submitted_and_durable_cursors_follow_receipts_across_epoch_reset` and
  `rejected_final_payload_is_replaced_by_gap_and_end_metadata` passed 1/1 each
  before the pressure test was added. The pressure test uses valid maximum
  scrollback row batches and a blocked fake backend to fill the actual 32 MiB
  queue quota; it verifies the final metadata gap/end and first timestamp.

The fake-backend cases prove scheduling, receipt, fallback, target-capacity,
and shell-progress behavior. They do not establish disk durability. The
real-store acceptance above covers durable live scrollback, screen checkpoint,
and end metadata. Parent-owned public-driver and integrated checks are recorded below. No
machine-reboot acceptance or phase-completion claim is made.

Roadmap exception: none recorded here; the parent owns shared roadmap status.

## Parent verification and review

The parent independently added `tests/p5_live_history_driver.rs`: 2/2 pass
through the public startup/driver API with an actual `HistoryStore`. They cover
retained rows, distinct normal/alternate checkpoints and natural end metadata,
plus child stop/final capture on driver Drop and maintenance-lock release.
Force captures and file handshakes avoid assuming the ordinary five-second
cadence. These establish software-level saved history, not physical power loss.

| Final check | Actual result |
| --- | --- |
| `python3 scripts/verify_non_ui.py` | 7/8, exit1; known binary pathname socket EPERM. |
| Root library / binary | 531 passed/4 ignored; 93 passed/1 failed. |
| All 28 integration targets, separately and serial within each target | 76 passed/17 failed/11 ignored; every target completed. |
| New live slice cases | 9 passed: coordinator3, runtime3, actual-store integration3. |
| Formatting, strict all-target Clippy, offline build | Passed. |
| Strict library rustdoc / doctest command | Passed; doctests ran0 cases. |
| Decoder / Python / component fingerprints | 39 / 31 passed; fingerprints match. |
| Documentation | 160 Markdown files, 0 findings in both verifier and final docs check. |

Commands: `python3 scripts/verify_non_ui.py`; every Cargo metadata test target
via `cargo test --locked --offline --test NAME -- --test-threads=1`;
`env RUSTDOCFLAGS='-D warnings' cargo doc --locked --offline --lib --no-deps`;
`cargo test --locked --offline --doc`. The parent also independently ran the
public-driver cases before the serial sweep. Raw local logs are
`/private/tmp/panelink-live-writer-verifier.log` and
`/private/tmp/panelink-live-writer-integrations/summary.json`; temporary logs
are supplementary, and this document is the durable result.

The unchanged integration failure set is `p3a_attach_host`5,
`p3a_default_open`6, `p3a_serve_pathname`1 and `p3b_close_runtime`5. Sixteen
fail before their feature assertions at server startup; the explicit serve
case reports `listener_permission_denied`. The remaining host stderr assertion
was previously reproduced at unchanged HEAD and fails again here. The binary
socket test reports OS code1/PermissionDenied. Failed/ignored cases do not
count as acceptance; host, pathname, provider and full reboot/layout gates
remain open.

Parent review corrected async fallback to a retained synchronous store,
metadata-only reset cursor attribution, repeated final submissions after slot
retirement, final-output ordering, and sticky-error observation at shutdown.
An initial synchronous retry run failed2/2 because `capture_failed()` included
the shared sticky flag in sync mode; the async-only distinction restored both
original cases, which pass in the final root run without weakening assertions.
The independent Luna reviewer found the original shutdown lock spanned receipt
waits. The final loop uses short guarded polls and unlocked sleeps/shutdown;
the reviewer confirmed the fix and found no remaining lifecycle blocker.
A narrow race may encode one extra candidate if the worker fails between the
initial receipt poll and sampling; later observations skip failed targets.
This is bounded wasted encoding, not a durable acknowledgment or PTY change.

All owners released their files. No dependency/vendor change, commit or push;
prior uncommitted work is preserved. Parent phases3/5 remain OPEN. The graph
connector returned `Transport closed`; source/tests are authoritative and no
current index refresh is claimed. Next: measure sustained output through this
live path, including owner progress, saved gaps and drain outcomes, before
making representative workload or UI latency claims.
