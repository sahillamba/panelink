# P3 bounded authenticated connection workers

Status: implemented and locally verified through a temporary Cargo harness.
The worker hub accepts only an already authenticated AuthenticatedStream;
parent integration and independent review subsequently completed (see INTEGRATION.md). This slice does
not create the listener, own process effects, or claim a persistent runtime.

## Implemented contract

src/server_io.rs adds WorkerHub, OwnerEnvelope, OwnerRequest, and OwnerReply.
The hub has an eight-slot active-worker cap, an eight-envelope sync_channel,
and no more than one thread per admitted connection. Each envelope has a
one-slot reply channel. A full owner queue rejects and closes that connection
through try_send; it never blocks a socket worker. The owner loop replies with
try_reply, which also cannot block.

The worker authenticates no client itself and accepts no raw UnixStream.
AuthenticatedStream::new / PrivateListener::accept perform the kernel same-user
check before a stream can reach WorkerHub::start. Each connection must begin
with a schema-valid Hello frame at request ID zero. The state owner creates the
Connection and Welcome; the worker verifies their generation, lease, selected
capabilities, and Hello nonce/capability relationship before writing Welcome.
A fixed-label owner Error during a valid handshake is sent at request ID zero
and closes the connection.

After Welcome, only Request frames with nonzero, strictly increasing IDs are
accepted. The worker validates schema-1 control requests and schema-2 session
requests before forwarding the exact payload to the owner. A client Hello,
Response, Error, Snapshot, or Event; an invalid request schema; a repeated ID;
or a zero ID closes the connection without owner dispatch. Each accepted
request is forwarded once and its worker waits for that envelope's reply
before reading another frame. Control replies must decode as schema-1 response
payloads. Describe replies must decode as schema-2 session info; other session
replies must decode as schema-1 responses. Detach is acknowledged with the
fixed four-byte schema-2 payload [0, 2, 0, 2] and then closes the attachment.

Reads use one-second absolute deadlines, writes use 250-millisecond absolute
deadlines, and each owner reply has a two-second deadline checked at intervals
no longer than 20 milliseconds. Socket errors, timeouts, malformed frames,
invalid owner reply roles, oversized responses, and invalid error labels close
the connection; no request or input is retried. Error payloads accept only
nonempty lower-snake-case labels up to 4,096 bytes. Debug output redacts
request/response payloads and error labels.

Only the hub's owning thread edits the worker-handle vector. Normal operation
joins only finished workers. Shutdown sets the shared stop flag and joins at
most eight workers; in-flight transport I/O remains capped by its socket
deadline, while owner-reply waits check stop at most every 20 milliseconds.
Stopping or losing a connection does not signal, cancel, or drop the effect
sink. The parent state-owner loop remains responsible for finishing an envelope
already accepted by the bounded queue even if its reply receiver has gone.
The request payload allocation is moved into its bounded owner envelope; the
transport frame is released before the worker waits for the reply.

## Verification

The temporary harness at /private/tmp/panelink-server-io-harness imports the
real wire, handshake, typed request/response, session, peer-identity, transport,
and worker modules by path. Its socketpair tests use a test-only fake
server::Connection and fake owner replies; no production connection token can
be constructed from wire fields. A separate compile check imported the real
src/server.rs, including its actual Connection API.

Actual commands and outputs:

    rustfmt --edition 2021 src/server_io.rs
    exit 0

    rustfmt --edition 2021 --check src/server_io.rs
    exit 0; no output

    CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo check --offline --manifest-path /private/tmp/panelink-server-io-harness/Cargo.toml --lib
    exit 0; Finished dev profile, with the real StateOwner and transport APIs

    CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-server-io-harness/Cargo.toml -- --test-threads=1
    exit 0; 116 passed, 0 failed, 2 ignored; worker_hub integration tests: 10 passed, 0 failed; doctests: 0

    CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --offline --manifest-path /private/tmp/panelink-server-io-harness/Cargo.toml --all-targets -- -D warnings
    exit 0; Finished dev profile

The ten socketpair cases cover the Hello/Welcome/Request/Response/Detach
roundtrip, one-in-flight-request ordering, request-before-Hello, client Error,
zero and duplicate request IDs, repeated Hello, malformed schema, invalid
owner error labels, oversized owner responses, handshake Error framing,
capacity-eight queueing, refusal of a ninth connection, bounded shutdown,
disconnect while awaiting an owner reply, and no automatic request retry.
The full imported-module test run includes the existing transport and peer
credential tests. Its two pathname-bind tests are explicitly ignored, not
counted as passes. P3-SOCKET-GATE.md records the actual sandbox EPERM pathname
bind attempt and the required external Apple Silicon macOS rerun; that gate
remains pending.

No dependency, manifest, lockfile, export, or shared runtime file was changed
by this worker. No whole Phase 3 completion is claimed.
