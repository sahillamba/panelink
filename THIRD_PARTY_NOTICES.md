# Third-party components

## Tracing and property-testing additions — 2026-11-02

The pinned tracing runtime and development-only proptest graph add 22 registry
packages. [The adoption review](docs/progress/TRACING-PROPTEST-DEPENDENCIES.md)
records exact features, runtime/development/target scope, build inputs and
remaining review gaps. License and copyright files from 21 published crate
archives are preserved under `third_party/licenses/<crate>-<version>/`; MIT is
selected where offered. The lock-only UEFI dependency `r-efi 6.0.0` declares
MIT OR Apache-2.0 OR LGPL-2.1-or-later but ships no license text in its archive;
its notice provenance remains an explicit review gap before distributing that
target. It is not built for Apple Silicon macOS. The inventory conservatively
flags the offered LGPL alternative; no LGPL license option has been selected.

The older component counts below describe their original checkpoints. Current
package rows, including development dependencies, are in the
[generated inventory](docs/progress/P6-DEPENDENCY-INVENTORY.md). Inclusion in this
file does not establish a security audit or firm approval.

Panelink's original application logic is independently written. The terminal core deliberately uses the components listed in [the prototype inventory](docs/progress/p2-core-review/INVENTORY.md). Their code is not covered solely by Panelink's intended application license.

Preserved upstream license/copyright/notice texts for the 17 selected third-party components (including local variants) are under `third_party/licenses/<crate>-<upstream-version>/`. VT100's MIT notice is also in `vendor/vt100/LICENSE`. MIT is the selected license option where offered; other offered license texts and copyright files are preserved as provenance. Carry these notices with distributed source/binaries and recheck them when dependencies change. No binary distribution/installer is produced by this checkpoint.

The local VT100 variant is version `0.16.2-panelink.2`, derived from upstream 0.16.2 (MIT, copyright 2016 Jesse Luehrs). See [local corrections](vendor/vt100/PANELINK.md), [patch](vendor/vt100/panelink.patch) and [provenance](vendor/vt100/PROVENANCE.json). It is a selected third-party library with local corrections, not an upstream-approved release or independently authored Panelink engine.

Rust standard-library and Apple system-library/platform components remain part of the toolchain/operating-system boundary. Agent development skills and models are not shipped application dependencies. This notice inventory is not firm approval or a security audit.

The local crossterm-input variant is version `0.29.0-panelink.1`, a parser/types-only extraction from Crossterm 0.29.0 (MIT, Copyright 2019 Timon). Its [notice](vendor/crossterm-input/LICENSE), [provenance](vendor/crossterm-input/PROVENANCE.md), [complete patch](vendor/crossterm-input/PATCH.diff) and [fingerprints](vendor/crossterm-input/FINGERPRINTS.json) are preserved. The original event/parser files remain for review. This variant excludes upstream host IO and is not an upstream-approved Crossterm release. Additional component evidence is in [P2-INPUT-REVIEW.md](docs/progress/P2-INPUT-REVIEW.md).

Signal-hook 0.4.5 and signal-hook-registry 1.4.8 use the MIT option, with both offered license texts retained under `third_party/licenses/`. The original copyright texts are preserved unchanged. [Signal review](docs/progress/P2-SIGNALS-REVIEW.md) records the one-shot flag-only use, disabled features and process-lifetime handler limitation.

The existing libc0.2.191 component is also a direct macOS/aarch64 dependency for
the original peer-credential wrapper. MIT remains selected, with its existing
notices preserved; no new package or feature is introduced by this edge. See
[peer credential review](docs/progress/P3-PEER-REVIEW.md).
