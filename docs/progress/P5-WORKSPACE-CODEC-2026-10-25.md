# P5 workspace-index codec — 2026-10-25

Status: bounded in-memory binary codec implemented and focused tests pass.
This is not registry storage, atomic writing, migration, runtime routing, tab
UI, crash recovery, or power-loss evidence.

## API and format

`workspace_index::codec::MetadataRecord` owns a validated
`WorkspaceIndex`, nonzero revision, and `next_id`. Its public API is
`new(index, revision, next_id)`, `revision()`, `next_id()`, `index()`,
`encode()`, and `decode(bytes)`. `CodecError` reports stable categories and
does not include metadata names. `MetadataRecord` debug output reports only
revision, allocator watermark, and project count.

Version 1 uses big-endian integers and a 56-byte header: magic `PLWKIDX1`,
version 1, zero flags, exact total length, nonzero revision, allocator
watermark, reserved legacy tab ID 1, active project ID (zero only for empty
indexes), project count, and six zero reserved bytes. A project has a 24-byte
header, UTF-8 name, and ordered tab records. Each tab has a 16-byte header and
UTF-8 name. The caps are 32 projects, 8 tabs per project, and 64 UTF-8 bytes
per name, giving an exact maximum of 23,352 bytes.

Decode rejects unsupported versions, nonzero reserved bytes, malformed or
truncated lengths, trailing data, invalid UTF-8/names, over-cap records,
duplicate project IDs or tab IDs, invalid selections, stale active IDs, and
an allocator watermark that is not at least 2 and strictly greater than every
allocated ID. It checks the total input cap and count/length bounds before
allocating project/tab/name data. IDs share the allocator namespace; project
and tab IDs are checked for uniqueness within their own types, while each
must remain below `next_id`. An inactive project may have no selection even
when it has tabs; an active project must have a valid selected tab. A name-free
custom Debug implementation avoids logging project/tab names.

The golden migrated-default fixture is Project 1 / tab 1 with revision 1 and
next ID 2. Its encoded size is 112 bytes. The maximum-size model encodes to
exactly 23,352 bytes and decodes back with order and selection preserved.
The codec has no checksum and makes no durability or corruption-repair claim.

## Verification

`rustfmt --edition 2021 src/workspace_index.rs src/workspace_index/codec.rs
tests/p5_workspace_codec.rs` completed. The requested locked/offline test
command completed with **6 passed, 0 failed**. Compilation emitted existing
unused-code warnings from the unfinished clipboard-worker integration and
`App::load_screen`; the codec test itself passed without warnings.
