# Managed-launch codec task — 2026-10-18

Parent-approved pure experimental codec contract for milestone 2. This does
not enable a runtime launch API, grant management capability or close the
descriptor-inheritance gate. Parent owns exports/shared documents. A worker
may edit only src/managed_launch_protocol.rs and its result note.

Use existing OperationKey, Outcome and TargetRef vocabulary. Schema is 6;
watch schema 5 remains separately proposed. All integers are big-endian. The
entire request, including prefixes and values, must fit 65,536 bytes. Borrow
path/argument/environment byte slices rather than requiring UTF-8; any decoded
view allocations must have explicit count/byte bounds. Debug and error text
must redact all caller values, keys, paths and target identities.

Launch request kind 1 has a 64-byte prefix:

| Bytes | Field |
| --- | --- |
| 0..2 | schema 6 |
| 2..4 | kind 1 |
| 4..28 | generation, lease, sequence (three nonzero u64) |
| 28..30 | rows, u16, 5..80 |
| 30..32 | columns, u16, 20..240 |
| 32..34 | argument count, u16, at most 256 |
| 34..36 | environment count, u16, at most 4096 |
| 36..40 | requested executable byte length, u32 |
| 40..44 | requested working directory byte length, u32 |
| 44..48 | client working directory byte length, u32 |
| 48..64 | reserved zero |

After the prefix: executable bytes, requested cwd bytes, client cwd bytes,
then each argument as u32 length followed by bytes, then each environment entry
as u16 name length, u32 value length, name bytes and value bytes. Consume the
body exactly; reject trailing/truncated data, overflow and counts inconsistent
with the remaining byte budget before allocating views.

All three path fields are nonempty, NUL-free, at most 4096 raw bytes. Client cwd
must start with `/`; no filesystem access/canonicalization belongs in this
codec. Requested cwd may be relative; requested executable is an absolute path
or a slash-free basename. Reject a non-absolute executable containing `/`.
Arguments may be empty but contain no NUL. Environment names are nonempty,
NUL-free, contain no `=`, are unique and must not start with `PANELINK_`;
values may be empty but contain no NUL. Environment ordering remains exactly
as transmitted for future byte-for-byte deduplication. Do not normalize it.

This smallest wire contract carries a captured client environment, not named
profiles or override layers. Runtime must use the existing explicit environment
builder and Panelink-owned values; daemon environment inheritance is forbidden.
CLI capture must remove Panelink-owned identity names before encoding; server
validation still rejects a forged reserved name. No runtime layer is assigned
in this task.

Launch-status query kind 2 is exactly 32 bytes: schema/kind at 0..4, original
operation key at 4..28, reserved zero at 28..32. It observes the original
launch result without reserving another operation or reconstructing arguments.
Capability/lease authority belongs to the eventual owner, not this codec.

Result kind 0x8001 is exactly 80 bytes: schema/kind at 0..4; operation key at
4..28; outcome u16 at 28..30; has-target u16 at 30..32 (0 or 1); full TargetRef
as five u64 at 32..72; reserved zero at 72..80. If has-target is 0, target bytes
must all be zero. If 1, all target components are nonzero and generation equals
the operation key generation. Outcome codes are explicit here: 1 Accepted,
2 Applied, 3 Failed, 4 Unknown. Other Outcome variants are invalid for launch.
Applied requires a target; Failed may have none or a reserved target; Accepted
and Unknown require the reserved target. A response does not authorize effects.

Provide documented encode/decode/encoded-length APIs with fixed redacted
errors. Validate completely before any encoder writes; propagate writer
errors. No filesystem, process, sockets, persistence, launch retry or provider
logic. Classify launch as DurableMutation and status as Query using existing
OperationClass; neither codec result itself authorizes dispatch.

Automated tests must cover every valid request/result kind, non-UTF8 paths,
empty argument/value, integer/count/path/dimension/key boundaries, total-byte
exact limit and limit+1, all reserved bytes, truncation/trailing data, unknown
schema/kind/outcome, target presence/mismatch, duplicate/reserved environment
names, all NUL positions and redacted Debug/errors. Include a writer that
fails and verify invalid input writes nothing. Use table-driven/mutation cases
where they check independent invariants rather than echo implementation.

Run focused locked/offline tests with
CARGO_HOME=/private/tmp/panelink-p2-cargo-home, formatting, strict Clippy and
documentation checks. Parent adds the module export and runs integration.
Record actual outputs and release ownership; no dependency, manifest, runtime
capability or commit is assigned. Preserve unrelated in-progress source work.
