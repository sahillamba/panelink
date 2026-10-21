# P5 committed-record corruption and partial-tail policy — 2026-10-21

Status: bounded policy/acceptance delivered and parent verified; known
blocked/failed acceptance is retained in P5-COMMITTED-CORRUPTION.md.
User requested this next slice. Use one Luna xhigh for public-store regressions
and parent for policy, independent fresh-process CLI checks and integration.
This scope does not complete Phase5 or establish physical crash durability.

## Policy

The existing format uses whole-file atomic replacement, not an append log.
A committed row segment is accepted only if its complete bounded framing,
identity, epoch, count, row ordering and sanitized cells validate. A committed
screen is accepted only as a whole validated checkpoint. Short headers,
truncated lengths/bodies, exact complete-row prefixes with missing declared
rows, trailing bytes, invalid identities/order/cells and unsafe files are
errors. Never return a valid-looking prefix as recovered data, infer the
missing end time, or silently truncate, repair, rename, delete or overwrite
corrupt committed records. Existing mutation preflights must refuse them.

There is no checksum or authenticated source: format-valid changed text or
metadata may remain readable. Structural validation is not arbitrary bit-rot,
authenticity or malicious-same-user tamper detection. Do not change the format,
add dependencies or promise detection the codec cannot establish. A checksum
migration or explicit forensic export/repair is separate future work.

Dot-prefixed recognized orphan temporary files are uncommitted. Existing bounded
locked cleanup discards them; they never outrank or replace a committed file,
even if the committed destination is invalid. Read-only commands never clean
up. Healthy other targets remain readable by explicit ID; per-buffer store
reads remain independent. The current combined CLI show/search fails the whole
requested target if any existing row/normal/alternate component is invalid,
with nonzero status and no partial content output. Missing components are
absence, not corruption. List is not an integrity certificate.

Pruning and cleanup are not directory-wide transactions; a later error may
leave earlier permitted maintenance effects. Refusal preserves the invalid
committed bytes. No terminal effects, shell launch, process resume or provider
conversation action can occur during history inspection. Existing normal
receipt, first-end, quotas and250ms wait stay unchanged.

## Ownership and checks

Luna owns ONLY new tests/p5_history_corruption.rs and
P5-COMMITTED-CORRUPTION-TESTS.md. Parent owns new
tests/p5_history_corruption_cli.rs, this task, result/policy note, canonical
spec and shared handoff. No source edits are required unless acceptance exposes
a real contract violation; parent reviews any proposed change first. No
commits/pushes or unrelated edits. Preserve prior dirty work. Graph-first code
discovery; requested ms-rust absent.

Worker: small actual-store fixture, every proper byte truncation, malformed
framing/ordering/cells, reopen/mutation refusal, valid other target/buffers,
checkpoint truncation, and honest valid-shaped-change limit. Avoid fsync per
truncation; bounded test-owned writes are sufficient for software corruption
fixtures. Run cargo test --locked --offline --test p5_history_corruption --
--test-threads=1 and rustfmt --edition 2021 tests/p5_history_corruption.rs;
record ACTUAL outputs and corrections in owned note. Parent independently
reviews and reruns focused checks, runs full permitted verifier and all
integration targets, strict rustdoc and final docs/fingerprints. Ignored/blocked
checks are never passes. No benchmark needed for a tests/policy-only slice.

Parent fresh-process CLI checks: rows-tail, corrupt normal, corrupt alternate,
zero-byte committed file, valid-looking OSC/control suffix and orphan temp
alongside invalid destination; show/search nonzero with empty stdout and fixed
content-free stderr. Snapshot bytes, paths, modes and mtimes across read-only
commands. Prove healthy target reads remain sanitized and unchanged, no runtime
files or configured shell marker appears. Maintenance cleanup discards only
recognized temporary while preserving invalid committed data, with singleton
exclusion and safe test teardown. This is process reopen evidence, not reboot.
