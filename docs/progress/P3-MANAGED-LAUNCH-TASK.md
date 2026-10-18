# Managed-launch next-work planning task — 2026-10-18

User explicitly requested another GPT-6-Luna agent on next work while live-watch
integration finishes. Exclusive owner ONLY docs/progress/P3-MANAGED-LAUNCH-PLAN.md.
Read ROADMAP.md, AGENTS.md and product prerequisites/latest INTEGRATION, then
P3-STORAGE.md, P3-LIVE-WATCH-TASKS.md and current P3-WATCH-* evidence. Graph
first; index if not indexed, source fallback on connector failure. Read sources
before deriving API claims. No source/exports/Cargo/shared docs/roadmap edits,
dependency additions, commits or push. Parent owns roadmap (append-only exception).

Prepare a concrete bounded next implementation plan for generic managed launch
and eventually multiple sessions, grounded in current StateOwner single target,
PtyEffects, StartedServer/LaunchJournal and bounded WorkerHub/ServerDriver.
This is design, not runtime implementation or a completed phase. User product
decisions stay unresolved; use scoped prototype proposals, clearly labeled.

Cover exact full request budget<=65536bytes incl metadata/paths/argv/env; explicit
client launch environment/cwd/executable resolution (no inherited daemon env),
nonreplayable input vs deduplicated launch, volatile secret-bearing request
retention, metadata-only Intent before effects, crash uncertainty after spawn,
atomic registry/result commitment, ID issuance/nonreuse/full target refs and
returned resource identity. Do not propose argv/env/input persistence.

Explain what current startup code can be reused safely and what cannot:
singleton storage lock lifetime, internal bootstrap lease, fresh explicit
server startup vs API launch, one mutable journal borrower, checksum/sync/error
handling, CLOEXEC creation races on macOS before enabling worker-time spawns.
No API launch until descriptor inheritance/spawn serialization reviewed.

Design bounded registry/effect state transition proposal and capacity admission
order; do not silently adopt10sessions with~60MiB core model each. Account for
terminal retained allocations, pending operation payloads/copies, notification
journal/subscriber limits and metadata records. Recommend smallest meaningful
prototype gate (e.g.2explicit sessions) and assumptions requiring parent review.
Hierarchy/layout choice remains open: a prototype registry must not freeze the
workspace/tab model or fabricate right-click support. No autonomous scheduling,
process replacement, provider identity or guaranteed recovery without evidence.

List exact modules/disjoint owner sets and dependency order for a future wave,
exact proposed typed public APIs and byte layouts where sufficiently grounded,
and unresolved questions as decisions rather than invented requirements.
Specify acceptance/fault tests: duplicate/conflict/lost ACK one child, pending
intent/spawn/result faults, registry full before spawn, context/lease/generation
errors, no command auto-replay after restart, mixed session read/input routing,
watch binding/cursor consistency/stale occupant, fair bounded pump, slow writer,
and explicit stop cleanup. Distinguish feasible unit/privatePTY/socketpair tests
from pathname/host/power-loss evidence. Identify release blockers honestly.

Verification: check all local Markdown links, no conflict/trailing whitespace,
git diff --check for tracked work (doesn't cover newfile; explicitly scan it).
Record actual graph/source/doc checks and source-based assumptions, not invented
test runs or model savings. Report concrete next bounded task recommendation,
risks/gotchas, evidence and release ownership. No external research required;
do not browse competitor software or install providers for this task.
