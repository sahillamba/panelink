# macOS peer credentials review — 2026-10-18

Parent decision: adopt direct `libc = { version = "=0.2.191",
default-features = false }` only on macOS/aarch64 for a small independently
written credential wrapper. This reuses an existing reviewed graph component;
no competitor implementation is imported. Std sockets remain the transport
candidate; interprocess/serde and nightly are not required for this call.

## API, ownership and native boundary

Installed Command Line Tools SDK `usr/include/unistd.h:673` declares
`getpeereid(int, uid_t *, gid_t *)`. Its `usr/share/man/man3/getpeereid.3`
describes effective UID/GID associated with connection establishment, success0
and failure-1 with errno. This authenticates connection credentials, not a PID,
provider identity or current credentials after privilege changes. Local source
is primary evidence for this installed SDK, not minimum-macOS certification.

Cached libc0.2.191 `src/unix/bsd/mod.rs:627` supplies the matching typed binding.
The wrapper takes `&UnixStream`; a private helper accepts `BorrowedFd` to permit
a real nonsocket error test. No ownership transfer, raw descriptor reconstruction
or close occurs. Initialized uid_t/gid_t stack outputs are read only on success.
The one unsafe call passes valid disjoint output pointers and a live borrowed
descriptor. Any OS error is captured immediately; no retry loop or socket read
is involved. Safe existing Rustix `process::geteuid` supplies the expected UID.
UID mismatch fails with PermissionDenied and a fixed message. GID is observation
only, never an alternate authorization path. No client-asserted identity is used.

## Exact graph, features, license and build impact

libc0.2.191 is already locked, checksum
`65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a`.
Existing Rustix, openpty, errno and signal crates already select it. The baseline
`cargo tree --locked --offline -e features -i libc` shows default/std enabled by
existing dependencies. New direct edge disables defaults; no feature is added.
Its manifest has only optional rustc-std-workspace-core (not selected), no build
dependencies. MIT option remains selected with preserved MIT/Apache texts under
`third_party/licenses/libc-0.2.191/`. Previous inventory/maintenance/advisory
observations remain dated evidence, not a fresh advisory audit.

Parent read its existing build.rs: target/compiler feature detection, rustc
version invocation and cfg output; platform-specific external tool probes are
for FreeBSD/Emscripten/VxWorks, not this target. No new build target, C compiler,
allocator, runtime, telemetry or package is introduced by this direct edge.
The saved effort is using maintained ABI definitions instead of handwritten
extern declarations or adopting an entire socket library for one native call.
Selected graph/features and tests must be rechecked after integration.

## Scope and remaining gates

This decision authorizes only peer credential observation and same-effective-UID
checking. Same-user processes can impersonate each other's application markers;
this is not per-agent isolation. Directory/path security, listener lifecycle,
deadlines, grants, handshake state machine and process dispatch remain separate.
Tests of the current user and injected numeric UID mismatch cannot establish
cross-user denial against a second actual OS user without such a fixture.

## Actual focused verification

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --lib
peer_identity -- --nocapture` initially passed3tests. Cargo updated only the
Panelink dependency list in the existing lockfile to add its direct libc edge.
The subsequent `--locked --offline` focused run attempted a fourth pathname
fixture: kernel socket-pair lookup, real ENOTSOCK propagation/file retention and
numeric mismatch passed; pathname bind failed with OS1/PermissionDenied/EPERM.
That case is now explicitly ignored with the sandbox reason; there is no silent
skip or pathname authentication pass claim. See P3-SOCKET-GATE.md for its exact
external rerun command and outstanding production transport gates.

Post-adoption `cargo tree --locked --offline -e features -i libc` confirms the
same0.2.191 and the same default/std feature set, with only the new Panelink edge.
Notices already cover this package; no new license files are needed.
