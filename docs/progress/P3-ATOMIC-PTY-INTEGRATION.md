# P3 Atomic PTY Opener Production Integration — 2026-10-18

**Result: integrated and verified on the current Apple Silicon host; release OS
floor remains unknown.** The production `TerminalSession::spawn` path now uses
creation-time close-on-exec for its PTY master and slave on `macos/aarch64`.
This changes PTY pair creation only. It does not enable API or worker-time
launch, and it does not set or verify a minimum supported macOS version.

## Production path

The private `terminal::pty` module is compiled only for `macos/aarch64`. It
opens the master with `posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC)`, then calls
`grantpt` and `unlockpt`. It obtains the slave path through a fixed 128-byte
`TIOCPTYGNAME` buffer and rejects empty or unterminated names before building a
`CString`. The slave uses rustix `fs::open` with `RDWR | NOCTTY | CLOEXEC`.
Both descriptors are held by `OwnedFd`, so failures in the later steps close
the descriptors. Unsupported creation flags fail the operation; there is no
fallback to a non-atomic PTY opener.

The requested window size is applied to the slave before the pair reaches the
spawn path. The helper leaves termios untouched, matching the previous
`openpty(None, Some(winsize))` behavior. Other Unix targets retain the previous
`rustix-openpty` opener through the common private `PtyPair` helper. No public
API, Cargo dependency, or feature was added. At this handoff, the existing
`SPAWN_LOCK` scope and child `setsid`/`TIOCSCTTY` setup remain in place.

## Production-path tests

The macOS/aarch64 unit tests call the new opener directly and verify
`FD_CLOEXEC` on both returned descriptors, the requested size from both PTY
ends, and the default canonical/echo line discipline. A child launched through
the existing `TerminalSession::spawn` confirms that descriptors 0, 1, and 2
refer to the same PTY and that the initial size is visible through descriptor
0.

A second self-exec test starts the current test binary through
`TerminalSession::spawn`, selects exactly one helper test, and passes an
explicit environment marker. The helper checks `isatty(0)`, the child's
session and process-group identity, `tcgetsid(0)`, and `tcgetpgrp(0)`. It emits
its success marker only when the PTY is the child's controlling terminal. This
check uses the inherited descriptor directly and does not access the host's
`/dev/tty` path.

The concurrent-spawn test releases two production spawn calls together, waits
for complete identity markers under a deadline, sends distinct input to each
child, checks that outputs do not cross, and shuts down both sessions. It
checks isolation and cleanup for these two sessions; it is not a complete
descriptor-table inheritance test.

An early shell-test revision checked `/dev/tty` directly and exited before
producing output because access was denied in this host environment. That test
was replaced with descriptor-based stdio and size checks plus the self-exec
controlling-terminal probe above. The final focused test run passed.

## Host evidence and validation

The tests ran on macOS 26.5.2, Darwin 25.5.0, arm64, with rustc 1.99.1. On this
host, all production opener and child integration tests passed. This result is
specific to the tested host. It does not establish behavior on an eventual
minimum macOS version; the release floor remains unset and needs its own
version-matched validation.

Final checks after the source changes:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib terminal
test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 219 filtered out

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p2_pty
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
Finished `dev` profile

cargo fmt --check
passed after the terminal integration changes and before the separate spawn-gate edits.
The latest workspace-wide run reports formatting differences in `src/transport.rs`;
that file is outside this integration's ownership. `rustfmt --check --edition
2021 src/terminal/mod.rs src/terminal/pty.rs` passes for the owned source files.

python3 scripts/check_docs.py
Checked 94 Markdown file(s); 0 finding(s).

git diff --check
passed
```

The independent [`P3 atomic PTY opener spike`](P3-ATOMIC-PTY-SPIKE.md) remains
useful source and host evidence, but it does not substitute for these
production-path tests. This integration covers only PTY pair creation. The
macOS standard-library status pipe, socket, accepted-socket, and other
descriptor paths remain outside its scope; it does not provide a process-wide
exec coordinator or protect against unrelated native spawns. See the
[`P3 spawn and descriptor gate`](P3-SPAWN-DESCRIPTOR-GATE.md). API and
worker-time launch remain disabled pending the broader gate review.

Parent integration checkpoint: the macOS/aarch64 spawn path now uses the
shared cooperative gate instead of its old private SPAWN_LOCK. It releases
the guard immediately after command.spawn returns. Parent independently ran
the final suite (315 passed, 13 ignored), strict all-target Clippy, normal
cargo fmt --check, build and documentation checks: all passed. The transient
transport formatting differences above were resolved by that file's owner.
