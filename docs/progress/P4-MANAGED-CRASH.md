# Phase 4 managed record process-kill checks

Status: software-level process-kill checks for joint managed/provider records
and Prepared/Spent recovery state are complete. They complement the passing
[native manual-recovery](P4-NATIVE-MANUAL-RECOVERY.md) and
[bridge-failure](P4-NATIVE-BRIDGE-FAILURE.md) results; those live checks remain
separate from these synthetic store-boundary tests. Managed normal-UI
foreign-project refusal and final verification now pass separately; see
[scoped Phase4 completion](P4-INTEGRATED-RESULT.md).

This acceptance slice exercises the production managed-provider and prepared
recovery stores across abrupt process termination. It complements in-process
codec, fault-injection, and state-machine tests. It does not claim machine
power-loss or filesystem/controller crash durability.

`tests/p4_managed_binding_crash.rs` launches its own integration-test binary as
a child for each boundary. The child emits only fixed marker labels; the
parent waits for each marker with a ten-second deadline, kills and reaps the
owned child, then reopens the same store. The fixture uses a private temporary
directory and synthetic target, project, provider version, executable digest,
launch operation, and session IDs.

The combined managed-provider record is initialized at revision 1. Process
termination is tested before revision 2 publication, after publication has
returned but before an owner acknowledgment marker, and after the fixture's
owner acknowledgment marker. Reopen verifies revision 1 in the first case and
revision 2 in both later cases. Each loaded combined row is checked for its
full managed target, project, provider version, executable digest, launch
operation key, lifecycle/outcome, and matching provider binding. The restored
binding state is always `Stale`; the historical `Running` snapshot does not
assert a live child or authorize automatic resume.

Prepared recovery is tested after the `Prepared` row is durable, after the
one-shot row transitions to `Spent` but before a fixture action marker, and
after a fixture-owned action and acknowledgment marker. Reopen preserves the
state in each case, duplicate spending of a reopened `Spent` row is refused,
and no prepared/spent record is treated as a selected provider binding. The
action marker is only a test fixture event; no provider is contacted.

Run on the supported Apple Silicon macOS host with the isolated Cargo caches:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --test p4_managed_binding_crash -- --test-threads=1
```

The integration file is platform-gated to macOS arm64. A skipped build on
another platform is not acceptance evidence. Passing process-kill cases show
which atomically published row is visible after these software-level kill
points; they do not establish persistence under power interruption or storage
hardware failure.
