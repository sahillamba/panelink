# P5 bounded final-tail admission — 2026-10-21

Status: scoped implementation delivered and parent-verified; see
[P5-FINAL-TAIL.md](P5-FINAL-TAIL.md). Live drain/final-metadata reliability
remains open. GPT-6-Luna xhigh implemented the bounded change on the user's
explicit request. Parent applies roadmap-orchestrator and owns review,
independent verification, canonical spec and shared status. This slice targets
the reproduced byte-pressure loss; it does not close all history/recovery work.

## Start and ownership

Read ROADMAP.md and SESSION.md first, then AGENTS.md, README.md, SPEC.md,
PREBUILD.md, PANELINK-DISCOVERY.md and P5-HISTORY-DIAGNOSTICS.md. Preserve all
prior uncommitted work. Requested ms-rust is absent. The code graph became
available this turn and parent rebuilt project Users-sahillamba-panelink in
fast/nonpersistent mode; use graph discovery before source searches.

Worker may edit only src/history_writer.rs, src/history_capture.rs and
P5-FINAL-TAIL.md (new result). Parent owns all shared docs, any public acceptance
tests, benchmark/raw reports and integration. Parent ownership of ROADMAP is
the worker append-only rule exception. No commits/pushes, manifests, vendor,
dependencies, storage codec changes or other production files.

## Problem and invariants

Diagnostic run2 burst rejected a final payload at the32MiB accepted-byte guard,
discarded all its candidate rows/screens and later drained the metadata queue.
An ordinary byte-pressure observation is distinct from actual final loss.
The existing prototype records final loss visibly and retains first end intent.

Preserve max16 accepted batches/32MiB including inflight, max8 target slots,
one regular plus one final receipt per target, deferred screen bounds, one
producer, FIFO, token identity, stage-only retries3/10ms, retained singleton
guard through late worker teardown, and one total250ms history drain. No new
retained row queue outside the writer quota, capacity waits/spins, filesystem
work on the owner, input replay or claims of complete transcript durability.
Existing synchronous capture compatibility is outside this change.

## Approved bounded policy

On an actual first final Bytes rejection only, attempt to retain the newest
complete-row suffix that fits currently available encoded-byte capacity,
preserving all final screen candidates and first end metadata. At most one
additional admission attempt; do not wait for capacity. Validate the complete
original batch before dropping a prefix, including malformed older rows.
Capacity lookup must be nonblocking and must not allocate a diagnostic record.
The one producer prevents other admissions between lookup and retry; worker
activity can release capacity but cannot increase it. Do not hold accounting
locks while scanning or dropping row bodies.

If no original row fits with all screens, or lookup is unavailable, preserve
the existing complete-loss plus metadata fallback. Initial Count/Contended
rejections also retain existing fallback. If rows are originally empty and
screens cannot fit, there is nothing to salvage. Never silently drop screens
to fit rows. No new public API is required: internal owned-batch helpers and
a bounded capacity observation are preferred.

Any discarded valid row prefix remains a visible capture failure with gap and
FinalPayloadDroppedBytes. This label will mean partial or complete final-payload
loss; parent must update canonical documentation. Do not label a suffix that
has been admitted as fully durable: only its exact committed receipt advances
durable cursor/screen state. Keep the last/newest row boundary and epoch honest;
first end time and screen metadata must survive unchanged. A second rejection
returns owned data without consuming a token; preserve all actual observed
reason bits and use an honest complete-loss fallback.

Parent reviewed and approved the worker's design recommendation. A snapshot
lock miss adds no admission reason because no admission was attempted; retain
the first actual Bytes rejection as the complete-loss cause. If the original
full batch now fits, retry it unchanged without inventing loss. A trimmed-prefix
loss bit remains sticky if a second actual rejection adds another paired loss
cause. For originally empty rows, a successful screen-only salvage uses the
submitted cursor, or ordinal0 in a newer epoch, without inventing row durability.

Implementation stays internal: a writer capacity snapshot and owned batch
suffix helper; no new public API. The worker must report any discovered
invariant conflict to parent before relaxing this contract.

## Required evidence after approval

Deterministic gated-backend tests must show full admission unchanged; actual
Bytes pressure retains the newest suffix, every screen, first end time and
gap; invalid original prefix fails validation; exact encoded-byte accounting;
no-fit/Count/Contended fallback; no token consumption on rejected retry;
submitted versus durable receipt separation and FIFO/epoch behavior. Existing
registry/lifecycle/timeout/failure tests stay intact. Include an actual-store
check decoding saved suffix content rather than only fake backend call counts
if possible within owned unit modules. No production test switch.

Run and paste ACTUAL command summaries in P5-FINAL-TAIL.md:

- cargo test --locked --offline --lib history_writer -- --test-threads=1
- cargo test --locked --offline --lib history_capture -- --test-threads=1
- cargo clippy --locked --offline --lib -- -D warnings
- rustfmt --edition 2021 src/history_writer.rs src/history_capture.rs

Match Rust2021 style; record what changed, design tradeoffs, failures fixed,
deviations, limits and released ownership. Notify parent when production code
freezes. Parent owns independent spot checks, two sequential release workload
runs with new artifacts (preserve all old raw reports), full non-UI verifier,
all integration targets if needed, and final spec/roadmap evidence. Do not run
benchmarks concurrently with parent. No failed/ignored case counts as a pass.
