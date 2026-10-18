# P2 decoder extraction result — 2026-10-17

Status: **disposable parser-only candidate built and verified; not adopted by the application.** The crate and all third-party edits are confined to `/private/tmp/panelink-host-decoder/`. No application, root manifest, vendor, or roadmap files were changed by this task.

The candidate is `crossterm-input` `0.29.0-panelink.1`, edition 2021, MIT. Its only dependency is `bitflags = 2.13.2`; the lockfile contains only `bitflags`. The upstream MIT notice is preserved. Source and extraction evidence are in `/private/tmp/panelink-host-decoder/PROVENANCE.md`, `PATCH.diff`, `FINGERPRINTS.json`, and `upstream/`. The archive SHA-256 and crates.io index checksum both equal `d8b9f2e4c67f833b660cdb0a3523065869fb35570177239812ed4c905aeff87b`; the recorded VCS revision is `36d95b26a26e64b0f8c12edfe11f410a6d56a812`. The copied upstream event and Unix parser files match the registry sources byte-for-byte.

The extraction keeps the Unix upstream parser and its 30 parser tests. The type extraction keeps `KeyboardEnhancementFlags`, `Event`, mouse/key types, and `InternalEvent`, while omitting event IO and terminal command implementations. It removes feature-only cfg attributes, makes paste events available, fixes module paths, and disables doctests that require the full Crossterm facade. `Decoder` uses one boxed 65,548-byte buffer, a 16,384-byte ordinary sequence cap and a 65,548-byte paste-frame cap, checks before storing bytes, and returns each event directly without an event queue. Paste length and UTF-8 are checked before allocating its returned `String`; invalid input permanently poisons the decoder and subsequent input returns the same redacted error. `finish_escape` resolves only a single pending ESC. Query/keyboard-negotiation/device-attribute `InternalEvent` responses are dropped without replies or retained queue state.

Narrow upstream corrections also replace lossy paste conversion, check one-based cursor/mouse coordinates before subtracting (so zero-coordinate control input returns an error instead of panicking), replace the raw-mode lookup with the documented `INPUT_IS_RAW = true` constant, and redact Debug output for `Event`, `KeyCode`, `KeyEvent`, `MouseEvent`, `Decoder`, and `DecodeError`. The internal parser error kind is `InvalidData` rather than `Other`, avoiding an MSRV-raising `Error::other` call while passing current Clippy. The `Display` behavior for upstream key types is retained; application diagnostics must use the redacted Debug path.

The nine added adapter tests cover UTF-8 and arrow input, ESC timeout behavior, all opener/closer split positions, exactly 65,536 paste bytes, the first framed byte beyond the cap, the ordinary ceiling, invalid UTF-8 and malformed-input poisoning with a command-shaped suffix, zero-coordinate panic cases, dropped internal query responses/no retained events, and nested Debug redaction. Total: 39 passing tests, including all 30 retained upstream parser tests.

Source inspection bounds parser work per call by the retained sequence size: incomplete CSI/paste paths use fixed-prefix/suffix checks; sequence completion may scan or convert at most the corresponding 16 KiB or 64 KiB buffer once. This is a source-based bound, not a measured scheduling deadline. No host IO loop, per-turn time budget, raw-mode guard, terminal restoration, clipboard API, PTY integration, or Apple Terminal behavior was exercised. The upstream parser remains permissive for some recognized internal response forms (the primary-device-attributes parser is a stub); such recognized `InternalEvent` values are dropped, while parser errors, invalid paste, and limit failures poison the decoder. Do not claim every noncanonical sequence is rejected. A future host adapter still needs the separately specified byte/time budgets, raw-mode ownership, fail-closed cleanup, and restoration.

An initial Clippy run found a dangling upstream command doc after extraction and `io::ErrorKind::Other`; both were corrected locally without a global lint allow. The final required checks are below.

