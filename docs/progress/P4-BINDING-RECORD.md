# P4 provider binding record codec — 2026-10-18

Status: bounded metadata codec implemented; validation results are recorded
below after parent exports the module for Cargo test discovery. This is a
serialization slice only. It adds no store, filesystem access, atomicity,
provenance, runtime identity, launch, or resume behavior.

## Record contract

`src/provider_binding_codec.rs` encodes a validated `Binding` as a bounded
network-endian record. The fixed header is 96 bytes:

| Offset | Bytes | Value |
| --- | ---: | --- |
| 0 | 8 | `PLBIND01` magic |
| 8 | 40 | `TargetRef`: generation, workspace, pane, session, occupant revision; five `u64` values |
| 48 | 8 | bridge instance (`u64`) |
| 56 | 8 | binding revision (`u64`) |
| 64 | 8 | verification Unix time in milliseconds (`u64`) |
| 72 | 2 | source tag: 1 `TuiRouteSnapshot`, 2 `OrderedSelection` |
| 74 | 2 | provider-version byte length |
| 76 | 2 | project-identity byte length |
| 78 | 2 | session-ID byte length |
| 80 | 16 | reserved, must be zero |

The body is exact provider-version bytes, opaque project-identity bytes, then
ASCII session-ID bytes. The decoder rejects unknown source tags, zero IDs or
ordering values, invalid field alphabets, NUL in project identity, truncation,
trailing data, and any nonzero reserved byte. Lengths and total input bounds
are checked before field allocation/copying. Maximum record size is 4,384
bytes (`96 + 64 + 4,096 + 128`). The encoder returns a `Vec<u8>` and validates
the complete binding before allocating it; it does not accept a writer.

The record intentionally preserves project identity and provider session ID
bytes. These fields are identity metadata, not credentials, and this codec does
not claim secret redaction for their persisted representation. It contains no
argv, environment values, terminal input, PID, transcript, or provider output.
Diagnostics are fixed and redacted.

`decode` returns a validated `Binding`, never a `LastConfirmedRecord`. Storage
integration must wrap a decoded value only after selecting its durable
last-confirmed slot. Restoration still starts stale. Serialization tests do
not demonstrate durable writes, atomic replacement, storage provenance,
crash recovery, or provider identity authenticity.

## Verification

| Command | Result |
| --- | --- |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline provider_binding_codec` | Exit 0; all 7 focused codec tests passed. |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings` | Exit 0; strict library and test Clippy passed. |
| `cargo fmt --check` | Exit 0. |
| `python3 scripts/check_docs.py` | Exit 0; 108 Markdown files checked, zero findings. |
| `git diff --check` | Exit 0. |

No dependencies or storage/runtime integrations were added. `src/lib.rs` and
shared integration documents remain parent-owned.
