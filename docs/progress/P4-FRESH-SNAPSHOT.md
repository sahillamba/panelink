# Phase 4 challenge-bound fresh snapshot slice

Status: implemented as an inactive component. The slice adds a version-2
outer protocol and grant-scoped Unix endpoints that couple each observation to
a newly issued snapshot challenge. It does not activate or wire an OpenCode
plugin, runtime owner, or durable writer. Full Phase 4 remains open.

## Wire contract

The fixed 24-byte network-order header is `PLBRSN02`, version 2 at byte 8,
kind at byte 10, payload length at byte 12, and challenge at byte 16. Kinds are
1 Hello, 2 Welcome, 3 SnapshotRequest, 4 SnapshotResponse, and 5 Provision.
Provision is sent server-to-source after a target-scoped grant exists; its
payload is a complete encoded version-1 Hello and its challenge is zero. Hello and
Welcome carry a complete encoded version-1 bridge message. SnapshotRequest
has an empty payload. SnapshotResponse carries a complete encoded version-1
Observation. Payloads are bounded by the existing codec's 4,388-byte maximum;
the version-2 header and payload must be exact.

Handshake packets require challenge zero. Snapshot requests and responses
require a nonzero challenge. The reader starts challenges at one and advances
without wrapping. It rejects any bytes already queued on the stream before it
writes a new challenge, then accepts only a response with the exact challenge
and configured target, project, provider version, and source instance. A failed
or partial operation poisons and closes the endpoint. Both endpoints check the
kernel peer UID before protocol bytes and configure nonblocking, close-on-exec,
and NOSIGPIPE flags.

## API and ordering

`FreshReader::accept(stream, grant, deadline)` consumes the existing scoped
grant, checks an exact full Hello, and sends the matching Welcome.
`FreshReader::snapshot(deadline)` issues one new challenge and returns one
scoped `Frame` only after its matching response.

After the server knows the managed target, `provision(&mut stream, &grant,
deadline)` sends the grant's Hello over the already-connected same-user stream.
The source can call `FreshSender::connect_provisioned(stream, deadline)` to
receive that Hello and continue with the normal Hello/Welcome handshake. This
avoids placing the capability secret in the source environment. It still trusts
same-UID peers and does not attest the source executable.

`FreshSender::connect(stream, hello, deadline)` completes the reciprocal
handshake. `FreshSender::respond_once(deadline, sample)` first reads and
validates a strictly increasing challenge, then calls its `FnOnce` sampler one
time, checks the resulting frame's full scope, and returns the frame with that
challenge. Invalid, duplicate, or regressed requests do not invoke the
callback. The source integration must call this method from its route sampling
loop; this module does not itself inspect provider UI state.

The callback ordering establishes that a cooperating source sampled after the
reader's request. The queued-byte check catches already-buffered responses
before the request. Same-user IPC and a grant still do not attest a provider
executable or protect against a malicious same-UID process. This is not an
atomic provider-state barrier, a crash-time freshness guarantee, or authority
to replay effects. Home, child, unknown, and unavailable observations remain
valid scoped frames for the existing receiver to interpret.

## Verification

Using `CARGO_HOME=/private/tmp/panelink-p4-cargo-home` and
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`:

```text
cargo test --locked --offline --lib provider_bridge_v2 -- --test-threads=1
7 passed, 0 failed, 0 ignored
cargo clippy --locked --offline --lib -- -D warnings
passed
rustfmt --edition 2021 src/provider_bridge_v2.rs
passed
```

The seven inline tests cover callback ordering/count, server-to-source grant
provisioning, prebuffered response refusal, duplicate and regressed challenges,
invalid request callback suppression, wrong source and response scope, partial
header EOF, absolute deadline expiry, packet kind/version/size bounds,
redacted diagnostics, and poisoning. The first compile attempt exposed three
local integration mistakes (a stale enum pattern, an inaccessible test-only
constant, and an unstable `UnixStream::peek` API); these were fixed before the
passing run. No provider plugin or runtime startup path is wired by this slice.
