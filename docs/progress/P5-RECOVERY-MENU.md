# P5 in-workspace Recovery menu item — result — 2026-10-21

Implements user decision 1 (in-app entry) from
[P5-RECOVERY-VIEWER.md](P5-RECOVERY-VIEWER.md). Worker: GPT-6-Luna (xhigh).
The worker spent ~50 minutes in repeated failed patch attempts and left the
tree non-compiling (duplicate imports/methods, moved value, missing test
fields); the parent stopped it and completed integration. No worker result
note existed; this note is parent-authored.

## Behavior

- Right-click menu: a sixth row "View saved layout" after "Close pane"
  (`MenuResponse::RecoveryRequested`); disabled with "No previous layout
  saved" when the previous slot is absent. Ctrl-B then `v` does the same.
- Availability is checked read-only (`LayoutStore::open` + `load_bytes`
  of the previous slot) only when the event could open/activate the menu.
- `state.rs` returns `Flow::ViewRecovery`; no IO in state.
- `host.rs` flushes pending typed keys, discards frame output and pending
  terminal input, then runs `recovery::run_in_workspace` on the SAME host
  guard (no second raw-mode enter/restore). The live server, panes and their
  output keep running. On quit, pending terminal input is discarded again,
  the decoder reset and the workspace fully repainted. Viewer keys never
  reach a pane. Load errors become a status-line message.

## Parent corrections

- Removed duplicate imports and duplicate `discard_pending_input`/`discard`.
- `Net::server_directory()` replaces an extra event-loop parameter (keeps
  Clippy's 7-argument limit; no moved-value error).
- `run_in_workspace` now reuses the attach loop's stop flag; registering
  signal-hook handlers per viewer open would accumulate process-lifetime
  handlers.
- A lone Esc resolved by the escape timer can never open the viewer.
- Updated two split-menu/state tests whose keyboard paths assumed Close was
  the last row (End/Up now land on the recovery row first).

## Verification (parent)

```text
cargo test --locked --offline --lib split_menu   -> 28 passed; 0 failed
cargo test --locked --offline --bin panelink     -> 101 passed; 0 failed
python3 scripts/verify_non_ui.py                 -> 8/8; 964 passed/0 failed/15 ignored
```

Not executed: a real Apple Terminal manual check of the in-workspace viewer;
the automated attach PTY suites (p3a_default_open 6/6, p3a_attach_host) pass
but do not open the viewer from inside a workspace.
