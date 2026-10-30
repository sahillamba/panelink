# Button redesign local client installation

Installed at 20261030T194810Z after explicit user request. Replaced the standalone executable `/Users/sahillamba/.local/bin/panelink` atomically; backup: `/Users/sahillamba/.local/share/panelink/backups/20261030T194810Z/panelink`.

Build command: `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target cargo build --locked --offline` (exit 0). This is a local development build from the dirty button-design checkout, not a clean release-manifest installation.

Installed SHA-256: `3cc71a2b4baede974926a9600140b1ad84c612468026ed6022c71f231262bdff`; independently matched the built artifact.

Previous client:

```json
{
  "package": "panelink",
  "version": "0.1.6",
  "revision": "daf28a63f062",
  "source_fingerprint": "a77454e2d57c93a9",
  "dirty": true
}
```

Installed client:

```json
{
  "package": "panelink",
  "version": "0.1.8",
  "revision": "fdd8664d3085",
  "source_fingerprint": "380bf34e62a21468",
  "dirty": true
}
```

Running server identity compared equal before and after:

```json
{
  "package": "panelink",
  "version": "0.1.8",
  "revision": "daf28a63f062",
  "source_fingerprint": "f565cab6c33f3b53",
  "dirty": true,
  "generation": 15642535525154316810,
  "selected_capability_mask": 1023,
  "selected_capabilities": [
    "read",
    "input",
    "events",
    "workspace_events",
    "pane_metadata",
    "screen_row_wraps",
    "history_cursor",
    "pane_operations",
    "agent_observation",
    "build_identity"
  ]
}
```

No server restart, pane input, workspace shutdown or runtime migration was performed. Existing attached clients retain their previous executable until detached with Ctrl-X and reopened. Native host/provider acceptance remains open. Earlier no-install entries describe the state before this installation.
