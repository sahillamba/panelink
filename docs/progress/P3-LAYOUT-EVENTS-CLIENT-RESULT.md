# Phase 3 workspace layout events: CLI client result

Status: Worker C implementation delivered for parent review, 2026-10-23.

`panelink events` now opens a separate workspace-observation context and
offers optional capability bit 8 on that context. A fresh context can receive
the grant; resuming an existing context cannot widen its stored capabilities
or change its target grant. When selected,
the CLI validates the complete schema-11 watch response, scope, page limit,
status/cursor relationship and consecutive sequences before output or cursor
advance. It emits every `layout_changed` row before reconciling the latest
layout, then derives pane open/close observations from that current topology.
`ResyncRequired` emits `layout_resync` and reconciles before continuing. Legacy
servers without the capability retain the polling path.

`--pane N` is pinned to the initial full target identity for the invocation.
The workspace connection is separate from target-scoped pane event
connections, so the latter retain their existing grant. Pane metadata streams
continue to deliver output, owner and explicit child-exit events; a pane-stream
gap reconciles that target before the loop continues. Workspace transport loss
returns an error and does not synthesize a child-exit observation.

Added a CLI regression scenario that closes the initially selected pane after
a split, where its same-generation survivor can occupy the selected index. The
test asserts `layout_changed`, the original pane's close observation, no
replacement `pane_opened`, and a final quiet record.

Verification performed by this worker:

- `rustfmt --check --edition 2021 src/p3_cli.rs src/agent_cli.rs tests/p3_agent_cli.rs` — passed.
- `CARGO_TARGET_DIR=/private/tmp/panelink-layout-client-target cargo check --bin panelink` — passed.
- `git diff --check -- src/p3_cli.rs src/agent_cli.rs tests/p3_agent_cli.rs` — passed.
- The targeted CLI regression passed with local socket permission enabled.
- The complete `p3_agent_cli` integration target ran serially after the gap
  scenario was added: 4 passed, 0
  failed. Its teardown assertion waits until the server process exits and the
  control socket is removed before checking the `no_workspace` response.
- `CARGO_TARGET_DIR=/private/tmp/panelink-layout-client-target cargo test --test p3_agent_cli real_cli_workspace_watch_gap_recovers_after_ring_overflow -- --exact --test-threads=1` — passed, 1 test / 0 failures. The test uses the real private server, waits for a `layout_changed` record, stops only its spawned observer, applies 70 successful alternating root resizes, resumes the observer, and asserts exactly one `layout_resync` plus the final quiet record. The temporary fixture removes its JSONL output after the test.

Parent owns serial combined-tree verification and integration review. This
slice does not establish Apple Terminal, provider identity, reboot, or complete
Phase 3 acceptance.
