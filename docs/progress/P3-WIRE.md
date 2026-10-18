# P3-A1 wire-envelope evidence

Status: implemented and locally verified on 2026-10-17. This completes only the
approved pure-framing slice. It does not implement typed operations, sockets,
peer authentication, identity issuance, a journal, a persistent server, or PTY
integration. Phase 2 Apple Terminal redraw/manual acceptance remains pending.

## Scope

`src/protocol_wire.rs` implements the parent-approved v1.0 envelope: a fixed
24-byte big-endian header, the six assigned message-kind numbers, zero reserved
flags, per-kind payload ceilings, and a borrowed opaque payload. Header decoding
validates the declaration before a caller can allocate a body. Complete-frame
decoding accepts one exact frame and borrows its payload. Encoding validates
before its first write and emits the header before the payload. No dependency or
manifest change was made. The parent-owned `lib.rs` export was not changed.

Wire limits implemented:

| Kind | Number | Payload ceiling |
| --- | ---: | ---: |
| Hello | 1 | 4,096 bytes |
| Welcome | 2 | 4,096 bytes |
| Request | 3 | 65,536 bytes |
| Response | 4 | 65,536 bytes |
| Snapshot | 5 | 2,097,152 bytes |
| Error | 6 | 4,096 bytes |

Only version 1.0 and flags 0 are accepted. Request ID zero is retained as
ordinary metadata; no request-ID authority or dispatch meaning is added. Payload
schemas and byte-string rules remain outside this implementation.

## Actual verification

Commands ran from `/Users/sahillamba/panelink`:

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib protocol_wire
   Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.20s
     Running unittests src/lib.rs (target/debug/deps/panelink-87dcbc1ebcc07c4c)

running 10 tests
test protocol_wire::tests::decoded_payload_borrows_the_input_buffer ... ok
test protocol_wire::tests::encoder_rejects_invalid_internal_frame_before_first_write ... ok
test protocol_wire::tests::frame_debug_and_errors_do_not_disclose_payload_canary ... ok
test protocol_wire::tests::u32_max_length_is_rejected_without_a_body_allocation ... ok
test protocol_wire::tests::golden_header_bytes_are_stable ... ok
test protocol_wire::tests::rejects_short_headers_and_truncated_bodies ... ok
test protocol_wire::tests::rejects_trailing_bytes_and_invalid_header_fields ... ok
test protocol_wire::tests::writer_failure_preserves_only_the_accepted_prefix ... ok
test protocol_wire::tests::all_message_kinds_round_trip_binary_payloads_and_zero_request_id ... ok
test protocol_wire::tests::every_kind_obeys_its_exact_payload_boundary ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 48 filtered out; finished in 0.00s
```

```text
$ cargo fmt --check
exit 0; no output
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s
exit 0
```

The targeted tests cover an independent golden frame, round trips for all six
kinds with binary payloads, request ID zero, invalid and truncated headers and
bodies, trailing and mismatched declared lengths, every kind's exact and
one-byte-over limit, `u32::MAX` header rejection, payload borrowing, invalid
frame rejection before writing, partial writer failure, and payload redaction
from frame diagnostics. The `u32::MAX` case exercises the allocation-free
`decode_header` path; it checks the error result rather than instrumenting the
allocator.

## Behavior boundaries and follow-up

There were no deviations from the approved P3-A1 contract. `write_all` can have
accepted a prefix when the writer fails; the error is propagated and bytes
already written cannot be rolled back. A transport using this codec must treat
an incomplete write as a failed connection/frame and must not restart the whole
frame on the same stream. The codec itself does not own or close a stream.

No socket, same-user peer credential, server generation or lease issuer, global
ledger accounting, expiry tombstone, launch journal, reconnect state transfer,
queue/backpressure, process effect, PTY, or host behavior was tested here. The
socket and persistence APIs called out as unverified in
[`P3-PROTOCOL-PLAN.md`](P3-PROTOCOL-PLAN.md) remain unverified. Proposed
acceptance scenarios in that plan are not results from this task.

Next bounded work should be parent review of this envelope, followed by the
planned P3-B extension to the existing admission ledger and a new
`docs/progress/P3-LEDGER.md` evidence file. Before that source change, the parent
should approve or replace the plan's provisional global connection, lease,
operation-count, retained-byte, and queue limits. P3-C socket implementation
must wait for exact std/Rustix API and permission/peer-identity review. Persistent
PTY adoption remains gated on the still-pending Phase 2 host acceptance and
separate parent review. Prototype assumptions and unresolved product choices
remain as recorded in `P3-PROTOCOL-PLAN.md`.

## Parent integration

Parent reviewed the complete source and independently ran the combined suite: **94passed /9ignored** (58library including10wire cases,21binary including new Ctrl-X dispatch test,4host fixture/preflight,11core PTY;0doctests), exit0. `cargo fmt --check`, strict locked/offline all-target Clippy and `git diff --check` exited0. The lib export is parent-owned. No new dependencies, manifest/lock changes, commits or pushes. No active ownership remains. No runtime handshake/lease/authentication/schema behavior follows from opaque framing. Socket consumers must validate the fixed header before allocating the body and include framing overhead in queue bounds.
