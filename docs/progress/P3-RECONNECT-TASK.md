# Actual socketpair/private-PTY reconnect test — 2026-10-18

Exclusive next worker: tests/p3_socket_detach.rs,
docs/progress/P3-RECONNECT.md. No production edits, exports, Cargo, shared docs,
commits or push. Read ROADMAP/AGENTS/required docs and P3-STATE-TASK,
P3-IO-TASK/P3-PTY-TASK; prior private-PTY logical test owner edits other files.

Use actual ServerDriver<PtyEffects>, AuthenticatedStream from UnixStream::pair,
WorkerHub framing, StateOwner and persistent ContextFile. ServerDriver is
parent-owned src/server_runtime.rs: new(state), attach(serverauthstream),
tick(||monotonic_secs), state(), active_connections(), stop_connections(clock).
Parent exports all modules; request any missing integration, don't edit them.

Prove one server-owned real shell PTY survives both abrupt socket loss and an
explicit Detach, then same generation/client lease resume. Spawn once with
explicit argv/env/cwd and bounded ready marker `PID:$$`. A read-loop shell
prints an application GOT marker in response to Submit, not just tty echo.
Use full returned TargetRef and exact owner revision via Describe and Claim.
Submit a unique token under a durably reserved ContextFile key, write its frame,
then close client without reading reply. Keep server pumping. Reconnect another
actual socketpair, resume exact lease in schema2 Hello, query operation status
until terminal, reconcile exact uncertain context key (no automatic input
retransmission). New bounded Read must show the same PID and one GOT marker.
An explicit same-key/same-payload duplicate can be sent as a negative proof of
server dedup, and must produce saved status without a second GOT marker. This
deliberate regression action is not an automatic client retry policy.
Explicit Detach ACK [0,2,0,2], then another resumed socketpair/new Read still
shows same child/current screen and exactly one GOT marker. Verify child alive
through attachments and no lease issued anew for these reconnects.

Keep server owner pumping on the test thread while socket exchange runs in a
bounded scoped helper, or a guarded bounded test server thread. No unbounded
channel send/join; client frame deadlines<=2s, total scenarios finite deadlines;
no fixed sleeps used as sole success evidence. On errors Drop must shut down
own fixtures, no arbitrary process lookup/kill. Do not infer provider identity
or power-failure recovery from PID observations. No pathname listener is used,
so sandbox denial should not block this scoped test. This is not Apple Terminal
compatibility or multi-client UI acceptance. Meaningful scenario coverage rather
than mirrored implementation assertions. Record ACTUAL commands/output and scope
limits, report gotchas, release files. Parent independently reviews/reruns.
