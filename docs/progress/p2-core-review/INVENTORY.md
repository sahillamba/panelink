# Adopted prototype component graph — 2026-10-17

Scope: current application manifest/lockfile after one-shot signal-flag adoption. The all-platform lockfile has 17 third-party components; the Apple-filtered graph has 14. Original core feature-tree/build artifacts remain the initial checkpoint; component filtered metadata/tree are host-components-metadata.json and host-components-tree.txt; latest probe graph is probe-metadata.json and probe-tree.txt. This inventory includes the local VT100 correction and all resolved platforms; only Apple Silicon macOS was reviewed for runtime use. License expressions are upstream declarations; preserved license/copyright texts are in `../../../third_party/licenses/`. Build-input review and limitations are in ../P2-REPLACEMENT.md.

| Package | Version | Apple graph | Declared license | Features | Source integrity |
| --- | --- | --- | --- | --- | --- |
| arrayvec | 0.7.8 | yes | MIT OR Apache-2.0 | none | d3fb67a6e08acf24fdeccbac2cb6ac4305825bd1f117462e0e6f2f193345ad56 |
| bitflags | 2.13.2 | yes | MIT OR Apache-2.0 | std | 3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06 |
| errno | 0.3.14 | yes | MIT OR Apache-2.0 | std | 39cab71617ae0d63f51a36d69f866391735b51691dbda63cf6f96d042b63efeb |
| itoa | 1.0.18 | yes | MIT OR Apache-2.0 | none | 8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682 |
| libc | 0.2.191 | yes | MIT OR Apache-2.0 | std | 65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a |
| linux-raw-sys | 0.12.1 | no | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | auxvec, elf, errno, general, ioctl, no_std, prctl | 32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53 |
| memchr | 2.8.3 | yes | Unlicense OR MIT | none | cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98 |
| rustix | 1.1.5 | yes | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | alloc, fs, process, pty, std, stdio, termios | 891efababe418670775f199f0d233d84843c227a0949a883ce15b37c78d6629d |
| rustix-openpty | 0.2.0 | yes | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | none | 1de16c7c59892b870a6336f185dc10943517f1327447096bbb7bb32cd85e2393 |
| unicode-width | 0.2.2 | yes | MIT OR Apache-2.0 | cjk, default | b4ac048d71ede7ee76d585517add45da530660ef4390e49b098733c6e897f254 |
| vt100 | 0.16.2-panelink.1 | yes | MIT | none | local path; vendor/vt100/PROVENANCE.json |
| vte | 0.15.0 | yes | Apache-2.0 OR MIT | none | a5924018406ce0063cd67f8e008104968b74b563ee1b85dde3ed1f7cb87d3dbd |
| windows-link | 0.2.1 | no | MIT OR Apache-2.0 | none | f0805222e57f7521d6a62e36fa9163bc891acd422f971defe97d64e70d0a4fe5 |
| windows-sys | 0.61.2 | no | MIT OR Apache-2.0 | Win32, Win32_Foundation, Win32_Networking, Win32_Networking_WinSock, Win32_System, Win32_System_Diagnostics, Win32_System_Diagnostics_Debug, default | ae137229bcbd6cdf0f7b80a31df61766145077ddf49416a728b02cb3921ff3fc |
| crossterm-input | 0.29.0-panelink.1 | yes | MIT | none | local path; vendor/crossterm-input/FINGERPRINTS.json |
| signal-hook | 0.4.5 | yes | MIT OR Apache-2.0 | none | b29ffa27d3d59a049bb328258de100f8012848d1b08d444e08bbc02ac468435e |
| signal-hook-registry | 1.4.8 | yes | MIT OR Apache-2.0 | none | c4db69cba1110affc0e9f7bcd48bbf87b3f4fc7c61fc9155afd4c469eb3d6c1b |

The extracted decoder uses only the already selected bitflags 2.13.2. unicode-width 0.2.2 is now also direct for rendering, with unchanged cjk/default features. Apple custom-build targets are libc/rustix plus signal-hook, whose selected build.rs is empty with extended-siginfo-raw disabled. No cc, extra native dependency or procedural macro is selected. Signal handlers are process-lifetime flags for the one-shot binary only; do not unregister during bounded cleanup. See ../P2-SIGNALS-REVIEW.md. See ../P2-INPUT-REVIEW.md for extraction, license, maintenance, advisory and API evidence.
