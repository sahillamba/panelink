# Phase 4 bridge wire and authorized Unix channel — 2026-10-24

Status: both scoped component slices implemented and parent-verified; see
[P4-BRIDGE-TRANSPORT-RESULT.md](P4-BRIDGE-TRANSPORT-RESULT.md). Two GPT-6 Luna High owners have exclusive new module/note
ownership. Parent owns Frame accessors, module exports, combined tests, review
and verification. Existing bridge/core/store and user workspace stay unchanged
except read-only Frame accessors. No dependencies or live activation.

## Wire contract (slice 1)

Independent bridge magic `PLBRDG01`, distinct from control protocol. Header32:
magic8, protocol u16=1 at8, kind u16 at10, body-length u32 at12, reserved16 at16
(all zero). Network byte order. Kinds1 Hello,2 Welcome,3 Observation.

Hello body: full TargetRef40 at0; bridge instance u64 at40; secret32 at48;
version length u16 at80; project length u16 at82; reserved4 at84; exact version
then project bytes. Secret is nonzero opaque OS-random capability material,
never a provider credential. Body min88, max4248. Welcome body is one nonzero
bridge instance u64, exact8 bytes. Observation body: target40; instance u64
at40; sequence u64 at48; scope u8 at56 (1 top-level,2 child); selection u8 at57
(1 session,2 Home,3 child,4 unknown,5 unavailable); source u8 at58
(0 absent,1 route snapshot,2 ordered selection); reserved u8 at59=0;
version/project/session lengths u16 at60/62/64; reserved u16 at66=0;
then exact version/project/session bytes. Non-session states require source0
and session length0. Selected session requires source1/2 and bounded nonempty
session ID. Observation body min68, max4356. All existing bounded identity
validators apply. MAX_BODY_BYTES=4356; MAX_MESSAGE_BYTES=4388.

Public protocol API: `Message::{Hello(Hello),Welcome{bridge_instance},
Observation(Frame)}`, `Hello::new(target,project:&[u8],version:&str,instance,
secret:[u8;32])`, read-only scope getters and crate-only secret getter;
`encode(&Message)`, `decode(&[u8])`, `decode_header(&[u8;HEADER_BYTES])` returning
validated Header with body_len/kind. All errors and Debug redact identities and
secret. Exact lengths, reserved bytes, enum tags and size caps are checked
before variable-field allocation. Wire bytes intentionally contain exact
identity metadata and the handshake capability; never log or persist them.

## Authorized Unix channel (slice 2)

Use existing macOS peer_identity::require_same_user before any input. Add no
listener or global socket path. A caller passes an already-connected owned
UnixStream. Enforce nonblocking finite absolute deadlines for the entire
message, both reads and writes, no timeout reset per byte, bounded allocation
only after validated fixed header. Poison and shut down after any partial IO,
timeout, malformed message, wrong message role or wrong scope; no retry/replay.
Set NOSIGPIPE and descriptor flags consistently with existing transport.

API: `Grant::new(...)` with secret supplied by trusted issuer, or `Grant::issue`
using32 OS-random bytes; configured full target/project/exact2.0.32/instance.
Grant is consumed by `BridgeReader::accept(stream,grant,deadline)`; it is not
Clone. Reader checks exact Hello scope and capability, then sends Welcome.
`BridgeSender::connect(stream,hello,deadline)` sends Hello only after kernel
peer verification, requires matching Welcome, and exposes only scoped
`send_observation(&Frame,deadline)`. Reader exposes only scoped
`read_observation(deadline)->Frame`. Neither exposes the raw stream/clones or
input/process/control authority. Sequence admission stays in BridgeReceiver.
Different-user refusal is a unit gate; same-user socketpair/pathname fixtures
are live local transport evidence, not provider executable attestation.

A grant proves possession of caller-issued scope, not provider authenticity.
Issuer integration must arrange a fresh nonreused source instance, secret
handoff and revocation. Same-UID hostile-process isolation, a retained global
replay registry, provider plugin activation, async durable writer/owner wiring,
production heartbeat policy and recovery UI remain open. On channel failure,
the owner must mark the receiver unavailable and retain last-confirmed/manual
fallback. It must check expiry after IO before advertising current state.

## Verification

Workers: focused locked/offline serial inline Rust tests, own-file rustfmt,
actual outputs and explicit release. Parent: independent malformed-length/
secret/scope review, combined channel→receiver→store tests, fragmentation and
stall/EOF poisoning, private pathname fixture, full serial Rust suite and seven
supporting checks. Test types and skipped/environment limits remain distinct.
Use CARGO_HOME=/private/tmp/panelink-p4-cargo-home and
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target to preserve the
activated target/debug/panelink binary. No commits or pushes.
