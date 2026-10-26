# P5 workspace routing acceptance — 2026-10-25

Status: focused CLI acceptance has been added in
[`tests/p5_workspace_routing.rs`](../../tests/p5_workspace_routing.rs). No
Cargo build or test command has been run for this change. The parent must run
the commands below after shared production compile/path fixes are complete.

## Acceptance expectations

- `workspace list` against a fresh private root reports the available legacy
  route and creates no registry, lock, server, terminfo, or context files.
- Two separately registered nondefault tabs are opened by explicit IDs. Each
  piped attach attempt reaches the expected `attach needs an interactive
  terminal` result after route startup; each server remains alive afterward.
  Their server generations, server locks, lifecycle locks, and UI context
  directories are independently scoped, with a marker in one
  context directory preserved while the other route opens.
- The no-selector `panes` and `stop` commands retain their Default/Main
  behavior while both nondefault servers are live. Neither command stops a
  nondefault server. `workspace stop 2 3` stops only tab 3 while tab 4's lock
  remains held; stopping tab 4 then releases its lock. Opening explicit IDs
  does not change the persisted active selection.
- Removal refuses a live server, a manually held server lock, an ambiguous
  stale PID record, and a socket endpoint with no verified server identity.
  A clean stopped route can be removed. Refused routes remain listed.
- Corrupt registry data and an unsafe symlinked route namespace fail closed
  without starting a server at the legacy default path or writing through the
  symlink. Fixture-owned live route servers are stopped by exact workspace ID
  from `Drop`, with a bounded stop-command wait and a PID/start-time-verified
  termination fallback, including on assertion panic. The fixture directory
  is removed only after its owned server locks are released.
- These tests do not claim manual terminal attach, in-client tabs, provider
  recovery, or process/reboot recovery acceptance.

## Parent verification commands

Parent focused run after source corrections: **5 passed,0 failed,0 ignored**
in1.54s. Log: `/private/tmp/panelink-p5-parent-routing-r2.log`. The parent
corrected an unsafe UID query in the fixture and added the never-opened-route
removal regression: it takes a real lifecycle lock before unregistering,
starts no server, and refuses a later open of the removed ID. The preceding
attempt failed compilation before any routing test ran and remains recorded
in `/private/tmp/panelink-p5-parent-routing.log`. The full source-frozen suite
and final review remain separate from this focused result.

Run serially, using the existing target and Cargo caches, after production
owners release the shared code:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --test p5_workspace_routing -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --bin panelink workspace_cli::tests -- --test-threads=1
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo fmt --check
```

Record each command and result separately. At this handoff, actual Cargo/build/
test runs: **none**. Local allowed checks completed: `rustfmt --edition 2021
--config skip_children=true tests/p5_workspace_routing.rs` and `git diff
--check`, both exit 0. No Cargo command was run.
