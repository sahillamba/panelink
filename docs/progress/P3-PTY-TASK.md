# Scoped private-PTY server lifetime proof — 2026-10-18

Exclusive next owner: src/server_pty.rs, tests/p3_detach.rs,
docs/progress/P3-PTY.md. Parent owns core/exports/main/Cargo/shared docs; StateOwner
and IO owners own their own files. Read required roadmap/AGENTS/product docs,
P3-STATE-TASK, P3-IO-TASK, P3-SESSION-TASK and P2 cleanup evidence first.
No new dependencies/commits/push. Use original logic and existing TerminalSession.

This is an experimental library adapter and private-PTY lifetime proof, not
production PTY adoption, Apple Terminal compatibility, completed Phase2 or M1.
Host Ctrl-X/clipboard gates remain. A private server experiment can test lifecycle
without raw host IO; do not invent host-validation evidence or claim process
recovery after server death. Public layout/API launch is outside this task.

Implement `PtyEffects::new(TerminalSession)` implementing server::EffectSink
from the StateOwner task. Existing core now has parent-added `text(&str,submit:
bool)` atomic literal text+optional CR, `interrupt()` actual SIGINT to still-owned
child group, `discard_pending_input()`. Do not edit core; report needed fixes.
Adapter owns session, nonzero revision(initial1), failure state if progress fails.
No socket/client lifetimes in this type. No borrowed host terminal/file.

validate is pure: Screen supported; LiveScrollback returns Unsupported until a
reviewed bounded core accessor exists (never silently substitute Screen).
Input requires live child/core and empty pending input queue;32KiB controls/text
already validated upstream, but do not accept >32KiB if called directly. One
effect slot means capacity checked before ledger admit. No separate unbounded
input copy. Read bounded text budget<=65446, max_lines1..1024, from engine-owned
sanitized cells, spaces for blank cells, skip wide continuation, LF between rows.
Stop at UTF8 boundaries before budget/line cap and set truncated truthfully.
Do not emit escape/control bytes or replay raw child output. Revision increments
checked when pump reads output/observes lifecycle changes; overflow fails closed.

Input Text/Submit/Paste enqueues once through core; no per-character partial
admission. Drain through bounded pump calls for at most100ms absolute deadline,
tracking request-byte writes (query replies appended after it are not input
completion evidence). Applied only after all request bytes accepted by PTY,
not app/task success. Queue rejection before writes is Failed; timeout/hard error
returns Partial if known bytes delivered, otherwise Unknown/Failed as justified.
Discard every pending suffix before returning terminal outcome, so owner can
release/handoff and no bytes cross to a new owner. Never return Accepted or retry.
Interrupt uses core.interrupt once; OS accepted signal means Applied, not exit.
Periodic poll pumps bounded progress even with no clients, preserves live state.
Expose current snapshot/session status for testing only as appropriate, no input
owner bypass in a user-facing API. Drop is explicit server-lifetime cleanup only.

Tests using one real shell PTY and current StateOwner APIs: child marker/pid is
constant across logical detach and reconnect; repeat same generation/lease;
new read is current; typed Submit produces one observed marker; lose its ack,
query status/reconcile persistent ClientContext and never resend input; duplicate
same key returns saved status without another marker; stale owner/target fails;
safe Unicode/bounded reads and escape/clipboard output remain sanitized.
Input literal+CR queue atomicity and actual SIGINT semantics should be covered by
real fixtures where practical, using ready markers before signals. Use bounded
polling and Drop cleanup, no daemon left behind. Snapshot markers must not rely
on echoed input to prove application handling (observe an application GOT marker).
IDs/PIDs in these test observations are not provider identity or durable recovery.

If StateOwner API is not present, coordinate with its owner, write adapter to
the exact trait task and independent core tests first. Parent separately proves
actual UnixStream socketpair drop/reconnect through server_io; this task's logical
detach is not a pathname network compatibility claim. No full-layout/resync/event
or multi-client terminal UI promises. Run focused Cargo after parent exports,
or temp harness initially. Record ACTUAL outputs, limits, deviations and gotchas,
release ownership. Parent independently verifies/reviews.