## Verification output

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --manifest-path /private/tmp/panelink-host-decoder/Cargo.toml
```

Actual output:

```text
   Compiling crossterm-input v0.29.0-panelink.1 (/private/tmp/panelink-host-decoder)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.33s
     Running unittests src/lib.rs (/private/tmp/panelink-host-decoder/target/debug/deps/crossterm_input-b6a7440a567486e0)

running 39 tests
test parse::tests::test_alt_ctrl ... ok
test decoder::tests::invalid_utf8_and_malformed_control_poison_without_suffix_events ... ok
test decoder::tests::debug_output_redacts_event_payloads_and_pending_bytes ... ok
test decoder::tests::decodes_utf8_and_arrow_across_byte_boundaries_once ... ok
test decoder::tests::finish_escape_resolves_only_single_escape ... ok
test decoder::tests::zero_terminal_coordinates_poison_without_panicking ... ok
test decoder::tests::internal_query_responses_are_dropped_without_queueing ... ok
test parse::tests::test_alt_key ... ok
test parse::tests::test_alt_shift ... ok
test parse::tests::test_esc_key ... ok
test parse::tests::test_parse_basic_csi_u_encoded_key_code ... ok
test parse::tests::test_parse_basic_csi_u_encoded_key_code_special_keys ... ok
test parse::tests::test_parse_char_event_uppercase ... ok
test parse::tests::test_parse_char_event_lowercase ... ok
test parse::tests::test_parse_csi ... ok
test parse::tests::test_parse_csi_bracketed_paste ... ok
test parse::tests::test_parse_csi_cursor_position ... ok
test parse::tests::test_parse_csi_focus ... ok
test parse::tests::test_parse_csi_modifier_key_code ... ok
test parse::tests::test_parse_csi_normal_mouse ... ok
test decoder::tests::paste_delimiters_may_be_split_at_every_boundary ... ok
test parse::tests::test_parse_csi_numbered_escape_code_with_types ... ok
test parse::tests::test_parse_csi_rxvt_mouse ... ok
test parse::tests::test_parse_csi_sgr_mouse ... ok
test parse::tests::test_parse_csi_special_key_code ... ok
test parse::tests::test_parse_csi_special_key_code_multiple_values_not_supported ... ok
test parse::tests::test_parse_csi_special_key_code_with_types ... ok
test parse::tests::test_parse_csi_u_encoded_key_code_has_modifier_on_modifier_press ... ok
test parse::tests::test_parse_csi_u_encoded_key_code_with_extra_modifiers ... ok
test parse::tests::test_parse_csi_u_encoded_key_code_with_extra_state ... ok
test parse::tests::test_parse_csi_u_encoded_key_code_with_types ... ok
test parse::tests::test_parse_csi_u_encoded_keypad_code ... ok
test parse::tests::test_parse_csi_u_with_shifted_keycode ... ok
test parse::tests::test_parse_event ... ok
test parse::tests::test_possible_esc_sequence ... ok
test parse::tests::test_parse_event_subsequent_calls ... ok
test parse::tests::test_utf8 ... ok
test decoder::tests::ordinary_sequence_ceiling_is_checked_before_store ... ok
test decoder::tests::accepts_maximum_paste_payload_and_rejects_first_excess_byte ... ok

test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check --manifest-path /private/tmp/panelink-host-decoder/Cargo.toml
```

Actual output: no output; exit status 0.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --manifest-path /private/tmp/panelink-host-decoder/Cargo.toml --all-targets -- -D warnings
```

Actual output:

```text
    Checking crossterm-input v0.29.0-panelink.1 (/private/tmp/panelink-host-decoder)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s
```

Parent adoption update: the candidate was independently reviewed/tested (39 passed) and all13 recorded fingerprints plus preserved originals/license verified. Its exact source/evidence is now in `vendor/crossterm-input/`. [P2-INPUT-REVIEW.md](P2-INPUT-REVIEW.md) is the adoption decision; the initial candidate status above describes this task's output before adoption. Root integration adds no upstream host IO and does not run dependency unit tests automatically. All ownership is released.
