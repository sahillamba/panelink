# Phase 4 bridge admission and binding store result — 2026-10-24

Follow-up: [bounded wire and scoped Unix channel](P4-BRIDGE-TRANSPORT-RESULT.md).

Status: **both scoped component slices implemented and parent-verified** with
two GPT-6 Luna High owners, exclusive files and parent integration/review.
The modules are exported by the library; application startup, live plugin
transport, owner dispatch, UI recovery and automatic resume are not enabled.
Full Phase4 remains open.

## Slice 1: versioned typed bridge admission

`src/provider_bridge.rs` wraps the existing binding core in a protocol-v1
receiver for exact OpenCode2.0.32. Full target/project/version/instance and
contiguous source sequence are required. Duplicate frames never renew a lease;
conflicts/gaps or wrong scope revoke eligibility. Home, selected child,
unknown and unavailable states cannot become a top-level binding.

A bounded caller-selected lease uses explicit host monotonic milliseconds.
The caller must tick idle receivers. Expiry is checked before admission and
confirmation; resumed delivery cannot silently restore eligibility. Explicit
reconciliation and a distinct fresh row are required. A separate nonzero
host Unix timestamp becomes the saved verification time. New binding revisions
continue above the caller's restored floor and across bridge replacement.
The mutable core is private; only eligible pending values reach the writer.

This is a typed contract, not a wire format or proof of source authenticity.
The trusted caller must issue nonreused instances, supply clocks, fail closed
on decode errors, enforce IO deadlines and check freshness after blocking IO.
Gap recovery currently requires contiguous missing metadata or a fresh bridge
instance and reconciliation; it does not authorize terminal-input replay.
Persisting an unconfirmed revision floor across process loss remains runtime
work. See [contract and tests](P4-BRIDGE-CONTRACT.md).

## Slice 2: durable record component and acknowledgment boundary

`src/provider_binding_store.rs` persists the existing bounded codec record in
one caller-selected canonical private per-target directory. It holds a lifetime
nonblocking exclusive lock, checks named directory/lock inode identity and
refuses unsafe files. Exact scope and increasing binding revision are required;
identical bytes are an explicit idempotent resynchronization.

The write order is exclusive temporary file, bounded write, file sync, atomic
rename, directory sync, success. Only success may confirm the core. Ambiguous
post-rename sync failure disables that store instance until reopen. Interrupted
temporary bytes are never promoted; explicit safe discard is separate. Malformed
committed bytes are preserved and refused. Diagnostics omit identity values.
See [storage contract and tests](P4-BINDING-STORE.md).

The parent-owned `tests/p4_bridge_binding.rs` verifies admission → storage →
confirmation, exact verification timestamp, rejection of expired acknowledgments,
no revival without reconciliation, old confirmation retained on save failure,
and stale restoration after a new bridge instance.

It also uses real child-process handshakes and SIGKILL at three boundaries:

| Kill boundary | Record after reopening | Restored state |
| --- | --- | --- |
| Pending observation, before storage | Previous revision1 | Stale |
| Storage succeeded, before core acknowledgment | New revision2 | Stale |
| Core acknowledged | New revision2 | Stale |

Every child is reaped, including failure paths. Reopening proves the kernel
released writer exclusion. The second row demonstrates lost-ack uncertainty:
a newer record can exist without acknowledgment being delivered. Neither the
record nor an acknowledgment proves the provider's selection at crash time.
These are process-kill/filesystem observations, not physical power-loss or full
machine-reboot acceptance. Fault-injected pre-rename/dir-sync failures are
separate unit evidence. No joint transaction with managed-session metadata or
server state-owner integration is claimed.

## Parent verification

All Cargo commands use locked/offline resolution with
`CARGO_HOME=/private/tmp/panelink-p4-cargo-home` and
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.

| Check | Actual result |
| --- | --- |
| Full serial Rust suite | **1136 passed, 0 failed, 15 ignored**,191.629s including isolated compilation; library743, main binary148, integration245 |
| Final focused provider library suite | **37 passed**,0 failed/ignored; includes core/codec and21 new bridge/store tests |
| Final combined integration target | **4 passed**,0 failed/ignored; includes three actual kill boundaries |
| Strict all-target Clippy | Pass |
| Formatting and isolated build | Pass |
| Input decoder | **39 passed**,0 failed/ignored |
| Component source fingerprints | Both match |
| Python discovery | **160 passed**,no failures/skips,8.287s |
| Documentation and whitespace | 295 Markdown files checked; zero findings |

The full Rust command ran separately with a360s deadline for fresh isolated
compilation; the seven supporting checks ran separately. This is not a claim
that the unmodified180s combined verifier ran8/8. Ignored tests remain ignored,
and the separate Phase3 human host gate remains open.

Receipts/logs: `/private/tmp/panelink-p4-20261024/bridge-store-*`, including
`bridge-store-cargo-summary.json`, `bridge-store-cargo-serial.log`,
`bridge-store-supporting.json` and `bridge-store-final-checks.json`.
The earlier160-test Python and live provider fixtures are retained as separate
evidence; no new provider process or prompt was needed for this component work.

Initial failures are preserved: the old temporary Cargo cache lacked locked
`libc`; an isolated cache was copied from the installed registry without
credentials/config. Development compile errors involved an extra `.as_str()`
on `&str` and the parent callback adapting `SaveOutcome` to `()`. A sequence-zero
test fixture was rejected and corrected. Initial Clippy reported a collapsible
condition and needless borrow; formatting required module ordering. Parent
applied equivalent style corrections and reran the final37+4 focused tests,
strict Clippy, formatting and build. No dependency/lockfile change.

The activated `target/debug/panelink` binary is unchanged at SHA-256
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
No live workspace, launcher or user/provider configuration changed.

## Next slices

Implement bounded transport encoding/handshake and source/target authentication,
then opt-in owner/worker integration with ordered storage completions and
reconciliation/expiry publication. Production heartbeat policy, cross-generation
managed-session linkage, manual last-confirmed recovery and user-visible failure
states remain open. New-conversation ID, nonempty copied-history fork and
same-client project switching remain unsupported gates. Preserve explicit
last-confirmed/manual fallback; no current-at-crash or automatic resume claim.
No commit or push.
