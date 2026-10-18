# Experimental server/control command frontend — 2026-10-18

Exclusive next worker: src/p3_cli.rs, docs/progress/P3-CLI.md. This is a binary
child module of main.rs. Parent owns main.rs integration/help/doctor, all library
exports, Cargo, shared docs and roadmap. No new dependency/commits/push. Read
ROADMAP/AGENTS/prerequisite product docs, relevant P3 parallel/state/session/IO
contracts and existing CLI bounded capture/launch helpers. Code graph first.

Expose `pub(super) fn run(args: Vec<OsString>) -> i32` where args includes the
leading server-probe or control subcommand. Parent routes only these names here.
macOS/aarch64 only; no raw host IO/TUI, auto daemonization or global config.

Commands (exact grammar, all paths absolute):

`server-probe --directory DIR --terminfo TERMINFO [--allow-input] -- EXE [ARG...]`
`control --directory DIR --context FILE init|show|describe|resync|detach`
`control --directory DIR --context FILE read TARGET`
`control --directory DIR --context FILE claim TARGET OWNER_REV`
`control --directory DIR --context FILE release TARGET OWNER_REV`
`control --directory DIR --context FILE handoff TARGET OWNER_REV RECIPIENT_LEASE`
`control --directory DIR --context FILE text|paste|submit TARGET OWNER_REV TEXT`
`control --directory DIR --context FILE interrupt TARGET OWNER_REV`
`control --directory DIR --context FILE status SEQUENCE`

TARGET is explicit five nonzero decimal IDs `generation:workspace:pane:session:
occupant_revision`. Mutations/read must use it verbatim, never resolve a name or
call Describe implicitly to select a target. Control init obtains a new lease
once; all subsequent invocations load same locked metadata context and resume
exact generation/lease; expired/changed generation returns error without reset.
show is local metadata only. describe/status/resync are observations permitted
while context uncertain; mutations/new Read reservations remain blocked until
explicit resync. resync queries uncertain key only and uses ContextFile.reconcile;
InProgress remains blocked. Never send stored/missing input again.

## Server startup interfaces

Reuse main's `prepare_probe(ProbeRequest { terminfo_directory, executable,
arguments })` (parent-private items accessible to child module) so explicit launch
environment/argv/cwd and existing capture limits remain. Create final DIR0700 if
absent (exclusive, otherwise existing validated, never auto-delete). Open
`panelink::server_storage::ServerStorage::open(DIR)` to lock/flush generation
BEFORE PrivateListener::bind(DIR), which must succeed before child spawn. Existing
endpoint/stale file refusal remains: never unlink it in CLI. Parent supplies
`panelink::server_startup::StartedServer::start(storage, &prepared.launch,
&prepared.arguments, &prepared.environment, allow_input) -> io::Result<Self>`.
StartedServer holds singleton storage + ServerDriver<PtyEffects>. Methods:
generation(), startup_outcome(), driver_mut(). Parent implementing this now;
do not write your own bootstrap spawn or journal path.

Register signal-hook flag-only SIGINT/SIGTERM/SIGHUP before child spawn; stop flag
requests explicit whole-server stop. Selected signal registrations live to process
exit, no unregister barrier (same reviewed one-shot policy). Main event loop
ticks driver with fresh elapsed monotonic seconds and bounded1ms listener accept;
attach authenticated stream through driver, refuse excess workers without spawn.
Connection IO stays in workers; no socket write on owner thread. Child may exit
while server stays inspectable. Stop workers then Drop StartedServer to stop/reap
child; Ctrl-C here is explicit experimental server stop, client socket loss is
detach. --allow-input opts into prototype one-workspace collaboration grants;
without it advertise/read only. No per-key prompts or arbitrary crossworkspace
policy choices. Print bounded startup identity/status, no environment/arguments.

## Client rules

Use AuthenticatedStream::connect(DIR, absolute deadline2s) (parent added reviewed
nonblocking connect) and existing typed codecs only. Fresh Hello nonce nonzero,
require READ, optional INPUT, no EVENTS. Existing context via
ContextFile::open_existing(FILE,deadline) holds exclusive lock through response/
ack; its generation/lease become resume fields. Welcome must validate nonce/
caps and match resumed identity. Context init checks existing file before new
lease request; create metadata0600 only through ContextFile::create. No implicit
parent directory creation outside explicit server directory; context parent must
already be private0700.

Each client frame requestID1,2,... nonzero increasing, Hello0. Response kind and
requestID must match; Error payload never printed raw. All IO uses absolute finite
deadlines, no auto retry loops or reconnect/resend. Validate target/text/owner
revision and full typed payload BEFORE reserve; key uses current next_sequence
for preflight. Then ContextFile.reserve persists uncertainty/highwater before
send; use that exact key. On validated matching terminal status/Read or explicit
Rejected (no effect), ack exact key; InProgress stays uncertain. IO/codec/key/target
mismatch leaves uncertainty for explicit resync. Status uses existing key and does
not reserve/ack a new sequence. Claim/Handoff TTL30seconds, Read Screen max4096
encoded bytes/24lines. Receipt not necessary for first frontend, document1024
unreceipted perlease cap and explicit API receipt availability (no hidden retire).

Outputs machine-readable JSON using small original formatter (no serde); escape
quotes/backslash/LF and control scalars correctly, no raw terminal controls. Info
shows explicit target string, ownerrevision/lease/expiry, child_exited; statuses
use fixed names, Read includes text/revision/truncated. No inferred task success.
No terminalinput/env/argv diagnostics, no Debug dump of frames or OS path values.
Fixed error labels; nonzero exit on rejected/uncertain/failure, syntax exit2.
success=0 (Partial/Unknown statuses reported but exit1). Minimal help can be
inside this module, parent adds root help lines. Avoid unbounded stdin/file reads.

Tests useful parser/target/JSON escaping/admission response handling limits,
malformed role/ID/Welcome generation mismatch via socketpairs where possible.
Real pathname CLI run remains explicitly external/pending if sandbox bind denied.
No claiming it passed because parser or socketpair tests pass. Parent wires module
in main; ask when ready and then run focused root tests/fmt-owned/strict Clippy.
Record ACTUAL results, deviations, gotchas; release ownership before parent edits.
