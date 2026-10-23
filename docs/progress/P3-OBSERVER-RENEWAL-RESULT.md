# Phase 3 workspace observer lease renewal

Status: bounded client renewal implemented for parent integration, 2026-10-23.

The workspace observer now starts a conservative local lease clock before its
initial Hello and proactively resumes its saved lease 300 seconds before the
30-minute TTL. Renewal reconnects without an AttachTarget trailer and requests
only `READ | WORKSPACE_EVENTS`. The client requires the exact saved generation
and lease and rejects any selected capability mask beyond those two bits.
Pane input and pane-event access may therefore disappear after the original
pane retires without blocking workspace observation.

Renewal is attempted at most three times, 15 seconds apart, while the local
expiry estimate remains valid. Transport failures may retry; unknown/expired
leases, generation changes, malformed handshakes and missing workspace grants
end the observer with an error. The renewal path never removes or replaces its
context file and never opens a fresh lease. A successful renewal resets request
IDs and schedule state. Workspace watches retain the caller's original cursor,
so a missed journal range produces the server's real `ResyncRequired` response
and the existing observer path reconciles before continuing.

Deterministic tests advance monotonic `Instant` values without waiting for the
production TTL. Socketpair handshake tests prove two successive same-lease
renewals, exact read/workspace-only negotiation, refusal of a generation
change, and refusal of a response that selects input as an extra capability.
Another socketpair test confirms the observer sends its original cursor after
renewal so the server can return an authoritative journal-gap result. The lease
schedule test covers the 25-minute renewal point, retry spacing, three-attempt
bound and expiry boundary.

Verification performed:

- `cargo test --bin panelink observer_ -- --test-threads=1` — 4 passed, 0
  failed.
- `cargo fmt -- src/p3_cli.rs` and `git diff --check -- src/p3_cli.rs` — passed.
- `cargo test --test p3_agent_cli -- --test-threads=1` — blocked in this
  execution context: all four existing integration scenarios failed to start
  the private listener with `listener_permission_denied`; these are not test
  failures attributed to renewal behavior.

Parent integration remains responsible for exercising renewal against the
multi-session server after retiring the original pane and validating the
server's same-grant workspace-only resume behavior. This slice does not
complete Phase 3 or claim full integration verification.

## Parent integration

The parent integrated and reviewed same-grant server renewal after retirement
of the original pane. Two logical-clock TTL cycles pass, retaining the same
lease/target and Read|Workspace connection, with expiry, generation change and
capability-widening refusal. All4 renewal client tests and all5 real agent CLI
integration scenarios pass in the final permitted-context serial suite. The
original sandbox listener failures above remain blocked executions, not passes.
See [the final result](P3-PANE-METADATA-RESULT.md).
