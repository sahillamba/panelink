# P5 outer-host drag mode acceptance — 2026-10-25

## Parent follow-up after transport correction

The transport owner fixed `multi_runtime::snapshot_matches` to validate marked
replies against the connection's negotiated wrap capability. The previous
legacy-only decoder rejected the new replies at the outgoing guard.

Parent ran the existing focused `p3a_attach_host` test executable against the
updated isolated binary: **1 passed,0 failed,4 filtered out in3.52s**. This run
reached the1002/1006 entry and reverse-order cleanup assertions and saved-termios
comparison. No skip path occurred. This is private-PTY proof only; human Apple
Terminal acceptance remains open. A Cargo attempt immediately before it was
blocked by an in-progress workspace-store module declaration; final source-
frozen Cargo verification remains required. The failed attempt below is
preserved as history and is superseded only within this focused proxy scope.

Status: **blocked before the host-mode assertions**. The requested focused
outer-PTY test built, started its fixture server, and put the attach client into
raw mode, but the first pane-ready wait ended with `connection lost`. This is
not host-mode acceptance and does not establish that the proposed 1002/1006
sequence works.

## Intended contract

The attached client needs held-button motion for drag selection. The host entry
sequence should enable button-motion mode 1002 and SGR mouse encoding 1006;
unrestricted any-motion mode 1003 must remain disabled. On detach, disable
1006, then 1002, then 1000, before restoring the alternate screen. The test
asserts these conditions in `tests/p3a_attach_host.rs`; the four-pane test also
expects 1002 to be disabled on detach.

## Focused attempt

Command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --test p3a_attach_host \
  attach_types_right_click_splits_keyboard_menu_and_detaches_leaving_children \
  -- --exact --nocapture --test-threads=1
```

The command ran under a 300-second process-group timeout and completed in
14.6 seconds with exit status 101 (no timeout). Cargo compiled `panelink` in
0.93 seconds. It emitted 16 dead-code warnings for the concurrently unfinished
clipboard/client-selection work; compilation succeeded.

Exact failing point: `tests/p3a_attach_host.rs:416`, inside
`Client::wait_screen("pane 1 READY", ...)`. The observed screen showed
`connecting...` followed by `connection lost`. The test had already received a
`state=running` server startup line and `wait_raw_mode()` had returned. No
mouse-mode assertion ran. The fixture cleanup ran as the test unwound; the
timeout wrapper was not needed.

## Diagnosis and limits

`src/ui_client/conn.rs` maps both handshake frame IO and later request frame IO
failures to the fixed label `connection lost`. Once a `PaneConn` exists,
`src/ui_client/net.rs::fetch_screen` issues the initial snapshot request and
disconnects the pane on transport failure. The server's UI dispatch negotiates
`CAPABILITY_SCREEN_ROW_WRAPS` per connection and builds the optional wrap
trailer for screen replies; the client offers that capability and records the
selected bit. The captured output does not establish whether the handshake
completed, whether the snapshot request was sent, or whether a reply stream
closed. It therefore does not prove a capability-negotiation defect or
justify a production change.

No test fixture or expectation correction is supported: the failure occurs
before the new host-mode assertions and before the UI reaches pane-ready. No
files outside this note and the assigned test files were changed for this
diagnosis. No logs or separate result artifact were produced by the run.

## Result

- Focused host-mode test: **failed before assertions**.
- 1002/1006 entry and reverse detach order: **not verified by this run**.
- No test was skipped or counted as a pass.
- Phase 3 Apple Terminal acceptance remains separate and open.
- Next diagnosis should capture the server-side reason for closing the attach
  stream or identify the exact first failed UI request before changing the
  fixture or production code. Keep transport/runtime sources read-only while
  their current owner is editing them.
