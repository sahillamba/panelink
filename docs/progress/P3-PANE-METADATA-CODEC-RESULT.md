# P3 pane metadata codec result

Status: bounded pure-codec implementation, 2026-10-23. This does not implement native process inspection, worker scheduling, server authorization, CLI output, capability negotiation, or complete Phase 3.

Added `src/pane_metadata_protocol.rs` with the accepted schema-12 query and observation wire layouts, fixed public sizes/capability/status codes, exact target validation, strict reserved/length/status checks, absolute UTF-8 path validation, and custom redacted `Debug` implementations. The codec performs no I/O. Query and response payload sizes are bounded at 48 bytes and 8,262 bytes; each path is at most 4,095 UTF-8 bytes. No truncation, lossy conversion, argv/environment access, or persistence behavior is included.

The checked-in `TargetRef` currently contains `generation, workspace, pane, session, occupant_revision`. The codec encodes and decodes those existing fields in that order; this follows the parent clarification that the task's earlier `window` label was a typo. Existing schema-10 pane replies are untouched. The module declaration/export remains parent-owned, so this worker did not modify `src/lib.rs`.

Unit cases are included for query round trips and malformed targets/headers; available and unavailable observations; reserved bytes; lengths and trailing data; unknown/inconsistent status codes; invalid UTF-8; relative, empty, control-character, NUL, and over-limit paths; exact maximum path length; and path/identity redaction in `Debug`. `rustfmt --edition 2021 src/pane_metadata_protocol.rs` and `git diff --check -- src/pane_metadata_protocol.rs` completed successfully. The tests have not been executed because the new module is not yet exported from the crate root; the parent can export it and run the focused test target as part of integration review.

Remaining boundary: this result establishes only codec behavior. Native libproc ABI/query checks, identity races, worker timeout/busy semantics, permission checks, capability gating and end-to-end CLI JSON acceptance remain unimplemented and must not be inferred from these tests.

## Parent integration

The parent exported the module, corrected the malformed-zero-length case to
respect trailing-byte validation order, unified the capability constant with
the handshake module, and ran all five codec tests successfully. They also
pass in the final full serial Cargo suite. See
[P3-PANE-METADATA-RESULT.md](P3-PANE-METADATA-RESULT.md) for integration evidence.
