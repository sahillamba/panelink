# Phase 4 bridge wire codec

Status: implemented as the bounded slice-1 codec; parent full integration and
verification pass in [the combined result](P4-BRIDGE-TRANSPORT-RESULT.md). This module defines the independent `PLBRDG01`
protocol-v1 envelope and typed Hello, Welcome, and provider observation
messages. It adds no dependencies and does not activate a provider or socket.

The fixed 32-byte network-order header validates magic, protocol, kind,
kind-specific minimum/maximum body size, and all reserved bytes before body
processing. Total messages are capped at 4,388 bytes. Hello bodies are 88 to
4,248 bytes; Welcome bodies are exactly 8 bytes; observation bodies are 68 to
4,356 bytes. Decoding requires one complete exact-length message and rejects
truncation, trailing bytes, invalid tags, invalid UTF-8 fields, zero target or
instance fields, zero observation sequence, all-zero Hello secrets, and invalid
selection/source combinations. Variable fields are copied only after fixed
header and exact body-length validation.

Hello project identity is opaque bounded nonempty bytes; provider versions use
the existing ASCII token rules and may be valid values other than the currently
admitted provider version. The channel grant may reject such values later.
Observation decoding delegates final identity checks to `Frame::new` and the
existing bounded selection/session validators. `Hello`, `Message`, `Frame`,
and the fixed `WireError` redact identity and capability material in Debug and
error output. Encoded messages contain sensitive identity metadata and the
handshake capability and must not be logged or persisted.

The module is owned in `src/provider_bridge_protocol.rs`; module-root exports
are parent-owned. Focused inline Rust tests cover message round trips, maximum
fields, malformed headers and observations, every truncated prefix, trailing
data, invalid UTF-8/ASCII/session values, and diagnostic redaction.

## Verification record

The required command was attempted after formatting the owned Rust source:

```text
rustfmt --edition 2021 src/provider_bridge_protocol.rs
CARGO_HOME=/private/tmp/panelink-p4-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo test --locked --offline --lib provider_bridge_protocol -- --test-threads=1
```

The first attempt stopped before compiling this module because the crate root
already declared `provider_bridge_transport` but its sibling source file had
not yet landed (`E0583`, file not found). After that file arrived, the first
compile briefly hit a duplicate helper in the concurrently written transport
module (`E0428`), which its owner resolved. The first completed test run
exposed a mistaken test expectation: a maximum Hello is smaller than a maximum
Observation because only observations carry the session field (5 pass, 1
fail). The assertion was corrected to verify each kind's exact bound. After
adding explicit malformed Hello and session cases, the final focused run
completed successfully:

```text
running 7 tests
test provider_bridge_protocol::tests::decoder_rejects_every_truncated_prefix_and_trailing_data ... ok
test provider_bridge_protocol::tests::diagnostics_redact_secret_identity_and_wire_error_is_fixed ... ok
test provider_bridge_protocol::tests::header_rejects_reserved_tags_and_kind_specific_bad_lengths ... ok
test provider_bridge_protocol::tests::hello_welcome_and_every_observation_state_round_trip ... ok
test provider_bridge_protocol::tests::malformed_hello_rejects_invalid_utf8_ascii_and_exact_length ... ok
test provider_bridge_protocol::tests::malformed_observation_tags_lengths_utf8_and_zero_sequence_are_rejected ... ok
test provider_bridge_protocol::tests::max_bounded_fields_fit_caps_and_invalid_hello_fields_are_refused ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 751 filtered out
```

Strict library Clippy with `-D warnings` also passed. Its earlier run reported
five needless-borrow diagnostics in the transport module; the channel owner
corrected those before the passing rerun.

This is focused codec evidence only; it does not establish transport, provider
authenticity, live activation, or full Phase 4 readiness.
