# Parallel server work — 2026-10-18

Parent owns module exports, Cargo/lock, shared docs, roadmap and integration.
Workers never commit/push or modify ROADMAP.md. All use original logic and
existing dependencies only. Read AGENTS.md, ROADMAP.md, README.md, SPEC.md,
PANELINK-DISCOVERY.md, PREBUILD.md and relevant progress evidence first. Check
code graph first; parent connector errors Transport closed, worker may work.
No fake peer authentication or pass claims for sandbox-denied pathname binds.
These are prototype contracts, not resolved release policy. P3 runtime work is
authorized while host checks remain pending; that does not complete those checks.

## Transport owner

Exclusive: src/transport.rs, docs/progress/P3-TRANSPORT.md. macOS/aarch64 only.
Existing std + Rustix + libc only. Do not edit peer_identity or wire codec.
Implement authenticated framed streams and private endpoint lifecycle.

`AuthenticatedStream::new(UnixStream)` must kernel-check require_same_user
before reading any bytes, then set_nonblocking(true). Keep stream private; no
unauthenticated reader path. `read_frame(deadline: Instant)` returns owned
header/kind/requestID and payload with redacted Debug (an OwnedFrame wrapper is
fine), first reads exactly24headerbytes into stack, decodes/validates length
before allocating body. Only current Hello/Welcome/Request/Response/Error kinds
allowed in this initial connection boundary (Snapshot/Event rejected unless a
future explicit negotiated runtime enables them). Body allocation <=65536;
wire's Hello/Welcome/Error <=4096 still enforced. EOF/short/error/timeout return
finite io errors and poison connection; no continued parsing after partial frame.
`write_frame(&Frame,deadline)` validates before any IO, writes header/body with
bounded nonblocking loops, no whole extra frame copy. Check absolute deadline
on each iteration, including Interrupted/continuous progress. WouldBlock sleeps
at most1ms until deadline; timeout/partial writes poison connection, never retry
whole frame. Preexpired deadline performs no IO. No background threads/queues.
Provide a read-only way to query poisoned state if useful. No raw stream clones.

`PrivateListener::bind(directory:&Path)` uses fixed socket basename control.sock.
Require absolute unambiguous path (reject dot/dotdot/trailing separators except
root), reject symlink path components. Validate ancestors are directories owned
by root/current euid and not group/other writable except root-owned sticky dirs.
Create final directory0700 exclusively if absent; otherwise require own UID and
exact0700. Hold directory File descriptor and record dev/ino. Confirm pathname
identity before/after binding. Refuse any existing socket/file/symlink with
AlreadyExists; **never automatically unlink a presumed stale endpoint**. This
explicit stale-path refusal is selected until a reviewed singleton lock can
prove no live owner. Bind listener under private dir, chmod socket0600, verify
file type/owner/mode, set nonblocking before exposing listener. Failed startup
and Drop remove only the socket created by this instance if dev/ino and directory
identity still match; preserve replaced/unrelated paths. Never remove directory.
Accept uses absolute deadline; authenticate accepted stream before returning it.
No accept or IO loop may block. Parent will audit directory race assumptions;
same-user processes are within the current trust boundary, not isolated agents.

Tests: socketpair auth/data/framing, bounded timeout on partial header/body and
stalled writer, malformed/oversize header before body, poison after EOF/partial,
deadline with available data, no replay, no raw payload debug. Path validation
and stale-file/symlink refusal tests run where possible without bind. Actual
pathname bind/permissions/connect/drop/replacement fixture is explicitly ignored
if sandbox requires external run; record actual attempt. Use temporary rustc or
temporary Cargo harness without editing manifest/root exports; parent integrates.

## Input ownership and reusable context owner

Exclusive: src/input_owner.rs, src/client_context.rs,
docs/progress/P3-OWNERS-CONTEXT.md. Existing dependencies only; no UI/process effects.

InputOwner is a pure finite lease state for one session. Public Owner enum
Client(u64 lease)/Human(u64 attachment), nonzero identifiers; snapshot includes
holder Option<Owner>, revision(initial1), expires_secs, pending Option<OperationKey>.
TTL1..30seconds prototype. Methods claim, release, handoff with expected revision,
caller identity (release/handoff must match current holder), now_secs; validate
must check exact holder/revision and unexpired lease. Claim while held rejects
(renew only explicit same-holder exact-revision claim, increment revision to
invalidate old refs). All revision/time overflow checks before mutation. pending
blocks handoff/release/renew/claim, even after lease expiry. `begin_input` validates
and pins one operation key; `finish_input(key)` clears only matching pending.
`expire(now)` releases due holder/increments revision only with no pending;
otherwise reports pending-needs-resolution and retains identity. No silent suffix
discard or auto-handoff; parent effect owner must finalize Partial/Unknown first.
Tests stale/expired/wrong owner/CAS/noninterleaving, overflow, pending expiry.

