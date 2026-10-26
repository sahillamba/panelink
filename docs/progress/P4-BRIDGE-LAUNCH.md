# Phase 4 bridge launch provisioning — 2026-10-24

## Current integration status

`run_serve` now owns this launch object alongside the live runtime and explicit
manual recovery controller when `--opencode-bridge` is supplied. Both `tui.ts`
and `recovery.ts` are embedded into the private plugin directory. The source is
limited to the original exact managed occupant and is dropped when that target
ends. Default startup and shell splits remain generic. Native checks are in
[P4-NATIVE-MANUAL-RECOVERY.md](P4-NATIVE-MANUAL-RECOVERY.md) and
[P4-MANAGED-BRIDGE.md](P4-MANAGED-BRIDGE.md). The component report below records
the earlier launch-only slice.


Status: opt-in launch preparation, schema-6 environment injection, and
per-target binding-directory creation are implemented. CLI wiring that owns
the `BridgeLaunch` beside the provider runtime remains parent integration;
default startup does not activate this bridge. Live OpenCode operation and
full Phase 4 remain open.

`BridgeLaunch::prepare(server_dir, generation, context, executable)` resolves
the native executable through the existing explicit launch resolver and
verifies the canonical executable bytes against the pinned SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`. It
uses `/usr/bin/shasum -a 256`, with a cleared helper environment, discarded
stderr and a bounded stdout read. Resolution and pin verification happen
before any generation directory is created. The current live hash must match
the pin; test-only verifier injection is confined to `cfg(test)` callers.

After verification, preparation creates a new mode-0700
`provider-bridge-<generation>` directory beneath the canonical private server
directory. It materializes the embedded original TUI plugin in a private
`identity-plugin/tui.ts` directory and writes a mode-0600 `opencode-config/cli.json`
with the current plural `plugins` setting pointing to that directory. It binds
the Unix listener before the child can spawn, then applies and checks mode
0600 and records the socket inode. The returned environment contains exactly
`OPENCODE_CONFIG_DIR` and `PANELINK_BRIDGE_SOCKET`; it carries no Hello or
grant capability. The canonical project directory bytes are exposed
separately for the owner/runtime.

`StartedMultiServer::start` delegates to its unchanged non-bridge path.
`start_with_provider_bridge` accepts a borrowed bridge environment, but
`PreparedLaunch::with_provider_bridge_env` is called only from the existing
managed-launch preparation closure, after schema-6 request validation. It
accepts exactly the config and socket keys, requires absolute bounded values,
and applies them only to the child environment. It does not read or mutate the
server process environment or add these values to launch metadata or the
journal.

After managed spawn establishes the complete target, `binding_path(target)`
creates a new private directory beneath `server_dir/provider-bindings` named
`{generation}-{workspace}-{pane}-{session}-{occupant_revision}`. The static
`binding_directory(server_dir, target)` mapping supports exact historical
lookup without scanning for a recent binding. A generation mismatch, invalid
identity, existing target directory, unsafe directory or symlink is rejected
without replacing or deleting existing state. Dropping `BridgeLaunch` closes
a listener it still owns and unlinks only the socket pathname whose current
device/inode still matches the socket it created. It does not recursively
delete generation or binding directories.
This supports fail-closed stale state handling; it does not implement grant
revocation or the runtime lifecycle join.

Focused Rust evidence: `cargo test --locked --offline --lib
provider_bridge_launch::tests -- --test-threads=1` passed **3 tests**. The
tests use the `cfg(test)` verifier hook to inspect generated config/plugin
contents, permissions, socket setup, exact target storage scope, unchanged
process environment, rejection of an unsupported live executable before
filesystem effects, stale-generation preservation and socket-inode-safe
cleanup. The default verifier test checks the real pinned hash policy against
`/bin/sh` and confirms it is refused before creating launch state. The
workspace sandbox denied pathname socket binding with `EPERM`; the same
focused locked/offline test command was rerun under approved local execution
and passed. `managed_launch_prepare::tests::provider_bridge_environment_accepts_only_the_two_private_launch_paths`
passed **1 test**, and `multi_startup::tests` passed **3 tests**, including
child-only bridge environment delivery after schema-6 validation. These are
launch-component evidence only, not a live provider, server runtime, recovery
or full Phase 4 acceptance result.
