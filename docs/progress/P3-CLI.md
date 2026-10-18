# P3 experimental server/control CLI

Date: 2026-10-18. Scope: `src/p3_cli.rs` and this evidence note. The CLI is
integrated as a binary child module and runs one foreground server with one
child plus short-lived control clients. It is experimental; this slice does
not claim Phase 3 completion or external pathname socket acceptance.

## Implemented

`run(Vec<OsString>)` strictly parses the `server-probe` and `control` forms,
requires absolute paths and explicit five-field numeric targets, bounds the
captured command to 268 arguments/65,536 bytes and text to the typed input
limit, and returns machine-readable JSON with fixed error labels. The parent
also wires `--help`/`-h` after either subcommand. A `TARGET` is always the
complete `generation:workspace:pane:session:occupant_revision` tuple. Owner
mutations require an explicit `OWNER_REV` obtained from `describe`; the CLI
never selects a target by name or resolves one through an implicit Describe.
It does not resolve pane names, run Describe implicitly, print raw errors,
frames, paths, terminal input, launch arguments, or environment values. JSON
strings escape terminal control scalars, quotes, backslashes, and Unicode line
separators.

`control init` refuses an existing context before contacting the server and
persists the newly issued generation/lease through `ContextFile::create`.
Other commands open and lock the existing context, resume its exact identity,
and validate the Welcome nonce, capabilities, generation, and lease. `show` is
local-only. `describe`, `status`, and `resync` are observations; resync queries
only the uncertain operation key. Read/input/ownership mutations encode and
validate the full typed request before persisting a reservation, send once,
and acknowledge only a validated terminal response or explicit rejection.
Transport/protocol mismatch leaves the reservation uncertain. InProgress
remains uncertain. No input is replayed, and no unreceipted operation is
silently retired; the owner exposes explicit receipt support and retains its
1024 unreceipted-result per-lease limit. The explicit `receipt THROUGH_SEQUENCE` command now exposes server retirement with uncertainty/range checks before IO and unchanged local metadata; see [P3-RECEIPT.md](P3-RECEIPT.md). Claim/Handoff TTL is 30 seconds. Screen
Read is requested and response-validated at no more than 4,096 bytes and 24
lines. The output includes the full target and current screen revision.

`server-probe` prepares launch state through the parent helper, uses the shared
transport directory validation, opens storage before binding, binds before
spawning, registers flag-only stop signals before child creation, checks the
locked storage identity before each driver tick, and serves in the foreground
with a one-millisecond bounded accept slice. It stops connection workers and
drops the started server on explicit stop. Input capability is opt-in; the
frontend requests Read and optionally Input/Events for new contexts; reconcile/watch require Read|Events, with no resume capability escalation.

The frontend now supports finite metadata watches and atomic reconciliation.
It does not implement indefinite subscriptions, pane layout, multiple
children/sessions, or provider identity/recovery. The currently integrated
suite totals and aggregate failures/ignored scenarios are kept in
[INTEGRATION.md](INTEGRATION.md); the focused counts below are only this
module's checks.

Parent integration subsequently wired subcommand help, the shared transport
directory validator, the 268-argument top-level bound, and response-side Read
size/line validation. The focused results below record the worker's run before
those integration refinements; use `INTEGRATION.md` for the current root-level
verification.

The parent-directed storage refinement adds `--initialize` next to
`--allow-input`. The first deliberate startup in a new empty dedicated
runtime directory must use it; normal startup uses open-only storage and never
recreates missing identity files. This prevents an old directory from being
silently treated as a fresh identity domain. The CLI does not unlink stale
endpoint paths.

## Actual verification

Commands ran from the repository on the installed Apple Silicon macOS target,
using the existing offline Cargo cache and a temporary target directory:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-cli-target cargo test --locked --offline --bin panelink p3_cli::tests -- --test-threads=1
exit 0; 6 passed, 0 failed, 0 ignored; 21 filtered out

CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-cli-target cargo clippy --locked --offline --all-targets -- -D warnings
exit 0; Finished dev profile

rustfmt --edition 2021 --check src/p3_cli.rs
git diff --check -- src/p3_cli.rs docs/progress/P3-CLI.md
exit 0; no output
```

The focused socketpair checks cover valid handshake and typed Describe exchange,
required Hello/Welcome roles, resumed-generation mismatch, and a mismatched
response request ID. Parser tests cover exact probe/control forms, explicit
target shape, invalid numbers, extra arguments, duplicate flags, and absolute
paths. JSON escaping checks quotes, backslashes, LF, ESC, C1 controls, and
U+2028.

A real pathname listener/client CLI run is not claimed. The existing
[P3-SOCKET-GATE.md](P3-SOCKET-GATE.md) records a pathname bind attempt denied by
the sandbox with EPERM and an ignored pending fixture; external Apple Silicon
macOS bind/accept/cleanup verification remains necessary. This worker did not
repeat that pathname test.

No dependency, manifest, lockfile, library export, `main.rs`, startup, transport,
or shared roadmap file was changed by this worker. Parent root `cargo check`
was reported passing during integration; it is not counted here as a command
run by this worker. File ownership for `src/p3_cli.rs` and this evidence note is
released for parent review.
