# P3-N3 explicit CLI receipts

Status: the bounded receipt action and its component checks are implemented.
This note does not claim a live server acceptance run or complete Phase 3.

The CLI accepts exactly
`control --directory DIR --context FILE receipt THROUGH_SEQUENCE`. The
sequence must be one nonzero decimal argument and stays within the existing
argument and absolute-path limits. While holding the context lock, the command
checks that no operation is uncertain and that `THROUGH_SEQUENCE` is below
`next_sequence`. It reports `operation_uncertain` or `receipt_out_of_range`
before attempting a connection. After that check, it resumes the exact stored
generation and lease and sends one existing `SessionRequest::Receipt` through
the bounded request exchange.

The response must decode as an existing typed response and match the exact
generation, lease, and through-sequence key. Only `Status(Completed(Applied))`
reports success, using the ordinary JSON status shape with `action: "receipt"`.
Other valid statuses and fixed rejection bodies fail. A fixed Error frame maps
to `server_rejected`; a wrong response key or body role, unexpected response
frame role, or malformed response maps to `response_invalid`.

Receipt does not reserve or acknowledge an operation, increment the local
sequence, alter uncertainty, write the context file, or store a local receipt
watermark. If its acknowledgment is lost, the caller receives a transport
failure while local metadata remains unchanged; a later explicit invocation
uses the same receipt identity, and the server's existing receipt operation is
idempotent. The CLI does not retry automatically.

## Verification

Using the existing writable Cargo cache, the final commands all exited 0:

- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --bin panelink p3_cli::tests` — 11 passed, 0 failed, 0 ignored.
- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --bin panelink -- -D warnings` — passed.
- `rustfmt --edition 2021 --check src/p3_cli.rs` — passed.

The first focused test run also passed, but reported one unused test variable.
The test was strengthened to compare the locked context file before and after
the receipt; the final run passed without warnings. Socketpair coverage checks
the exact receipt body, matching successful response, wrong key, non-applied
status, wrong body role, malformed response, and fixed server Error. A locked
context regression verifies uncertainty and out-of-range rejection happen
before the injected connect closure can run, with metadata unchanged.

No dependency or shared-file changes were made. Ownership of
`src/p3_cli.rs` and `docs/progress/P3-RECEIPT.md` is released to the parent for
review and runtime integration.
