# P3 single-target state owner — 2026-10-18

Status: implemented and verified in a path-import harness; parent integration
and source review remain pending. This is a synchronous in-memory admission
owner around a fakeable effect boundary. It does not complete Phase 3 or claim
an authenticated runtime, durable generation/recovery, a real PTY adapter, or
Apple Terminal behavior.

## Delivered contract

`src/server.rs` adds `EffectSink`, `ReadResult`, opaque `Connection`,
`SessionReply`, redacted `ServerError`, and `StateOwner<E>`. `new` creates a
fresh volatile ledger; `with_ledger` adopts the exact supplied ledger so the
parent can preserve an internal launch lease/key and continue issuing client
leases above its high-water mark. Pre-existing ledger leases are not resumable
client connections unless StateOwner has its own grant record for them.

The owner validates one nonzero target and one generation. It negotiates READ
and, when explicitly enabled at construction, INPUT; it never advertises
EVENTS. Capability requirements are validated before lease issue/renewal. A
new lease records the capabilities actually selected for that lease. Resume
requires the same generation and a known, unexpired grant, renews the existing
ledger lease, and reuses its identifier. Failed resume never silently creates
a replacement. The 64-lease bound comes from the ledger; it also counts any
active parent-supplied internal lease.

Control handling decodes the complete bounded payload, verifies the live
connection, matching connection lease, and capability before consulting
duplicate state. `existing_admission` only observes an exact recorded key and
payload; it never dispatches. Exact duplicates return the saved operation
status before target/owner/preflight checks, and read duplicates return status
rather than cached screen text. New operations require the current target,
applicable input owner, and pure sink preflight before `Ledger::admit`. Only
`Admission::Execute` invokes the sink. Capacity and precondition failures
return fixed rejection codes without reserving the key.

For input and interrupt operations, the owner prepares an `InputOwner` clone
with the exact operation pinned, admits the key, commits that pin, calls the
sink once, completes the ledger with a terminal outcome, then clears the exact
pin. Unexpected `Accepted` is recorded as `Unknown`. The sink contract requires
synchronous handling and accounting for any unwritten suffix before return;
input is never automatically resent. `Applied` describes the adapter's
reported interface-level outcome, not child-application success.

Read requests reserve 90 bytes of their requested complete-response budget for
schema metadata and pass only the remainder as the text budget. Before any
text is encoded, the owner checks nonzero revision, requested line count,
response-codec text rules, and complete encoded size. Invalid or over-limit
sink output completes as `Failed` and is not returned. A `String` supplies
valid UTF-8 by type; the response codec rejects control scalars other than LF.
The sink must still provide sanitized display text, not raw terminal control
sequences.

Claim, release, and handoff are deduplicated as exact session-payload mutations.
Each prepares a cloned owner transition before admission and commits it only
after `Execute`. Handoff accepts only another active lease with an INPUT grant.
Describe returns current single-target/client-owner/child-exit metadata and
fails closed if a Human owner cannot be represented by the current schema.
OperationStatus is observation-only. Receipt retires completed ledger results
and returns the defined schema-1 Applied status without a process effect.
Detach returns `SessionReply::Detached` and leaves the sink, owner, and ledger
intact so the transport can acknowledge and close only that attachment.

`poll(now)` explicitly expires ledger leases, removes expired grant metadata,
expires input ownership, then calls one bounded sink poll. A pinned input owner
remains pinned when expiry is due. This module has no queue, worker thread, or
connection reference count. Socket peer authentication and attachment
admission remain the caller's responsibility.

## Fake-sink coverage

The 20 StateOwner tests cover: exact duplicate input dispatch once and changed
payload conflict; duplicate ownership mutation before changed owner revision;
stale target/owner/lease and missing capabilities with no admission/effect;
observation-only status; sink capacity and pending-input rejection before
admission; Partial, Unknown, and unexpected Accepted input results without
replay; full read metadata budgeting, line/revision/control validation, and
suppression of invalid text; negotiation before issue and no EVENTS; same-
generation lease resume and old-generation failure without replacement;
`with_ledger` preservation and lease watermark; detach without sink teardown;
vacant/client Describe and rejection of a Human owner; expiry, pending pin,
64-active-lease bound, and time overflow; and handoff to unknown/non-input
leases. These are deterministic fake-sink tests only.

## Verification

Code graph check completed before implementation: project
`Users-sahillamba-panelink` reported `ready`, 2,053 nodes and 6,561 edges. Graph
search located the typed control, session, response, and ledger modules; source
inspection then fixed their exact APIs.

Temporary path-import harness files are outside the repository under
`/private/tmp/panelink-state-cargo-harness`. No manifest, lockfile, crate
export, socket, PTY, generation storage, or shared roadmap file was changed by
this task.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-state-cargo-harness/Cargo.toml -- server::tests --test-threads=1
```

Actual result: 20 passed, 0 failed, 0 ignored; 97 filtered out; 0 doctests.

The complete imported-module harness was also run:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-state-cargo-harness/Cargo.toml -- --test-threads=1
```

Actual result: 117 passed, 0 failed, 0 ignored; 0 doctests. The total includes
20 StateOwner tests plus tests from the path-imported protocol/codec/owner
modules; it is not a whole-crate Cargo test.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --offline --manifest-path /private/tmp/panelink-state-cargo-harness/Cargo.toml --all-targets -- -D warnings
```

Actual output: `Finished dev profile ...` with exit status 0.

Command:

```text
rustfmt --edition 2021 --check src/server.rs
```

Actual output: no output; exit status 0.

An initial harness run found that a read-only claim returned a `ServerError`
instead of the fixed Unauthorized response already used for denied control
operations. The handler was aligned to return the typed rejection before
duplicate lookup; the final full harness above passed. A source review also
caught and corrected `with_ledger` resetting the supplied ledger; its regression
now proves the caller's ledger history survives adoption.

## Limits and handoff

Lease/operation state remains volatile and caller-supplied logical times must
be monotonic. The ledger's 4,096 permanent lease-record, 4,096 retained
operation, 8 MiB logical-payload, and 1,024 per-lease unreceipted-operation
limits still apply. The metadata grant map is bounded by the 64 active-ledger
lease limit. No provider identity, clipboard, events, autonomous work, stored
history, live-scrollback capability, durable retry, or runtime process
authorization is established here. A fake sink cannot prove real input write,
drain, child lifecycle, scheduling, or host restoration behavior.

The parent owns exports, root Cargo integration, durable generation/journal
startup, and real PTY/runtime adapters. Worker file ownership for
`src/server.rs` and this evidence file is released for parent review.
