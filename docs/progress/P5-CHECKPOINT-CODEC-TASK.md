# Read-only screen checkpoint codec — parent contract, 2026-10-18

This adopts one milestone-5 prerequisite, not history persistence, retention,
recovery or full-emulator serialization. Exclusive owner files: new
`src/screen_checkpoint.rs`, `tests/p5_screen_checkpoint.rs`, and
`P5-CHECKPOINT-CODEC.md`. Parent owns export/shared docs. No dependencies.
Use the existing terminal snapshot and original application code.

## Representation

Encode one validated `ScreenSnapshot` plus metadata to bounded bytes. Decode to
a borrowed read-only checkpoint view, never to live emulator input/state. No
API writes to a PTY, renderer, shell, clipboard or provider. No raw control-stream
or input log is stored. Normal and alternate checkpoint types stay distinct;
this module does not select cadence or merge alternate redraws into scrollback.

Fixed network-endian header, 96 bytes: magic `PLSCRN01` at 0..8; full TargetRef
as five nonzero u64 at 8..48; output sequence u64 at 48 (zero initial allowed);
capture Unix milliseconds u64 at 56 (nonzero); screen revision u64 at 64
(nonzero); rows u16 at 72, columns u16 at 74, cursor row u16 at 76, cursor column
u16 at 78; buffer byte at 80 (0 normal, 1 alternate); cursor-hidden byte at 81
(0/1); reserved 82..96 zero. Dimensions match existing core: rows5..80,
columns20..240, cursor strictly in range. Application-cursor/bracketed-paste and
other live modes deliberately have no executable recovery representation.

Body is exactly rows*columns row-major cells, each exactly 48 bytes:
text_len u8 at 0, flags u8 at 1, foreground4 at 2..6, background4 at 6..10,
text UTF-8 in fixed32-byte region10..42 (unused tail allzero), reserved42..48zero.
Flags: bit0 wide lead, bit1 wide continuation, bit2 bold, bit3 dim, bit4 italic,
bit5 underline, bit6 inverse; bit7 zero. Colors: 4 bytes, tag0 default requires
remaining3zero; tag1 indexed has index at byte1 and remaining2zero; tag2 RGB
uses all remaining3bytes; other tags invalid. Use the existing vt100::Color
variants when reading snapshot getters, without adding a dependency.

Text length0..32, valid UTF-8; reject all Unicode control characters and the
same unsafe display scalars as terminal::unsafe_for_host (U+2028/2029/061C/
200E/200F, U+202A..202E and U+2066..206F). No silent replacement on decoding:
corrupt/unsafe records reject. Wide flags cannot both be set. Wide lead requires
following continuation within the same row; continuation requires previous
wide lead and empty text. A continuation may retain its own rendition. Empty
ordinary cells are allowed. Do not infer glyph width from Unicode databases.

Maximum record =96 +80*240*48 =921,696 bytes. Reject oversized input before
allocation or iteration; validate header/dimensions/checked size and exact
payload length before examining cells. Encode validates fully before output
allocation/writes; decode returns immutable borrowed metadata/cell text with
private validated state (no callers can fabricate an unsafe decoded view).
Iterator cannot panic for arbitrary invalid bytes; malformed bytes never escape
validation. Debug/errors redact target/text/color payload data. No zeroization,
secret-free-content, crash durability or storage-provenance claim is made.

## Automated checks

Use independent golden byte fixtures for header offsets and representative
cells; exercise min/max dimensions, all target/metadata zeros, cursor limits,
all reserved/padding bytes and unknown flags/color/buffer/bool codes. Roundtrip
Unicode and supported attributes/colors; reject NUL/ESC/OSC-bearing controls,
invalid UTF-8, unsafe bidi controls, invalid text lengths and wide topology.
Try every truncated prefix of a small record and trailing/oversized bodies.
Maximum-size record succeeds and one extra byte rejects; no allocation from
untrusted counts before checked lengths. Include redaction assertions.

Private-PTY integration: child emits styled Unicode and normal/alternate content
plus a synthetic OSC52 clipboard canary; capture checkpoints through real
TerminalSession snapshots, encode/decode them, assert correct buffer tagging
and retained safe cells, and no stored ESC/query/OSC stream. Capture alt before
child switches back through an input handshake. Decode/search data causes no
PTY input/spawn or clipboard operation; provide no such API. Use explicit cwd/
env, bounded deadlines, RAII cleanup; no /dev/tty, pathname socket or operator.
This observes only captured screens, never claims overwritten frames survive.

Run focused module/integration tests with locked/offline Cargo and writable
CARGO_HOME, strict all-target Clippy, package fmt, docs checker/diff. Record actual
outputs and limitations in the owned note; parent reviews and integrates.
