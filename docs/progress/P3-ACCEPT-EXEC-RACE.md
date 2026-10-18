# P3 accepted-socket exec race — 2026-10-18

## Fixture and scope

The transport test accepted_socket_exec_race_excludes_accepted_descriptor
uses a real PrivateListener::bind pathname, a connected UnixStream client,
and the listener's actual UnixListener::accept syscall. A test-only channel
barrier runs inside with_spawn_gate_until, after the production shared gate
has been acquired and immediately before that syscall. While the barrier is
held, the fixture:

- starts a managed TerminalSession::spawn contender and waits for its
  start-channel acknowledgment;
- calls the actual crate::spawn_gate::acquire_until with a short deadline
  from a second thread and requires TimedOut;
- requires that the managed spawn has not returned before the barrier opens.

After release, the accept thread returns the accepted stream to the test and
the managed child runs the current library test executable with an explicit
environment and the private fixture directory as its working directory. The
child uses the shared bounded Darwin descriptor scanner in clean mode. It must
exit with the scanner's success status and marker, which means its descriptor
table contained only the three PTY stdio descriptors and those descriptors
shared one controlling terminal session. The listener, client, and accepted
stream stay open in the parent until after the child exits.

The barrier is a test-only closure around the real production gate helper and
the listener's real accept() call. The test does not call public
PrivateListener::accept, because that method has no barrier seam; its
existing tests continue to cover authentication and deadline behavior. This
fixture isolates the accepted-socket CLOEXEC race and does not add a production
hook, API, dependency, or launch capability. The scanner's separate
inheritable-descriptor positive control is owned by the descriptor-probe test
work.

The fixture and gate probe use bounded channel waits. An RAII release guard
opens the accept barrier on assertion failure, and the scoped threads are
joined before the private test directory is removed. Run the fixture with one
test thread because other library subprocess tests do not all coordinate
through the spawn gate.

## Verification evidence

The library test target compiled offline with exit 0 using:

~~~sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib --no-run
~~~

The focused non-ignored transport tests passed serially: 23 passed, 0 failed,
and 2 pathname tests were ignored. This verifies the existing transport tests
and the child helper's parent-mode behavior; it does not run the ignored race
fixture.

The first and only actual attempt in this environment used the new fixture and
the serial exact filter:

~~~sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib transport::tests::accepted_socket_exec_race_excludes_accepted_descriptor -- --exact --test-threads=1 --nocapture
~~~

It exited 101 at PrivateListener::bind: macOS returned EPERM
(PermissionDenied, “Operation not permitted”). The failure occurred before
the accept thread, barrier, gate contention probe, or child exec started.
Therefore this attempt establishes no race, CLOEXEC, or child-scan pass.
The fixture is ignored in ordinary test runs because pathname binding is
restricted in this environment.

Run the ignored test in an automated macOS arm64 environment that permits
private pathname sockets with:

~~~sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib transport::tests::accepted_socket_exec_race_excludes_accepted_descriptor -- --ignored --exact --test-threads=1 --nocapture
~~~

A successful run must report one passing test and the child's clean-scan
marker. The accepted-socket/managed-exec race remains unresolved until that
run succeeds. The cooperative gate covers Panelink paths that use it; this
fixture makes no inheritance guarantee for unrelated native or injected
process-spawn callers.
