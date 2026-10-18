# Next bounded Phase3 wave — 2026-10-18

User requested parallel subagents for the next phase. This advances remaining
Phase3 contracts without declaring Phase2 host acceptance complete. Three
GPT-6-Luna xhigh owners have disjoint edit sets. Parent owns module exports,
canonical docs, roadmap, integration and independent verification. Workers do
not edit ROADMAP.md (exclusive parent exception to append-only roadmap rule),
commit, push, add dependencies or touch another owner's files.

FIRST read ROADMAP.md, AGENTS.md, README.md, docs/SPEC.md,
docs/PANELINK-DISCOVERY.md, docs/PREBUILD.md and latest INTEGRATION.md. Use code
graph first; if connector fails, report once then inspect source. No ms-rust
installation/compliance claim. Preserve current changes. Match existing style.

## N1: metadata notification codec

Own ONLY src/event_protocol.rs and docs/progress/P3-NOTIFICATION-CODEC.md.
Pure std Rust, no socket, PTY, capability advertisement or subscriptions.
Export public Copy/Eq types Event { cursor:u64,target:TargetRef,body:EventBody }
and EventBody::{ScreenChanged { revision:u64 },
InputOwnerChanged { revision:u64,owner_lease:u64 },ChildExited}.
Export EVENT_SCHEMA_VERSION:u16=3 and EVENT_PAYLOAD_BYTES:usize=72,
encode_event(&Event)->Result<[u8;72],EventError>,
decode_event(&[u8])->Result<Event,EventError>, Event::validate().

All integers big-endian; exact72byte layout:
schema u16 at0, kind u16 at2 (Screen1,Owner2,Exited3), reserved u32 at4=0,
cursor u64 at8 nonzero, target's five u64 fields at16..56 in existing order,
revision u64 at56, owner_lease u64 at64. All target fields nonzero.
Screen revision nonzero/owner_lease zero; Owner revision nonzero and owner_lease
zero (vacant) or nonzero (client owner); Exited revision/owner_lease both zero.
No Human owner, exit code, raw output, input, args/env, provider status or text.
Reject all invalid lengths/schema/kinds/reserved/invariants. No unknown-field
leniency. Custom Debug redacts identifiers/cursor/revisions. Fixed errors expose
no input. Encoding validates before producing bytes. The existing Event wire
envelope remains kind7/requestID0; this payload alone enables nothing live.

Test exact golden bytes/round trips, every body invariant, invalid target/cursor,
unknown schema/kind/reserved/trailing/truncated bytes and Debug redaction.
Use a temporary Rust harness or root after parent export. Record actual outputs:
`rustfmt --edition 2021 --check src/event_protocol.rs` and focused tests.

## N2: bounded generation-scoped metadata event journal

Own ONLY src/event_journal.rs and docs/progress/P3-NOTIFICATION-JOURNAL.md.
Pure std Rust using N1's exact public API (coordinate changes with parent).
Export EventCursor {generation:u64,sequence:u64}, EventJournal, JournalError,
MAX_RETAINED_EVENTS:usize=64, MAX_RETAINED_ENCODED_BYTES:usize=6144.
EventJournal::new(generation)->Result<Self,JournalError> rejects zero;
current_cursor()->EventCursor starts sequence0;
push(target:TargetRef,body:EventBody)->Result<EventCursor,JournalError> validates
before mutation, requires target generation match, assigns next nonzero cursor
with checked_add and validates Event, returns cursor. Retain newest64 events
using VecDeque preallocated64, pop before push on overflow (never65). No payload
strings, sinks, IO, locks, dispatch, output snapshots, hidden clock or IDs.
read_after(cursor:EventCursor,limit:usize)->Result<Vec<Event>,JournalError>
returns ordered earliest retained events strictly after cursor, <=limit1..64.
Reject wrong generation, future sequence and zero/>64 limit. Sequence0 is valid
initial observation. Cursor older than the last evicted sequence fails with
CursorExpired; equal to last evicted is valid (next event still retained).
Never return a partial stream as current. current_cursor denotes latest fact,
not completion or dispatch authority. retained_len()/retained_encoded_bytes()
report <=64 and len*(72+24)<=6144 (logical encoded accounting, not RSS).
No coalescing, resubscription, persisted history, EventsLost wire payload or
automatic resync: caller must reconcile current authoritative state on expiry.
Debug redacts cursor/events. Tests must include exact64/65 rollover, expiry
boundary, pagination/empty reads, generation/future/invalid limits, no mutation
on invalid push, cursor exhaustion (test-only fixture) and byte accounting.
Coordinate N1 in a temporary harness; parent exports after release. Record
actual tests and owned-file rustfmt. Do not edit N1.

## N3: explicit CLI receipts

Own ONLY src/p3_cli.rs and docs/progress/P3-RECEIPT.md. Add
`control --directory DIR --context FILE receipt THROUGH_SEQUENCE` with exactly
one nonzero numeric argument and existing strict path/argument parser bounds.
Use existing SessionRequest::Receipt schema; no new wire operation. Open/lock
existing context, refuse ANY uncertain operation before contacting server
(`operation_uncertain`), and reject sequence >= context.next_sequence before
IO (`receipt_out_of_range`). Resume exact stored identity, send once with
existing bounded exchange and decode exact matching key {gen,lease,through}.
Only Status Completed(Applied) counts as receipt success; other roles/statuses,
wrong keys/malformed bodies fail closed. Fixed server Error remains failure.
Successful receipt emits normal JSON status with action receipt. It never
reserves/increments/ACKs/rewrites local context, clears uncertainty, retries,
generates a replacement key or automatically retires anything. Lost ACK leaves
receipt result uncertain to caller but creates no effect authority; explicit
same receipt is server-idempotent. No new local receipt watermark/storage.
Add to subcommand help and tests. Refactor a small helper if useful so actual
socketpairs test exact Receipt body/matching response, wrong identity/status,
and show local context metadata unchanged. Parser tests reject0/missing/extra;
meaningful locked-context test proves uncertainty/out-of-range fail before IO.
Do not touch main.rs/README/context/server/other test files. Parent handles
shared docs. Actual commands:
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --bin panelink p3_cli::tests`
`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --bin panelink -- -D warnings`
`rustfmt --edition 2021 --check src/p3_cli.rs`.

Report what built, actual commands/results, initial failures, deviations,
gotchas, remaining limits and explicit file ownership release. No aggregate
phase completion, live notification/watch capability or host compatibility
claim follows from these component tasks. Parent review decides next runtime
integration separately; the existing runtime continues not to advertise Events.
