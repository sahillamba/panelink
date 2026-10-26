# Phase 5 attached-client clipboard worker — 2026-10-25

## Scope

Added a private `ui_client::clipboard::ClipboardWriter` service. It owns one
worker, one active payload, and at most one queued payload. `try_send(String)`
performs a nonblocking bounded enqueue with a 1 MiB per-selection cap and
explicit active-plus-queued byte accounting. `poll_result()` returns only
fixed redacted status classes. `shutdown()` and `Drop` reject later writes,
discard queued data, cancel an active process, and join the worker after its
child is killed and reaped.

The production sink directly launches `/usr/bin/pbcopy` with piped stdin and
null stdout/stderr. Rustix sets the worker-owned stdin descriptor nonblocking;
the worker supervises writes and child exit against one absolute deadline.
No shell, text arguments/environment, child diagnostics, clipboard read, or
extra thread is used. Selection bytes are not formatted or logged. No
dependencies, vendor files, server/PTY code, or event-loop wiring changed.

Tests inject an in-memory sink or a private mode-0700 fake executable. They do
not invoke the host `pbcopy` command or modify the host clipboard.

## Verification

Command:

```text
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --bin panelink ui_client::clipboard -- --test-threads=1
```

Result: 7 passed, 0 failed, 0 ignored; 151 filtered out. Covered exact injected
bytes, fake-process success, early exit, blocked-stdin timeout, cancellation
and queued-write discard, queue-full behavior, and byte-limit rejection.

## Lifecycle limit

The module declaration is present, but the attached-client event loop does not
construct or poll this service yet. Rust therefore reports its production
constructor as unused until the parent-owned lifecycle integration wires it.
This slice does not establish Apple Terminal or real-user clipboard
acceptance, nor does it close any Phase 5 gate.
