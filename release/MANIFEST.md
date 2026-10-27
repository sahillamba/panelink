# Panelink release manifest

- Commit: `106066813299302ab4425b73ac95068d7eb09c41`
- Tag: `none`
- Dirty checkout: `true`
- Rust compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Cargo: `cargo 1.99.1 (5f94df478 2026-08-27)`
- macOS: `26.5.2` (arm64)
- Cargo.lock SHA-256: `7d281bb10eec384b2632f0d8118288051093d25be9f958b64aa05a33a9975b16`
- Vendor tree SHA-256: `7a8d3ea12b1e9b62b026e360f7781a745836d28c8809a511b552b59b3102ff76` (231 files)
- Notices SHA-256: `41584cd72018cde0458d96d423cd75eb94fa4067a5b98b1bed0f01e1a63c5298`
- Binary: `release/panelink-release` (3567760 bytes)
- Binary SHA-256: `c0af2dd88dce4c4dc3e14d16ed46a85dbba715aab3218f0dda81eedf1c893576`
- Build: `cargo build --locked --offline --release` in 10.735 s
- Independent build SHA-256 identical: `true`

## Linked libraries

- `	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1356.0.0)`
- `	/usr/lib/libiconv.2.dylib (compatibility version 7.0.0, current version 7.0.0)`

Reproducibility is an observation for these two builds, not a general guarantee.

## Historical dirty build

This manifest describes the local scrolling-repair build recorded on 2026-10-27,
not the subsequent consolidation checkpoint or pointer/wide-terminal changes.
The JSON records a dirty checkout but no digest of its source diff, so the
commit field alone cannot reproduce that source state. The ignored binary
matched the recorded size and hash during consolidation; it is not included
in this source PR or presented as a reviewed release. See
[the consolidation record](../docs/progress/WORKTREE-CONSOLIDATION-2026-10-27.md).
