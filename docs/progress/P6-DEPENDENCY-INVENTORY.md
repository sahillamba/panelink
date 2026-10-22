# Phase 6 dependency and license inventory

> Parent review note (2026-10-21): `cargo metadata --offline` exits 101 because
> it resolves ALL targets and `linux-raw-sys` (Linux-only) is not in the local
> registry cache; that is why every row is flagged `metadata-unavailable` and
> three Linux/Windows-only crates show UNKNOWN licenses. The macOS aarch64
> build graph (`cargo tree --target aarch64-apple-darwin`) contains 14 non-root
> packages and excludes linux-raw-sys, windows-sys and windows-link. Their
> published licenses are MIT OR Apache-2.0 family upstream, but that was not
> verified offline here. `memchr` is `Unlicense OR MIT` (MIT option usable).
> crossterm-input notice is present in THIRD_PARTY_NOTICES.md; its adoption
> record is P2-INPUT-REVIEW.md rather than P2-REPLACEMENT.md. Advisory
> scanning remains blocked (no cargo-audit/cargo-deny). This is an inventory,
> not a firm security approval.

Status: generated from Cargo metadata and a normal/build dependency tree using locked offline commands.

Package rows: 17 (7 direct, 10 transitive).
Metadata command exit: 101; tree command exit: 0.

| Package | Version | Source | License expression | Role | Build script | Proc macro | Pulled by direct dependency | Review flags |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arrayvec | 0.7.8 | registry | MIT OR Apache-2.0 | transitive | no | no | vt100 | metadata-unavailable |
| bitflags | 2.13.2 | registry | MIT OR Apache-2.0 | transitive | no | no | crossterm-input, rustix, rustix-openpty | metadata-unavailable |
| crossterm-input | 0.29.0-panelink.1 | vendored | MIT | direct | no | no | crossterm-input | metadata-unavailable |
| errno | 0.3.14 | registry | MIT OR Apache-2.0 | transitive | no | no | rustix, rustix-openpty, signal-hook | metadata-unavailable |
| itoa | 1.0.18 | registry | MIT OR Apache-2.0 | transitive | no | no | vt100 | metadata-unavailable |
| libc | 0.2.191 | registry | MIT OR Apache-2.0 | direct | yes | no | libc, rustix, rustix-openpty, signal-hook | build-script-review, metadata-unavailable |
| linux-raw-sys | 0.12.1 | registry | UNKNOWN | transitive | no | no | rustix, rustix-openpty | build-targets-unknown, metadata-unavailable, unknown-license |
| memchr | 2.8.3 | registry | Unlicense OR MIT | transitive | no | no | vt100 | metadata-unavailable, non-permissive-or-unreviewed-license:Unlicense |
| rustix | 1.1.5 | registry | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | direct | yes | no | rustix, rustix-openpty | build-script-review, metadata-unavailable |
| rustix-openpty | 0.2.0 | registry | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | direct | no | no | rustix-openpty | metadata-unavailable |
| signal-hook | 0.4.5 | registry | MIT OR Apache-2.0 | direct | yes | no | signal-hook | build-script-review, metadata-unavailable |
| signal-hook-registry | 1.4.8 | registry | MIT OR Apache-2.0 | transitive | no | no | signal-hook | metadata-unavailable |
| unicode-width | 0.2.2 | registry | MIT OR Apache-2.0 | direct | no | no | unicode-width, vt100 | metadata-unavailable |
| vt100 | 0.16.2-panelink.2 | vendored | MIT | direct | no | no | vt100 | metadata-unavailable |
| vte | 0.15.0 | registry | Apache-2.0 OR MIT | transitive | no | no | vt100 | metadata-unavailable |
| windows-link | 0.2.1 | registry | UNKNOWN | transitive | no | no | rustix, rustix-openpty, signal-hook | build-targets-unknown, metadata-unavailable, unknown-license |
| windows-sys | 0.61.2 | registry | UNKNOWN | transitive | no | no | rustix, rustix-openpty, signal-hook | build-targets-unknown, metadata-unavailable, unknown-license |

## Vendored component notice cross-check

| Component | Vendor directory | `THIRD_PARTY_NOTICES.md` | `P2-REPLACEMENT.md` | Result |
| --- | --- | --- | --- | --- |
| crossterm-input | present | mentioned | missing | review-needed |
| vt100 | present | mentioned | mentioned | matched |

## Advisory scan

Originally blocked (tools absent). With the user's approval (2026-10-21) the
parent installed `cargo-audit 0.22.2` via Homebrew and ran it against
`Cargo.lock` (18 crate entries including Panelink and target-gated crates):

```text
$ cargo audit --json
database: RustSec advisory-db commit 4b587f00996ebe5c05a73eaef105ef241e876414,
          last updated 2026-10-21T08:20:38+02:00, 1348 advisories
vulnerabilities: found=false, count=0
warnings (unmaintained/unsound/notice): none
```

The first attempt failed with a network timeout fetching the advisory DB; a
retry succeeded. Raw JSON: [p6-raw/cargo-audit.json](p6-raw/cargo-audit.json).
This is a point-in-time advisory check, not a security approval; vendored
local patches (vt100, crossterm-input) are not covered by RustSec beyond
their upstream crate names. cargo-deny was not installed or run.

Raw Cargo stdout/stderr, exit codes, package rows and command strings are retained in [p6-raw/dependency-inventory.json](p6-raw/dependency-inventory.json).