ClientContext is metadata-only reusable client state for one generation/lease;
nonzero generation/lease/next sequence, initial next1, uncertain Option<OperationKey>.
Persistent `ContextFile::open(path)` requires existing safe private parent0700,
own UID, regular own0600 file; O_NOFOLLOW and existing Rustix flock exclusive
nonblocking with absolute deadline (no advisory indefinite wait). Guard retains
file+lock through admission acknowledgment. New file created exclusive0600;
existing metadata record exactly fixed bytes, schema1, generation/lease,
next_sequence and uncertain key (no bodies, env, targets, outputs or credentials).
Do not silently initialize missing/invalid/stale state. Expose explicit create
with generation/lease and open; reject wrong expected generation/lease on open.
Reserve: require no uncertainty, take next key, increment checked highwater and
persist+sync **with uncertain=reserved key before returning key for dispatch**.
Ack(key) clears uncertainty only after exact response, persists+sync before unlock.
Reconcile(key, OperationStatus) is observational; require exact uncertain key,
clear on Completed or NotAdmitted; InProgress remains blocked. Never resend input.
Durable storage failure poisons guard and never returns a dispatchable key.
Crash/truncated record fails closed; no automatic reset/replacement. Keep record
bounded (<=128bytes), no variable allocations from untrusted lengths; fixed Debug.
Tests metadata-only encoding, reopen uncertain crash, two locks can't overlap,
sequence persists across invocation, no sequence reuse after rejected admission,
invalid/symlink/unsafe perms, old generation, fs failure fail-closed. If API choice
unavailable report; do not invent APIs or add dependencies.

## Response codec owner

Exclusive: src/response_protocol.rs, docs/progress/P3-RESPONSE.md. std only.
Pure borrowed schema1 response payload, max65536bytes including metadata.
Prefix36bytes: schema u16@0=1, responsekind u16@2, key generation/lease/sequence
u64@4/12/20 (all nonzero), bodylen u32@28, reserved u32@32=0.
Kinds:1 Status,2 Read,3 Rejected. Status body exactly1byte: NotAdmitted0,
InProgress1, Applied2, Failed3, Partial4, Unknown5 (use OperationStatus/Outcome;
Completed(Accepted) invalid). Read body: TargetRef five u64@0..40, source u8@40
Screen1/LiveScrollback2, screen_revision u64@41(nonzero), truncated u8@49(0/1),
text_len u32@50, text UTF8@54. Target generation matches key; IDs nonzero.
Read text only sanitized display text: reject Unicode control scalars except LF;
no ESC, C0/C1, tab, CR. max1024 lines (empty0 else1+LFcount); caller maxbytes counts
entire responsepayload including prefix/body. UTF8 boundary truncation is a
producer responsibility; codec rejects any overlimit rather than silently clips.
Rejected body exactlyu16 fixed code:1Unauthorized,2StaleTarget,3InputBusy,
4StaleOwner,5ExpiredOwner,6Capacity,7Unsupported,8InvalidRequest,9UnknownContext.
Public borrowed Response{key,body:ResponseBody}, validate/encoded_len,
encode_response_payload(&Response,&mut impl Write), decode_response_payload.
Validate everything before write, preserve partial prefix on IOerror no resend.
Custom Debug redacts read text/IDs; fixed error labels no user bytes. No runtime
read truncator/server/CLI or authorization claims. Tests literal golden examples
for all kinds, roundtrips, every trunc/trailing, schema/kind/length/IDs/mismatch,
controls/invalidUTF8/oversized/meta budget and writer error before/after prefix.

## Verification and handoff

Workers format only owned files and record ACTUAL commands/output. Temporary
harnesses under /private/tmp may import existing modules by path or use a temp
Cargo manifest with existing exact dependencies. Parent exports modules and runs
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline`
and fmt/strict all-target Clippy. Report changes, outputs, deviations and gotchas;
release ownership before parent source edits. Never mark the entire phase done.

Next wave after review: state owner must bind capability/grant/target/context/
owner validation to ledger admission before one bounded effect sink, then socket
handshake state and fake/real-process detach/reconnect tests. Production PTY
adoption still retains the recorded host gate; independently testing private-PTY
server lifetime must not be labeled Apple Terminal acceptance. Generation storage
and secret-free launch journal must precede operational API launch.
