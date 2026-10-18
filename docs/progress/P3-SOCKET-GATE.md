# macOS socket peer-identity source gate

Latest operator update: the corrected pathname CLI scenario is confirmed passing
outside the sandbox. See [P3-PATHNAME.md](P3-PATHNAME.md). Earlier EPERM attempts
below remain evidence of local restrictions, not a pending external CLI gate.

Current integration update 2026-10-18: the private listener, authenticated
nonblocking connect, bounded frame IO and one-child command frontend are now
implemented. See [P3-TRANSPORT.md](P3-TRANSPORT.md), [P3-NET-REVIEW.md](P3-NET-REVIEW.md)
and [P3-CLI.md](P3-CLI.md). Existing endpoints are refused rather than presumed
stale and removed. Actual authenticated socketpair reconnect passed, including
lost input acknowledgment and explicit detach with the same live child; see
[P3-RECONNECT.md](P3-RECONNECT.md). Filesystem pathname acceptance remains pending
because bind returns EPERM in this environment. The source-choice history below
predates this integration and is not a current claim that runtime is absent.

Earlier status, 2026-10-18: a stable native credential path is now reviewed and
implemented in `src/peer_identity.rs`; see [P3-PEER-REVIEW.md](P3-PEER-REVIEW.md).
Three focused tests pass for kernel socket-pair credentials, nonsocket failure
and numeric UID mismatch. This is not an operational listener. A pathname
socket fixture was attempted and failed at bind with EPERM in this sandbox;
it is explicitly ignored pending an external run, not counted as a pass.

## Actual local evidence — 2026-10-17

- Installed Rust/Cargo baseline remains 1.99.1 on aarch64-apple-darwin as recorded in integration evidence.
- Exact selected Rustix 1.1.5 source at `/private/tmp/panelink-p2-cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/rustix-1.1.5/src/net/sockopt.rs`, lines 1681–1694, exposes `socket_peercred` under `cfg(linux_kernel)`. It is not a macOS API. A source search found no `getpeereid`/`LOCAL_PEERCRED` wrapper in that selected crate's source. The current manifest enables std/fs/process/termios, not net; simply adding net would not make this Linux-only function available on macOS.
- The installed toolchain's rustdoc source, `share/doc/rust/html/src/std/os/unix/net/stream.rs.html`, source lines 258–272, marks `UnixStream::peer_cred` unstable under `peer_credentials_unix_socket`. Its adjacent Apple implementation uses platform credential calls, but this is not a stable public entry point on the tested toolchain.
- A disposable compile-only fixture in `/private/tmp/panelink-peer-api-sibtn12i/peer_api.rs` called `UnixStream::peer_cred` without feature gates. `rustc --edition 2021` exited 1 with E0658, unstable library feature `peer_credentials_unix_socket`. Compilation failed before any fixture binary ran; no socket was opened by this check.

The local documentation/source reads and compile failure are evidence about these exact versions, not a claim that no Rust library can provide this functionality. No internet-version assumption is needed for these observations. Cargo source caches are disposable; version/function/conditional and the recorded compiler error are the durable handoff.

## Reviewed choice and required next gate

The parent reviewed and selected an original borrowed-descriptor wrapper over
libc0.2.191's typed `getpeereid` binding. The new macOS/aarch64 direct edge reuses
the existing exact package and features; only Panelink's dependency edge changes
in Cargo.lock. Safe Rustix geteuid supplies the expected effective UID. The
wrapper propagates OS failure and rejects mismatching UID with PermissionDenied.
No client-supplied identity, PID inference or nightly is used. This closes the
native API/source-choice gate, not filesystem transport acceptance.

Then prove a private user-owned socket directory, safe bind/stale-path handling, correct permissions, actual kernel peer UID checks, and finite nonblocking/deadline behavior on Apple Silicon macOS before an operational listener. Same-user authentication still does not isolate agents from other processes running as that user. Broader agent grants/input ownership remain a separate contract.

Typed handshake/state-owner work can continue independently with fake peers/effects, but must not be advertised as authenticated socket or live process support. Persistent PTY adoption also retains its separately recorded Phase 2 host gate.

External focused command (not run successfully here):

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib peer_identity::tests::pathname_connection_reports_same_user_in_a_private_fixture -- --ignored --exact --nocapture
```

This fixture creates its own0700 directory and0600 socket, observes both peer
UIDs and removes only its fixture. It does not test stale endpoint takeover,
hostile directory replacement, another OS user, production accept deadlines or
server lifetime. Those remain explicit acceptance work for the transport task.
