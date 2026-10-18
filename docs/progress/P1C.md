# P1C: launch environment and executable resolution

Status: implementation complete. The targeted launch tests pass, and the owned
Rust file passes `rustfmt --check`.

## Implementation

`src/launch.rs` provides a dependency-free `Environment`, client-scoped
`LaunchContext`, value-redacting `Debug` implementations, ordered environment
construction, and launch path resolution. Environment precedence is account
defaults, client context, ordered set/unset layers, then Panelink-owned values.
Caller `PANELINK_*` values are rejected, owned `TERM` is required and
nonempty, and names/values are validated without reading process-global
environment variables.

Resolution bases relative working directories on the client's absolute
directory, canonicalizes existing directories and selected executable targets,
checks regular-file and Unix executable mode, and searches only the explicit
effective `PATH`. It validates every `PATH` component before searching and
rejects missing `PATH`, empty components, relative components, and relative
executable paths containing a separator. The result carries requested and
canonical executable paths plus the canonical working directory; it does not
contain the environment. No process is spawned and no launch data is persisted.
Resolution is filesystem inspection, not a TOCTOU-safe spawn authority; a later
launcher must account for a target changing between inspection and spawn.

Unit scenarios are included for precedence/unsets, reserved and invalid
environment data, owned `TERM`, debug redaction, client isolation, explicit
executable and working-directory resolution, PATH ordering, and PATH
validation. They are proposed executable checks encoded in source, not
completed checks.

## Verification evidence

After the Rust toolchain became available, ran the targeted test and formatted
only the owned Rust file:

| Command | Actual result |
| --- | --- |
| `cargo test --lib launch` | Exit 0: 12 passed, 0 failed, 8 filtered out. |
| `rustfmt src/launch.rs` | Exit 0; formatted the owned source file. |
| `rustfmt --check src/launch.rs` | Exit 0; no formatting changes required. |

`cargo fmt --check` was initially attempted before the toolchain became
available and failed with `zsh:1: command not found: cargo`. It was not rerun
package-wide because other phase files are concurrently owned. The local test
run establishes compilation of the library and launch scenarios on the current
host; it does not establish Apple Terminal or macOS compatibility.
