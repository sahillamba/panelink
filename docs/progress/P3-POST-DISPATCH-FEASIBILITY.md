# Phase 3 post-dispatch client-death barrier feasibility

2026-10-23. Read-only parent/Luna feasibility audit for the roadmap's proposed
acceptance after Submit reaches server dispatch but before ledger admission.
No code or executable behavior changed in this audit.

## Finding

The current integration fixture cannot deterministically pause the real server
inside this interval without a new injection seam. It launches the real
`panelink` server as a separate process and owns the real Submit CLI child; its
authenticated proxy can hold the request before upstream delivery, or hold a
response after the server has processed the request. Once the proxy forwards
Submit, the real owner handles validation and admission synchronously on its
owner path. The production `PtyEffects::validate` is a bounded synchronous
capacity/availability check and exposes no barrier. Delaying the child, adding
a sleep, or observing socket timing would not prove that the request reached
the exact pre-admission point.

`SessionHandler::handle_input` provides a deterministic internal boundary:
clone the current input owner and begin a candidate pin, call
`EffectSink::validate`, then call `ledger.admit`; only after admission does it
commit the live owner pin and perform input. An in-crate unit test can supply a
test `EffectSink` whose `validate` blocks on a bounded channel, signal that the
handler reached validation for the exact Submit, and inspect behavior around
release. Such a test exercises `StateOwner::handle_control` directly. It does
not prove socket ingress, real CLI persistence, or that a real CLI process died
after dispatch.

The `#[cfg(test)]` library unit-test build cannot expose a test sink to the
separately compiled production binary used by `tests/p3_inflight_client.rs`.
Bridging the real CLI to an injectable server would need a dedicated
in-process/test-support harness or a production-visible hook. The former
requires separate design to preserve the real context/CLI process evidence; the
latter is outside the no-production-hook boundary of this slice.

## Decision and next bounded action

Do not attempt a real-CLI post-dispatch kill using timing, PTY pressure, or the
current proxy. The existing
[pre-delivery acceptance](P3-PRE-ADMISSION-CLIENT-DEATH-RESULT.md) remains
valid for its narrower boundary. The next suitable
automated slice is a deterministic in-crate `StateOwner` unit test with a
blocking fake `EffectSink::validate`, if this internal-dispatch evidence is
useful. Name it as handler-boundary evidence; do not call it real client-death
or socket-ingress acceptance. A real CLI death at this point remains unproved
unless a separate test-only in-process integration architecture is designed.

Keep admitted live-owner-pending/PTy interruption, human four-pane, provider
identity/recovery, and reboot recovery separate. Do not add automatic input
resend, new retry policy, a production barrier hook, wire/storage changes, or a
new dependency as part of this feasibility work.

## Source evidence

- `src/server.rs:1372-1419` — handler order: candidate owner begin, sink
  validation, ledger admission, live owner pin commit, effect, terminal ledger
  completion and pin clear.
- `src/server.rs:584-592` — `StateOwner::handle_control` dispatches directly
  through the session handler and is exercised by in-crate unit tests.
- `src/server_pty.rs:988-1025` — the production PTY validation path is
  synchronous and has no test latch.
- `src/server_io.rs:311-465,641-658` and `src/server_runtime.rs:151-191` —
  socket worker submits to the server owner; owner dispatch is serialized.
- `tests/p3_inflight_client.rs` — its server fixture is a separate binary
  process, and its proxy can stop before forwarding or after receiving a
  terminal server reply, not inside handler validation.

## In-crate handler-boundary test delivered — 2026-10-23

Added `server::tests::submit_validation_barrier_precedes_admission_and_duplicate_does_not_replay`
in `src/server.rs`. A test-only sink blocks the first exact Submit inside
`EffectSink::validate`; the test observes its revision/text and confirms no
input effect has occurred. Releasing validation returns `Completed(Applied)`
with one input call; the exact duplicate returns the cached terminal result
without another validation or input effect.

The focused parent rerun passed: **1 passed, 0 failed**, with 716 library tests
filtered out. This is deterministic internal handler-boundary evidence only.
It does not provide a socket ingress barrier, real CLI child death, concurrent
ledger-status observation during the barrier, or admitted owner-pending/PTy
interruption evidence. The real-CLI pre-delivery test remains separate. The
package formatting check (`cargo fmt -- --check`), Markdown checker and diff
whitespace check passed. The broader `cargo fmt --all -- --check` also inspects
vendored `vt100` files and reports their existing formatting differences; no
vendored files were changed. No production behavior changed.
