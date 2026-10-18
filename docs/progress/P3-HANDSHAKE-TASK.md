# P3 handshake codec task — 2026-10-18

Scope: pure private v1.0 Hello/Welcome payloads, not a server, authentication,
lease issuance, durable generation or capability implementation. Parent owns
shared module exports, roadmap, canonical docs and integration. Worker owns only
`src/handshake_protocol.rs` and `docs/progress/P3-HANDSHAKE.md`; no commits.

Use std only. Schema u16=1, reserved u16=0. All integers big endian.
Hello exactly28bytes: schema@0, reserved@2, nonce u64@4, required capabilities
u64@12, optional capabilities u64@20. Nonce nonzero. Required/optional must be
disjoint. Unknown bits are syntactically accepted in Hello, allowing negotiation
to reject unknown required bits and ignore unknown optional bits.
Welcome exactly40bytes: schema@0, reserved@2, echoed nonce u64@4, generation
u64@12, lease u64@20, selected capabilities u64@28, ttl seconds u32@36.
All IDs nonzero; ttl exactly1800 (existing ledger policy). Known capability
bits: READ=1, INPUT=2, EVENTS=4, KNOWN=7. These are protocol vocabulary only;
no running server currently advertises or implements them. Welcome rejects
unknown selected bits; zero selected bits allowed.

Public Copy/Eq Hello/Welcome structs with these numeric fields, validation,
`encode_hello(&Hello)->Result<[u8;28],HandshakeError>`, `decode_hello(&[u8])`,
equivalent encode/decode Welcome, `negotiate(&Hello, available:u64,
generation:u64, lease:u64)->Result<Welcome,...>`, and
`validate_welcome(&Hello,&Welcome)->Result<(),...>`. Names may follow style.
Negotiate rejects unknown available bits and any required bits outside
available; selects required | (optional & available). It validates IDs through
Welcome, does not issue/renew leases or authenticate the connection. Document
that a runtime supplies only actually implemented/authorized capabilities after
peer authentication, and supplies a server-issued lease and unique generation.
Validate_welcome enforces both structs' validity, echoed nonce, all required
selected, and no selected capabilities outside required|optional. Envelope
version validation remains in protocol_wire; no alternate version negotiation.
Error enums have fixed labels without received bytes/IDs; custom Debug may omit
nonce and identifiers (prefer it for future logging discipline).

Tests: independently literal golden bytes for both payloads; roundtrips; every
truncation and extra byte; invalid schema/reserved; zero IDs; invalid ttl;
required/optional overlap; unknown required negotiation failure versus optional
ignore; server availability unknown rejection; missing required, unsolicited
selection and mismatched nonce Welcome rejection; envelope integration uses
Hello/Welcome kinds and existing wire codec. No sockets/effects/dependencies.

FIRST read ROADMAP.md, AGENTS.md, README.md, docs/SPEC.md,
docs/PANELINK-DISCOVERY.md, docs/PREBUILD.md, src/protocol_wire.rs and existing
codec style. Try graph index first; fall back on source if Transport closed.
Verify standalone initially (parent exports only after delivery):
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib handshake_protocol`
after parent integration. Worker may use a temporary rustc test harness with
path modules for protocol_wire and handshake_protocol; do not edit lib.rs.
Use rustfmt on your owned file only. Record actual outputs and limitations,
never mark a full phase done; no changes to ROADMAP.md (parent exclusive).
