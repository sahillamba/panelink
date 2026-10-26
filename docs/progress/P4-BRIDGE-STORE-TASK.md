# Phase 4 bridge admission and last-confirmed storage — 2026-10-24

Status: both component slices implemented and parent-verified. See
[P4-BRIDGE-STORE-RESULT.md](P4-BRIDGE-STORE-RESULT.md). Two GPT-6 Luna High owners implement exclusive new Rust
modules and their inline tests; parent owns exports, combined acceptance,
review and shared planning notes. No dependencies, live workspace restart,
provider configuration or automatic resume are authorized by this slice.

## Slice 1: typed versioned admission

`provider_bridge` binds one complete target, canonical project identity, exact
provider version and source instance. Protocol version1, source sequence and
host receipt time govern ordering and lease eligibility. Exact duplicates do
not renew. Gaps/conflicts, wrong scope, unknown/Home/child identity and expiry
invalidate current eligibility while retaining the prior last-confirmed record.
Fresh rows after explicit reconciliation are needed to restore eligibility.
Binding revisions continue above the restored confirmation across instances.
Only an eligible pending value may enter the durable writer callback.

The module connects to the existing pure binding core. Transport encoding,
peer authentication, the provider plugin and live state-owner dispatch remain
separate integration work; typed frames alone do not prove source authenticity.
The caller-selected bounded lease is prototype policy, not a measured SLA.

## Slice 2: last-confirmed storage

`provider_binding_store` uses the existing bounded record codec in one private
caller-selected per-target directory. Held nonblocking exclusive writer lock,
validated directory/file descriptors, exact scope and monotonic revision checks
protect its single slot. File synchronization precedes atomic rename; directory
synchronization precedes success. Post-rename uncertainty disables writes until
reopen. Interrupted temporary bytes are never promoted into confirmation.

Restoration always starts stale. A committed record may exist even when its
acknowledgment was lost; neither that case nor success proves current-at-crash
provider selection. No terminal content or credentials are stored.

## Verification

Workers run focused locked/offline serial library tests and format their owned
files. Parent independently reviews admission/storage boundaries, runs combined
storage acknowledgment and process-interruption scenarios, then serial Rust and
supporting repository checks. Fault-injected syscall ordering, actual process
kill/reopen behavior and full machine/power-loss recovery are distinct evidence.
All unresolved identity and human host gates from the prior handoff remain open.
