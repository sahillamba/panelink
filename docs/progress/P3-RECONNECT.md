# Socketpair detach and reconnect proof — 2026-10-18

Status: the bounded actual Unix socketpair/private-PTY scenario passed locally.
This is a focused integration test using the current `ServerDriver`, worker
framing, `StateOwner`, `PtyEffects`, `TerminalSession`, and persistent
`ContextFile`. Parent reviewed the complete scenario and independently reran it
as part of the combined suite: one integration test passed in0.10seconds.
See [INTEGRATION.md](INTEGRATION.md) for final verification.

## Scenario and observations

`tests/p3_socket_detach.rs` launches one `/bin/sh -c` child with an explicit
argument vector, TERM/PATH map, and private working directory. The test attaches
both ends of `UnixStream::pair()` through `AuthenticatedStream::new`, so the
same-user peer check runs before either side exchanges frames. The server side
is handed to `ServerDriver<PtyEffects>`; the test thread calls `tick` while a
scoped helper performs bounded client socket exchanges.

The initial connection uses a fresh Hello, then obtains the complete target and
owner revision from Describe. It creates the metadata-only ContextFile for the
Welcome generation/lease, claims input ownership using that exact target and
revision, and confirms the new revision through Describe. A bounded schema-1
Read observes the shell's `READY-P3-<pid>` marker. The client then durably
reserves a Submit key, writes the exact Request frame, and closes its socket
without reading the response or clearing the context's uncertainty marker.
The shell prints `GOT-P3-<pid>` only after its read loop handles the token; tty
echo does not count as application handling.

The test keeps the server driver pumping after the abrupt close and confirms
the child remains live. A second authenticated socketpair sends schema-2 Hello
with the exact original generation/lease. It queries OperationStatus for the
uncertain key until terminal, verifies the response key, opens the same context
file, and reconciles that exact key. The client does not automatically resend
the Submit. A separately labeled same-key/same-payload Request is then sent as
a deliberate dedup regression probe; it returns the saved Applied status.

To detect a delayed duplicate effect, the test subsequently submits a distinct
check token. The shell prints `CHECK-P3-<pid>-<handled-count>` after consuming
all earlier input. The observed `CHECK-P3-<pid>-1` marker proves the shell
processed the duplicate probe's position in the input stream while handling
the original token exactly once. The test then reads current screen state and
checks the same ready PID and exactly one `GOT-P3-<pid>` marker.

That connection sends Detach and checks the exact `[0, 2, 0, 2]` response
before the worker closes. A third socketpair resumes the same generation and
lease, describes the same target and live child, and makes a new bounded Read.
The new screen still contains the same ready PID, ordered check marker, and
exactly one application GOT marker. The test waits for each connection worker
to close and confirms the child is still alive after both abrupt socket loss
and explicit Detach.

Client frame reads and writes each have absolute two-second deadlines. The
client exchange, screen-marker loops, and worker-close waits are bounded by
finite scenario deadlines. Sleeps only pace polling; success is based on
decoded protocol responses, current bounded reads, shell-emitted markers, and
worker/child state. Fixture cleanup is owned by Rust drops; the test performs
no process lookup or arbitrary kill. It uses no pathname listener.

## Actual verification

```text
rustfmt --edition 2021 tests/p3_socket_detach.rs
```

Exit 0.

```text
rustfmt --edition 2021 --check tests/p3_socket_detach.rs
```

Exit 0; no output.

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_socket_detach -- --test-threads=1
```

Exit 0: 1 passed, 0 failed, 0 ignored; test duration 0.16 seconds.

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --test p3_socket_detach -- -D warnings
```

Exit 0; strict Clippy finished without warnings.

## Limits and handoff

The fixture constructs a volatile `StateOwner` with a caller-supplied test
generation. It does not exercise the parent's persistent generation/journal
startup adapter, server process death, power-loss recovery, pathname listener,
Apple Terminal, multiple clients, a user-facing client, provider identity, or
OpenCode recovery. The PID is an in-scenario process-continuity observation,
not a durable process identity. This result does not establish general host
compatibility or Phase 3 completion.

Only the requested test and this evidence file were edited. No dependency,
manifest, export, production source, commit, or push was changed. Ownership is
released for parent review and independent rerun.
