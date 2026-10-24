# Phase 4 managed Panelink/OpenCode bridge probe — 2026-10-24

Status: the bounded real-process probe is implemented. The latest managed run
confirms initial provider snapshots for both targets and keeps the attach UI
alive through the Ctrl-O wait, but the exact native picker title and subsequent
route change are not proven. Full Phase 4 remains open.

`scripts/probe_panelink_provider_bridge.py` reuses the pinned-native OpenCode
bootstrap, fresh HOME/XDG fixture, and loopback-only `sandbox-exec` boundary
from `probe_opencode_v2_identity.py`. It compiles the repository's probe
terminfo into a private fixture directory, starts two separate managed
`panelink serve --opencode-bridge` children with the original embedded plugin,
and opens the first server through Panelink's real attach PTY. The sandbox
allows local Unix sockets for the private bridge while denying non-loopback
network access. It does not edit user OpenCode configuration or use an
external provider.

The intended acceptance requires an initial exact-target `last_confirmed`
snapshot for each serve; a guarded native Ctrl-O A-to-B-to-A switch with
ordered revisions while B remains unchanged; the exact project/Home route
becoming stale and explicit return to A; and exact-target historical binding
lookup after graceful stop, including `current_at_crash=false` and
`automatic_resume=false`. It also requires the fixture API to verify that the
two created sessions contain no user messages. No prompt is submitted. Output
contains only fixed classifications/statuses, numeric targets and revisions,
and boolean UI markers; provider session IDs, TUI text, credentials and raw
serve diagnostics are discarded. Each run owns and reaps its Panelink children
and temporary fixture.

Current managed run command:

```sh
PANELINK_RUN_P4_MANAGED_BRIDGE=1 python3 -m unittest -v \
  tests.test_panelink_provider_bridge_probe.PanelinkProviderBridgeProbeTests.test_real_managed_bridge_acceptance
```

The first attach failure was a probe-consumer bug: while waiting for provider
snapshots, the fixture stopped draining the attach PTY. Once its retained output
reached 512 KiB, the inherited reader consumed only one 8 KiB chunk per poll;
the application then correctly reported `terminal output made no progress`.
The probe now drains through `WouldBlock` continuously, retains a bounded
512 KiB rolling transcript, and reports total and retained byte counts without
storing raw screen output. A separate fixture read lease lets the probe inspect
only fixed markers from the provider pane while the attach client holds its own
context lock.

The latest run used
`/private/tmp/panelink-p4-bridge-store-target/debug/panelink`. The probe
verified native OpenCode `2.0.32` and pinned SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`, then
started **2** managed servers with distinct exact targets. Each reached
`last_confirmed` at revision 1, and passive server B remained unchanged. The
client PTY passed its raw-mode gate and stayed alive through the native picker
wait. The observer read 2,380 screen bytes in memory and returned fixed
markers: OpenCode title and both fixture session labels present, generic
“sessions” text present, and neither the exact “Open session or project” nor
“recent sessions” picker title present. The filter therefore did not produce a
safe unique target row, so the probe withheld Enter and classified the switch
as unproven. The managed pane remained live/applied. After both servers
stopped, exact-target
historical CLI lookup validated both managed-linked records at revision 1 with
`current_at_crash=false` and `automatic_resume=false`. Therefore the test still
returns `managed_session_switch_not_proven`; no A-to-B-to-A or Home acceptance
is claimed. The allowlisted summary reported
`cleanup_completed=true`, `prompt_submissions=0`, `user_message_absence_verified=true`,
`passive_b_unchanged=true`, managed pane status `live/applied`,
`serve_lines_valid=true`, and
`network_scope=loopback_only_sandbox`; no raw IDs, paths, screen text, or
credentials were emitted. The Panelink attach environment is limited to
`PATH`, private `HOME`/`ZDOTDIR`, and `PS1`, so OpenCode's server password and
plugin socket capability are not passed to the attach process.

The attach client's bounded terminal error reporter now makes a fixed stderr
fallback when `/dev/tty` cannot be opened or written. The fallback emits only
the existing fixed error label through a nonblocking endpoint; it includes no
exception details, paths, environment values, target identifiers, or screen
contents. A disposable PTY negative control verified that a fixed connection
failure label reaches the terminal.

A separate disposable shell-pane check isolated Ctrl-O delivery from native
OpenCode's picker behavior. A managed `/bin/sh` pane ran `stty raw -echo` and
read one byte with `dd`; after attach reached raw mode, the probe sent only
Ctrl-O (`0x0f`). The shell's numeric output confirmed byte `15`, the attach
client remained alive, and the fixture reaped the client and server. This
confirms the generic Panelink attach/input-owner/PTY path forwards Ctrl-O
exactly. It does not prove the native OpenCode picker opened or that a session
route changed; those still require a safely visible unique row and a later
fresh provider observation.

Focused local tests:

```text
python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py
python3 -m unittest -v tests.test_panelink_provider_bridge_probe
Ran 8 tests: 7 passed, 1 live-native test skipped, 0 failures.

`cargo test --locked --offline --bin panelink ui_client::host::tests -- --test-threads=1`
Ran 3 focused host tests: 3 passed, 0 failures.

python3 -m unittest -v tests.test_opencode_bridge_plugin
Ran 9 tests: 9 passed, 0 failures (approved local execution).
```

The plugin fixture and managed acceptance require pathname Unix sockets. The
default workspace sandbox denied socket binding with `EPERM`; the plugin's
focused fixture was rerun under approved local execution and passed. The real
managed probe itself ran under its own loopback-only macOS sandbox with local
Unix socket access enabled. It confirms managed snapshots but does not prove
session switching or full Phase 4 completion.

The tightened picker attempt adds a fixture-only, read-only decoder for the
exact `managed-provider.record` slot while its writer is live. It opens only
the deterministic full-target path with no-follow semantics, verifies private
owner/modes and stable inode, checks the bounded `PLMPRV01`/`PLBIND01` frames,
and matches the entire target, canonical project, pinned provider version, and
expected fixture session ID in memory. It emits only the target, revision,
version, and SHA-256 of the session ID. This is test instrumentation; it does
not bypass or change the production writer lock.

The latest managed attempts confirmed initial A exactly at binding revision 1.
Before Ctrl-O, a fresh Panelink control read returned the OpenCode screen. The
attach transcript then advanced after Ctrl-O and showed the generic `sessions`
heading with both exact fixture labels. After sending the exact B title in one
bounded write (matching the native fixture's input method), both the attach
screen reconstruction and a new managed-child `control read` still showed the
picker heading and both target and competitor labels. The probe therefore
withheld Enter; no live B binding at revision 2 was observed. The new control
read distinguishes this from a transcript-only stale-row artifact: the
managed child's current screen also retained both labels. The title was not
visible as a unique filtered row, so whether the native picker accepts input
in a different focus state remains unresolved.

After shutdown, exact-target historical CLI lookups matched both fixture IDs in
memory, but each record remained at revision 1. Identity inspection passed
while the required revision-advance gate did not. These runs also verified no
user messages before or after, kept passive B unchanged, emitted no prompt,
and completed cleanup. The fixture suite now includes a regression asserting
that the probe never sends Enter when the attach view and a fresh managed-child
read both show the competing row. A-to-B-to-A and full managed acceptance
remain unproven.
