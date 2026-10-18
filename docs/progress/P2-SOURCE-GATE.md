# Phase 2 source gate — 2026-10-17

Result: candidate set rejected unchanged for this experiment. Registry access is restored; P2B remains unstarted because source review fails the documented allocation and launch/cleanup gates. This is a scoped integration finding, not a general security verdict about the upstream projects.

## Acquisition and reproducible evidence

`cargo fetch --locked` in the application exited 0, but the application has no dependencies and that command alone does not prove registry access. `CARGO_NET_RETRY=0 CARGO_HTTP_TIMEOUT=10 cargo search alacritty_terminal --limit 1` then exited 0 and returned 0.26.0. A disposable review package at `/private/tmp/panelink-p2-review` resolved the exact candidate settings. Its initial fetch exited 101 because the default `~/.cargo/registry` cache is not writable in this environment; this was a filesystem restriction, not DNS failure.

With `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`, `CARGO_NET_RETRY=0`, and `CARGO_HTTP_TIMEOUT=10`, `cargo fetch --manifest-path /private/tmp/panelink-p2-review/Cargo.toml` exited 0 and downloaded all 77 resolved third-party packages, including other platforms. Locked/offline metadata, target-filtered metadata, and `cargo tree --target aarch64-apple-darwin -e normal,build,features` each exited 0. The target-filtered graph has 46 reachable third-party packages.

The durable [review manifest](p2-review/Cargo.toml), [review lockfile](p2-review/Cargo.lock), [target tree](p2-review/tree-aarch64-apple-darwin.txt), and [inventory](p2-review/INVENTORY.md) preserve candidate resolution without modifying the application's manifest or lockfile. All 77 downloaded archive SHA-256 values were independently compared with the review lockfile and matched. Sources/cache under `/private/tmp` are disposable; reacquire using the review package if they disappear. Set `CARGO_HOME` to a writable cache; do not replace Panelink's root manifest with the review manifest.

These artifacts establish versions, declared licenses, enabled features, declared build-script/procedural-macro targets, and archive integrity. They do not establish full notice review, maintenance assessment, advisory disposition, native/build-source review, build overhead, firm approval, or compatibility. No candidate dependency was compiled and no candidate build script was executed in this review.

## Failing engine bounds

Source paths below are relative to the versioned crate archive named in each row; exact checksums are in the inventory. Codebase-memory indexing/search preceded source inspection.

| Boundary | Source evidence | Disposition |
| --- | --- | --- |
| At most eight extra combining scalars per cell | `alacritty_terminal-0.26.0/src/term/cell.rs:125-129` stores `zerowidth` in `Vec<char>`; `Cell::push_zerowidth`, lines 164-167, unconditionally appends. `src/term/mod.rs:1062-1087` routes successive zero-width input to the previous cell without advancing the cursor. | Fails unchanged. A fixed grid/history size cannot bound one cell's growing combining allocation. |
| Control strings bounded to 16 KiB before allocation | Alacritty's published `Cargo.toml` enables `vte/std` and `vte/ansi` even with Alacritty defaults disabled. In `vte-0.15.0/src/lib.rs:55-70`, the std parser uses `Vec<u8>` for OSC storage; `Parser::action_osc_put`, lines 544-552, checks fullness only without std, then appends. | Fails unchanged. An unterminated OSC can grow across arbitrarily many 16 KiB feed chunks before any clipboard/title event can be dropped. |
| Conservative 64 MiB retained engine bound | The two preceding stores have no finite configured limits in this candidate path. | Cannot derive the required bound. Smaller dimensions/history and observed RSS do not fix these stores. |

VTE's no-std fixed OSC buffer does not fix the selected graph: Cargo features are additive and Alacritty explicitly enables std. Filtering events after parsing, sanitizing the renderer, or truncating a final snapshot happens after the problematic allocation. No second parser, post-allocation workaround, upstream fork, or relaxed limit was introduced.

An independent gpt-6-luna xhigh source review confirmed both failures and checked public configuration: `Term::Config` (`src/term/mod.rs:332-365`) exposes history and OSC52 policy but no combining/control-string limits; VTE's custom-size parser constructor is only available without std. Disabling OSC52 cannot bound raw OSC storage. This review changed no files and ran no candidate builds/tests. The parent inspected the decisive source paths independently before recording the result.

## Additional portable-pty findings

The candidate's public builder API is insufficient evidence of contract compliance. Source inspection found:

- `portable-pty-0.9.0/src/cmdbuilder.rs:498-539` (`CommandBuilder::as_command`) filters the requested cwd with `is_dir()` and falls back to the user's home when that check fails. A prevalidated directory removed before this conversion could silently launch elsewhere. It also inserts `SHELL` before applying the explicit map; an explicit map omitting `SHELL` is therefore not reproduced exactly. `env_clear()` on the builder does not remove this later insertion.
- `src/unix.rs:228-298` (`PtyFd::spawn_command`) constructs the std command, attaches cloned slave descriptors to stdin/stdout/stderr, installs a pre-exec hook, calls `setsid` and optionally `TIOCSCTTY`, then calls `spawn`. Those session/terminal setup failures are propagated. This does not repair the earlier cwd fallback.
- `src/unix.rs:394-404` (`UnixMasterWriter::drop`) writes newline plus the configured EOF byte with `write_all`. This is an implicit child-input effect and has no explicit deadline in this path. A bounded, side-effect-free cleanup path has not been established.

These are source observations, not runtime reproductions. The candidate is not approved unchanged. Resize, signal races, descendant cleanup, all fd lifetimes, query conformance, full dependency/build review, and host behavior remain unverified. No PTY was created during this review.

## Verification and next action

Parent reran `cargo fmt --check`, `cargo test --locked --offline`, and `cargo clippy --locked --offline --all-targets -- -D warnings`: all exited 0; 20 library tests and one binary test passed, zero doctests. Rust/Cargo remain 1.99.1. `command -v opencode` returned no executable. These are foundation checks only.

P2A has reached the experiment's explicit stop condition. Next action: review an alternative engine/backend or a narrowly scoped upstream correction that provides the allocation and exact-launch/cleanup contracts, before resolving/adopting a replacement candidate set. Complete the remaining dependency review only for a viable set. Keep the application dependency-free and P2B–P2E unticked until that gate passes. No source-control commit/push was made.

Final documentation verification: `git diff --check` exited 0; 33 local documentation links resolved, with no trailing whitespace or conflict markers in the checked roadmap/progress/spec/readiness files. Application lockfile still contains only Panelink; the separate review lockfile contains 77 dependencies plus its disposable root package.
