# Snapshot renderer result

Implemented `terminal::render::render` as a pure streaming renderer from the core's sanitized `ScreenSnapshot`. It rejects undersized viewports before writing, emits only the contract's basic ANSI mode/cursor controls and typed SGR attributes, revalidates cell scalars and widths, and uses bounded ASCII fallbacks for malformed cells. It keeps no frame buffer and propagates the first writer error. No dependency or manifest change was made.

The seven focused tests use parser-derived snapshots plus synthetic private-cell canaries. They cover Unicode, combining and matched-wide cells; OSC 52/title/control/bidi/separator sanitization; malformed-width and orphan-cell fallbacks; style, colors and cursor controls; pre-write viewport rejection; partial writer failure; and an 80x240 worst-content snapshot through a counting sink. Every cell in the maximum case contains a 22-byte narrow-base-plus-seven-combining-scalar string and all five attributes with RGB foreground/background set to 255; the sink counts exactly 1,506,040 emitted bytes, below the 4 MiB limit.

Verification completed offline with the existing lockfile. `CARGO_TARGET_DIR` kept build output under `/private/tmp`:

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p2-render-target cargo test --locked --offline --lib terminal::render
running 7 tests
test terminal::render::tests::undersized_viewport_rejects_before_the_first_write ... ok
test terminal::render::tests::writer_failure_propagates_after_partial_output_without_restarting_frame ... ok
test terminal::render::tests::revalidation_falls_back_for_expanding_replacements_and_positions_next_cell ... ok
test terminal::render::tests::renderer_emits_only_typed_style_colors_and_clamped_cursor_controls ... ok
test terminal::render::tests::wide_and_orphan_continuation_fallbacks_stay_inside_their_rows ... ok
test terminal::render::tests::engine_snapshot_renders_unicode_combining_wide_cells_and_sanitized_controls ... ok
test terminal::render::tests::maximum_snapshot_streams_under_four_mib_without_a_frame_buffer ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 30 filtered out; finished in 0.02s
```

```text
$ cargo fmt --check
exit 0 (no output)
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p2-render-target cargo clippy --locked --offline --all-targets -- -D warnings
Checking panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s
exit 0
```

No deviations from the renderer contract were identified. Width validation is scalar-based and deliberately does not promise grapheme, emoji-sequence, font, or host-terminal width equivalence; the fallback limits malformed cells but cannot remove that compatibility uncertainty. These tests do not exercise host terminal IO, mode restoration after errors/panics, host resize, input, or Apple Terminal behavior. Basic ANSI support remains an untested host assumption. Parent owns integration review and all subsequent host-guard, outer-PTY, and release decisions; this result is not a dependency-adoption or release approval.

Parent integration update: source reviewed, worst-content frame bound spot-checked, brittle exact encoding-length assertion removed while retaining the <=4 MiB bound; full root suite49, fmt and Clippy passed after integration. The emitted-byte measurement is an observation of the current encoding. Direct unicode-width approval/export are parent-owned changes in P2-RENDER.md, not agent manifest edits. All ownership released; host IO/restoration remains the next gate.
