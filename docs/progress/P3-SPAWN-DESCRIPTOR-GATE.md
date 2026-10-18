# P3 spawn and descriptor gate — 2026-10-18

**Verdict: NO-GO for API or worker-time launch.** The macOS PTY path opens
descriptors before setting close-on-exec, and the current mutex coordinates
only calls through `TerminalSession::spawn`. The audit also found non-atomic
close-on-exec windows in the installed Rust standard library's macOS socket,
accept, and pipe paths. No race test was run for this source-only review.

The current one-child bootstrap is narrower: source search found one
application child-spawn callsite, and startup completes it before accepting
connections or starting connection workers. Calls through
`TerminalSession::spawn` serialize with one another. This bounds observed
Panelink-controlled concurrency during bootstrap, but it does not establish a
general descriptor-inheritance guarantee against an independent Rust or native
process spawn. Do not treat the bootstrap as a pass for future concurrent
launch.

## Evidence reviewed

The required roadmap, integration checkpoint, development instructions,
README, canonical spec, discovery and prebuild notes, and managed-launch plan
were read before source review. The codebase-memory project
`Users-sahillamba-panelink` was ready; it was fully refreshed because the
application sources are untracked and a git-diff freshness check can miss
them (2,944 nodes and 11,702 edges after refresh). Graph search located
`TerminalSession::spawn`, startup, the state owner, and worker paths. Source searches and line-by-line source inspection supplied
the descriptor-level evidence below; graph results alone were not used to
establish behavior.

The resolved application graph is pinned by [Cargo.toml](../../Cargo.toml) and
[Cargo.lock](../../Cargo.lock). Relevant exact versions are `rustix-openpty`
0.2.0 (lock entry and checksum at Cargo.lock:84–93), `rustix` 1.1.5
(Cargo.lock:71–82), and `libc` 0.2.191 (Cargo.lock:40–44). The reviewed local
registry source was under
`/private/tmp/panelink-p2-cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/`.
The installed toolchain is rustc/cargo 1.99.1. The standard-library source
references below are from that installed toolchain's `share/doc/rust/html/src`
tree, not from a separate upstream revision.

`cargo tree --locked --offline -e features -i rustix` resolves `alloc`, `fs`,
`net`, `process`, `std`, and `termios`; it does not activate rustix's `runtime`
feature. Rustix 1.1.5 source contains optional low-level `fork`/`execve`
wrappers, but no Panelink callsite uses them and the runtime feature is not in
the selected application graph.

## Findings, ordered by gate severity

### High — macOS PTY descriptors are not close-on-exec at creation

