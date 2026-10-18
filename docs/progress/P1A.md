# P1A — dependency-free CLI foundation

Status: implemented; the binary unit test and CLI behavior checks pass. Shared formatting and lint checks remain outside this phase's targeted verification.

## Delivered

- Added the single `panelink` Cargo package (edition 2021, MIT metadata, `publish = false`) without dependencies or a fabricated lockfile.
- Added a library root exporting the `protocol` and `launch` modules.
- Added CLI handling for default help, `--help`/`-h`, `--version`, and `doctor`. The doctor reports the target OS/architecture and that runtime support is not implemented, then exits 1. Unknown commands and extra arguments exit 2.
- Added a unit test for the help, doctor, and unknown-argument exit codes.
- Kept the executable free of socket, PTY, daemon, clipboard, and launch behavior. The README describes the current limits and usage.
- Review correction: `run` returns `i32`, matching `std::process::exit`; help text uses ASCII punctuation.

## Verification

The toolchain was initially unavailable, then became available. Targeted formatting and P1A verification produced these results:

| Command | Result |
| --- | --- |
| `rustfmt --edition 2021 src/main.rs src/lib.rs` | Exited 0. |
| `cargo test --bin panelink` | Exited 0; 1 test passed, 0 failed. |
| `cargo run -- --help` | Exited 0; printed CLI usage and that the multiplexer runtime is not implemented. |
| `cargo run -- --version` | Exited 0; printed `panelink 0.1.0`. |
| `cargo run -- doctor` | Exited 1; printed target `macos/aarch64` and `Runtime support: not implemented (experimental CLI foundation only).` |
| `cargo run -- --unknown-option` | Exited 2; printed a generic argument error and help hint. |
| `git diff --check` | Exited 0 with no output. |

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` were not run in this targeted pass. No full integrated lint or verification claim is made here.
