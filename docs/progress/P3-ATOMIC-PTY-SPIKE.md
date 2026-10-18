# P3 Atomic PTY Opener Spike — 2026-10-18

**Result: bounded host prototype passes; not adopted by the runtime.** On the
tested Apple Silicon host, `posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC)` is
accepted and returns a master with `FD_CLOEXEC` set. Apple Libc's current
published source forwards the supplied flags to `open("/dev/ptmx", flags)`;
the installed macOS `open(2)` manual documents `O_CLOEXEC` as setting
`FD_CLOEXEC` as part of open. That source path plus the observed call result
supports the atomic-creation candidate on this host. The `fcntl` observation
and child test alone would not establish atomicity.

This remains integration-test-only evidence. It does not change the
`rustix-openpty` production path, establish behavior on the minimum supported
macOS version, or close the broader P3 spawn and descriptor gate. API and
worker-time launch remain disabled.

## Host and source evidence

The run host reported macOS 26.5.2, Darwin 25.5.0, arm64, SDK 26.5. The
installed SDK declares `posix_openpt(int)` in `_stdlib.h:242`, marks
`O_CLOEXEC` as implicitly setting `FD_CLOEXEC` in `sys/fcntl.h:153`, and
defines `TIOCPTYGNAME` as an ioctl with a 128-byte output size in
`sys/ttycom.h:165`. The local `open(2)` manual says the flag marks the new
descriptor close-on-exec; the local `posix_openpt(3)` manual lists `O_RDWR`
and `O_NOCTTY` as its documented flags and says other mode bits can produce
`EINVAL`. That older/manual contract makes runtime verification necessary.

The [Apple `posix_openpt(3)` manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/posix_openpt.3.html)
documents the portable `O_RDWR` and `O_NOCTTY` flags. The
[Apple Libc `grantpt.c` source](https://github.com/apple-oss-distributions/Libc/blob/main/stdlib/grantpt.c)
currently implements `posix_openpt` by forwarding its flags to
`open("/dev/ptmx", flags)`. Its implementation is published on the moving
`main` branch, not pinned to the tested OS build, so the source-to-host mapping
is strong but not a release-matched source audit. The
[Apple `open(2)` manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/open.2.html)
provides the `open` interface reference.

The lockfile resolves `libc` 0.2.191, `rustix` 1.1.5, and
`rustix-openpty` 0.2.0. In the cached rustix source, `OpenptFlags::CLOEXEC`
is unavailable on Apple targets; its `openpt` wrapper calls libc
`posix_openpt` there. `rustix-openpty` 0.2.0 calls `libc::openpty` on
non-Linux targets and sets close-on-exec afterward with `fcntl`, as its source
comments describe. This spike therefore calls the already-present libc
binding directly for the master and passes `O_CLOEXEC` to rustix's safe slave
`open` call. No manifest or dependency changes were made.

The spike avoids rustix's public `ptsname` API because its cached implementation
may grow a `Vec` on `ERANGE`. It uses the SDK-defined 128-byte
`TIOCPTYGNAME` output request in one documented ioctl wrapper and rejects a
missing NUL terminator. `grantpt` and `unlockpt` use small libc wrappers.
Descriptors are owned with `OwnedFd`; no PTY pathname or environment data is
printed. Unsafe code is confined to the libc creation, grant/unlock,
fixed-buffer ioctl, identity-inspection, duplicate-control, and ownership
wrappers in the test.

## What the integration test checks

[`tests/p3_atomic_pty.rs`](../../tests/p3_atomic_pty.rs) is gated to
`macos/aarch64`. It checks `FD_CLOEXEC` on the master immediately after
`posix_openpt`, launches a separate copy of the test executable before opening
the slave, then opens the slave with `O_CLOEXEC` and runs another exec check
while both descriptors are owned. The child checks the identity
`(st_dev, st_ino, st_rdev, st_mode)` at each original descriptor number. A
closed slot is accepted; a slot reused for a different file is not mistaken
for the PTY. The probe only checks those recorded descriptor slots; it does
not scan the child's entire descriptor table.

A positive control duplicates the owned master with `F_DUPFD`, verifies that
the duplicate lacks `FD_CLOEXEC`, and requires the exec child to see that
exact descriptor identity. The duplicate is dropped before the normal
master-only and master-plus-slave probes. A unique child exit code prevents an
`--exact` filter that matches zero tests from being mistaken for success.
Child execution is deadline-bounded and its guard kills and reaps the child on
timeout or polling error. PTY traffic is bidirectional, nonblocking, capped at
256 captured bytes, and bounded by one two-second deadline. The test also sets
the slave window size and reads it back through the master.

These runtime checks show that this host accepts the chosen flags, reports
the close-on-exec state, and does not preserve the tested PTY identities
through the exec checkpoints. Atomicity rests on the separate source and
`open(O_CLOEXEC)` evidence above; this test does not force a competing spawn
into the instruction-level interval around descriptor creation.

## Validation and limits

Focused validation on the host:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_atomic_pty
running 1 test
test atomic_pty_opener_has_cloexec_private_io_and_window_size ... ok
test result: ok. 1 passed; 0 failed

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --test p3_atomic_pty -- -D warnings
Finished `dev` profile

rustfmt --check --edition 2021 tests/p3_atomic_pty.rs
passed
```

The targeted formatter and whitespace/local-link checks passed. A repository
wide `cargo fmt --all -- --check` reports existing formatting differences in
`vendor/vt100`; those out-of-scope vendored files were left unchanged.

Parent independently verified the final focused test (1 passed, 0 ignored),
full locked/offline suite (303 passed, 13 ignored), strict all-target Clippy,
the project's normal `cargo fmt --check`, and `git diff --check`: all passed.
The normal formatter command is distinct from the broader `--all` invocation
above. Initial targeted Clippy reported two style errors, both corrected
before final verification. Parent reviewed the source and strengthened child
success detection, the inheritance control and child/control cleanup.

The test binary has no tests on other targets, so a zero-test result elsewhere
is not platform evidence. The project has not set a minimum macOS version; the
SDK declaration and this host run do not establish the availability or
semantics on that future floor. Recheck against the eventual floor and a
version-matched Apple implementation before production consideration.

Even if this opener is later adopted, it would address only PTY master and
slave creation. It does not repair the other macOS close-on-exec windows
recorded in [P3-SPAWN-DESCRIPTOR-GATE.md](P3-SPAWN-DESCRIPTOR-GATE.md),
including the standard-library status pipe and socket/accept paths. It does
not establish a process-wide exec coordinator or protect against unrelated
native spawn calls. The multi-session launch gate stays closed pending that
broader review and parent approval.

