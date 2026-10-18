# Bounded authenticated connection workers — 2026-10-18

Exclusive next owner: src/server_io.rs, docs/progress/P3-IO.md. Parent owns
exports/shared docs/main/runtime loop; other owner writes src/server.rs using
P3-STATE-TASK.md and another writes session_protocol.rs under P3-SESSION-TASK.md.
Read those tasks/AGENTS/ROADMAP/prerequisite product docs first. Do not edit
transport.rs after this task begins; parent reviews it. No new dependencies,
commits/push. macOS/aarch64 only. No process IO or policy in worker.

Implement a bounded worker hub for already authenticated AuthenticatedStream.
MAX_CONNECTIONS=8, one worker per admitted connection, one complete incoming
control frame at a time per worker, bounded hub mpsc sync_channel capacity8;
per-envelope reply sync_channel1. No reader/writer clones; the one worker is the
only socket writer and does not touch effects/state/PTY. No blocking send to hub:
try_send or reject/drop connection if full. No unbounded thread or vector growth.
Use absolute socket deadlines (read1second, write250ms), response wait2seconds
with stop checks at <=20ms intervals. Connection read/send timeouts close it;
never replay an input or operation because of EOF/timeout/lost response.

Public names may be idiomatic, but expose bounded hub receiver and submit/start
methods sufficient for parent event loop. Hub holds stop Arc<AtomicBool>, <=8
JoinHandles and shared active count; only parent/hub mutates its worker list.
Reap only finished joins during normal loop. Drop sets stop and joins bounded
workers; no self-join, no indefinite channel recv/send. Stop checks around
handshake/request loops; deadlines cap queued read/write despite stop. If a hub
message is already accepted, connection loss does not cancel its process effect;
the parent StateOwner must still finish admitted work, even if reply receiver
has gone. Drop/stop never signals or drops server's effect sink/PTY.

Public owner-message request types:
- Hello(SessionHello), no preexisting connection token.
- Request { connection: server::Connection, payload:Vec<u8> }.
Envelope has request and a reply SyncSender. Reply types:
- Welcome { connection: server::Connection, welcome:handshake_protocol::Welcome }
- Response(Vec<u8>)
- Error(&'static str) fixed labels only
- Detached (emit fixed4byte Response payload schema2/kind2 = [0,2,0,2], then close).
Payload Debug must redact. Parent calls StateOwner::connect/handle_control/
handle_session on the one state-owner thread and sends these replies. Worker
keeps the Connection returned by parent; never constructs one from wire fields.

Wire state machine: first frame must Hello/requestID0, decode_session_hello;
auth is already guaranteed by AuthenticatedStream constructor before this read.
Send Hello to owner, require Welcome reply, encode via existing welcome codec
and emit Welcome/requestID0. Later only Request frames, requestID>0 strictly
increasing within the connection, no repeated/second Hello, Response/Error from
client, Snapshot/Event. Each uses one envelope/reply before next request read.
Reply Response body<=65536, fixed Error labels<=4096. Unexpected reply role,
invalid codec/frame, wrong ID or malformed request schema closes connection;
parent validates request contents before dispatch. No lease issuance here.

Current transport has no bounded client connect function; don't add a blocking
UnixStream::connect to public library. Parent may separately review enabling
existing Rustix net primitives for client connect. Tests may use UnixStream::pair.

Tests with fake owner/actual socketpair: authentication precedes Hello reads;
valid Hello/Welcome/Request/Response roundtrip; request before hello/secondhello/
zero or duplicate IDs/oversized roles rejected; disconnect waitingreply releases
worker but does not cause automatic repeated envelope; malformed Error doesn't
leak content; ninth connection refused before spawning; worker channel bounded;
shutdown joins under documented bounds. Parent later adds StateOwner+realPTY
detach/reconnect test, not AppleTerminal/pathname compatibility. Avoid tests that
assume OS thread scheduling at exact milliseconds. Actual outputs only.

Use temp Cargo harness importing dependent files, or wait for stable APIs when
needed while writing independent worker logic/tests. Do not change root exports
or run root rustfmt (owned file only). Report outputs/deviations/gotchas and
release ownership. No whole Phase3 completion claims.
