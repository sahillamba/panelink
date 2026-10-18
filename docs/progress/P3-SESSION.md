# P3 session payload codec

Status: bounded session Hello/request/info codecs implemented and locally
verified; parent integration and source review remain pending. These codecs
define metadata payloads only. They do not authenticate, issue or renew leases,
validate live targets or owners, admit ledger operations, dispatch mutations,
or detach a runtime connection.

## Implemented contract

`src/session_protocol.rs` exposes `LeaseRef`, `SessionHello`,
`SessionRequest`, `SessionInfo`, `SessionProtocolError`, and the
`encode_session_hello` / `decode_session_hello`,
`encode_session_request` / `decode_session_request`, and
`encode_session_info` / `decode_session_info` functions.

Fresh Hello uses the existing exact 28-byte schema-1 handshake codec.
Resumable Hello is exactly 44 bytes with schema 2, the same nonce and
capability fields, and a nonzero prior generation/lease pair. The codec
preserves unknown capability bits for later negotiation; it does not make
capability or lease-validity claims.

Session requests use schema 2 and opcodes 1 through 7 for Describe, Claim,
Release, Handoff, OperationStatus, Receipt, and Detach. Their exact sizes are
4, 80, 76, 88, 28, 28, and 4 bytes. Claim and Handoff TTLs must be 1 through 30
seconds. Every key and target identifier, expected owner revision, receipt
identity, and recipient client lease is validated before encoding; key and
target generations must match. Request encoding validates before allocating
its bounded 88-byte maximum output buffer. Debug output includes only the
operation kind.

SessionInfo is a fixed 72-byte schema-2 response of kind 1. It validates the
target and owner revision, the child-exited byte, zero reserved bytes, and the
owner-lease/expiry relationship. This first schema represents a client owner or
a vacant owner; human ownership is not represented as a client lease. Owner
mutation acknowledgments remain the existing schema-1 `Response::Status`; a
caller observes the latest revision through SessionInfo afterward. Receipt is
result retirement only, and Detach closes an attachment only; neither codec
path authorizes replay or stops a child.

## Verification

The graph index reported `ready` with 1,944 nodes and 6,490 edges. Graph search
located the existing Hello, response, operation-status, and TargetRef APIs;
direct source inspection confirmed their field and validation contracts.

A temporary harness at `/private/tmp/panelink_session_test.rs` imported
`protocol.rs`, `protocol_wire.rs`, `control_protocol.rs`,
`handshake_protocol.rs`, `response_protocol.rs`, and this module by path. Actual
checks:

- `rustc --edition=2021 --test /private/tmp/panelink_session_test.rs -o /private/tmp/panelink_session_tests` — exit 0. Rustc emitted one warning for the pre-existing `Ledger::generation` method being unused by the standalone harness.
- `/private/tmp/panelink_session_tests` — exit 0; **87 passed, 0 failed, 0 ignored**, including all 11 session codec tests.
- `/private/tmp/panelink_session_tests session_protocol::tests` — exit 0; **11 passed, 0 failed, 0 ignored**.
- `rustfmt --edition 2021 --check src/session_protocol.rs` — exit 0, no output.

The first harness compile exposed a missing local `write_target` helper, which
was added before the successful build. An early test expected a schema-2 Hello
truncated to exactly 28 bytes to return a length error; the decoder correctly
routes 28-byte payloads through the existing schema-1 decoder and returns its
unsupported-schema error, so the test now checks that behavior. No contract
deviations or dependency changes were made. The session module is not exported
by this worker; parent Cargo integration, full crate checks, and review remain
pending.
