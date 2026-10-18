# P3 transport spawn gate result — 2026-10-18

Implemented Step A of the spawn coordination contract. The crate-private
`spawn_gate` module provides blocking `acquire()` for synchronous creation and
`acquire_until(deadline)` for deadline-bounded paths. Poison fails closed with
the fixed `spawn_gate_poisoned` error. Deadline expiry returns
`spawn_gate_timeout`; the original deadline is checked before each retry and
after successful acquisition, with sleeps capped at 1 ms and the remaining
time.

`transport.rs` now gates only the descriptor-creation windows:

- `PrivateListener::bind`: one `UnixListener::bind` call, including std's
  CLOEXEC setup. Validation and post-bind pathname checks/chmod remain outside.
- `AuthenticatedStream::connect`: rustix socket creation through the first
  CLOEXEC flag set. Nonblocking setup, connect, peer checks, waits, and protocol
  I/O happen after release. If setup fails, owned-fd RAII closes the socket.
- `PrivateListener::accept`: one nonblocking std accept call. The guard drops
  before deadline handling, peer authentication, retry, or readiness wait.

The callback helper has no production hook beyond the normal gate APIs. Its
transport tests substitute a separate test-only gate, verify lock ownership
synchronously from inside the callback, and prove release on error and before
the accept `WouldBlock` wait and authentication path. Gate unit tests use an
isolated `Gate` for poisoning and channels to establish exclusion. No public
transport schema, capability, managed launch path, or dependency changed.

This protects only Panelink code that cooperates with the gate. After this
transport implementation, the parent integrated the macOS/aarch64
`TerminalSession::spawn` path with the same gate, holding it through PTY setup
and `command.spawn()` return; non-macOS targets retain their existing private
lock. The parent reports four focused production integration tests passed.
Arbitrary native-library or injected spawn calls remain outside the
binary-level guarantee. Singleton startup remains separate from future
worker-time API launch, which is not implemented.

## Verification

Successful final Cargo checks used
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home` with `--locked --offline`.

| Check | Result |
| --- | --- |
| `cargo test --lib spawn_gate::tests` | Exit 0; 4 passed, 0 failed. |
| `cargo test --lib transport::tests` | Exit 0; 22 passed, 0 failed, 1 ignored (23 tests selected). |
| `cargo fmt --check` | Exit 0. |
| `cargo clippy --all-targets -- -D warnings` | Exit 0. |
| `python3 scripts/check_docs.py` | Exit 0; 95 Markdown files, 0 findings. |
| `git diff --check` | Exit 0. |

The ignored pathname-listener fixture records the environment's prior single
`EPERM` result from `UnixListener::bind`; it was not retried in this run and is
not counted as passing evidence. The accepted-socket/managed-exec descriptor
race still needs a successful private-path host run with the deterministic
pre-accept barrier and child descriptor scan. Parent owns the integrated full
suite and source review after its terminal-gate adoption.

## Handoff

Parent owns the `lib.rs` module export, which is now present, and has completed
the macOS/aarch64 Step B replacement of `TerminalSession::spawn`'s private
lock. The accepted-socket race remains outstanding; API launch stays disabled
until parent review and successful descriptor-race evidence. This result note
records Step A verification; it does not mark phase 3 complete or reopen
managed launch.

Parent independently verified the final integrated suite (315 passed,
13 ignored), strict all-target Clippy, formatting, build and documentation
checks. Earlier default-CARGO_HOME attempts lacked the offline libc cache;
using the documented cache above resolved that environment issue. A complete
child descriptor scan and actual accepted-socket race fixture still need to
be implemented before the successful host proof can be obtained.
