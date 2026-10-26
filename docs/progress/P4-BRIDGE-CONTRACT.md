# P4 typed provider bridge admission contract

Follow-up: [separate wire and scoped Unix channel](P4-BRIDGE-TRANSPORT-RESULT.md).

Status: implemented as an in-process typed frame receiver around the provider
binding core. This slice does not add a wire codec, peer authentication,
production socket, plugin lifecycle, server activation, or provider recovery.
It does not claim current provider identity at a crash.

## Contract

The receiver admits protocol version 1 frames for exactly OpenCode 2.0.32.
Each immutable frame carries the complete configured target, bounded opaque
project identity, exact provider version, nonzero bridge instance, contiguous
nonzero source sequence, top-level or child scope, and one selection state:
an exact bounded session ID with its evidence source, Home, Child, Unknown, or
Unavailable. Frame and error debug output is fixed and redacted.

BridgeReceiver owns its ProviderBinding and does not expose mutable core
access. New and restored receivers require request_reconciliation; a distinct
fresh top-level session frame after that request is needed before a pending
binding becomes visible. Invalidations, Home, child activity, ambiguity,
unavailability, clock regression, or lease expiry close pending/current
eligibility while retaining the binding core's last-confirmed record and
revision watermark. A wrong bridge instance requires explicit bridge
replacement. Source sequence gaps can recover only by replaying the missing
contiguous rows after reconciliation or by replacing the bridge; reconciliation
does not forgive a source gap.

The host supplies a monotonic millisecond clock to receive, confirm_pending,
and tick. Lease duration is an explicit caller policy bounded to 100–60,000
ms; it is a prototype policy, not a production default. Expiry is checked
before admitting a frame and at confirmation. An exact duplicate of the latest
full frame is idempotent and never renews the lease, even when the caller
supplies a later wall timestamp. A distinct fresh row for the same current
session renews the lease without creating another pending revision. Selected
session observations also receive a separate nonzero host wall timestamp,
stored as the binding's verification time.

The receiver issues binding revisions monotonically for each distinct
observation, retains its floor across bridge replacement, and requires the
restorer to supply a floor at least as high as the restored confirmed
revision. confirm_pending(revision, now_ms, writer) checks expiry and exact
revision before calling the immutable writer. Writer failure and stale
acknowledgments never promote a pending binding.

## Public integration surface

- LeasePolicy::new(lease_ms) validates the bounded caller policy.
- Frame::new(...) validates and owns the typed version-1 frame.
- BridgeReceiver::new(...) and restore(...) bind target, project, provider
  version, bridge instance, policy, and revision floor.
- request_reconciliation() establishes the current source-sequence
  watermark; receive(frame, monotonic_ms, verified_at_unix_ms) admits a row.
- tick(monotonic_ms) expires idle leases. mark_unavailable(monotonic_ms)
  lets a transport/decoder fail closed when it cannot construct a valid frame.
- pending, last_confirmed, state, and revision_floor are read-only.
- confirm_pending is the only path to durable confirmation.
- replace_bridge(new_instance) resets source sequencing while preserving
  the confirmed binding and host-issued revision floor.

Typed frame construction errors happen before receiver admission. A transport
must call mark_unavailable when decoding or protocol validation fails so an
unreadable source cannot leave a prior current identity eligible.

## Boundaries and next work

The caller remains responsible for source authenticity and sequence ordering.
It must issue a fresh, nonreused source instance for each replacement, call
`tick` before exposing time-sensitive state while delivery is idle, and check
freshness again after a potentially blocking writer before advertising current
eligibility. Read-only accessors do not consult a wall clock. This synchronous
component does not establish an IO deadline for the external writer. A restored
revision floor is caller-supplied; persisting any unconfirmed watermark across
process loss is separate runtime work.
This module does not serialize frames, authenticate peers, discover or launch a
plugin, listen on a socket, persist bindings, or resume provider sessions. The
parent verified the store and process-kill/reopen acknowledgment boundary in
[P4-BRIDGE-STORE-RESULT.md](P4-BRIDGE-STORE-RESULT.md).
The existing binding core validates identity values and performs immutable
writer acknowledgment; this receiver adds typed frame ordering, source
sequence admission, reconciliation gating, host receipt expiry, and revision
issuance around that core.

## Verification

Command:

    CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --lib provider_bridge -- --test-threads=1

Result: exit 0; 12 passed, 0 failed, 0 ignored, 734 filtered out. The requested
P2 Cargo cache path was empty, so the parent supplied a prepared cache at
/private/tmp/panelink-p4-cargo-home and a dedicated target directory to avoid
touching the active launcher build. Rust formatting was applied only to
src/provider_bridge.rs with rustfmt --edition 2021.

The first focused run exposed a test fixture attempting to construct sequence
zero, which the frame validator correctly rejects before receiver admission.
The regression scenario now admits sequences one and two before submitting
sequence one again; the final run above passes. Inline tests cover frame
ordering, duplicates and conflicts, gaps, scope/configuration mismatch, lease
boundaries and idle expiry, explicit reconciliation, child/Home/Unknown/
Unavailable invalidation, timestamp handling, writer failure and stale
acknowledgment, revision restore and bridge replacement, bounds, overflow, and
redaction.
