# Phase 4 private recovery endpoint

Status: this slice implements bounded private IPC between an explicit UI/CLI
requester and the managed-session owner. It carries user intent and fixed
status replies only. It does not query the provider, spend prepared tickets,
save bindings, navigate, or prove which session is selected; runtime-owner
integration and Phase 4 acceptance remain open.

`RecoveryEndpoint::bind(path, target, project)` requires an existing canonical
directory owned by the effective user with mode `0700`. It creates one Unix
socket with mode `0600`, checks peer UID before reading bytes, and removes only
the socket device/inode it created. A nonblocking worker accepts one
connection/request at a time and forwards validated records through a
capacity-one channel. Listener creation and each nonblocking `accept` syscall
use the cooperative spawn gate so Panelink-managed exec does not inherit an
endpoint or accepted descriptor during creation. Accept gate acquisition is
retried in bounded 10 ms slices; the gate is released before client
authentication and request processing. The initial request/header read has a 300 ms deadline;
after complete validation the owner response may wait up to ten seconds.
`poll()` is a nonblocking owner-loop operation.

Each `RecoveryRequest` contains the full target tuple and exact project bytes,
plus one `Prepare { session_id }`, `Resume { ticket, revision }`, or `Status`
command. The socket packet uses a 16-byte `PLRECV01` version-1 header and a
bounded body capped at 4,608 bytes. The body starts with the five `u64be`
target fields, `u16be project_len`, and project bytes. Prepare adds
`u16be id_len` and a 1–128 byte ASCII ID; Resume adds a nonzero 16-byte ticket
and nonzero `u64be` revision; Status adds no fields. Every request must match
the endpoint's exact target and project scope.

Replies have a fixed 26-byte payload and carry no provider ID, path, free-form
error, or diagnostic text. They encode `Prepared(ticket, revision)`,
`Confirmed(revision)`, `Pending(revision)`, `ManualRequired(fixed reason)`,
`Unavailable`, `Busy`, or `Uncertain`. The `exchange` helper performs one
same-user connection and one request/response, bounded by the caller deadline
and a ten-second maximum. It never reconnects or retries. A lost response after
a durable spend or route action stays uncertain.

The worker detects EOF, extra bytes, timeout, and endpoint shutdown while
waiting for the owner, sets the request's cancellation flag, and returns no
reply. The owner must check `is_cancelled()` before provider lookup/navigation
and immediately after persisting a one-shot spent transition. Cancellation can
be shared with an IO worker via `cancellation_flag()` and can race with an owner
action; a completed spend is never rolled back or replayed.

Dropping the endpoint sets the worker stop flag, detaches rather than joins the
worker, and removes only the recorded socket inode. The listener is nonblocking,
and client reads/reply waits observe shutdown in 10 ms polling slices, so a
stalled owner request cannot make endpoint drop wait on the client. A focused
regression holds a request without replying, drops the endpoint, and checks
that drop returns promptly, the request becomes cancelled, and the client
unblocks. This confirms thread shutdown behavior; it does not claim that
unrelated same-user code or unmanaged exec paths honor the cooperative gate.

## Verification

The in-module tests exercise private pathname IPC and fixed replies, exact
scope refusal, oversize packet rejection before owner dispatch, client
disconnect cancellation, whole-request deadlines, bounded worker shutdown
while awaiting an owner reply. The implementation uses the shared cooperative
gate around listener creation and nonblocking accept; the transport module's
separate tests exercise the gate's descriptor-creation and accept contract.
They do not establish production UI/CLI integration or provider recovery.
