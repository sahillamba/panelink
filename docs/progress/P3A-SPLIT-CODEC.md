# P3A wave 1 task B — schema-7 pane-split codec

Status: implemented 2026-10-18 as a pure, std-only library codec in
`src/split_protocol.rs`. It is groundwork for the wave-2 split transaction,
not a split operation. It is exported through a temporary
`pub mod split_protocol;` line in `src/lib.rs`, which the parent owns and must
review.

## Scope and limits

- Pure validation and encoding. No ledger admission, layout mutation, runtime
  route, socket, PTY, persistence or capability bit is added or chosen.
- How wave 2 admits this payload in `OperationLedger` is not decided here.
  Because one request carries the complete schema-6 launch, exact-payload
  deduplication over the whole request covers target, expected layout
  revision, direction and every launch field (including environment values).
- No new dependencies. The schema-6 codec is reused through its public
  `decode_launch_request`/`encode_launch_request`; private fixed-offset
  helpers are duplicated locally, and `managed_launch_protocol.rs` is
  unchanged.

## Wire format (big-endian, matching schema 6)

Request, kind 1 (`DurableMutation`):

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | schema 7 |
| 2 | 2 | kind 1 |
| 4 | 24 | operation key: generation, lease, sequence |
| 28 | 40 | target: generation, workspace, pane, session, occupant revision |
| 68 | 8 | expected layout revision |
| 76 | 2 | direction: 1 Left, 2 Right, 3 Up, 4 Down |
| 78 | 18 | reserved, zero |
| 96 | rest | one complete schema-6 launch request |

Rules: key, target and revision components are nonzero; target generation
equals key generation; the embedded launch key equals the outer key exactly;
the total is at most `MAX_OPERATION_PAYLOAD_BYTES` (65,536), so the embedded
launch is at most 65,440 bytes; reserved bytes are zero. The embedded decoder
consumes all remaining bytes, so trailing data surfaces as
`InvalidLaunch(TrailingBytes)`. The size cap is checked before any parsing.
The decoded `SplitRequestView` exposes `key`, `target`,
`expected_layout_revision`, `direction`, `launch` (`LaunchRequestView`) and
`launch_bytes` (the exact borrowed schema-6 slice for the existing launch
coordinator). The embedded launch's rows/columns are the caller's planned
new-pane size; the codec checks only the schema-6 5–80 × 20–240 bounds.

Status query, kind 2 (`Query`), exactly 32 bytes: schema, kind, key, four zero
bytes.

Result, kind `0x8001`, exactly 128 bytes:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | schema 7 |
| 2 | 2 | kind `0x8001` |
| 4 | 24 | operation key |
| 28 | 2 | outcome: 1 Accepted, 2 Applied, 3 Failed, 4 Unknown, 5 Partial |
| 30 | 2 | new-target flag, 0 or 1 |
| 32 | 8 | committed layout revision |
| 40 | 40 | existing target, always present |
| 80 | 40 | new target, all zero when absent |
| 120 | 8 | reserved, zero |

Result rules: both targets are fully nonzero and share the key generation; a
new target shares the existing workspace and differs in pane and session.
`Failed` requires revision 0 and no new target (no commit). Every other
outcome requires a nonzero revision and a new target, because `Accepted`,
`Partial` and `Unknown` all follow layout commit in `PANE-LAYOUT.md`.

## Errors and redaction

`SplitCodecError` has static messages; schema-6 failures map to
`InvalidLaunch(CodecError)`, whose inner enum is data-free. `SplitEncodeError`
mirrors schema-6 `EncodeError` (`Invalid`/`Writer`, writer error redacted in
`Debug`/`Display`). Every encoder validates fully before its first write.
`Debug` for request, view, status query and result prints only
`<redacted>`.

## Tests (14, all passing)

Golden bytes for request prefix plus embedded launch, status query and result;
four-direction round trips and unknown direction codes; every zero key,
target and revision field; generation mismatch; each embedded key component
mismatch; schema/kind; nonzero reserved bytes; truncation at every prefix and
launch length; trailing bytes; invalid embedded launch (dimensions, schema);
largest legal 65,536-byte request accepted and one byte over rejected by both
encoder and decoder; zero writer calls on every validation failure plus writer
error redaction; distinct bytes for direction, revision, target and one
environment byte changes; result outcome/revision/new-target and sibling
rules; Debug and error text containing no digits or launch data.

## Verification (2026-10-18)

- `cargo test --locked --offline --lib split_protocol`: 14 passed, 0 failed.
- `rustfmt --edition 2021 --check src/split_protocol.rs`: clean.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: clean.
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps`: clean.

## Decisions for parent review

- `Partial` is a valid result outcome (code 5), unlike schema 6, because the
  split contract reports a committed split with a failed launch or resize as
  `Partial`. Codes 1–4 match schema 6; `Accepted` stays valid as in schema 6.
- `Accepted` and `Unknown` require the new target, as schema 6 requires its
  target for those outcomes.
- Target generation mismatch is a distinct `GenerationMismatch` error rather
  than schema 6's combined `InvalidTarget`.
