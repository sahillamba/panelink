# P3 child descriptor inheritance result — 2026-10-18

The test-only `tests/support/fd_probe.rs` helper provides a reusable child
probe for the accepted-socket/exec race fixture. Its public test support API is
`PROBE_ENV`, `SUCCESS_EXIT`, `clean_value()`, `inherited_value(fd)`, and
`run_if_requested()`. With the probe environment variable absent, the helper
returns `false` and the caller's exact libtest helper test exits normally.
When configured, it scans and exits with code 42 only after all requested
checks pass; failures emit a fixed redacted marker and exit 43. Markers start
on a fresh PTY line so a long libtest test name cannot wrap across them.

## Inventory and checks

On macOS, the helper calls `proc_pidinfo(PROC_PIDLISTFDS)` into one fixed
4,096-entry buffer and validates the returned byte count against
`sizeof(proc_fdinfo)`. It fails closed on an error, malformed result, duplicate
or negative descriptor, or a completely filled buffer; that last case could
mean truncation. It does not use `RLIMIT_NOFILE` as the inventory bound. The
child compares consecutive inventories and retries up to three times when
they differ, then fails closed if it cannot obtain a stable pair. For each
reported descriptor, `fstat` confirms it is still open; `EBADF` alone is
treated as a closed slot. The reported open count is the number of successful
`fstat` checks in the stable scan.

The clean case requires every open descriptor above 2 to be absent. It also
requires descriptors 0, 1, and 2 to be TTYs in the child's session, with all
three `fstat` identities equal (device, inode, mode, and device number). The
positive-control case instead requires the inherited descriptor's opaque
`fstat` identity to occur somewhere in the child inventory, independent of
its descriptor number. Identity values and descriptor paths are not printed.

The Darwin SDK defines `PROC_PIDLISTFDS` as a list of `proc_fdinfo` records
and `PROC_PIDLISTFD_SIZE` as the record size in `sys/proc_info.h`; `libproc.h`
declares `proc_pidinfo` as available from macOS 10.5. The reviewed Apple XNU
`proc_pidfdlist` implementation holds the descriptor-table lock while it
copies the list and uses a null-buffer sizing query that can overestimate
capacity. This helper avoids that size query, uses its fixed cap, and treats a
full result as inconclusive. The reviewed source is the upstream
[`bsd/kern/proc_info.c`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c)
on `main`, inspected 2026-10-18; it is supporting source review, not a pinned
kernel revision or a release-floor claim.

The binding comes from the existing locked `libc` 0.2.191 source. In the
installed macOS 26.5 SDK, `libproc.tbd` maps `_proc_pidinfo` to the install name
`/usr/lib/system/libsystem_kernel.dylib`, which is re-exported through
`/usr/lib/libSystem.B.dylib`. The test executable's `otool -L` output lists
`libSystem.B.dylib` and `libiconv.2.dylib`; it has no separate `libproc`
runtime dependency. No package was installed and no Cargo dependency changed.

## Automated evidence

`tests/p3_fd_inheritance.rs` runs the exact helper test in a real
`TerminalSession` PTY using an explicit executable, environment, and temporary
working directory. Each child is pumped until PTY EOF and exit, with a
four-second deadline. The clean test holds a CLOEXEC file and socket pair
open in the parent through child exit. The positive-control test duplicates an
owned file to descriptor 64 or higher without `FD_CLOEXEC`, lowers only the
child's soft `RLIMIT_NOFILE` to 16 before scanning, and drops the parent's
duplicate immediately after spawn. A third case puts the same deliberately
inheritable descriptor through clean mode and requires exit 43 with the
`unexpected_descriptor` failure marker and no success marker. A test-local
mutex serializes these controlled fixtures so the intentional leak cannot
contaminate a concurrent clean child.

On this host, the final targeted run reported:

| Scenario | Result |
| --- | --- |
| Exact child helper without probe environment | Passed; normal libtest return. |
| CLOEXEC file and socket pair omitted by exec | Passed; child inventory count `0003`. |
| Expected inherited descriptor found above lowered child soft limit | Passed; child inventory count `0004`; descriptor number at least 64, child soft limit 16. |
| Deliberately leaked descriptor rejected in clean mode | Passed; exit 43 and `unexpected_descriptor`. |

The environment was macOS 26.5.2 (build 25F84), Darwin kernel 25.5.0,
arm64, with Xcode macOS SDK 26.5 (build 25F70) and rustc 1.99.1. This is
current-host automated evidence only. It does not establish behavior on the
selected minimum macOS release, which remains undecided. The scan is bounded
to the descriptor list returned by the current process API and the fixture's
stable observation window; it does not prove absence of descriptors injected
by arbitrary native code or of a hostile concurrent table mutation. Unknown
open descriptors in the controlled fixture fail the clean check rather than
being hidden by paths or file contents.

## Verification

Successful focused checks used
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home` and offline locked Cargo
resolution. The full integrated race test and final suite remain for the
parent to run after the sibling fixture is complete.
