# P3 authoritative watch owner — 2026-10-18

## Delivered scope

`StateOwner` now tracks a bounded `EventJournal` from sequence zero. The
constructor records the screen revision, vacant owner revision, and child-exit
state as a baseline without inventing an initial event. Events are advertised
only when the sink reports a nonzero screen revision and tracking remains
healthy. Resume negotiates against the intersection of the lease's original
grant and current availability, so Events cannot be added later or retained
after tracking becomes unavailable.

Public control/session handling and polling synchronize committed screen,
owner, and exit observations in that order. Duplicate, rejected, and preflight
requests add no event when metadata did not change. Owner expiry is detected by
poll. Reconcile and event-page reads also synchronize after authorization and
exact-target checks; neither performs a hidden sink poll. An unrepresentable
Human owner, missing/zero/backward screen revision,
owner metadata inconsistency, or journal append failure permanently disables
event delivery for this owner. Those failures do not replace normal control or
poll outcomes, stop the child, revoke input, or claim rollback of an already
applied input.

`reconcile` authorizes the exact live Read|Events connection and target, reads
one bounded sanitized screen, synchronizes metadata, and checks that the read
revision still matches the sink revision before binding session info, text,
and journal cursor. A changed screen is journaled before a mismatched snapshot
is rejected. `read_events` returns bounded ordered pages, explicit cursor and
target errors, and an empty latest-cursor hint when retention expired.

## Verification

- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib server::tests:: -- --nocapture` — exit 0, **29 passed, 0 failed, 0 ignored**.
- `rustfmt --check src/server.rs` — exit 0.
- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings` — exit 0.
- Focused cases cover capability negotiation/resume, snapshot and cursor
  consistency, event ordering and expiry, duplicate/rejected requests, cursor
  errors/retention, invalid budgets and text, revision races, unavailable
  tracking with continued input, sequence exhaustion after accepted input,
  Human-owner fail-closed behavior, and Debug redaction.

The first focused test run had two test failures. The Debug assertion searched
for the generic word `screen`, which also appears in field names; it now checks
a unique sensitive text sample. The codec enforces the schema's global line
ceiling, so the owner also needed to enforce each request's smaller `max_lines`
before encoding; a regression now covers that request limit. These corrections
did not change existing input behavior.

The focused owner checks do not establish real socket/PTY delivery; the parent
owns that integration and full-runtime verification. Phase 2 host acceptance
and broader Phase 3 completion remain open.
