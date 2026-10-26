# Phase 3 server-death audit — 2026-10-23

Read-only audit of the current integrated working tree. This note separates
implemented behavior and existing evidence from proposed server-death
acceptance. It does not close Phase 3 or claim crash recovery for PTYs,
descendants, or provider state.

## Findings

| Area | Current behavior and evidence | Limit |
|---|---|---|
| Graceful `SIGTERM` | `run_serve` registers `SIGINT`, `SIGTERM`, and `SIGHUP` into a stop flag, leaves its accept/tick loop, settles connection work, and drops the driver and listener (`src/p3_cli.rs`: `register_stop_signals`, `run_serve`). `MultiServerDriver::drop` stops workers and sends HUP to each managed process group, polls for up to one second, sends KILL if needed, and observes for a further bounded second before final writer shutdown (`src/multi_runtime.rs`: `Drop for MultiServerDriver`; `src/server_pty.rs`: `prepare_driver_shutdown`, `advance_driver_shutdown`). `PrivateListener` removes only the socket inode and directory identity it originally bound (`src/transport.rs`: `SocketCleanup`). | This is an orderly server path and applies only to the process groups the runtime owns. It does not promise termination of descendants that leave those groups or retain their own descriptors. A shutdown timeout remains bounded and does not mean every descendant exited. |
| Graceful host evidence | `tests/p3a_serve_pathname.rs::serve_binary_splits_over_the_pathname_socket_and_stops_on_sigterm` exercises the real serve process, requires a successful exit/stopped record, checks pathname socket removal, and observes that storage is available after stop. `tests/p3a_attach_host.rs::reattach_after_server_restart_replaces_dead_contexts` separately checks a fresh generation and replacement of old attach-client context files after orderly stop/restart. | Neither test is a server `SIGKILL` acceptance. The context-replacement test uses orderly stop, and its client path cleans the stale attach context itself. |
| Abrupt server death | `tests/p3a_default_open.rs::killed_server_leaves_a_stale_socket_that_the_next_open_recovers` kills only the fixture's background server, observes that its pathname socket remains, then opens a new workspace and types `BACK3`. | It does not identify the old managed shell, compare full old/new target and generation, exercise a saved old control context against the replacement, or prove that no old input was replayed. It makes no assertion about child or descendant cleanup. |
| Endpoint recovery and locking | Zero-config open holds `lifecycle.lock` across endpoint inspection, stale cleanup, spawn, and readiness. A live `server.lock` selects the existing server; an owned stale socket with no lock is removed and startup proceeds; foreign entries are refused (`src/default_open.rs`: `decide_endpoint`, `ensure_server`). The listener's own drop cleanup is inode-checked and does not remove a replacement entry (`src/transport.rs`: `PrivateListener`, `pathname_listener_authenticates_and_removes_only_its_socket`). | The pathname listener intentionally refuses to take over an occupied endpoint. Stale cleanup is owned by the higher-level zero-config opener after it observes the singleton lock is free. Direct `serve` does not take the zero-config lifecycle lock, while `ServerStorage` provides the singleton server lock. |
| Generation fencing | `ServerStorage` holds a singleton lock and advances/flushes the persistent generation before use (`src/server_storage.rs`: `ServerStorage::open`, `publish_generation`). A unit test proves a second concurrent open is rejected and a later open advances generation. Server resume validates generation before reusing a lease (`src/server.rs`: resume/handshake validation). | The generation unit test is not a process-crash integration test. The current abrupt-server test does not assert generation fencing or old-context rejection across a replacement server. |
| PTY master and descendants | The orderly driver shutdown explicitly signals the managed process group before `TerminalSession` closes its master. In an abrupt server death, Rust destructors and explicit HUP/KILL escalation do not run; the OS closes the server's PTY master descriptors. | Master closure and terminal/session behavior do not establish a product guarantee that every child or descendant terminates. A descendant may change process group/session or keep a slave descriptor open. Do not infer a universal cleanup contract from a shell exiting in one fixture. |

## Proposed bounded acceptance

The parent-owned contract is recorded in
[`P3-SERVER-DEATH-TASK.md`](P3-SERVER-DEATH-TASK.md). Strengthen only the
existing `p3a_default_open` server-kill scenario as one fixture-local
crash/restart test. Keep it deterministic by avoiding an in-flight request:

1. Start the default workspace and initialize an old control context. Capture
   the full original `TargetRef` (including generation) and exact shell PID
   through a private fixture file written by shell `$$`. Wait for a unique
   split-printf marker rather than command echo, and arrange for it to append
   exactly once to a private external marker file. If needed, install an
   explicit `trap 'exit 0' HUP` so this cooperative shell's response to master
   closure is deterministic.
2. Detach/drop the old client, send `SIGKILL` only to the exact
   fixture-owned live server PID, check the signal result and bounded server
   termination, and confirm the stale socket remains. Wait boundedly for this
   cooperative shell to exit; a zombie counts as exited, without asserting
   immediate reaping.
3. Open a fresh default workspace using the existing explicit fresh-shell
   selection. Require a different server identity, generation, and full
   target. The old control context must fail against the new generation rather
   than silently reinitialize; a fresh control context must succeed.
4. Confirm the external marker file is unchanged after restart. Then send a
   unique new marker and require exactly one confirmation from the replacement
   shell. Detach normally, explicitly stop the replacement, and verify
   socket/server cleanup.

The test may wait for the exact cooperative shell PID it captured, but must
not send signals to arbitrary PIDs or use process enumeration to find
descendants. Do not assert that PTY master closure kills arbitrary or
HUP-ignoring descendants. Any failure cleanup must be limited to fixture-owned
resources and must not treat persisted PIDs as safe cleanup authority.

This scenario proves stale endpoint recovery, generation/context fencing,
fresh target creation, a cooperative shell's observed response to PTY master
closure, and no replay of the external marker into the replacement workspace.
It does not prove orphan-free server death, cleanup of arbitrary descendants,
crash-safe operation ledgers, recovery of in-flight input, physical reboot
recovery, or provider identity.

## Evidence status

This is a source-and-test audit only. No source code was changed, and no tests
were run. The proposed scenario is acceptance guidance, not completed evidence.
