# Herdr-style pane input and recognized agents

Status: source implemented and bounded private-PTY verified, user selected 2026-10-27. Existing-server activation complete; real provider acceptance remains open. Parent owns this
plan and integration. This is not full Herdr parity or phase completion.

## Accepted direction

User accepted the shared local input experience and explicitly included agent
list/prompt/wait. Idle UI attachments must not reserve every pane. Implement
per-batch Claim -> existing input -> confirmed Release using existing operation
keys, rather than weakening server owner checks. This protects each admitted
batch, not a human editing turn or unfinished draft. Explicit retained legacy
claims still block other writers; no claim stealing, idle heuristic or replay.
A one-shot is three separately keyed operations, not one server composite.
Do not release after uncertain/partial/failed input. If a layout preflight fails after claim but before any input key is reserved, release that exact unused claim. A release failure after a
known Applied write reports delivery confirmed and release unconfirmed.

## Ownership and interfaces

- UI worker owns only src/ui_client/net.rs and conn.rs (inline tests). Remove
  background/split claims; claim around keys/paste/app wheel, confirm releases.
- Pane CLI worker owns src/agent_cli.rs, new src/agent_cli/scoped.rs,
  src/p3_cli.rs, tests/p3_shared_input.rs. Add pane submit N TEXT [--json] as
  one complete text+Enter submission with automatic scoped ownership. Keep
  legacy pane send/claim/release semantics. Add WorkspaceLink operation key
  inspection and agent-observation/guarded-submit transport methods below.
- Codec worker owns new src/agent_protocol.rs, src/agent_detection.rs only.
- Parent owns native observation, server dispatch, recognized CLI module,
  routing/help integration, docs, tests not assigned above and parent verification.

All workers read this plan and AGENTS.md first; no commits, subagents, live user
pane input or build/test commands. Parent serializes compilation and tests.

## Recognized-agent wire contract

New optional capability CAPABILITY_AGENT_OBSERVATION=128. Schema14, big endian,
strict reserved bytes/trailing data/size validation; no legacy bytes change.
Query:48 bytes; schema u16 at0, kind u16=1 at2, reserved4 at4,
TargetRef five u64 at8..48.
Observation:96 bytes; schema14, kind0x8001, reserved4, target at8..48,
AgentIdentity pid:u64 at48, start_seconds:u64 at56, start_microseconds:u64 at64,
AgentKind:u8 at72 (Unknown0/Codex1/OpenCode2), AgentState:u8 at73
(Unknown0/Idle1/Working2/Blocked3/Exited4), reserved6 at74,
screen_revision:u64 at80, sampled_at_mono_ms:u64 at88.
Unknown identity may have all-zero pid/start fields; recognized identity must
be nonzero pid/start_seconds and microseconds<1000000. Kind Unknown cannot
make readiness claims. No executable paths/argv/env persisted in this codec.
GuardedSubmit:128-byte prefix plus bounded UTF8 text; schema14, kind2,
reserved4; OperationKey three u64 at8..32, target at32..72,
owner_revision:u64 at72; AgentIdentity pid/startsec/startusec at80/88/96,
kind:u8 at104; reserved7 at105; expected_screen_revision:u64 at112;
text_len:u32 at120; reserved4 at124; text at128. Text uses existing input
bounds and excludes control scalars. Entire payload <=65536. Exact complete
payload is ledger equality; duplicate lookup precedes stale agent/target checks.
Response uses existing keyed ResponsePayload outcome/rejection protocol.

Public Rust interface in agent_protocol:
AgentKind, AgentState, AgentIdentity {pid:u64,start_seconds:u64,
start_microseconds:u64,kind:AgentKind}; Observation {target:TargetRef,
identity:AgentIdentity,state:AgentState,screen_revision:u64,
sampled_at_mono_ms:u64}; GuardedSubmit<'a> {key:OperationKey,target:TargetRef,
owner_revision:u64,identity:AgentIdentity,screen_revision:u64,text:&'a str};
Request<'a> {Query(TargetRef), Submit(GuardedSubmit<'a>)};
encode_query(TargetRef)->Result<Vec<u8>,Error>;
encode_observation(&Observation)->Result<Vec<u8>,Error>;
decode_observation(&[u8])->Result<Observation,Error>;
encode_submit(&GuardedSubmit)->Result<Vec<u8>,Error>;
decode_request(&[u8])->Result<Request,Error>.
All identifiers/debug output redacted. AgentKind/State expose as_str() static.
agent_detection::classify(kind:AgentKind,text:&str)->AgentState is conservative:
known approval markers first, known working markers, then provider-specific
bottom prompt markers; default Unknown. This is screen-marker observation,
not provider turn completion, conversation identity or authorization.

WorkspaceLink public(crate) methods (CLI worker implements):
observe_agent(&mut self,target:TargetRef)->Result<Observation,&'static str>
guarded_submit(&mut self,target:TargetRef,owner_revision:u64,
observation:&Observation,text:&str)->Result<OperationStatus,&'static str>
last_operation_key(&self)->Option<OperationKey>
Require selected capability128; failure label agent_observation_unsupported.
Use reserve_mutation_key, exact keyed response validation and existing
uncertainty/receipt rules; never resend input. Native observations and guarded
submit require a supporting server; old servers must return unsupported.

## Parent recognized-agent layer

