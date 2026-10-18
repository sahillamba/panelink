# P3-A2b event-envelope evidence

Status: event framing and encoded-size limits are implemented in the existing
pure wire codec. The required local checks pass; parent independent review and
integration remain pending. This task does not start the persistent server or
claim that runtime events are available.

## Scope

`src/protocol_wire.rs` adds message kind `Event` as numeric kind 7. Event frames
must use request ID zero; construction, header decoding, and encoder
prevalidation all reject a nonzero event request ID with the fixed
`EventRequestIdNotZero` error. Kind 8 remains unknown. Existing kind numbers,
payload caps, and the original Hello golden bytes are preserved.

The v1.0 envelope remains private and experimental. Decoders that predate this
addition reject kind 7. A future handshake must negotiate event capability
before a runtime emits events. This codec defines no event payload, subscription,
queue, dispatch, or authentication behavior.

The documented public limits describe validated frame sizes. They do not reserve
memory or establish queue enforcement:

| Kind | Number | Payload limit |
| --- | ---: | ---: |
| Hello, Welcome, Error | 1, 2, 6 | 4,096 bytes each |
| Request, Response, Event | 3, 4, 7 | 65,536 bytes each |
| Snapshot | 5 | 2,097,152 bytes |

With the 24-byte header, maximum encoded control/event frames are 65,560 bytes
and maximum snapshot frames are 2,097,176 bytes. A future writer policy reserving
one maximum snapshot plus one maximum control reply would account for 2,162,736
bytes per writer, or 17,301,888 bytes for eight writers. That accounting still
needs parent source review; this task does not implement or advertise those
queues.

## Tests and verification

The wire tests cover all seven kinds and their exact and one-byte-over payload
limits, an independent kind-7 golden frame with request ID zero, kind-8
rejection, and nonzero event-ID rejection at construction, header decoding, and
encoder prevalidation before any write. A counting writer verifies maximum
encoded Request, Event, and Snapshot frame lengths against the documented
limits. Existing golden bytes, borrowing, writer-failure, and payload-redaction
coverage remain in place.

Required local verification ran from `/Users/sahillamba/panelink` after the
parallel typed-codec owner restored `src/control_protocol.rs`:

```text
$ cargo fmt --check
exit 0; no output
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib protocol_wire
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.02s
     Running unittests src/lib.rs (target/debug/deps/panelink-87dcbc1ebcc07c4c)

running 13 tests
test protocol_wire::tests::decoded_payload_borrows_the_input_buffer ... ok
test protocol_wire::tests::encoder_rejects_invalid_internal_frame_before_first_write ... ok
test protocol_wire::tests::all_message_kinds_round_trip_binary_payloads_and_zero_request_id ... ok
test protocol_wire::tests::event_requires_zero_request_id_at_construction_decode_and_encode ... ok
test protocol_wire::tests::frame_debug_and_errors_do_not_disclose_payload_canary ... ok
test protocol_wire::tests::maximum_control_and_snapshot_encodings_match_frame_limits ... ok
test protocol_wire::tests::rejects_trailing_bytes_and_invalid_header_fields ... ok
test protocol_wire::tests::every_kind_obeys_its_exact_payload_boundary ... ok
test protocol_wire::tests::event_golden_header_uses_kind_seven_and_zero_request_id ... ok
test protocol_wire::tests::golden_header_bytes_are_stable ... ok
test protocol_wire::tests::rejects_short_headers_and_truncated_bodies ... ok
test protocol_wire::tests::u32_max_length_is_rejected_without_a_body_allocation ... ok
test protocol_wire::tests::writer_failure_preserves_only_the_accepted_prefix ... ok
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 72 filtered out; finished in 0.00s
exit 0
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.46s
exit 0
```

The first workspace `cargo fmt --check` attempt occurred during the other
owner's edit window and exited 1 because `src/lib.rs` referenced the temporarily
absent `src/control_protocol.rs`. After that file was restored, the exact
workspace format check above, targeted test, and strict Clippy all exited 0.

No dependency, manifest, lockfile, or parent-owned export was changed. Work is
limited to `src/protocol_wire.rs` and this evidence file; ownership is released
to the parent for independent review and integration.

## Parent integration

Parent reviewed the complete codec sources and added a cross-component regression in control_protocol.rs: the complete encoded target, owner revision and command kind participate in exact ledger payload equality. A known duplicate cannot dispatch twice; changing any of those fields under the same key conflicts. This is server-side duplicate defense, not authorization for a client to resend uncertain input.

Final parent `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline -q` exited 0: **122 passed / 9 ignored** (86 library + 21 binary + 4 host fixture/preflight + 11 core PTY; 0 doctests). Formatting, strict locked/offline all-target Clippy and diff checks passed. No dependency/manifest/lock changes. The one-shot CLI remains the only interactive executable path; typed schemas and Event framing are pure library foundations. No complete server/CLI/event subscription or authentication claim; no phase-completion commit or push.
