# Phase 3 killed input-client acceptance result

Status: **scoped acceptance passed; full Phase 3 remains open.** This evidence
covers a real `panelink control submit` child killed after the server returned
a validated matching `Completed(Applied)` response, while a private,
one-connection authenticated proxy withheld that response. The caller received
no response. This is the post-completion/pre-client-ack boundary; it does not
cover death before admission or while the owner is still applying input.

The final targeted run passed **1/1 in 0.84s** in
`/private/tmp/panelink-p3-inflight-shutdown-corrected.log`. Earlier attempts
are preserved: the worker's restricted run stopped at startup with
`listener_permission_denied` before reaching the scenario;
`/private/tmp/panelink-p3-inflight-parent.log` failed a JSON newline assertion
after the key/recovery stage; and
`/private/tmp/panelink-p3-inflight-corrected.log` reached shutdown but used the
default-workspace stop path without its PID record. The fixture assertions
and shutdown were corrected; the final run explicitly SIGTERMs and reaps its
owned serve child and verifies socket cleanup. The final source also adopts
the child before startup polling, requires normal proxy join, and uses bounded,
non-panicking cleanup on unwind.

The acceptance relays the original Submit once and holds only a validated
upstream reply matching request ID and key after validating the request target
and owner revision. The test
verifies the submit child is alive at that barrier, then SIGKILLs and reaps
that exact child. Reopening the same context shows the captured key remains
uncertain and the sequence advanced exactly once. Read-only status reports
that key as Applied and leaves uncertainty intact; resync clears it without
changing generation, lease or next sequence. Repeated resync reports clear.
No payload is resent. The same child PID and full target remain live; the
private append record has one handled token; and an ordered CHECK reports
count 1 with the same child. The test explicitly stops its owned server child
with SIGTERM and confirms socket cleanup. It makes no arbitrary-descendant
cleanup claim.

Formatting and strict all-target Clippy passed in
`/private/tmp/panelink-p3-inflight-reviewed-clippy.log`. The final targeted
test log is above. Final complete locked/offline Cargo with `--no-fail-fast`
and serial test threads: **1089 passed/0 failed/15 ignored** in**157.19s**
(library712/binary142/integration235 across53 targets). The600s bounded runner
did not time out. Seven supporting checks pass: decoder39, Python77,
component fingerprints, fmt, strict all-target Clippy, build and docs272/0.
Final documentation/whitespace checks pass after handoff updates. Full log:
`/private/tmp/panelink-p3-inflight-full.log`; supporting log:
`/private/tmp/panelink-p3-inflight-noncargo.log`. The earlier1088/0/15 suite
remains a separate checkpoint; this new run is the combined-tree evidence.

No production policy, dependency, wire format or persistence format was
introduced. Existing working changes remain uncommitted on
`codex/p3-scrollback-integration`, based on `9f3d8a0`; no push or Phase 3
completion is claimed. Human host, owner-pending/server-death, provider and
physical-reboot acceptance remain open.

Native GPT-6-Luna High workers implemented the exclusive new test and performed
a separate read-only barrier audit/source review; Luna Medium drafted this
evidence note. Parent specified the boundary, corrected and independently
verified the fixture, and owns the combined test result. Native collaboration
was the launcher. No cost telemetry is available. All worker ownership is
released; ms-rust remains unavailable. Prior application/test/script files
match156 turn-start hashes; only the new integration test changes executable
test behavior in this slice. Original source checkout untouched.
