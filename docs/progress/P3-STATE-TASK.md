# Single-session admission owner task — 2026-10-18

Exclusive next worker: src/server.rs, docs/progress/P3-SERVER.md. Parent owns all
exports/shared docs, journal/generation/PTY/socket adapters/CLI. Read ROADMAP,
AGENTS, required product docs, P3-PARALLEL-TASKS and P3-SESSION-TASK first.
Use existing dependencies/codecs only, no commits/push or effects outside fake
tests. Typed response/session/owner modules are separate owners; read their
stable APIs, do not edit them. Parent added Ledger::existing_admission: None is
an observation, never Execute. Only Ledger::admit can reserve/dispatch.

## Scope

One flat target, one effect sink, up to64active reusable leases, no layout or API
launch. Parent supplies generation from flushed singleton storage. Multiple
connections may reconnect to a lease but socket lifetime does not own the sink.
Current release policies remain open; prototype grants explicitly configured at
construction for this workspace (read always, input only if allow_input=true).
No provider identity, autonomous work, clipboard, events or live scrollback claims.

Public `EffectSink` trait (worker defines in server.rs):
`validate(&self, command: Command<'_>) -> Result<(), RejectionCode>` pure capacity/
supported-source preflight with no effects;
`read(&mut self, source: ReadSource, max_text_bytes: usize, max_lines: u16)
 -> Result<ReadResult, Outcome>`;
`input(&mut self, command: Command<'_>) -> Outcome` synchronous bounded terminal
result only, never Accepted. Adapter must drain or discard pending suffix before
returning; it never replays a partial/uncertain input;
`poll(&mut self) -> Result<(), Outcome>`;
`child_exited(&self) -> bool`.
ReadResult { text:String, revision:u64 nonzero, truncated:bool }, Debug hides text.
No effect queues or threads in this module: one synchronous slot, preflighted
before admit. Backend input/drain bounds are parent-owned, no claims about a real
server scheduling guarantee from fake sink tests.

`StateOwner<E:EffectSink>::new(generation:u64,target:TargetRef,effects:E,
allow_input:bool) -> Result<Self,ServerError>` validates target IDs and matching
generation. Own Ledger, InputOwner, E and bounded lease capability records.
No allocations from untrusted counts. `effects()/effects_mut()` for parent-only
poll/testing as needed. `target()` and `owner_snapshot()` read-only.
ServerError fixed/redacted, code() fixed str; wrap ProtocolError without dumping
user content. Parent socket boundary authenticates before calling this owner.

`connect(&SessionHello, now_secs) -> Result<(Connection,Welcome),ServerError>`:
Connection private fields lease/capabilities (public getters), constructed only
here, no client-supplied capability/auth boolean. Negotiate required capabilities
against READ and optionally INPUT, never EVENTS. Validate negotiation first using
dummy nonzero lease, then issue or resume ledger lease. Resume must match this
generation and existing unexpired record, then renew and reuse ID; never silently
issue a replacement on failed resume. Active leases map<=64; maintenance removes
expired grant metadata, ledger retains permanent tombstones. Track checked expiry
now+1800. No connection reference counting or per-disconnect expiry; transport
owns connection-count admission separately. Connection selected capabilities are
intersection of actual server grant and Hello offer, not authorization from
caller data. Always validate supplied request key lease==Connection.lease.

`handle_control(&Connection, payload:&[u8], now_secs)->Result<Vec<u8>,ServerError>`:
decode/validate all bytes, bound per request; verify connection live and command
capability before accessing payload duplicate state. `existing_admission` checks
exact bytes and returns prior InProgress/Completed response BEFORE target/owner
revision/live capacity checks. It never dispatches. New requests then require
exact TargetRef, applicable owner, sink.validate, read max_bytes>=90metadata
bytes. Reserve through Ledger::admit only after all checks. Only Execute invokes
read/input once. For input: pin InputOwner::begin_input, effect, terminal outcome
complete and finish_input; unexpected Accepted becomes Unknown. No early clear
of owner while suffix unresolved; adapter contract returns only after discard.
For read: text budget=request.max_bytes-90; ensure result controls/UTF8/lines and
complete encoded metadata fit requested cap. Invalid/overlimit result completes
Unknown or Failed and never emits raw text. Revision must be nonzero. Applied
means bytes/signal delivered under adapter contract, not task/application success.
Queries with duplicated keys return saved status, not a cached/stale screen body;
new observations use new keys. Full responsepayload<=65536 via response codec.

`handle_session(&Connection,payload,now)->Result<SessionReply,ServerError>`;
SessionReply::{Info(SessionInfo),Payload(Vec<u8>),Detached}. Describe returns
current single target/owner/client-holder or vacant plus child_exited; Human
holder cannot be cast to Client (reject unsupported if encountered).
OperationStatus uses key matching this generation/connectionlease and ledger
status only (never dispatch). Receipt verifies generation/connectionlease, calls
ledger receipt, returns existing schema1 Status Completed(Applied) for key with
sequence=through (receipt itself creates no new operation). Detach does nothing
to effects/ledger/owner and asks socket layer to close after an acknowledgment.

Claim/Release/Handoff are DurableMutation keys with exact encoded payload
deduplication. Check connection capability INPUT/live lease, then exact duplicate
before current target/owner preconditions. Prepare candidate InputOwner clone;
validate claim/release/handoff on candidate BEFORE ledger admit. Caller is
Owner::Client(connection.lease). Handoff recipient must be another active lease
with INPUT grant in this one workspace, never a guessed/passed arbitrary ID.
After Execute commit candidate and complete Applied; duplicate returns old status
even after owner rev changed. No process effect for ownership operations.

`poll(now)` expires ledger/grants and owner explicitly, then makes bounded sink
progress; a pending owner expiry must stay pinned until effect resolution. No
public method translating old keys to new generation/context. Reconnect read
returns current state; no replay of terminal bytes/queries/input.

Tests actual fake-sink call counters: duplicate same key once; changed bytes
conflict; stale target/owner/lease/capability rejects with no ledger/sink change;
duplicate returns before stale owner (claim increments revision); status cannot
dispatch; busy/capacity before admission; suffix Partial/Unknown doesn't replay;
old generation resume fails/new lease not issued; live resume keeps lease and
input owner; expiry invalidates; detach leaves same sink alive; read output
metadata budget, UTF8/controls/revision validation; read-only negotiation rejects
input without effect; handoff rejects unknown/non-input recipients.

Verify temporary Cargo harness path-import modules (no exports/manifest edits).
Format owned file only. Record ACTUAL outputs, deviations, gotchas and release
ownership; parent independently tests. Never mark full Phase3 complete.
