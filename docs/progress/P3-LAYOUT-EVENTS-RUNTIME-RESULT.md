# Workspace layout-event runtime adapter — 2026-10-23

Implemented schema-11 request classification and dispatch in
`src/multi_runtime.rs`. Reconciliation and watch replies use one bounded
`Response` frame; the adapter does not emit transport `Event` frames. The
connection worker requires both read and workspace-event capabilities, while
the owner independently validates the lease, generation, workspace scope and
selected capabilities. Before writing any response bytes, the worker decodes
the full response and validates the request namespace, reconciliation layout
namespace, watch status/count, page limit, event consecutiveness relative to
the request cursor, terminal cursor, and nonregressing resync hint.

Positive waits share the existing eight-entry pane/workspace watch budget and
are revisited on owner ticks. They settle on stop and retain a bounded worker
reply deadline. Existing pane-watch dispatch and reply behavior is unchanged.

The new private-PTY/socketpair target starts a real shell and exercises
workspace reconciliation, read-only repeated reconciliation, capability
denial, wrong-workspace and future cursors, pending-watch responsiveness to a
separate layout query and root resize, wake on the committed resize, identical
retained-event reads, journal-gap resynchronization after 65 further resizes,
and a stable immediate timeout at the latest cursor. A second pending watch is
cleared when the driver stops its connections; the worker may close before its
queued `server_stopping` reply is observable.

## Verification

Commands:

```text
CARGO_TARGET_DIR=/private/tmp/panelink-layout-runtime-target cargo test --locked --offline --test p3_workspace_events -- --test-threads=1
CARGO_TARGET_DIR=/private/tmp/panelink-layout-runtime-target cargo test --locked --offline --lib workspace_adapter_tests -- --test-threads=1
```

Observed results: integration `1 passed; 0 failed; 0 ignored` (1.25 s); runtime
unit checks `2 passed; 0 failed` (0.00 s; 693 filtered out). The integration
test is gated to Apple Silicon macOS. These are private socketpair/private-PTY
results; they do not establish Apple Terminal acceptance or the full suite
result.
