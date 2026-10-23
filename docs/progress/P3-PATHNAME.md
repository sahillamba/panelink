# P3 pathname and short-lived CLI fixture

Date: 2026-10-18. Scope: `tests/p3_pathname.rs` and this evidence note. The
fixture is one explicitly ignored macOS/aarch64 acceptance test. It does not
establish pathname listener acceptance because the local sandbox denied bind.

## Current bounded CLI acceptance — 2026-10-23

Parent explicitly ran the existing ignored fixture against the current
integrated working tree with pathname sockets enabled:

```sh
cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --exact --ignored --test-threads=1
```

**1 passed/0 failed/0 ignored**,1filtered out,1.20s. This now includes the
expanded reconciliation and finite-watch assertions: matched target/owner/
screen/cursor; context bytes unchanged by observation; changed metadata event
page after input; explicit timed_out status for an empty zero-wait poll;
same generation/lease/child after detach/reconnect; singleton refusal and
socket cleanup. No source/test edits or ignore-attribute removal were needed.
The standard suite still reports this fixture ignored; the explicit result
is separate evidence, not an extra pass added to its1088/15 count. Raw output:
`/private/tmp/panelink-p3-contract-pathname.log`. Human Apple Terminal,
provider/reboot and other ignored cases are unaffected. Historical failures
and earlier operator confirmation below remain preserved.

## Fixture contract

`pathname_server_cli_context_reuse` creates a unique fixture beneath
`/private/tmp` and sets the fixture root, runtime directory, and terminfo
directory to mode 0700. It compiles the checked-in `panelink-probe` entry with
`/usr/bin/tic` into the fixture. It then launches the actual Cargo-built
`panelink` binary through `CARGO_BIN_EXE_panelink`, using an explicit benign
environment, working directory, executable, and controlled shell loop.

The shell loop prints a ready marker with its PID and prints one numbered GOT
marker only when it receives the exact unique submitted token. Short-lived
`control` invocations use the same fixture context file. The assertions check
the returned five-field target and owner revision, stable generation/lease
across invocations, claim then submit outcomes, the same child PID and exactly
one GOT marker, explicit detach followed by Describe/Read on the same live
child, and a second server's refusal while preserving the original endpoint
device/inode. On a successful host run it checks directory/socket ownership
and modes, sends SIGTERM only to the spawned foreground server, waits within a
fixed deadline, and verifies listener endpoint cleanup.

Each child stdout and stderr reader drains concurrently, stores at most 16 KiB
per stream, and writes those bounded bytes to mode-0600 logs inside the private
fixture. Each child has a finite wait; the process guard signals only its own
PID, allows three seconds for graceful cleanup on unwind, then has a bounded
kill fallback. The local JSON reader handles only the fixture's top-level
machine-readable fields and JSON string escaping; it is not a general-purpose
JSON implementation. The fixture makes no host-terminal, provider, or
performance claim.

## Actual verification

Commands ran on Rust/Cargo 1.99.1, aarch64-apple-darwin, using the existing
offline Cargo cache and temporary target directory
`/private/tmp/panelink-p3-pathname-target`:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname --no-run
exit 0; integration-test executable built

CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname
exit 0; 0 passed, 0 failed, 1 ignored

CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo clippy --locked --offline --test p3_pathname -- -D warnings
exit 0; Finished dev profile

rustfmt --edition 2021 --check tests/p3_pathname.rs
awk 'length($0) && $0 ~ /[[:blank:]]$/ { print FILENAME ":" FNR ": trailing whitespace"; bad=1 } END { exit bad }' tests/p3_pathname.rs docs/progress/P3-PATHNAME.md
exit 0; no output
```

The ignored fixture was explicitly attempted:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --ignored --exact --nocapture
exit 101; 0 passed, 1 failed
```

The actual CLI returned `{"ok":false,"error":"listener_unavailable"}` before
announcing `running`. A single diagnostic call to `PrivateListener::bind` in
the same fixture directory returned raw OS error `EPERM` (1, Operation not
permitted). The test reported this as an expected sandbox blocker and failed;
it is not counted as a pass. The test compiled and reached the pathname gate,
but did not exercise control init, claim, submit, reconnect, second-server
refusal, successful SIGTERM shutdown, or endpoint cleanup. Those behaviors
remain pending a rerun in an environment that permits pathname Unix socket
bind/accept.

External rerun command:

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --ignored --exact --nocapture
```

No production source, dependency, manifest, lockfile, shared roadmap, or
unowned test was changed. This fixture does not mark Phase 3 complete.
Ownership for `tests/p3_pathname.rs` and this note is released for parent
review.

## Operator failure and parser correction — 2026-10-18

The operator ran the ignored test outside the sandbox. It passed server startup
and private socket checks, then failed at `init generation`. The fixture stored
command output as `stdout=<JSON> stderr=...`; `assert_ok_action` extracted its
JSON object, but subsequent numeric/null readers received the labeled capture
directly and failed before reading a field. This was a test-helper defect, not
evidence of a missing generation in the CLI response.

All typed field readers now share object extraction in `json_field`. A new
nonignored regression checks bare JSON and labeled capture with the full u64
generation range, lease and null uncertainty. Parent observed the regression
fail before the fix and pass afterward. The focused target passed1test with
1pathname test ignored; focused strict Clippy, formatting and diff checks pass.
The corrected full pathname scenario still requires an external rerun. No
later input/reconnect/cleanup acceptance is inferred from the operator's failed
run. Production Rust code is unchanged.

## Corrected external rerun confirmed — 2026-10-18

After the parser correction and external rerun command, the operator replied
"passed". Record the corrected pathname CLI scenario as operator-confirmed
passing: its assertions cover startup, context reuse, ownership/input, same-child
detach/reconnect, singleton refusal and endpoint cleanup. No raw rerun output,
new timing, version record or measured count was provided. Earlier sandbox
EPERM and the initial parser failure remain historical evidence. This does not
complete Phase2 host checks or the remaining Phase3 capabilities.

## Agent-captured pass on new machine — 2026-10-18

On the new development Mac (see latest INTEGRATION.md), the parent ran
`cargo test --locked --offline --test p3_pathname -- --ignored` directly:
`pathname_server_cli_context_reuse ... ok`, 1 passed in 0.68 s. Pathname bind
and accept are permitted in this environment, so EPERM no longer blocks this
fixture here. Remaining Phase 3 runtime capabilities are unaffected.
