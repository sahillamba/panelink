# Phase 3 pre-admission client-death result

2026-10-23. Implemented the fixture-only pre-delivery acceptance specified in
[the task](P3-PRE-ADMISSION-CLIENT-DEATH-TASK.md). The separate production
server receives the proxy's Hello but never receives the held Submit frame.
The proxy validates the exact target, owner revision, text, operation key and
request ID before notifying the parent. The parent verifies and SIGKILLs the
owned Submit CLI Child, then drops the held frame and both streams.

After process death, the same persistent context contains exactly the captured
uncertain key and one advanced sequence. Direct status and resync report
`not_admitted`; status leaves uncertainty intact, while resync clears it without
changing generation, lease or sequence. Repeated resync remains clear. An
explicit shell CHECK reports zero handled tokens, the private effect record is
absent, and the original target, child PID and server remain live. The test
explicitly stops the owned server and verifies endpoint cleanup. Client
context, server status and shell effect are independently observed.

Parent verification:

- `cargo test --locked --offline --test p3_inflight_client -- --test-threads=1`:
  **2 passed, 0 failed, 0 ignored** in 1.21s, including this acceptance and the
  existing post-completion/pre-ack case.
- `rustfmt --check tests/p3_inflight_client.rs`: passed.
- `python3 scripts/check_docs.py`: 274 Markdown files, 0 findings.
- `git diff --check`: passed.

Luna's first focused attempt exposed a mismatched readiness marker in the new
fixture; it was corrected, and Luna's focused rerun passed 1/1 in 0.57s. The
parent then reran the complete target above. No full Cargo suite was run for
this test-only change. `cargo fmt --all` touched vendored formatting during
the worker run; those files were clean at turn start and were restored from
HEAD, so no vendor changes remain.

This acceptance proves caller death after durable key reservation and before
server delivery/admission. It does not prove death after server dispatch but
before ledger admission, admitted owner-pending or PTY interruption, human
four-pane acceptance, provider recovery, or reboot recovery. No production code,
input policy, wire/storage format, dependency or manifest changed. Full Phase 3
remains open. Work is uncommitted; nothing was pushed.
