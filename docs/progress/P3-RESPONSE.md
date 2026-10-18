# P3 response payload codec

Status: bounded borrowed response codec implemented and worker checks passed;
parent integration/review completed; see INTEGRATION.md. This module defines response payload bytes
only. It does not implement a runtime read truncator, authorization, target
resolution, server dispatch, socket I/O, or client behavior.

## Contract and API

`src/response_protocol.rs` implements schema 1 with the approved 36-byte
big-endian prefix and a 65,536-byte complete-payload ceiling. Status, Read, and
Rejected kinds map to the existing `OperationStatus`, `Outcome`, `TargetRef`,
and `ReadSource` types. `Completed(Accepted)` is rejected because Accepted is
not terminal. Read responses validate nonzero key/target/screen identifiers,
matching generations, the source and truncation fields, UTF-8 display text,
disallowed control scalars, and the 1,024-line limit. UTF-8 text is borrowed;
decoding allocates no body storage.

The public API is `Response<'a> { key, body }`,
`ResponseBody::{Status, Read { target, source, screen_revision, truncated,
text }, Rejected}`, `RejectionCode`, `ResponseError`,
`decode_response_payload`, and `encode_response_payload`. `validate()` and
`encoded_len()` validate the full payload budget; `encoded_len()` includes the
36-byte prefix. A producer must compare that complete length with the `max_bytes`
from the corresponding Read request before encoding. The codec never clips
text. Custom Debug output omits keys, target IDs, revisions, and read contents.
Encoding validates before the first write and propagates partial writer errors.

## Verification

The graph index reported `ready` with 1,620 nodes and 4,850 edges. Graph search
located the existing exported `OperationStatus` and `Outcome` in
`src/protocol.rs`; source inspection confirmed the response dependencies.

A temporary harness at `/private/tmp/panelink_response_test.rs` imported
`protocol.rs`, `protocol_wire.rs`, `control_protocol.rs`, and this module by
path. Actual checks:

- `rustc --edition=2021 --test /private/tmp/panelink_response_test.rs -o /private/tmp/panelink_response_tests` — exit 0. Rustc emitted one warning that the pre-existing `Ledger::generation` method is unused in this standalone harness.
- `/private/tmp/panelink_response_tests` — exit 0; **61 passed, 0 failed, 0 ignored**, including 14 response codec tests.
- `/private/tmp/panelink_response_tests response_protocol::tests` — exit 0; **14 passed, 0 failed, 0 ignored**.
- `rustfmt --edition 2021 --check src/response_protocol.rs` — exit 0.

The first harness compile omitted `protocol_wire.rs`, which the existing
control-protocol tests import; adding that module fixed the harness. The first
response-test run also caught a short literal Read golden payload. The fixture
was corrected and the complete harness rerun successfully. No dependency,
manifest, export, or shared-file changes were made. Parent integration and
independent source review were subsequently completed. Parent added a regression
that a mismatched key/target generation is rejected by encoding before any
write, matching the decoder's check. Final combined results are in
[INTEGRATION.md](INTEGRATION.md).
