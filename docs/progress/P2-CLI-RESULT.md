# PTY probe CLI result

Implemented the exact one-shot command form `panelink pty-probe --terminfo <absolute-dir> -- <executable> [arguments...]` in `src/main.rs`. Parsing and argv-limit failures return 2 before environment capture, filesystem preflight or host access. The supported macOS/aarch64 branch validates and canonicalizes the explicit terminfo directory, captures `current_dir` and `vars_os` once into `LaunchContext`, resolves with the existing launch helpers, and passes an explicit map with owned `TERM=panelink-probe` and `TERMINFO`. Client environment capture is capped at 4,096 entries and 1 MiB of name/value bytes; the prospective final map is checked against those caps before construction after accounting for the owned overrides. Child argv is capped at 256 arguments and 64 KiB across executable plus arguments. No process-global environment is modified, and diagnostics omit paths, argv and environment values. CLI syntax and preflight errors use ordinary stderr. Errors returned by the host-probe call use its best-effort bounded `/dev/tty` reporter when both stdin and stdout are terminals; it opens a separate nonblocking, close-on-exec descriptor and gives output at most 100 ms, with no fallback to inherited stderr. Runtime errors with non-terminal stdin or stdout retain stderr reporting.

The binary-private `p2_probe::run` dispatch is compiled only for macOS/aarch64. Child status is returned unchanged, signal exit maps to `128 + signal`, explicit quit to 0 and interruption to 130. Other targets return the fixed unsupported message with status 1. Help describes the experimental one-shot child probe; doctor remains status 1 and states that the persistent multiplexer/server and reconnect are unsupported.

CLI tests do not launch a child or access a host TTY and do not mutate/read process-global environment. They cover accepted and malformed syntax, preserved executable/argument order, exact and over-limit argv/environment bounds, final owned-environment bounds, explicit launch resolution/cwd and owned TERM/TERMINFO without spawning, and exit-code mapping. The `/usr/bin/env` resolution fixture is inspected only; it is not executed.

## Verification

The focused binary test command passed after adding runtime-error routing:

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p2-cli-target cargo test --locked --offline --bin panelink
Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.43s
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

`rustfmt --check --edition 2021 --config skip_children=true src/main.rs` exited 0 with no output. The 13-test target includes the CLI tests and concurrently added host-probe tests. The new routing branch was compile-tested but not activated against a live TTY; doing so would exercise host I/O. No host error was deliberately induced.

Direct binary observations on Rust 1.99.1 `aarch64-apple-darwin`:

| Invocation | Exit | Output |
| --- | ---: | --- |
| `panelink --help` | 0 | Printed the probe syntax and stated persistent runtime/reconnect are not implemented. |
| `panelink --version` | 0 | `panelink 0.1.0` |
| `panelink doctor` | 1 | Printed target `macos/aarch64`, one-shot probe scope and unsupported persistent runtime. |
| `panelink pty-probe --terminfo relative -- /bin/sh` | 2 | `error: invalid command syntax; try 'panelink --help'` |
| `panelink pty-probe --terminfo /private/tmp/panelink-nonexistent-term-info-cli-check -- /bin/sh` | 1 | `error: terminfo directory is unavailable or invalid` |
| `panelink --unknown-option` | 2 | `error: invalid command syntax; try 'panelink --help'` |

The missing-terminfo case stops at preflight before capturing the client environment or entering host IO. No valid probe was run against the agent's terminal.

Strict whole-target Clippy remains pending the concurrent host-owner cleanup and parent integration run. The last `cargo clippy --locked --offline --all-targets -- -D warnings` reported host-file issues: unused `ProbeError::StopRequested` and `clippy::single_match` in `src/p2_probe.rs:661`; that result predates the latest host edits and has not been rerun here. The CLI source itself had no reported lint after its explicit environment-entry counter was changed to `enumerate`. Parent owns the integrated rerun after host source formatting/lints and outer-PTY verification. This report does not claim host terminal restoration, an interactive terminal UI, or release approval.

Parent final integrated checkpoint supersedes the earlier pending full-tree checks: 16 binary tests passed (eight CLI and eight host-unit checks), formatting and strict all-target Clippy passed. The full application suite passed66 tests with nine host scenarios explicitly ignored for /dev/tty EPERM. Live TTY error reporting and complete host runtime compatibility remain unverified.
