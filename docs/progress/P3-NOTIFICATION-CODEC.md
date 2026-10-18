# P3 N1 metadata notification codec

Status: N1 implementation complete; parent review and integration remain
pending. This pure Rust module defines and validates one fixed metadata event
payload. It does not add subscriptions, socket or PTY I/O, capability
advertisement, or live event delivery.

## Contract

`src/event_protocol.rs` exports `Event`, `EventBody`, `EventError`,
`EVENT_SCHEMA_VERSION = 3`, `EVENT_PAYLOAD_BYTES = 72`, `encode_event`,
`decode_event`, and `Event::validate`. `Event` carries a nonzero cursor, the
existing five-field `TargetRef`, and one of `ScreenChanged`,
`InputOwnerChanged`, or `ChildExited`. The codec uses the existing target field
order: generation, workspace, pane, session, occupant revision.

The payload is exactly 72 bytes: schema and kind u16s, a zero u32 reserved
field, cursor u64, five target u64s, revision u64, and owner lease u64. All
integers are big-endian. Required cursor, target identifiers, and screen/owner
revisions must be nonzero. A screen event encodes owner lease zero; an owner
event accepts zero for vacancy or a nonzero client lease; child exit requires
both trailing values zero. Length, schema, kind, reserved-field, and body
violations fail with fixed errors. Debug output redacts the cursor, target, and
body identifiers. No owner name, exit code, terminal contents, input, launch
metadata, provider status, or text is represented.

The existing outer wire envelope remains Event kind 7 with request ID zero.
This payload codec alone does not enable or advertise runtime Events.

## Verification

`rustfmt --edition 2021 --check src/event_protocol.rs` exited 0 with no output.

The requested temporary harness used a minimal `TargetRef` definition and
included the owned module directly. `rustc --edition=2021 --test
/private/tmp/panelink-event-codec-harness.rs -o
/private/tmp/panelink-event-codec-tests` exited 0 with no output. Running
`/private/tmp/panelink-event-codec-tests` exited 0: **6 passed, 0 failed**.
Coverage includes the exact golden layout, every body round trip, vacant and
client-owner leases, zero cursor/target/revision rejection on encode and
decode, invalid lengths/schema/kind/reserved bytes/body fields, and Debug
redaction.

After the parent exported the module, the focused crate check
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline
--lib event_protocol` exited 0: **6 passed, 0 failed, 212 filtered out**.

The first temporary-harness compile caught a golden fixture with 70 bytes
instead of 72; two omitted trailing zero bytes were added. The corrected
harness and crate checks above both pass. No dependency or non-owned file was
changed. Full phase verification and event runtime integration remain with the
parent. N1 releases ownership of `src/event_protocol.rs` and this evidence file
to the parent for review.
