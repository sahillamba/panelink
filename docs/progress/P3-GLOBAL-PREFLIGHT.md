# Global request preflight seam

## Scope

`GlobalAuthority` now performs an immutable preflight for control and owner
mutation requests. Each returns a fixed private decision: `Continue`, a normal
rejection code, or a known operation status. The preflight requires live lease
authority, checks key/capability consistency, and checks the one global ledger
for an exact cached result or changed-byte conflict. It does not select or
inspect a target, reserve a new key, mutate a lease, or invoke a sink.

`SessionHandler` renders rejected/known preflight decisions and only sends
`Continue` through the existing target staleness and pinned-scope checks,
command/owner validation, and actual `Ledger::admit` path. The read and input
paths still require actual admission before any corresponding effect. Owner
mutations likewise retain candidate-state validation before admission and only
commit after the ledger reserves the operation. No endpoint, public type,
wire-schema or permission change is introduced.

The response encoder is now a private free function. `preflight_reply` uses it
to create the same bounded status/rejection payloads without borrowing a
`TargetState` or generic effect sink. `Continue` maps to no reply and carries no
execution authority; only a later `Ledger::admit` can authorize dispatch.

## Evidence

- `cargo test --locked --offline server::tests::`: 33 passed, 0 failed.
- `cargo test --locked --offline --test p3_socket_detach`: 2 passed, 0 failed.
- `cargo test --locked --offline --test p3_managed_launch`: 2 passed, 0 failed.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --offline --no-deps`: passed.
- `python3 scripts/check_docs.py`: 123 Markdown files checked, 0 findings.
- The added global-preflight test verifies both control and owner new-key
  decisions leave usage unchanged and keys NotAdmitted; exact completed results
  render from global state, changed-byte reuse remains a global conflict, and
  missing authority fails before admission. It calls the global preflight and
  reply encoder directly, without routing through a `SessionHandler`.
- Existing scope tests still verify wrong-target Submit is denied even when
  that selected target's input owner validly names the connection lease.
- `git diff --check` passed for the two task-owned files.

## Limits

This is a private routing prerequisite. No multi-session router exists, and
preflight decisions do not certify a target, establish additional authority,
or authorize process effects. The one-target runtime remains unchanged.
