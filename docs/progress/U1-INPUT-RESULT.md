# U1 typed keyboard support — result — 2026-10-22

Status: implementation and focused verification complete; assigned source files
released to the parent for integration review. The pane UI schema remains 8.
Codes 1–12 retain their previous meanings and byte encodings. Codes 13–20 now
represent the bounded control, Alt character, function-key and navigation-key
variants. Old servers reject unknown key codes during request decoding; no
mixed-version compatibility or host-terminal compatibility claim is made.

## Behavior

The core accepts only typed values. `Control(u8)` accepts 1–26 and emits that
single byte. `AltChar(char)` accepts a non-control scalar and emits ESC followed
by its UTF-8 encoding. `Function(u8)` accepts F1–F12 and emits the specified
SS3/CSI sequence. Delete, Insert, Page Up, Page Down and BackTab emit CSI 3~,
2~, 5~, 6~ and Z respectively. Invalid core values return `InvalidKey`.
Debug output redacts Control and Alt character contents.

The codec maps codes 13–15 to Control, AltChar and Function; codes 16–20 map to
Delete, Insert, PageUp, PageDown and BackTab. Scalar bounds and Unicode validity
are checked on both encoding and decoding; all nonzero reserved fields and
unknown codes continue to fail closed. Key input stays bounded and classified
as non-replayable Input. Existing golden request bytes still pass.

The client retains Ctrl-B prefix/NAV, Ctrl-X detach and Ctrl-C Interrupt. Other
plain Ctrl-letter events map to Control values; plain F1–F12, Alt printable
characters (with optional Shift), Delete/Insert/Page keys and Shift-Tab map to
their typed forms. Release events and unsupported modifier combinations are
ignored. No arbitrary host escape stream or mouse key handling was added.

The disposable `panelink-probe` terminfo profile now advertises the key
sequences it represents: Home/End, F1–F12, Delete, Insert, Page Up/Down and
BackTab. Control bytes are terminal input values rather than named terminfo
capabilities. The profile has no generic Alt-character capability; the client
recognizes Alt-modified character events independently of this probe profile.

## Verification

- `cargo test --locked --offline --lib ui_protocol -- --test-threads=1` —
  passed: 11 tests, 0 failed, 544 filtered out. This includes old golden bytes,
  new strict round trips, malformed scalar rejection, reserved-byte checks,
  key count limits and Input classification.
- `cargo test --locked --offline --lib terminal::unit_tests -- --test-threads=1`
  — passed: 11 tests, 0 failed, 544 filtered out. This includes all control
  bytes, Alt UTF-8, F1–F12 and navigation sequences, invalid values and debug
  redaction.
- `cargo test --locked --offline --bin panelink ui_client::keys -- --test-threads=1`
  — passed: 4 tests, 0 failed, 95 filtered out. This includes decoded host
  sequences, reserved shortcuts, releases, optional Shift with Alt and rejected
  modifier combinations.
- `rustfmt --check --edition 2021 src/terminal/mod.rs src/terminal/unit_tests.rs
  src/ui_protocol.rs src/ui_client/keys.rs` — passed.
- A repository-wide `cargo fmt --check` was also attempted. It reported
  formatting differences in the concurrently edited `src/ui_client/state.rs`
  owned by U2; this worker did not alter that file. The first formatter check
  also identified formatting in the U1 files, which was corrected before the
  final scoped check above.

No dependencies, vendor files, roadmap/state files, commits or pushes were
changed by this worker. Apple Terminal host behavior and any old-server/new-
client interaction beyond fail-closed unknown-code decoding were not tested.
