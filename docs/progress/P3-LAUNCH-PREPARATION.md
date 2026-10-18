# Phase 3 launch preparation adapter — 2026-10-18

## Scope and result

Implemented the bounded adapter adopted in
[`P3-LAUNCH-PREPARATION-TASK.md`](P3-LAUNCH-PREPARATION-TASK.md). It prepares
an already decoded request and selected prototype target before the existing
coordinator's durable Intent, and its consuming `spawn` method delegates only
to the reviewed `TerminalSession::spawn` path. The module is exported only on
macOS/aarch64. It adds no endpoint, authority, dependency, account profile, or
daemon-environment input.

The adapter validates the five target identities and generation match,
canonicalizes the explicitly supplied absolute terminfo directory, copies
request paths, arguments, and environment with Unix byte-preserving
`OsString` conversion, and creates `LaunchContext` from the captured client
cwd/environment. It uses empty account defaults and no override layers, then
calls the existing environment builder and launch resolver. Only the owned
`TERM`, `TERMINFO`, `PANELINK_GENERATION`, and `PANELINK_SESSION` values are
added or replace caller values. The canonical executable, working-directory,
and terminfo paths are bounded to 4096 raw bytes. Errors and `Debug` output use
fixed text and carry no request data or operating-system error details.

The existing `p3_managed_launch` integration tests now use this adapter with
the coordinator-selected target and an explicit private fixture directory as
`TERMINFO`. The children are simple `/bin/sh` processes and do not load a
compiled terminfo entry; this fixture does not establish full-screen host
acceptance. The tests still cover exact-byte duplicate replay without
preparation or respawn, conflicting bytes, two live children, capacity
rejection before preparation, post-spawn journal uncertainty, fresh input
acknowledged by the same child PID, and cleanup after the coordinator drops.

## Automated checks

Using the locked offline cache at `/private/tmp/panelink-p2-cargo-home`:

- `cargo test --locked --offline --lib managed_launch_prepare::tests::` — 8
  passed.
- `cargo test --locked --offline --test p3_managed_launch` — 2 passed.
- `cargo clippy --locked --offline --all-targets -- -D warnings` — passed.
- `cargo fmt -- --check` — passed.

`cargo fmt --all -- --check` was also tried. It reports formatting differences
in `vendor/vt100`; no vendor files were changed. The package formatter check
above passes for the Panelink targets.

## Limits and blocked observation

An early test attempt to create a non-UTF-8-named filesystem fixture failed at
`create_dir` with `EPERM` in this sandbox. That attempt is recorded as blocked,
not as a passing path-resolution test. The replacement tests send a raw
non-UTF-8 path through the request codec and the real adapter/resolver, then
assert a fixed redacted failure for the missing path. They do not claim that
this sandbox accepts a non-UTF-8 filesystem entry. A separate passing fixture
proves actual preparation against private ASCII paths while argv and an
environment value retain non-UTF-8 bytes.

This adapter is a preparation prerequisite only. The parent still owns the
integrated source review and verifier; the host fixture does not demonstrate
compiled terminfo availability, launch-endpoint policy, or broader terminal
compatibility.
