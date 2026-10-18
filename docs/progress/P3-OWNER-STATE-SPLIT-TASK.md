# Owner state split — bounded prerequisite, 2026-10-18

This adopts only a private data-layout refactor toward milestone 3. It adds no
multi-session dispatch, authority, protocol, capability, or lifecycle behavior.
The current runtime remains one target. Exclusive future owner: src/server.rs
and P3-OWNER-STATE-SPLIT.md; parent owns all other files and verification.

Introduce one private `TargetState<E: EffectSink>` containing exactly the
per-target state currently held directly by `StateOwner`:

```rust
struct TargetState<E: EffectSink> {
    target: TargetRef,
    input_owner: InputOwner,
    effects: E,
    journal: EventJournal,
    notifications_available: bool,
    last_screen_revision: Option<u64>,
    last_owner_observation: Option<OwnerObservation>,
    last_child_exited: bool,
}
```

Keep generation, allow_input, ONE Ledger and ONE grants table directly on
`StateOwner`; add `session: TargetState<E>`. Move the existing constructor fields
into that one session value. Update internal field references explicitly and
preserve every public signature, const accessor, error, validation order,
deduplication-before-mutable-precondition behavior, effect order and output.
Do not use Deref tricks, active-target swaps, cloning a Ledger/grant table,
unsafe aliases, extracting/replacing live handles, or introducing a second
session yet. Do not move code to other files in this step. Keep Debug redacted
and do not require E: Debug. Only existing unit fixtures' private-field paths
may change; do not weaken assertions, rename away coverage or add ignored tests.

Existing semantic tests provide the regression evidence. Run the exact owner
filter `cargo test --locked --offline --lib server::tests::`, real socketpair
and managed-launch integration tests as available, strict all-target Clippy,
package fmt and docs/diff checks with the writable Cargo cache. Parent runs the
integrated verifier after review. No mirror-the-implementation tests are needed
for private grouping alone. Record actual counts and unchanged boundaries.
No dependencies, shared documents, module exports or commits.

Next stage must specify an actual borrowed global-authority/per-session handler
seam and ownership integration with ManagedLaunchCoordinator before introducing
a collection of sessions. This data split alone does not settle those APIs.
