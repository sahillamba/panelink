# Phase 3B — pane close runtime and CLI (2026-10-20)

The schema-9 route now authenticates the caller's existing connection,
authorizes the close against its own lease, and executes the keyed close once
on the owner thread. Status queries observe the same key and pane queries return
the current layout geometry and lifecycle. `panelink panes` lists numbered panes
of the default workspace; `panelink close N` selects the full target at its
observed layout revision. Low-level `control panes`, `control close TARGET REV`
and `control close-status SEQUENCE` expose the same operations with JSON output.
The CLI never reissues a close after a lost acknowledgment; it queries status
on the same lease/key and reports uncertainty if the result cannot be proven.
The last-pane reply has a bounded linger before the server exits.

The attach client offers a confirmed Close pane menu item and Ctrl-B then `x`.
Closing or observing a shell exit collapses the layout, discards stale client
input and connections, and expands the surviving pane. TerminalSession reports
whether SIGKILL escalation was needed without changing restore-first cleanup.
Session IDs remain non-reusable after retirement. The initial two-*live*-pane
capacity still applies, while repeated close/re-split cycles are bounded by
the server generation's launch journal and ledger limits (see
[P3B-CLOSE-CORE.md](P3B-CLOSE-CORE.md)).

## Executed checks

- `python3 scripts/verify_non_ui.py`: 8/8 checks passed. Root Rust library
  500 passed/4 ignored, binary 93 passed, full run 15 ignored; vendored decoder
  39 passed; Python 31 passed; component fingerprints matched; fmt, strict
  Clippy, build and Markdown checker (148 files, 0 findings) passed.
- `cargo test --locked --offline --test p3b_close_runtime`: 2 passed, 0 ignored.
  The private outer-PTY fixture covers split, pane listing, explicit close,
  escalation, stale revisions, close status, re-split with fresh identities,
  natural shell exit, last-pane shutdown and Ctrl-B x y in the real client.
- `cargo test --locked --offline --test p3a_default_open`: 6 passed, 1 ignored.
- A source review found `decode_pane_reply` indexed short untrusted frames before
  checking their size. The decoder now checks the fixed length first; its test
  rejects every truncated prefix.

The stop runs synchronously on the owner thread for up to roughly two seconds;
other requests wait. A stop with unconfirmed child exit leaves the pane in
place and reports `unknown`. Close results and layouts are volatile and do not
make server-crash recovery possible. Phase 2 host acceptance and the remaining
Phase 3 runtime gates are still open. No provider identity or durable terminal
history is claimed.