Production [TerminalSession::spawn](../../src/terminal/mod.rs#L807) acquires
`SPAWN_LOCK` at lines 821–823, calls `openpty` at line 824, and does not return
from the wrapper until its close-on-exec setup has finished. The locked
`rustix-openpty` 0.2.0 source at `src/lib.rs:59–107` uses `libc::openpty` on
non-Linux targets, then calls `set_cloexec` separately for controller and user
descriptors. Its source comment explicitly calls this non-atomic. The helper at
`src/lib.rs:226–232` performs `F_GETFD` followed by `F_SETFD(FD_CLOEXEC)`.
Between `openpty` returning and each successful `fcntl`, those PTY descriptors
can be inherited by a concurrent fork/exec or native spawn. There is also an
interval after the controller flag is set but before the user descriptor flag
is set. The Linux branch uses a different atomic path and does not establish
macOS behavior.

The later clones are a separate case: `try_clone_to_owned` uses
`F_DUPFD_CLOEXEC` in the installed Rust standard library source
`std/os/fd/owned.rs:113–129`. `TerminalSession::spawn` uses those clones for
the controlling-terminal hook and the three `Stdio` streams (source lines
829–841). The child hook calls `setsid` and `TIOCSCTTY` before exec; the three
stream clones are deliberately mapped to descriptors 0, 1, and 2. This does
not close the earlier openpty window or prove that unrelated descriptors are
absent from the child.

### High — `SPAWN_LOCK` is a local PTY-spawn mutex, not a process-wide exec gate

`SPAWN_LOCK` is private to `src/terminal/mod.rs:40` and is held across openpty,
descriptor setup, `Command` construction, and `command.spawn()` (lines 821–855).
This prevents two concurrent `TerminalSession::spawn` calls from crossing
each other's openpty window. It does not cover any independent `Command`, raw
libc spawn, or native-library spawn.

The production source tree has one explicit child-spawn callsite:
`TerminalSession::spawn` at line 855. `StartedServer::start` invokes it once
for the fixed startup target at [server_startup.rs:36–96](../../src/server_startup.rs#L36).
`run_server_probe` binds the listener and registers signal flags before this
startup call, then begins accepting and attaching connections only after the
child has started ([p3_cli.rs:1064–1103](../../src/p3_cli.rs#L1064),
[p3_cli.rs:1130–1144](../../src/p3_cli.rs#L1130)). Production connection
workers use `thread::Builder::spawn` to run `run_worker`; they do not spawn
processes ([server_io.rs:211–243](../../src/server_io.rs#L211)). Other
`.spawn` occurrences in runtime tests and `p2_probe` are thread creation.

The production child installs a `pre_exec` closure. In the installed Rust
standard library, a nonempty closure makes the `posix_spawn` path return
`None` (`std/sys/process/unix/unix.rs:456–475`), so the current call uses the
fork/exec fallback (`:71–104`). The fallback creates its child-status pipe
before forking. That pipe itself is also subject to the macOS post-hoc
close-on-exec behavior described below. The existence of a mutex around
Panelink's PTY call does not serialize a process spawn performed outside that
mutex. No raw `fork`, `posix_spawn`, or `execve` call was found in application,
test, or vendored Rust sources; native code outside those sources remains
outside this source search's guarantee.

### High — macOS standard-library sockets, accepted sockets, and pipes have post-hoc flags

The installed Rust 1.99.1 source shows these macOS paths:

| Descriptor source | Source behavior | Panelink use and relevance |
| --- | --- | --- |
| `std::process::Command` status pipe | `std/sys/pipe/unix.rs:8–43` uses `pipe()` and then sets close-on-exec on each end on targets without `pipe2`; macOS is in this fallback. | The current `pre_exec` child spawn uses this pipe. `SPAWN_LOCK` covers this pipe only for calls through `TerminalSession::spawn`; an independent spawn can overlap it. |
| `UnixListener::bind` socket | `std/sys/net/connection/socket/unix.rs:95–104` uses `socket()` then `set_cloexec()` on macOS. | [PrivateListener::bind](../../src/transport.rs#L280) uses `UnixListener::bind`. It is created before the current startup child, so the observed bootstrap has no Panelink worker racing it. |
| `UnixListener::accept` result | The standard-library fallback at `std/sys/net/connection/socket/unix.rs:243–273` uses `accept()` then `set_cloexec()`; macOS does not use the listed `accept4` branch. | [PrivateListener::accept](../../src/transport.rs#L327) runs after bootstrap. A future launch routed through the same owner loop can avoid overlap with this call by sequencing it; an independent spawn is not covered. |
| Rustix client socket | `rustix::net::socket` creates a plain socket; its API documents `socket_with` for flags (`rustix-1.1.5/src/net/socket.rs:11–50`). Panelink then sets `FD_CLOEXEC` with `fcntl` (`transport.rs:120–129`). | This is the CLI connection path, normally in a separate client process. It is another non-atomic window if reused inside a process that can spawn children concurrently. |
| `UnixStream::pair` | The standard library uses `socketpair()` followed by `set_cloexec()` for both descriptors on macOS (`std/sys/net/connection/socket/unix.rs:112–144`). | The application occurrences are test socketpairs; not a production server descriptor source. Tests may run spawn helpers concurrently. |

The standard-library fork fallback and these socket paths rely on descriptor
flags to close extra descriptors at exec; the reviewed process-spawn path does
not close the full descriptor table. A child crossing one of these creation
windows can therefore retain the still-unflagged descriptor through exec. The
standard-library behavior is version-specific evidence from the installed
toolchain and must be rechecked if the supported Rust toolchain changes.

### Lower risk in the current path — explicit file opens and descriptor clones set the flag at creation

The persistent storage and client-context paths pass `OFlags::CLOEXEC` on
their `open`/`openat` calls. Server storage's directory descriptor uses it at
[server_storage.rs:65–69](../../src/server_storage.rs#L65), and file opens use
`FILE_FLAGS` with it at lines 21–24 and 81–95. The long-lived `server.lock`
uses `flock` on that already-open descriptor (lines 186–214); it is not a new
descriptor-creation path. Client context files and parent directories also
include `CLOEXEC` in their open flags
([client_context.rs:206–217](../../src/client_context.rs#L206),
[client_context.rs:266–274](../../src/client_context.rs#L266),
[client_context.rs:448–456](../../src/client_context.rs#L448)).

The `/dev/tty` descriptors in [p2_probe.rs](../../src/p2_probe.rs#L137) use
`OFlags::CLOEXEC` at open (lines 138–149 and 199–210). `File::open` uses
`O_CLOEXEC` in the installed standard-library source (`std/sys/fs/unix.rs:1227–1243`),
including the storage seed and root directory opens. Rust's owned-fd clone
uses `F_DUPFD_CLOEXEC` as noted above. These reviewed paths do not have the
open-then-fcntl interval found in openpty, sockets, accept, and pipes.

One transient file-descriptor source is worth keeping distinct: server
initialization calls `std::fs::read_dir(path)?.next()` at
[server_storage.rs:178–185](../../src/server_storage.rs#L178). The installed
standard library routes this through `opendir` (`std/sys/fs/unix.rs:1847`);
the directory iterator is dropped before listener binding and child startup.
The reviewed Rust callsite does not show the underlying libc descriptor flags,
so no general foreign-spawn claim is made for that internal implementation.
Likewise, path canonicalization/metadata helpers run during launch preflight,
before the child spawn; their libc internals were not treated as proof against
an arbitrary concurrent native exec.

No application-created anonymous pipe or TCP listener was found. `mpsc` and
bounded queues are in-memory synchronization; they do not create file
descriptors. The selected `signal-hook::flag::register` path sets atomic flags
and its registry stores callbacks (`signal-hook-0.4.5/src/flag.rs:153–159`);
it does not create the optional self-pipe used by other signal-hook APIs.
Terminal parser and decoder dependencies do not add process-spawn paths in the
reviewed call tree.

## Process-spawn inventory

The repository-wide Rust-source search found these process paths:

| Path | Classification |
| --- | --- |
| [terminal/mod.rs:834–855](../../src/terminal/mod.rs#L834) | Sole production `Command::spawn`; called by explicit single-child startup and the one-shot probe. Protected by `SPAWN_LOCK`. |
| [client_context.rs:791–799](../../src/client_context.rs#L791) | `Command::output` self-child helper inside `#[cfg(test)]`; test-only. |
| [tests/p2_host.rs:57–68](../../tests/p2_host.rs#L57), [146–172](../../tests/p2_host.rs#L146), [641–671](../../tests/p2_host.rs#L641), [928–942](../../tests/p2_host.rs#L928) | Test subprocesses: terminfo compiler, outer supervisor, PTY fixture child, and CLI preflight. They run in the test harness, not the server process. |
| [tests/p3_pathname.rs:123–147](../../tests/p3_pathname.rs#L123) | Test child helper with piped stdout/stderr; its callers cover the test CLI, server, and terminfo fixtures. |
| [server_io.rs:229–234](../../src/server_io.rs#L229) | `thread::Builder::spawn`, not a process exec. `server_runtime` and `p2_probe` `.spawn` callsites are also scoped/thread workers or test helpers. |

The other application-source `openpty` hits are test-only fixtures in
`p2_probe.rs:1112, 1151, 1293, 1521` and `tests/p2_host.rs:127, 642`. They use
the same macOS wrapper and can overlap test subprocesses; they are not
additional production launch entry points.

The lockfile-resolved runtime graph contains the direct and transitive
dependencies shown by `cargo tree --locked --offline`: local `crossterm-input`
and `vt100`, `rustix-openpty`, `rustix`, `libc`, `signal-hook`,
`signal-hook-registry`, `unicode-width`, `vte`, `arrayvec`, `memchr`, `itoa`,
`bitflags`, and `errno`. No additional child-spawn call was found in these
selected sources. This is a source inventory, not a promise about future
dependencies, dynamically loaded code, or arbitrary native callers.

## Unsupported claims and assumptions

- “The spawn mutex closes the macOS inheritance race” is unsupported. It
  serializes only `TerminalSession::spawn` calls, not every possible exec path.
- “There is no inherited-descriptor risk because the process has one child” is
  unsupported as a general statement. One-child startup has no second
  Panelink-controlled process spawn in the inspected path, but no guarantee
  was established against concurrent foreign/native spawns.
- “All accepted sockets and standard-library pipes are atomically CLOEXEC” is
  false for the installed macOS standard-library paths reviewed here.
- The audit assumes the current `macOS/aarch64` target and installed
  rustc/cargo 1.99.1. The minimum supported macOS version is still unset.
  Availability and semantics of any replacement PTY or spawn primitive must
  be checked against the selected minimum OS before implementation.
- No managed-launch API, worker-time launch, multi-session registry, cursor
  schema, or capability is adopted by this report. No descriptor race test or
  host acceptance test is reported as passing.

## Smallest next implementation gate

Keep worker-time launch disabled. The next bounded implementation should be a
descriptor-only macOS PTY opener spike, separate from API or registry work:

1. Select and source-review a macOS PTY creation path that sets close-on-exec
   atomically for both the controller and user descriptors. One candidate to
   verify is a `posix_openpt`/slave-open sequence with `O_CLOEXEC`; this report
   does not claim Apple availability or suitability for the current minimum OS.
2. Keep all Panelink process effects on one owner path and require every
   Panelink child-exec path to use the same coordinator. Worker threads must
   continue to forward requests only.
3. Decide whether foreign/native concurrent spawn is in scope. A Panelink mutex
   cannot fence a library that does not use it. If it is in scope, the design
   must address the standard-library error pipe and socket/accept creation
   windows too; replacing openpty alone is not sufficient evidence.
4. Add deterministic regression coverage before enabling launch: hold a newly
   opened PTY descriptor at the pre-CLOEXEC boundary, start an unrelated child,
   and have that child inspect the inherited descriptor set; run two concurrent
   managed PTY spawns; and verify the child receives only the intended PTY on
   descriptors 0/1/2. A private PTY and child process are sufficient for these
   checks without `/dev/tty`. Add an analogous socketpair/accepted-socket
   boundary test if those paths may overlap launch. Use an Apple Silicon host
   run to validate the chosen system calls and minimum-OS behavior; terminal UI
   acceptance remains a separate host-only check.

Only after that bounded gate has source-backed behavior and regression evidence
should the parent resume the managed-launch API sequence in
[P3-MANAGED-LAUNCH-PLAN.md](P3-MANAGED-LAUNCH-PLAN.md#L30). The plan's
global ledger, identity, cursor, capability, and storage decisions remain
proposals for parent review.

## Ownership and verification

This audit owns only this report. No source, Cargo, roadmap, integration, or
shared planning file was changed, and no commit or push was made. Report
ownership is released to the parent for review; the API-launch gate remains
closed pending the bounded descriptor work above.
