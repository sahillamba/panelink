# Phase3 owner discovery — accepted bounded contract

2026-10-23. Parent-reviewed next slice under the user's continue Phase3
request. Preserve all prior uncommitted scrollback/pane-run work. Do not
reset files to HEAD: prior features are not committed. No dependency, wire,
server-state or persistence format changes are required for this slice.

## Decision

Current SessionInfo already supplies the authoritative full target, owner
lease, owner revision and expiry. UI and automation use client leases; the
server cannot authenticate a human/agent/provider role or caller-chosen label.
Expose only this server-issued identity. No new registration/name/role
protocol or per-agent security isolation is implied. This is the bounded
owner-identity portion of Phase3; richer authenticated roles remain unproved.

## CLI behavior

`pane list --json` retains the existing string `input_owner` with values
none, you, or client:<lease> (replace the unsupported agent:<lease> role claim).
Add `input_owner_info` per pane:

- kind: none or client.
- id: null for none, otherwise string generation:owner_lease.
- is_caller: true only if nonzero owner_lease equals this pane link's lease.
- revision: exact SessionInfo.owner_revision, even when unowned.
- expires_at_server_secs: null for none, otherwise exact owner_expires_secs.

Expiry is a deadline on the server runtime's monotonic seconds clock, not
wall time or a client countdown. Pending input may retain an owner past that
deadline; do not treat an elapsed expiry as authorization or a free owner.
SessionInfo is a point-in-time read. Per-pane owner/metadata samples across
pane list are not an atomic workspace snapshot. Layout/target freshness
checks remain; claims and input still require server-authoritative owner CAS.

Text pane listing uses none, you, or neutral client:<generation>:<lease>
and includes owner revision + server deadline. Never infer an owner name from
`--as`, context filename, PID, cwd, process title or provider metadata.
Do not persist any new metadata or widen grants. Result contains no env/input
or arbitrary caller labels. Existing claim/send/release behavior is unchanged.

## Implementation ownership and checks

Luna implementation owns src/agent_cli.rs only. Keep current pane_run module
dispatch and all earlier edits. Store each full SessionInfo with its caller
lease through pane_list; construct legacy string + structured owner from the
same observation. Use a small pure helper for JSON output and focused tests
for none, own owner, other owner, same lease number in different generations
and generation-scoped formatting; expiry stays a raw deadline.

Luna acceptance owns new tests/p3_owner_identity.rs only. Use private bounded
fixtures. Two named callers: initial none, A claims -> A sees you/is_callertrue,
B sees client/is_callerfalse with SAME id/revision/deadline, B cannot implicitly
claim/send, release A -> none/revision increases, B claims -> new owner ID.
Verify stable identity on repeated CLI reconnects and cross-generation reuse
is distinguished (deterministic pure test can cover generation reuse). Verify
raw JSON fields and neutral text; omit self-asserted caller names. Do not wait
30s for TTL; existing logical-clock core tests cover expiration/pending state.
Do not modify existing tests or production outside ownership. No commits.

Parent owns shared docs/design, independent review, final full locked/offline
serial suite and seven other verifier checks. Worker tests must report ACTUAL
outputs; no broad concurrent sweeps, vendor formatting or phase-completion
claims. Native collaboration is the launcher.