Native current PTY foreground-group leader PID + kernel birth identity +
executable basename codex/opencode gives process recognition. Snapshot classifier
reports Idle/Working/Blocked/Unknown, labeled heuristic. New guarded-submit
validates exact foreground identity, kind, screen revision and Idle at server
admission immediately before existing Submit. No screen/title-only identity.
Unrecognized wrappers/processes fail closed. No provider conversation claim.
agent list/get/read/prompt/wait consume generation/occupant/process-bound refs;
waits default30s and bounded maximum. Prompt waits observe working then settled,
never claim per-prompt turn completion. No model calls during verification.

## Acceptance

- [x] UI idle does not claim; known input batches release; uncertain input retained.
- [x] Two attached clients and independent pane submit succeed without detach.
- [ ] Races/partial writes/lost acknowledgments do not replay or steal claims.
- [x] Codec bounds/unknowns, process recognition and conservative states checked.
- [ ] Agent prompt rejects replaced PID/birth/target, blocked/unknown/stale screen.
- [x] Bounded agent waits distinguish observation from correlated completion.
- [x] Installed build, skill, help, proposal and durable evidence updated.

## Parent verification and installation — 2026-10-27

Installed client/server artifact: panelink 0.1.3, optimized locked/offline release,
SHA-256 `dd0a9c064d6a77bb0afe996bc73f8fb9d0eb7f9d7ddf280475581a685adc6364`.
`~/.local/bin/panelink` remains linked to this worktree's target/release/panelink;
previous 0.1.2 artifact saved at ~/.local/bin/panelink.before-0.1.3-20261027.
The corner badge uses the client package version; existing attachments retain
old code until detached and reattached. Global Panelink skill links to this
repository copy. No commit, push, user-pane input or server restart performed.
Root /Users/sahillamba/panelink remains clean; both transfer safety stashes retained.

Final serial commands/results:

- cargo test --locked --offline --lib --bin panelink -- --test-threads=1:
  library 898 passed, 4 ignored; client 247 passed, zero failures.
- cargo test --locked --offline --test p3_agent_cli --test p3_recognized_agents
  --test p3_shared_input --test usability_input_pty -- --test-threads=1:
  7 + 2 + 2 + 2 passed, zero failures. Private recognized fixtures compile a
  deterministic C executable named codex; no real provider/model calls.
- p5_workspace_tabs: three unaffected outer-PTY scenarios passed; pointer
  scenario failed at return-to-tail, then passed isolated after the extent fix
  (14.01 seconds). The full four-test target was not rerun after that fix.
- strict locked/offline lib/binary Clippy, cargo fmt --check, git diff --check,
  optimized release build pass. Ten affected Markdown documents checked with
  scripts/check_docs.py, zero findings; worker additionally checked skill
  frontmatter and seven affected relative-link sets.

Private evidence includes two simultaneous live UI attachments, UI key input
followed by an independent pane submit, both seeing an independently generated
marker; retained explicit claims refuse scoped submissions. Recognized fixtures
exercise process-bound discovery/get/read, different-birth refusal, guarded
prompt delivery/release, blocked/unknown no-input refusal and bounded waits.
Server tests cover stale observations, exact-key duplicate results without
revalidation, payload mismatch and owner-revision refusal. Socketpair tests cover
idle no-claim and release refusal/lost response. This is bounded private-PTY
coverage, not a full fault matrix or real Codex/OpenCode screen acceptance.

Parent corrections: capability 128 propagated through global negotiation;
query/read use dedicated agent context lane; compile errors corrected; malformed
schema test now actually changes the schema; pre-submit no-input effect stays
none despite a successful claim. Independent reviewer found a layout-preflight
claim leak: the exact unused claim now receives a keyed release before refusal.
Its race cleanup is source-reviewed, not separately runtime-injected.

Scrollbar correction retains the one-read wheel fast path, explicitly tags a
known history extent, and permits dragging from the known oldest row back to
live tail. Equal offset/extent alone no longer means the range is unknown.
Unit coverage includes middle and oldest positions, plus private pointer proof.

Earlier failures retained: initial codec malformed-schema test assertion, missing
negotiated capability in first native fixture run, one parallel workspace-event
failure (isolated and final serial reruns pass), and a later parallel lib run
with three history/provider-store timing/lock failures (final serial run passes).
No assertions were weakened to relabel these as passes.

Activation boundary: read-only installed CLI inspection found two panes in the
existing older server, both held by old UI leases. agent list returns
agent_observation_unsupported as intended. Updating a binary does not replace
these live processes. Shared input requires reattaching the new UI and releasing
or expiring old leases; recognized-agent commands require a newly started server.
Restarting the existing workspace ends its shells, so this remains pending user
approval. No claim of provider conversation identity, full Herdr parity, full
phase completion, or zero-lag performance is made.

## Live activation — 2026-10-27

User explicitly approved restarting and ending both existing shells. Installed
0.1.3 `panelink stop` returned workspace stopped; default open started a fresh
shell from /Users/sahillamba/panelink after its saved-layout prompt. Generation
advanced from 15642535525154316807 to 15642535525154316808. The saved layout was
not replayed; current workspace has one fresh shell. The temporary verification
UI displayed v0.1.3 and was detached with Ctrl-X, preserving the server/shell.

Installed agent list succeeds both while attached and after detach. The plain
zsh shell correctly reports recognized:false/kind:unknown; no real provider was
launched. Pane list while attached reports input_owner:none, and bounded pane
read succeeds. No child input or provider/model prompt was sent. Existing-server
activation is complete; real Codex/OpenCode recognition/state acceptance remains
separate. Root main stays clean. Run panelink to reattach the updated client.
