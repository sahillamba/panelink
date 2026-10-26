# P5 pane operations wire slice

Date: 2026-10-25
Status: codec and focused contract tests implemented; no Cargo/build/test or runtime verification was run by this owner.

## Scope and boundary

This slice adds capability vocabulary (CAPABILITY_PANE_OPERATIONS = 64),
schema-13 request and authoritative-view codecs, and focused wire tests. It does
not offer capability 64 from a runtime path, dispatch schema-13 payloads, or
change legacy schema-8 UI requests and schema-9 layout replies. Capability
selection in the handshake codec is only a pure negotiation result; server
availability must continue to mean implemented and authorized behavior.

Mutation status and reconciliation remain the existing response/status
protocol. There is no new operation reply or duplicate outcome field. A client
uses the original operation key with existing operation status, then queries
the authoritative schema-13 view.

## Schema-13 offsets

All integer fields are big-endian. TargetRef is five u64 values in this order:
generation, workspace, pane, session, occupant revision. OperationKey is
generation, lease, sequence (u64 each). All fixed-size records reject
truncation and trailing bytes. Every request and view starts with schema u16 at
offset 0 and kind u16 at offset 2.

### Requests

| Kind | Name | Length | Offsets |
| ---: | --- | ---: | --- |
| 1 | ViewQuery | 8 | 0 schema u16; 2 kind u16; 4 reserved u32, zero |
| 2 | Swap | 116 | 0 header; 4 key (24); 28 expected layout revision u64; 36 source target (40); 76 destination target (40) |
| 3 | DividerSet | 80 | 0 header; 4 key (24); 28 expected layout revision u64; 36 target (40); 76 divider index u8; 77 first cells u16; 79 reserved u8, zero |
| 4 | ZoomSet | 80 | 0 header; 4 key (24); 28 expected layout revision u64; 36 target (40); 76 zoomed u8 (0 or 1); 77 reserved 3 bytes, zero |

Every mutation requires a nonzero key and expected revision, a complete
nonzero target (or targets), and target generation equal to key generation.
Swap targets must be distinct and share one workspace. Divider index is 0..2;
the wire bound for first cells is 1..239, and runtime geometry must check the
actual selected split. Zoom is strictly boolean. No codec check substitutes
for runtime target, ancestry, capability, workspace-owner, or layout-revision
validation.

### Authoritative view

Kind 5 is exactly 504 bytes:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | schema 13 |
| 2 | 2 | kind 5 |
| 4 | 424 | existing schema-9 LayoutReply, byte-for-byte |
| 428 | 1 | zoom-target-present flag (0 or 1) |
| 429 | 3 | reserved, zero |
| 432 | 40 | complete zoom target, or all zero when absent |
| 472 | 32 | four confirmed-size records, 8 bytes each |

Each confirmed-size record is presence u16 (0 or 1), reserved u16 zero,
rows u16, columns u16. An absent entry requires all remaining bytes zero.
Every occupied spatial layout slot requires a present size in range 5..78 by
20..238; every empty slot requires absence. A zoom target must exactly match an
occupied target.

LayoutReply remains the full, unprojected partition and target registry. Its
legacy pending_confirmed continues to mean the confirmed actual PTY size
differs from the underlying pane content size. If present, it must equal the
explicit confirmed size. If absent, that actual size must equal the underlying
pane content size. Therefore the owner must set it for a zoomed pane whenever
the PTY size differs from its underlying rectangle, even when the zoom resize
itself succeeded. Clients derive effective display size from zoomed_target
and root dimensions; the codec never projects or mutates the underlying
layout.

## Files and checks

Changed only src/handshake_protocol.rs among existing files; added
src/pane_operations_protocol.rs, tests/p5_pane_operations_wire.rs, and this
evidence note. The module root is intentionally not edited; the parent must
export it when integrating. No dependency was added.

The new tests cover request and view round trips, golden schema-13 swap and
legacy root-resize records, malformed lengths/schema/kind/reserved bytes,
zero identifiers/revision, generation/workspace mismatch, duplicate targets,
divider bounds, strict zoom booleans, explicit view-size consistency,
capability offered/unoffered negotiation, and debug redaction. The test file
also asserts the legacy schema-9 LayoutReply stays exactly 424 bytes. These
tests were authored but not run by this scoped owner.

Unnegotiated dispatch gating cannot be established by a pure codec. It remains
pending parent/server integration. Runtime capability offer, operation
status/query flow, owner resize semantics, client rendering, host acceptance,
and reboot/restore behavior are also outside this slice and remain unverified.

## Parent integration and focused verification

Parent exported the new module from lib.rs and corrected the validator's
confirmed-size upper bounds from outer 80x240 to content 78x238, matching the
specified contract. The locked/offline serial wire target passed **6 tests,
0 failed, 0 ignored**: `/private/tmp/panelink-p5-pane-wire-r1.log`.
Runtime capability offer and dispatch remain disabled pending owner/transport
implementation. This focused codec pass is not end-to-end pane acceptance.
