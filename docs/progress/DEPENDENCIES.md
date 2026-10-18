# Foundation dependency and tooling record

Updated 2026-10-17. Scope: ROADMAP phase 1 only.

Phase 2 candidates and unresolved adoption evidence are tracked separately in [P2-DEPENDENCIES.md](P2-DEPENDENCIES.md). No application dependencies have been added.

## Shipped code

Phase 1 selects zero third-party crates and the Rust default allocator. Rust standard-library implementation and Apple platform dependencies still exist; zero direct crates is not an audit certification. No copied application implementation or competitor assets. No external source downloaded during implementation. No PTY/emulator, database, async runtime, network client, logging framework, or allocator crate selected yet.

## Development tools

- gpt-6-luna at xhigh: P1A/P1B/P1C implementation tasks; parent agent supervises architecture and integration.
- codebase-memory index: project Users-sahillamba-panelink; local graph for discovery only.
- Rust compiler/Cargo: 1.99.1 became available during implementation. `rust-toolchain.toml` selects the installed stable toolchain with rustfmt/Clippy; tested compiler 1.99.1 is recorded. Cargo generated `Cargo.lock`; it contains only Panelink. All phase 1 integrated checks passed. See INTEGRATION.md.
- ms-rust skill: requested, absent locally. Web-inspected community guideline snapshot recorded in PREBUILD.md; not installed, pinned, or executed. No compliance stamp.

## Before adding a dependency

Record exact version, features, required APIs, implementation effort saved, full dependency tree and licenses, native code/build scripts/procedural macros, advisory findings/disposition, and measured build impact when tools are available. Terminal emulation merits established libraries: lack of downloads now is not a reason to implement a custom parser. Keep future runtime dependencies separate from agent skills.
