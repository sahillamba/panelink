# P3 handshake codec — implementation evidence

Date: 2026-10-18. Scope: pure v1.0 Hello/Welcome payload validation and
encoding under `src/handshake_protocol.rs`. This slice does not implement a
server, peer authentication, capability support, lease issuance/renewal,
durable or unique generation creation, sockets, or runtime effects.

## Delivered

- Exact 28-byte Hello and 40-byte Welcome payloads with big-endian fields,
  schema 1, zero reserved fields, nonzero identifiers, and fixed 1800-second
  Welcome TTL.
- Capability constants for READ, INPUT, and EVENTS. Hello preserves unknown
  bits; negotiation rejects unknown server availability and unavailable
  required bits, selects required plus available optional bits, and ignores
  unknown optional bits. Welcome permits zero selected bits and rejects
  unknown selected bits.
- `validate_welcome` checks both payloads, nonce echo, required capabilities,
  and that the response selects only offered capabilities.
- Fixed error labels and custom Debug implementations that redact handshake
  nonces and server identifiers.
- Tests for independent literal golden payloads, round trips, every truncation
  and trailing byte, malformed schema/reserved fields, zero IDs, invalid TTL,
  capability negotiation and validation failures, debug redaction, and Hello /
  Welcome framing through the existing envelope codec.

All identifiers and capability availability remain caller-supplied values.
The runtime must authenticate the peer first and advertise only features it
actually implements and authorizes. The codec does not make those claims true.
The parent owns shared module exports and crate integration.

## Verification

The checkout did not have an indexed project entry. A task-requested fast,
nonpersistent code graph indexing attempt succeeded before implementation:

```text
project: Users-sahillamba-panelink
status: indexed
mode: fast; persistence: false
nodes: 775; edges: 3446
excluded: target, docs, third_party, .git, vendor
```

The owned Rust file was formatted:

```text
$ rustfmt --edition 2021 src/handshake_protocol.rs
exit_code=0
```

A temporary harness at `/private/tmp/panelink-handshake-harness.rs` imported
`src/protocol_wire.rs` and `src/handshake_protocol.rs` by path. Compilation:

```text
$ rustc --edition=2021 --test /private/tmp/panelink-handshake-harness.rs -o /private/tmp/panelink-handshake-tests
exit_code=0
```

Harness output:

```text
running 25 tests
test handshake_protocol::tests::negotiates_known_available_bits_and_ignores_unknown_optional_bits ... ok
test handshake_protocol::tests::hello_golden_bytes_are_stable_and_independently_decodable ... ok
test handshake_protocol::tests::redacts_nonce_and_server_identifiers_from_debug_output ... ok
test handshake_protocol::tests::rejects_every_truncation_and_any_trailing_byte ... ok
test handshake_protocol::tests::handshake_payloads_use_the_existing_hello_and_welcome_envelope_kinds ... ok
test handshake_protocol::tests::rejects_unknown_selected_capabilities_and_allows_zero_selection ... ok
test handshake_protocol::tests::rejects_unknown_server_availability_and_unavailable_required_bits ... ok
test handshake_protocol::tests::rejects_unsupported_schema_and_nonzero_reserved_fields ... ok
test handshake_protocol::tests::rejects_zero_identifiers_invalid_ttl_and_overlapping_masks ... ok
test handshake_protocol::tests::valid_payloads_round_trip ... ok
test handshake_protocol::tests::validates_required_bits_offers_nonce_and_no_unsolicited_selection ... ok
test handshake_protocol::tests::welcome_golden_bytes_are_stable_and_independently_decodable ... ok
test protocol_wire::tests::all_message_kinds_round_trip_binary_payloads_and_zero_request_id ... ok
test protocol_wire::tests::decoded_payload_borrows_the_input_buffer ... ok
test protocol_wire::tests::encoder_rejects_invalid_internal_frame_before_first_write ... ok
test protocol_wire::tests::event_golden_header_uses_kind_seven_and_zero_request_id ... ok
test protocol_wire::tests::event_requires_zero_request_id_at_construction_decode_and_encode ... ok
test protocol_wire::tests::frame_debug_and_errors_do_not_disclose_payload_canary ... ok
test protocol_wire::tests::every_kind_obeys_its_exact_payload_boundary ... ok
test protocol_wire::tests::golden_header_bytes_are_stable ... ok
test protocol_wire::tests::maximum_control_and_snapshot_encodings_match_frame_limits ... ok
test protocol_wire::tests::rejects_short_headers_and_truncated_bodies ... ok
test protocol_wire::tests::rejects_trailing_bytes_and_invalid_header_fields ... ok
test protocol_wire::tests::u32_max_length_is_rejected_without_a_body_allocation ... ok
test protocol_wire::tests::writer_failure_preserves_only_the_accepted_prefix ... ok

test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit_code=0
```

There were no contract deviations. Crate-level Cargo verification remains for
the parent after adding the shared module export; no full Phase 3 completion is
claimed here.

Parent integration: exported module, preserved unknown optional bits through a
payload roundtrip, added a decoded-invalid-field matrix and an assertion that
the advertised TTL matches the ledger's expiry policy. Final crate suite passed
139tests with10explicit ignores; strict Clippy passed. See INTEGRATION.md for
the initial unrelated intermittent child-reap assertion and its successful
focused/final reruns. Final worker graph refresh indexed832nodes/3759edges.
