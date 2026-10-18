# Non-UI verification runner

`python3 scripts/verify_non_ui.py` runs the fixed offline checks for delivered
components from the repository root, regardless of the caller's working
directory. It runs root Rust tests, the vendored parser-only decoder tests,
component source fingerprint checks, formatting, strict Clippy, Rust build,
the repository's Python unit tests, and
the Markdown checker in sequence. Immediately after the root Rust tests, it
runs:

```sh
cargo test --locked --offline --manifest-path vendor/crossterm-input/Cargo.toml
```

Each command has a 180-second timeout, inherits stdout/stderr, receives no
stdin, and is isolated in a process group so a timed-out group can be signaled.
The direct child gets a bounded two-second reap wait; the summary does not
claim that grandchildren were reaped. If the direct child remains unreaped,
the check status is `timed_out_cleanup_incomplete`. A failed check does not
skip later checks.

Cargo uses the caller's `CARGO_HOME` when set. Without it, the runner selects
`/private/tmp/panelink-p2-cargo-home` only when that directory already exists
and is writable. The runner never downloads dependencies or launches provider
processes. It prints a fixed JSON summary containing check names and statuses;
the summary does not include command arguments, environment values, or child
output. Child stdout and stderr are inherited as-is, so their diagnostics are
outside the summary's redaction guarantee.
Exit status is 0 when every check passes, 1 when any check fails, and 2 when
setup is invalid.

The summary always sets `component_verification_only` to true,
`milestones_complete` to false, and `host_provider_gates` to
`not_certified`. This runner does not certify the five non-UI milestones,
pathname socket acceptance, host terminal behavior, or live OpenCode identity.

Focused automated tests use an injected command runner and do not start Cargo
or recursively execute the Python suite. Timeout process cleanup, bounded
post-kill waiting, and interrupt cleanup are tested with mocked child
processes. `--help` prints fixed usage; unknown arguments return setup status 2
without echoing the supplied argument.

## Verification record

The owner ran the focused Python suite (11 tests), Python bytecode
compilation, and the documentation checker after adding the decoder command.
The direct locked/offline decoder command passed 39 tests. The runner tests
inject their process runner, so they do not spawn Cargo or recursively execute
the Python suite. The parent ran the earlier six-command integrated verifier
before the decoder check was added; the seven-command integrated rerun is
pending parent review. These checks validate the runner tooling, not the
project milestones.

Parent review corrected timeout cleanup and interruption handling, then added
normalization of a relative caller CARGO_HOME before the child changes directory.
An empty explicit CARGO_HOME is a setup error. The original focused tooling
suite passed 10 tests. The updated suite passes 11 tests after adding exact
command-list, failure-continuation, and injected-runner coverage. The
integrated run follows the owner state split review.

The parent ran the integrated verifier after reviewing the private owner-state
split and before the decoder command was appended: exit0, all six checks
passed. Rust400passed/14ignored, Python19passed, strict Clippy, formatting and
build passed; docs115files/0findings. Host/provider gates remain uncertified
as the output explicitly records. That six-check result does not include the
new vendor decoder check; the current seven-check integrated rerun remains
pending.

Parent subsequently reviewed the complete seven-command list and ran it:
exit0, all seven checks passed, root Rust412passed/14ignored plus separate
decoder39passed, Python20passed, strict Clippy, formatting, build and
124Markdown/0findings. This supersedes the pending seven-check status above.
It includes the global preflight and final scope-guard corrections; it does
not yet include the subsequently assigned multi-owner assembly implementation.

The final eight-check verifier now includes component source fingerprints.
Parent reviewed its nonblocking regular-file opens, path/hash validation and
actual cumulative byte limit, and independently passed22focused tooling tests
and the live fingerprint check. The full integrated run passed **root Rust431,
14ignored; separate decoder39; Python31; both source manifests match; fmt,
strict all-target Clippy, build and133Markdown/0findings**. A separate strict
rustdoc run passed. This includes multi-owner assembly, all session variants,
scoped reads and both real-PTY integration cases. All acceptance exclusions
above remain in force.
