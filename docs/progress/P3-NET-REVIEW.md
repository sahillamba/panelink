# Bounded client connect API review — 2026-10-18

Parent selects existing exact Rustix1.1.5 `net` feature only for macOS/aarch64,
to create a NONBLOCK/CLOEXEC Unix socket and connect without a blocking std
connect call. This is a feature expansion, not a new package/version/license or
an endorsement of TCP/remote access. Application code uses only AF_UNIX here.

Reviewed cached src/net/socket.rs `socket`, `connect`, `getpeername`,
src/net/sockopt.rs `socket_error` and `set_socket_nosigpipe`, and matching
src/backend/libc/net/syscalls.rs. Typed OwnedFd closes on every failure; no raw
ownership reconstruction or new application unsafe binding. On Apple the backend
does not define SocketFlags::NONBLOCK/CLOEXEC. An initial compile of that assumed
API failed E0599; the parent corrected it to safe existing fs::fcntl_getfl/setfl
and io::fcntl_getfd/setfd wrappers before connect/exposing the descriptor. This
is not an atomic CLOEXEC syscall guarantee on that platform. Prototype startup
spawns its only child before connection workers begin; API launch concurrency
must revisit descriptor inheritance before enabling further spawns.

Installed SDK connect.2 documents EINPROGRESS for incomplete nonblocking connect.
Implementation initiates once and on EINPROGRESS checks absolute deadline,
SO_ERROR and connected peer address with bounded1ms polling. Other errors fail
closed (including interrupted/queue-full connect); no retries of requests/input.
SO_NOSIGPIPE is explicitly enabled before converting OwnedFd to UnixStream,
then existing kernel same-user authentication occurs before handshake IO.

Rustix feature `net` additionally selects its already locked linux-raw-sys/net
feature on applicable Linux graphs; this target uses libc. No optional package,
build dependency, proc macro, allocator, runtime or build-script target is added.
The existing exact inventory/checksums/notices remain; previous advisory and
maintenance observations remain dated evidence. Parent must check resolved graph
after adoption and record real pathname connection as pending if sandbox bind
continues to fail. Socketpair tests do not establish pathname connect behavior.

Parent post-adoption `cargo tree --locked --offline -e features -i rustix`
exited0 and showed exact rustix1.1.5 with alloc/std, fs, net, process and termios;
net is a direct Panelink feature edge. Strict all-target Clippy and the combined
suite passed after adoption; see INTEGRATION.md for final counts. No new package
was selected. Pathname connection remains externally pending.
