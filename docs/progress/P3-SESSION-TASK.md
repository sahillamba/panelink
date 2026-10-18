# Session control and reconnect payloads — 2026-10-18

Exclusive worker: src/session_protocol.rs, docs/progress/P3-SESSION.md. Parent
owns exports, roadmap/shared docs. Existing std/codecs only; no IO/effects or
dependency/manifest changes. Read P3-PARALLEL-TASKS.md/prerequisites first. Do not
edit completed response_protocol.rs. No commits/push. This is private prototype
schema evolution, not a claim of live reconnect or API completeness.

## Resumable Hello wrapper

Public SessionHello { hello: handshake_protocol::Hello, resume: Option<LeaseRef> }
and LeaseRef { generation:u64, lease:u64 }, both nonzero. New connections encode
the existing exact28byte schema1 Hello. A resume explicitly uses44byte schema2
Hello: same fields/offsets as schema1 through28, schema u16=2, reserved0; old
generation u64@28 and old lease u64@36. Both resume fields must be nonzero.
Decode exact28/schema1 via existing decode_hello; exact44/schema2 validates same
nonce/capability rules plus resume IDs. Reject all alternate lengths, flags,
versions. encode_session_hello/decode_session_hello; maximum44bytes. This neither
issues nor renews a lease; runtime checks generation/live lease/auth before Welcome.

## Session Request payload schema2

All integers big endian. schema u16@0=2, opcode u16@2. No other prefix for the
two keyless observations Describe/Detach. Public SessionRequest Copy/Eq enum:

- Describe opcode1, exactly4bytes. Current single-session inventory observation.
- Claim opcode2,80bytes: OperationKey3u64@4..28, TargetRef5u64@28..68,
  expected owner revision u64@68, ttl seconds u32@76 (1..30).
- Release opcode3,76bytes: same key/target/revision, no ttl.
- Handoff opcode4,88bytes: same key/target/revision, recipient client lease
  u64@76(nonzero), ttl seconds u32@84 (1..30). Human handoff encoding deferred.
- OperationStatus opcode5,28bytes: OperationKey3u64@4..28, no target.
- Receipt opcode6,28bytes: generation u64@4, lease u64@12, through sequence
  u64@20. All nonzero, this cumulative receipt retires results but dispatches no
  process effect. It does not authorize replay or reset a client high-water mark.
- Detach opcode7,4bytes. Only closes this attachment; never stops a child.

All keys/targets/revisions nonzero; keygen==targetgen. Decode exact lengths,
reject unknown schema/opcode/truncation/trailing/invalid ttl before use.
encode_session_request returns bounded Vec (<=88); validate before allocation.
Keep deterministic metadata bytes suitable for ledger equality and no raw Debug.

## Single-session info Response payload schema2

Public SessionInfo { target:TargetRef, owner_revision:u64, owner_lease:u64,
owner_expires_secs:u64, child_exited:bool }. Exactly72bytes: schema u16@0=2,
kind u16@2=1, TargetRef5u64@4..44, ownerrevision u64@44,
ownerlease u64@52 (0vacant), expiry u64@60 (0vacant), exited u8@68=0/1,
reserved3bytes@69=0. IDs/revision nonzero; ownerlease==0 iff expiry==0. The
initial prototype describes client owner or vacant; a human owner cannot be
misrepresented as client ownership. encode/decode_session_info exact fixedarray.
It is an observation only, not a TargetRef authorization token, input grant,
provider identity, guaranteed task success or durable checkpoint.

Owner mutations return existing schema1 Response::Status for their key;
caller observes SessionInfo afterward to get current revision. Exact duplicate
mutations return prior status before mutable ownership/target preconditions.
Status and Receipt remain observations/retirement, never dispatch authorization.
Frame dispatch: Hello kind accepts SessionHello wrapper; Request schema1 routes
to control_protocol, Request schema2 routes here; Response schema1 to response
codec, Response schema2 to SessionInfo. Error framing/state machine are parent-owned.

Tests independent golden layouts (Hello resume, Claim, Info), every opcode,
strict IDs/ttl/generation/length/version/reserved, roundtrip, old Hello support,
unknown required capabilities remain negotiation's responsibility, no payload
or identifiers in custom Debug. Use temp rustc harness importing existing codecs
and record ACTUAL output; parent integrates/validates Cargo tests, fmt/Clippy.
Match existing style; report changes/deviations/gotchas and release ownership.
