# Phase 6 dependency and license inventory

Status: generated from Cargo metadata and a normal/build/dev dependency tree using locked offline commands.

Package rows: 39 (10 direct, 29 transitive).
Metadata command exit: 0; tree command exit: 0.

| Package | Version | Source | License expression | Role | Build script | Proc macro | Pulled by direct dependency | Review flags |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| arrayvec | 0.7.8 | registry | MIT OR Apache-2.0 | transitive | no | no | vt100 | none |
| autocfg | 1.5.1 | registry | Apache-2.0 OR MIT | transitive | no | no | proptest | none |
| bitflags | 2.13.2 | registry | MIT OR Apache-2.0 | transitive | no | no | crossterm-input, proptest, rustix, rustix-openpty | none |
| cfg-if | 1.0.5 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest, tracing-subscriber | none |
| chacha20 | 0.10.2 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| core_detect | 1.0.0 | registry | MIT/Apache-2.0 | transitive | no | no | proptest | none |
| cpufeatures | 0.3.1 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| crossterm-input | 0.29.0-panelink.1 | vendored | MIT | direct | no | no | crossterm-input | vendored-notice-cross-check |
| errno | 0.3.14 | registry | MIT OR Apache-2.0 | transitive | no | no | rustix, rustix-openpty, signal-hook | none |
| getrandom | 0.4.3 | registry | MIT OR Apache-2.0 | transitive | yes | no | proptest | build-script-review |
| itoa | 1.0.18 | registry | MIT OR Apache-2.0 | transitive | no | no | vt100 | none |
| lazy_static | 1.5.1 | registry | MIT OR Apache-2.0 | transitive | no | no | tracing-subscriber | none |
| libc | 0.2.191 | registry | MIT OR Apache-2.0 | direct | yes | no | libc | build-script-review |
| linux-raw-sys | 0.12.1 | registry | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | transitive | no | no | rustix, rustix-openpty | none |
| memchr | 2.8.3 | registry | Unlicense OR MIT | transitive | no | no | vt100 | non-permissive-or-unreviewed-license:Unlicense |
| num-traits | 0.2.19 | registry | MIT OR Apache-2.0 | transitive | yes | no | proptest | build-script-review |
| once_cell | 1.21.4 | registry | MIT OR Apache-2.0 | transitive | no | no | tracing, tracing-subscriber | none |
| pin-project-lite | 0.2.17 | registry | Apache-2.0 OR MIT | transitive | no | no | tracing | none |
| proptest | 1.12.0 | registry | MIT OR Apache-2.0 | direct | no | no | proptest | none |
| r-efi | 6.0.0 | registry | MIT OR Apache-2.0 OR LGPL-2.1-or-later | transitive | no | no | proptest | non-permissive-or-unreviewed-license:LGPL-2.1-or-later |
| rand | 0.10.3 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| rand_core | 0.10.1 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| rand_xorshift | 0.5.0 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| regex-syntax | 0.8.11 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| rustix | 1.1.6 | registry | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | direct | yes | no | rustix | build-script-review |
| rustix-openpty | 0.2.0 | registry | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | direct | no | no | rustix-openpty | none |
| sharded-slab | 0.1.7 | registry | MIT | transitive | no | no | tracing-subscriber | none |
| signal-hook | 0.4.5 | registry | MIT OR Apache-2.0 | direct | yes | no | signal-hook | build-script-review |
| signal-hook-registry | 1.4.8 | registry | MIT OR Apache-2.0 | transitive | no | no | signal-hook | none |
| thread_local | 1.1.10 | registry | MIT OR Apache-2.0 | transitive | no | no | tracing-subscriber | none |
| tracing | 0.1.44 | registry | MIT | direct | no | no | tracing | none |
| tracing-core | 0.1.36 | registry | MIT | transitive | no | no | tracing, tracing-subscriber | none |
| tracing-subscriber | 0.3.23 | registry | MIT | direct | no | no | tracing-subscriber | none |
| unarray | 0.1.4 | registry | MIT OR Apache-2.0 | transitive | no | no | proptest | none |
| unicode-width | 0.2.3 | registry | MIT OR Apache-2.0 | direct | no | no | unicode-width | none |
| vt100 | 0.16.2-panelink.2 | vendored | MIT | direct | no | no | vt100 | none |
| vte | 0.15.0 | registry | Apache-2.0 OR MIT | transitive | no | no | vt100 | none |
| windows-link | 0.2.1 | registry | MIT OR Apache-2.0 | transitive | no | no | rustix, rustix-openpty, signal-hook | none |
| windows-sys | 0.61.2 | registry | MIT OR Apache-2.0 | transitive | no | no | rustix, rustix-openpty, signal-hook | none |

## Vendored component notice cross-check

| Component | Vendor directory | `THIRD_PARTY_NOTICES.md` | `P2-REPLACEMENT.md` | Result |
| --- | --- | --- | --- | --- |
| crossterm-input | present | mentioned | missing | review-needed |
| vt100 | present | mentioned | mentioned | matched |

## Advisory scan

This generator does not execute an advisory scan. See the dated [security review](../SECURITY-REVIEW.md) and [tracing/proptest adoption review](TRACING-PROPTEST-DEPENDENCIES.md) for separately executed audit evidence.

Raw Cargo stdout/stderr, exit codes, package rows and command strings are retained in [p6-raw/dependency-inventory.json](p6-raw/dependency-inventory.json).
