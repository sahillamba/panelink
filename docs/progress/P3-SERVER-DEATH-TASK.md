# Phase 3 server-death acceptance — bounded contract

2026-10-23. Continue the roadmap without changing crash policy or adding a
supervisor. This slice strengthens the existing default-open SIGKILL scenario.
Preserve all accumulated uncommitted changes. No dependency, wire or storage
format changes are planned.

## Accepted scope

A graceful SIGTERM runs driver teardown and bounded HUP/KILL escalation.
SIGKILL cannot run destructors or flush pending history/layout. OS closure of
the last PTY master may hang up the foreground process group; this is not a
guarantee that arbitrary HUP-ignoring, background or detached descendants die.
Do not promise orphan-free server-crash behavior or adopt persisted PIDs as
safe cleanup authority. Provider/reboot and crash-loss bounds remain open.

Default-open already serializes inspect/cleanup/start under its lifecycle
lock, checks the server storage lock and endpoint type/ownership, and creates
a new durable generation. Direct `serve` refuses an existing endpoint; it is
not a stale-socket cleanup interface. Fresh invocation is explicit user action,
not replay of an earlier launch/input request. Recovery chooser selection of a
fresh shell remains explicit in the existing test helper.

## Acceptance task

Strengthen only `killed_server_leaves_a_stale_socket_that_the_next_open_recovers`
in tests/p3a_default_open.rs; add narrowly scoped local helpers as needed.

1. Start the existing private default workspace and initialize an old control
   context. Capture full target/generation and the exact shell PID through a
   private file written by shell `$$`; wait for a unique split-printf marker
   that is not satisfied by command echo. This fixture uses ordinary zsh;
   install an explicit `trap 'exit 0' HUP` if needed for deterministic
   cooperative hangup behavior. Record only this bounded cooperative case.
2. Detach/drop the old client, SIGKILL only the fixture-owned live server PID,
   check the signal result and bounded server termination, and assert its
   socket is left behind. Wait boundedly for the captured cooperative shell
   to exit (zombie is exited, not live; do not assert immediate reaping).
3. Start a fresh default-open client (existing explicit fresh-shell selection).
   Assert new generation/full target and new server identity. The old control
   context must fail against the new generation; do not silently initialize
   over it. Fresh control context succeeds. Assert no automatic old input
   replay using an external private append marker written exactly once before
   death, unchanged after restart. Old live output is not new live output.
4. Send one unique new marker, prove it appears once, detach normally, stop
   replacement explicitly and verify socket/server cleanup. Retain bounded
   failure cleanup of owned resources; never kill arbitrary persisted PIDs.

Worker owns only tests/p3a_default_open.rs; parent owns shared docs/design.
Do not change production code based on a missing acceptance assertion. Report
actual targeted test output and any inability to establish a claim. No commits.
Native GPT-6-Luna High collaboration is the launcher. Parent independently
reviews and runs complete serial offline tests plus supporting checks.
