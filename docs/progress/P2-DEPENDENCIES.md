# Phase 2 dependency review — 2026-10-17

Status: source access restored and candidate graph resolved; source gates reject the set unchanged. See [source-gate evidence](P2-SOURCE-GATE.md) and the [77-package review inventory](p2-review/INVENTORY.md). Full dependency review and adoption remain incomplete. Application Cargo.toml and Cargo.lock remain the dependency-free foundation. These exact review candidates are not an approved manifest or corporate approval. The preliminary findings below describe earlier work; the source-gate record supersedes the historical access blocker.

Latest checkpoint: [P2-REPLACEMENT.md](P2-REPLACEMENT.md) supersedes the application-manifest status above. The original set below stays rejected unchanged; a separate 14-package core graph (11 Apple-target packages) is adopted with a documented local VT100 correction and Rustix/openpty. Host input remains unselected. This file preserves the original-candidate review history.

## Proposed experiment inputs

| Candidate | Proposed Cargo settings after review | Purpose and alternative |
| --- | --- | --- |
| alacritty_terminal 0.26.0 | `version = "=0.26.0", default-features = false` | Engine grid, modes and VT parser; avoids independently implementing terminal compatibility. No desktop renderer. |
| crossterm 0.29.0 | `version = "=0.29.0", default-features = false, features = ["events", "bracketed-paste"]` | Host raw mode, events and controlled drawing; avoids another host input parser. |
| portable-pty 0.9.0 | `version = "=0.9.0", default-features = false` | Next PTY candidate because engine's convenience launcher conflicts with launch requirements. Native PTY ownership and explicit command construction; not yet selected. |

Ratatui, direct serialization crates, async runtime, allocator replacements and a logging backend are unnecessary for this bounded experiment. This does not remove transitive dependencies with similar purposes.

The engine's [published manifest](https://docs.rs/crate/alacritty_terminal/0.26.0/source/Cargo.toml.orig) declares Apache-2.0 and defaults to optional Serde. Disabling defaults avoids that feature. Its direct requirements include base64, bitflags, home, libc, log, parking_lot, polling, regex-automata, unicode-width and vte; Unix adds rustix-openpty, rustix and signal-hook. The parser already enables its std/ansi features. The Windows requirements are separate target dependencies. These are requirements, not a resolved transitive inventory.

Crossterm's [published manifest](https://docs.rs/crate/crossterm/0.29.0/source/Cargo.toml.orig) declares MIT. The proposed features retain event reading and paste events while excluding Windows support, derive helpers, serialization, async streams and OSC52. On Unix, events enable mio, signal-hook and signal-hook-mio; rustix, parking_lot, bitflags and document-features also require review. Disabling the clipboard feature does not by itself enforce child clipboard denial.

Portable-pty's [published manifest](https://docs.rs/crate/portable-pty/0.9.0/source/Cargo.toml) declares MIT, empty default features and no package build script. Its direct requirements include anyhow, downcast-rs, filedescriptor, libc, log, nix, serial2 and shell-words; optional serialization remains disabled. The serial support and extra OS abstractions increase review/build scope even when Panelink only needs Unix PTYs. Do not add a direct anyhow/log dependency merely because these libraries use them.

## Material PTY API finding

The engine's [Options API](https://docs.rs/alacritty_terminal/latest/alacritty_terminal/tty/struct.Options.html), inspected as 0.26.0, exposes additional UTF-8 environment variables, not replacement of the complete child environment. Its [Unix launcher source](https://docs.rs/alacritty_terminal/latest/src/alacritty_terminal/tty/unix.rs.html), inspected as 0.26.0, builds a Command without clearing inherited variables, consults ambient USER/HOME/SHELL, and ignores the chdir result. Its Drop implementation sends SIGHUP then waits without a deadline. Therefore `tty::new`/`from_fd` are unsuitable unchanged for Panelink's explicit environment and fail-on-cwd-error contract. This is an integration assessment, not a finding that the upstream terminal's own use is incorrect.

Do not mutate the parent's global environment, introduce an env-clearing shell wrapper, or weaken the spec to use that launcher. Retaining the engine for emulation while reviewing a separate PTY owner is the next path. Portable-pty's [CommandBuilder API](https://docs.rs/portable-pty/latest/portable_pty/cmdbuilder/struct.CommandBuilder.html), inspected as 0.9.0, offers env_clear, OsStr arguments/environment and cwd. Review its actual Unix spawn path for failed cwd handling, unexpected environment injection, fd ownership, process groups and bounded teardown before selection. API availability is not proof of those behaviors.

The engine's [Event API](https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/event/enum.Event.html) separates ClipboardLoad/ClipboardStore from PtyWrite, color and size requests. Panelink must drop both clipboard events without invoking clipboard callbacks or logging contents, including when no viewer exists. Only bounded engine-generated terminal replies go back to the PTY; neither child byte streams nor arbitrary event strings go to the host. Query behavior still requires source inspection and executable tests.

Portable-pty's [builder source](https://docs.rs/portable-pty/latest/src/portable_pty/cmdbuilder.rs.html), inspected as 0.9.0, initially snapshots ambient environment in its constructors and derives Debug over the stored arguments/environment. Any adapter must immediately clear that snapshot before applying the explicit map and must never log/debug the builder. This observation does not complete review of its Unix spawn path.

## Adoption gate and acquisition

Before any manifest edit, acquire registry sources in an approved build environment and review the complete resolved graph for aarch64-apple-darwin and the distributable lockfile. Record exact versions/checksums, enabled features, licenses/notices, native code, build scripts/procedural macros, maintenance/release evidence and dated advisory results with dispositions. No full graph, advisory scan or build-impact measurement was possible here. Published versioned docs establish API candidates, not source checksum verification or an audit.

Use a disposable review package outside the application manifest to resolve these settings first. Proposed commands there: `cargo generate-lockfile`, `cargo metadata --locked --format-version 1`, and `cargo tree --locked --target aarch64-apple-darwin -e normal,build,features`. Inspect all packages and build inputs; these commands alone do not produce license/advisory approval. Review source and notices, then import an approved lock/vendor set if needed and build with `--locked --offline`. Do not reconstruct crate archives from web excerpts or bypass checksums. No remote installer, downloaded build script or source generator was executed.

## Actual resume evidence

Parent rechecked rustc/Cargo 1.99.1. `cargo fmt --check`, `cargo test --locked --offline` (20 library + 1 binary tests), and `cargo clippy --locked --offline --all-targets -- -D warnings` passed. These remain foundation checks.

`CARGO_NET_RETRY=0 CARGO_HTTP_TIMEOUT=10 cargo search alacritty_terminal --limit 1` exited 101: config.json could not be downloaded because index.crates.io could not resolve. `command -v opencode` found no executable on the active PATH. Web documentation was readable through the research tool; that does not make crate archives available to Cargo. No phase 2 build, PTY test, Apple Terminal observation, provider identity check or performance claim is recorded.
