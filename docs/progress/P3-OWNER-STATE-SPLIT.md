# StateOwner target-state split — 2026-10-18

This bounded refactor groups the existing per-target data inside
`StateOwner<E>` into one private `TargetState<E>` value in `src/server.rs`.
The group contains the target reference, input owner, effect sink, event
journal, notification availability, last screen revision, last owner
observation, and last child-exit observation. `StateOwner` retains its
generation, input policy, single global operation ledger, and reusable lease
grants directly. Its constructor moves the existing values into this group.

No dispatch, authority, protocol, capability, lifecycle, or multi-session
behavior was added. Public method signatures and const accessors remain the
same. Existing test fixtures only changed their private field paths; semantic
assertions and test coverage were preserved. No extra tests, dependencies,
exports, or shared-document changes were made.

The graph query for `StateOwner` could not run because the graph service
reported that `Users-sahillamba-panelink` was not indexed. Direct source
inspection was used instead.

## Verification record

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline
--lib server::tests::` passed all 29 selected tests. The socketpair integration
target `p3_socket_detach` passed 2 tests, including reconnect/detach on the
same server-owned PTY. The managed-launch integration target
`p3_managed_launch` passed 2 tests. These checks confirm existing behavior for
this private layout refactor; they do not establish multi-session dispatch or
complete any milestone.

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` passed.
Strict all-target Clippy (`cargo clippy --locked --offline --all-targets -- -D
warnings`) passed. The documentation checker reported 1 file and 0 findings;
`git diff --check -- src/server.rs` passed. The parent owns the integrated
verifier run and review.
