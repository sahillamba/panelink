# P5 workspace routing CLI slice — 2026-10-25

Status: implementation added; no Cargo command was run as requested. The parent
must compile and run the focused checks using the existing locked/offline
target cache before treating this slice as verified.

## Implemented

- Added opt-in `workspace list`, `project create`, `tab create`, `select`,
  `open`, `stop`, and `remove` dispatch before the legacy no-argument and
  existing command paths. IDs require nonzero decimal `u64`; names must be
  UTF-8 and are validated by the bounded metadata codec. Diagnostics are fixed
  labels and do not print paths or environment values.
- Added lazy additive Default/Main initialization for mutating/routing
  commands. New project and first-tab IDs consume separate sequential values
  from the persisted watermark. List prints stable IDs, user names, order, and
  active/selected markers. If the registry is absent, list reports that the
  registry is uninitialized and that the legacy Default/Main route is
  available; it reads no registry lock and creates no directories/files.
- Project 1 / tab 1 resolves to the original application root paths. Other
  numeric routes use `projects/<project-id>/workspaces/<tab-id>/server/` and
  `contexts/`, with shared application `terminfo/`. Namespace components are
  created and checked one at a time. Socket path length is checked before
  namespace creation/start.
- Explicit open validates that the exact route exists, drops the registry
  store lock, then prepares the route. It revalidates registration while
  holding that route's lifecycle lock immediately before ensure/start, so a
  concurrent remove cannot orphan a newly started server. Stop targets only
  that route and uses its existing lifecycle lock plus recorded PID/start-time
  verification. Remove checks liveness under the route lifecycle lock, then
  reloads the registry and revision through the store while still holding the
  lifecycle lock; a running, ambiguous, or unsafe route is retained.
- Added a focused integration fixture for additive default preservation,
  create/select persistence, invalid route refusal, read-only missing-registry
  list, and corrupt-registry refusal.

## Scope and verification

Only `src/main.rs`, `src/default_open.rs`, the new `src/workspace_cli.rs`, and
the new `tests/p5_workspace_routing.rs` were changed for code. The UI client
API and `TargetRef` remain unchanged. Existing no-selector dispatch stays on
the legacy default path. There is no in-client tab UI or hot switching.

Completed: `rustfmt --edition 2021 --config skip_children=true src/main.rs
src/default_open.rs src/workspace_cli.rs tests/p5_workspace_routing.rs` and
`git diff --check` (both exit 0). Cargo compilation/tests were not run by
instruction. Proposed parent checks, serialized against its other work:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --test p5_workspace_routing -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --bin panelink workspace_cli::tests -- --test-threads=1
```

The fixture tests cover metadata and refusal behavior; they do not establish
manual Apple Terminal attach behavior, server survival after client detach,
multi-tab UI, or provider recovery. A stale/ambiguous PID record deliberately
blocks removal pending successful route stop or explicit repair.

## Parent review corrections — 2026-10-25

The implementation follow-up corrected the owned-file compiler findings:
`open_routed_workspace` now types its closure result as `Result<(), OpenError>`,
endpoint checks use the existing `EndpointState::Absent` variant, and
`route_exists` binds the iterator result before returning so the registry
record outlives its borrow. The unused stopped-state helper was removed.

`workspace list` now validates existing app-root ancestors and the final
private directory without creating a directory, lock, or file. A genuinely
missing root still reports the uninitialized registry; unsafe roots and
corrupt registry contents fail closed. App-root preparation now validates the
configured path before canonicalizing existing components, rejecting a
symlink at the selected root and normalizing macOS `/tmp` and `/var` aliases
for the strict registry store.

Workspace stop and remove now validate route directories without creating a
new namespace. Destructive liveness checks distinguish lock-probe errors from
a free lock: an unverified PID record, any residual endpoint, unsafe lock
state, endpoint inspection error, unreadable or dangling PID entry, or
authenticated live endpoint blocks success/removal. Stop signals only the
verified recorded process and reports uncertainty if the endpoint remains
after its server lock is released. Removal stays metadata-only.

Rust formatting and whitespace checks are the only local checks performed
for this follow-up. Cargo compilation, tests, and strict Clippy remain for the
parent; this record does not claim they pass. The unrelated
`src/ui_client/host.rs` borrow/warning findings remain with the selection r3
owner.

## Parent review after r2

The parent found a first-open/removal race in the missing-namespace shortcut:
removal could observe no directory, while an opener created it, validated
registration and started a server before the metadata deletion. Removal now
materializes the private namespace and takes its lifecycle lock even for a
never-opened tab before reloading/mutating the registry. This intentionally
creates bounded empty namespace/lock files on removal; it starts no process
and deletes no history. The open and removal paths share lifecycle-then-
registry lock order. Stop retains its no-creation path for absent runtimes.
Existing server/context directories are also checked for private real-directory
identity before destructive liveness probes, refusing symlink redirection.
Final routing acceptance and source-frozen integrated checks remain required.

The final Luna read-only review identified that removing inactive Default/Main
would leave plain `panelink` able to start an unregistered legacy runtime.
The parent now refuses `workspace remove 1 1`, preserving the registry bytes
and the legacy route. An integration regression selects another project
first and proves the refusal preserves both metadata and the absent runtime.
The generic pure model remains independent of this CLI compatibility policy.
