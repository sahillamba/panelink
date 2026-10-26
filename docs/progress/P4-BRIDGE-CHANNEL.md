# Phase 4 authorized provider bridge channel

Status: implemented as an inactive, same-user Unix transport slice; parent
full verification passes in [the combined result](P4-BRIDGE-TRANSPORT-RESULT.md). The module
accepts an already connected UnixStream; it creates no listener, socket path,
provider process, plugin, or runtime owner integration.

Grant::new accepts caller-supplied opaque 32-byte capability material, while
Grant::issue reads 32 bytes from /dev/urandom. Both bind the complete target,
project bytes, exact provider version 2.0.32, and nonzero source instance.
BridgeReader::accept consumes the grant, checks kernel peer credentials before
reading, validates Hello scope and secret, and sends Welcome. BridgeSender
performs the same peer check before sending Hello and requires Welcome to echo
the configured instance. The two endpoint types expose only scoped
observation operations, never the stream or its descriptor.

Reads and writes use nonblocking streams and one absolute Instant deadline for
a complete message. The fixed 32-byte header is validated before bounded body
allocation. Any failed handshake or observation operation shuts down the
stream; a returned endpoint also becomes permanently poisoned after an
operation failure. Debug output and fixed error messages omit targets, project
identities, provider versions, session IDs, and capability bytes. Grant drop
uses a best-effort fill on its stored secret bytes; this is not a secure-memory
erasure guarantee for all copies.

This is a possession grant delivered by a trusted issuer, not proof of provider
executable identity. Single-use means the grant value is consumed by one
accept call; this slice has no global replay registry or revocation service.
Same-UID hostile processes are not isolated. Fresh source instance assignment,
secret handoff/revocation, provider plugin activation, persistent writer and
owner integration, heartbeat policy, and recovery UI remain separate work.
The channel authenticates possession and transports metadata; it does not
prove when the provider observed a route. Buffered observations are not a
selection-ordering or fresh-reconciliation barrier. Runtime integration must
establish fresh snapshot semantics and bound queues before enabling identity
claims. Channel failure handling in the owner must mark the receiver unavailable,
preserve last-confirmed/manual fallback, and recheck expiry after IO.

Focused inline socketpair coverage verifies a same-user authenticated
handshake, the OS-random grant-to-Hello handoff, scoped observation delivery,
wrong secret/target/project/version/instance and message roles, wrong
observation target, unsupported sender version rejection, partial-header/body
EOF, oversized-header refusal before body read, stalled handshake/observation/
output, one absolute deadline during fragmented input, permanent poisoning,
and redacted diagnostics. These tests do not prove pathname socket permissions,
different-UID refusal, provider executable identity, plugin lifecycle, or
durable recovery. Parent-owned integration checks cover the pathname channel
through the bridge receiver and store.

Focused command used:

    CARGO_HOME=/private/tmp/panelink-p4-cargo-home
    CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target
    cargo test --locked --offline --lib provider_bridge_transport -- --test-threads=1

Result: 12 passed. Focused strict Clippy also passed with
cargo clippy --locked --offline --lib -- -D warnings. The activated
target/debug/panelink binary was not used or replaced.
