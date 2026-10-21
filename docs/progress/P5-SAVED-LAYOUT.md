# ADR-P5-LAYOUT-01: bounded saved layout and read-only restart

**Status:** Accepted as scoped prototype engineering design after parent and
independent review. Codec/store/runtime/CLI remain unimplemented.
**Date:** 2026-10-21
**Deciders:** Parent engineering decision within the existing two-pane prototype;
workspace/tab hierarchy and provider/product choices remain with the user.

## Context and observed implementation

R-05/R-06 request retained layouts/history after restart while distinguishing
old terminal data from processes and provider conversations. Current code has
an equal two-pane split root, full immutable TargetRef identities, an in-memory
layout revision and separate saved row/screen files. It has no saved-layout
store or recovery view. The initial launch creates a new live generation and
PTY through StartedMultiServer::start; it does not restore a layout.

Source inspected through the code graph then exact snippets:

- src/pane_layout.rs: LayoutSnapshot and PaneLayout::snapshot expose root,
  spatial pane rectangles, divider and revision; collapse handles two-to-one.
- src/server/multi_session.rs: finish_pane_exit retains the last exited leaf
  rather than making an empty layout; collapse_exited_panes/execute_close and
  resize_layout affect desired geometry independently of confirmed PTY sizes.
- src/multi_runtime.rs: MultiServerDriver::drop stops children before its
  bounded history drain. Saving after that stop would confuse shutdown with
  deliberate individual-pane removal.
- src/multi_startup.rs: StartedMultiServer::start uses a fresh ledger/lease and
  explicit client launch context; saved IDs must never feed this launch path.
- src/server_storage.rs: claim_history_writer is exclusive to one history
  worker and retains a duplicate singleton lock after timeout. It cannot be
  reused to claim a second independent layout worker.
- src/ui_client/state.rs: focus is client-local. There is no authoritative
  shared focus value to persist.

These are current code observations, not executed saved-layout acceptance.
Older P3A-LAYOUT.md is groundwork evidence; later runtime/source is authoritative.

## Decision and scope

Persist two bounded fixed-size historical layout slots for the current default
workspace in its existing server storage domain: current and the immediately
preceding saved generation. It is a reading/navigation
snapshot: root dimensions, equal split orientation, spatial pane ordering and
historical full targets. The record represents accepted desired geometry;
it does not certify actual PTY resize completion, checkpoint freshness, shell
state or provider identity. No generalized tree, arbitrary ratios, tabs or
workspace catalog is introduced.

Exclude focus, titles, executable paths, cwd, arguments, environment/profiles,
input, paste, PID, provider IDs and screen/row content. This avoids inventing a
safe persistent-launch-metadata classification before that policy is reviewed.
History remains in its current files; layout stores references, not copies.
Privacy boundaries and sanitized/debug-redacted diagnostics stay in force.

Restart inspection is read-only and separate from the live owner. Every saved
TargetRef is historical: never dispatch it, revive a lease, install it as a
new live occupant, replay a launch journal or start input/process/provider work.
A future explicit relaunch must obtain fresh context, allocate fresh targets
in the new generation and retain old targets only as historical links. That
mapping is not implemented by this contract. Existing plain panelink startup
behavior is unchanged: when its server is absent it still starts a fresh shell
from the current explicit invocation context. That is not a replay of any saved
command or historical target. No automatic historical recovery/relaunch is
enabled here. The two-slot rotation below prevents that first fresh-shell save
from destroying the preceding generation's only topology snapshot.

## Bounded version-one record

Reviewed component format, frozen for the first codec slice:
exactly160 bytes, all integers unsigned big-endian, no variable fields or
allocation proportional to untrusted lengths. Magic includes the version.

| Byte offsets (end exclusive) | Field |
| --- | --- |
| 0..8 | ASCII PLLAY001 |
| 8..16 | historical server generation, nonzero |
| 16..24 | historical workspace, nonzero |
| 24..32 | snapshot sequence, nonzero and independent of layout revision |
| 32..40 | last accepted layout revision, nonzero |
| 40..48 | observation Unix milliseconds, nonzero; not a commit or ordering clock |
| 48..50 | desired root rows |
| 50..52 | desired root columns |
| 52 | pane count:0,1,2 |
| 53 | axis:0 none,1 side-by-side,2 stacked |
| 54..64 | zero reserved |
| 64..104 | spatial first TargetRef: generation/workspace/pane/session/occupant_revision |
| 104..144 | spatial second TargetRef, same field order |
| 144..160 | zero reserved |

Nonempty roots use5..80 rows/20..240 columns. One pane requires axis0,
first target nonzero and second allzero. Two panes require axis1/2, both full
nonzero targets sharing header generation/workspace with distinct pane IDs and
distinct session IDs. Derive the existing one-cell divider and equal rectangles
using first=floor((dimension-1)/2), second=remainder; every leaf must satisfy
the current5x20 minimum and80x240 maximum. Spatial order absorbs Left/Right or
Up/Down placement; no historical split gesture needs storage. A snapshot adapter
must compare derived rectangles/divider with the complete LayoutSnapshot and
refuse any mismatch rather than approximate or normalize invalid geometry.

An empty tombstone has count0,axis0,rows0,columns0 and both allzero targets;
header generation/workspace/sequence/revision/time remain nonzero. It means no
panes remained at the last observed terminal-empty transition. It suppresses a
stale nonempty snapshot after orderly last-pane exit; history is not deleted.
The last live revision is retained because the current layout does not issue a
new revision for last-leaf exit. Sequence, not revision, orders this tombstone.

Decoder rejects any short/long/trailing/unsupported/reserved/invalid record as
a whole. Debug/error values expose fixed labels only, with IDs/time redacted.
No checksum/authenticity or rollback-resistance claim: format-valid alteration
may remain readable, as with existing history. A future format migration must
be explicit; never repair a corrupt or unsupported destination automatically.

## Owner observation and shutdown semantics

A future single owner adapter tracks a fixed canonical candidate and a checked
u64 snapshot sequence within its storage generation. It emits only on a changed
canonical desired topology/geometry or transition to empty, not polling, repeated
same-key results, focus changes or same-size pending-resize retries. The first
observation is sequence1. Checked exhaustion or missing valid clock disables
saving with a fixed diagnostic and leaves shells/history running. Time may go
backward; it never orders records. The worker retries identical owned bytes,
including sequence/time, rather than regenerating metadata.

| Trigger | Candidate policy |
| --- | --- |
| First applied/retained-unknown launch with a valid actual owner layout | Observe that layout; no speculative pre-launch record. |
| Split committed with an acquired retained child, including resize Partial/Unknown | Observe the resulting owner layout, not the request or preview. |
| Rejected split/close/resize or duplicate with unchanged canonical layout | No new candidate/sequence. |
| Valid root resize, including desired geometry with pending PTY resize | Observe the accepted desired geometry; do not label actual sizes confirmed. |
| Individual close/natural exit leaving survivor(s) | Observe resulting topology after owner settlement. |
| No live layout pane remains after individual closes/natural exits | Emit empty tombstone even though the existing owner retains a last exited leaf. |
| Client detach/focus/menu/reattach | No independent candidate. |
| Workspace stop/SIGTERM/driver teardown with live panes | Freeze last pre-stop candidate before any stop effect; shutdown-caused exits cannot replace it with empty. |
| Teardown after all panes already exited/closed | Preserve the already-observed empty candidate. |

Empty publication requires an owner-serialized post-close/post-poll observation
with positively recorded SessionState::Exited for EVERY target in the last
accepted layout. Unknown, missing effects, failed state lookup or an individual
child_exited observation without completed owner lifecycle settlement is not
terminal-empty proof. At least one uncertain/live target prevents an empty
publication. The owner must distinguish live/effect-retained Unknown/Exited;
do not interpret missing effects or a storage error as proof every child exited.
On an inconsistent state, disable snapshot publication and keep the last good
record. A driver-shutdown marker is set before prepare_driver_shutdown and
before further natural-exit collapse; teardown is not an ordinary close event.
This marker does not change the established child-stop or input contracts.

## Future store, writer and durability contract

Files are separate from history: private server/layout/ directory0700,
current.layout and previous.layout files0600 owned/single-link/regular,
exact160 bytes each. Startup/explicit setup may create/validate the directory
before interactive operation; all later layout writes/syncs occur on the worker. A read-only
open does not create the directory, touch server identity counters or acquire
a new live generation. Mutations require the existing server singleton or a
verified maintenance guard. A future layout-specific exclusive writer claim
retains its own duplicate of that same singleton lock through all possible IO,
receipts/diagnostics and backend teardown. History and layout claims may coexist
for distinct files; two layout workers may not. This explicitly refines SPEC's
earlier generic one-metadata-writer wording into one exclusive writer per
disjoint stream. History owns only its history directory, layout only its layout
directory; generation counters/launch journal keep their existing owner. No
writer may mutate another stream or claim a cross-stream atomic commit. Never release a detached
layout writer's exclusion just because its driver or history writer ended.

Use a bounded async channel with one queued candidate, one in-flight candidate
and one owner-deferred latest candidate, at most3x160=480 encoded bytes plus
fixed metadata and bounded storage scratch. New changes replace only the
owner-deferred candidate. Submission never waits for queue space or disk.
The owner attempts to flush its latest deferred candidate each tick. At
teardown it makes one final NONBLOCKING try-submit before closing admission.
If both accepted slots remain occupied or the submission is contended/failed,
the owner-deferred latest remains unsubmitted: record a fixed
layout_final_unsubmitted/incomplete result and its bounded last-observed key,
then drop it. Never claim it entered the worker or delay history drain waiting
for capacity. Only accepted work drains. A skipped/coalesced intermediate
version is not a successful durable save; observers distinguish last observed,
last submitted and last durably confirmed keys plus sticky unavailable/incomplete
state. No automatic retry with a new sequence/time is synthesized at teardown.
Do not conflate replaceable layout snapshots with nonreplayable PTY input.

Store ordering is (generation,snapshot_sequence) within this server storage
domain. The writer/store mutation capability binds expected generation to the
held live ServerStorage::generation and expected workspace explicitly to its
owner. It verifies that the layout directory is exactly that server's layout
child and retains the corresponding singleton; a caller cannot supply an
unrelated directory plus another server's guard. Every incoming candidate must
match that bound generation/workspace; a format-valid stale/future candidate is
refused even if its key would otherwise sort later. Existing historical slots
may have older generations but must match workspace and never exceed the bound
live generation. Read-only opens have no live-generation requirement. No
automatic writer restart resets the owner's sequence within the same generation. Same key/same
bytes retries must directory-sync before acknowledgment; same key/different
bytes and lower keys are refused. For every higher same-generation sequence, a lower layout revision is refused.
A changed nonempty candidate requires a STRICTLY greater layout revision. Equal revision is permitted only for the
single canonical nonempty-to-empty transition; the empty candidate may also
carry a greater revision if coalesced changes preceded final exit. Once empty
is durable, no further changed candidate in that generation is accepted.
Changed nonempty data at equal/lower revision and repeated empty with a new
sequence/time are refused. Byte-identical same-key retries remain allowed and
must re-sync; sequence/time alone cannot create a valid topology change. Restored or
copied backups have no monotonicity guarantee.

Before replacing a valid current slot with the first snapshot of a higher
generation, preserve EXACT current bytes in previous.layout and finish that
archive's directory sync BEFORE renaming the new current. This includes a
normal plain-panelink fresh-shell startup after the old server ended; no
historical relaunch was requested or implied. Subsequent same-generation saves
touch only current. Thus one normal restart leaves the prior generation
available through the explicit previous view. A further successful generation
rotation may replace that older previous slot; retention is bounded to current
and the immediately preceding saved generation, not every process/server run.

Preflight BOTH existing slots before any rotation mutation. An existing previous
record must have an older generation than current, or be exactly the same
current bytes from an interrupted rotation. A newer previous generation or
same-generation/different bytes is inconsistent and refused. On retry an exact
archived duplicate must re-sync the directory before current replacement. A
crash/cancel/error between the archive and current renames can leave both slots
containing the old snapshot; that is a valid interrupted rotation, not two
completed generations. Never treat archive-only success as incoming save
acknowledgment. The current receipt/key advances only after current's final
directory sync. A retry after an uncertain current rename sees matching new
current and must sync it without rotating previous again. A missing current
with a present previous is refused for mutation rather than silently resetting
the authoritative chain; previous remains explicitly inspectable.

Corrupt, unsupported, unsafe or unexpected-scope slots are preserved and saving
fails closed; valid previous is never an automatic fallback hiding invalid
current. When BOTH slots are absent, the first explicit runtime snapshot may
create current without previous. Missing read data is absence, not permission
to fabricate old layout/process state. Reader validation is per requested slot;
it can inspect valid previous even when current is invalid. No multi-file atomic
transaction is claimed, and old history files are not deleted by rotation.

Atomic replacement: validate held/named private-directory identity and existing
committed record; create one recognized exclusive nofollow/cloexec0600 temp,
write exact bytes, file sync/fullfsync, rename into verified destination,
directory sync. Pre-rename error leaves old destination; post-rename sync error
can expose new bytes without acknowledgment. Retry exactly the same160 bytes,
at most3 attempts with10ms worker delay. Terminal failure closes this writer
only and keeps PTYs/history available. Existing history-temp cleanup must never
scan/promote/delete layout files. Layout-temp cleanup preflights at most128
directory entries and16 recognized temporaries under exclusion before deleting
any. Recognized names are exactly .current.layout-PID-COUNTER.tmp or
.previous.layout-PID-COUNTER.tmp with canonical nonzero decimal PID/u64 counter.
Each candidate must be a private single-link regular file at most160 bytes;
unknown entries are preserved, unsafe candidates/bounds fail closed. Sync the
directory even on an empty retry. Never promote a temporary or repair either
committed slot; existing deletion may be partial after a later error.

At driver teardown use one shared total250ms metadata-drain wait budget for
history and layout, not two sequential250ms waits. The deadline must start at
the existing metadata-drain boundary after child stop/capture; child-stop time
remains separate. Close both admissions before waiting. Each worker can retain
its lock after timeout because blocking syscalls cannot be interrupted. No new
layout stage starts after cancellation; an already-running final sync may
finish but does not turn a timed-out caller result into drained. There is no
history/layout multi-file transaction and no durability-at-Drop guarantee.

## Read-only restart view contract

Planned separate CLI entry: panelink layout show, or panelink layout show
--previous. Extra/duplicate flags are usage errors. Default selects only current;
--previous selects only previous, with no silent substitution.
Dispatch must precede default_open::ensure_server/prepare and every live-startup
path. It validates the entire requested layout before stdout, labels its slot
and prints historical generation,
workspace, snapshot sequence/time, desired dimensions/orientation and spatial
historical target IDs, or an explicit empty-workspace snapshot label. Missing,
invalid or unsupported is a fixed sanitized error/nonzero status and no partial
stdout. It never starts/attaches/stops a server, initializes identity, invokes
cleanup, executes stored data, follows a provider resume reference or guesses
the newest history target. No restored layout is injected into live control.

In the first CLI slice history availability is not certified or loaded: print
that rows/screens are separate read-only sources available through the existing
history commands by the exact saved target. Pruning or clear may remove them;
missing/corrupt history does not make valid layout metadata unreadable. Future
combined recovery must label missing/invalid sources independently and keep
source timestamps; no cross-file atomic snapshot is implied.

A later terminal recovery viewer may derive a display layout for its current
content viewport using existing equal-split rules. If too small, show a bounded
status and retain the saved topology; no crop/merge or PTY resize is inferred.
That UI, explicit relaunch mapping and any default-startup recovery chooser
need separate implementation and acceptance. This ADR freezes no tabs/sidebar,
launcher/provider behavior or global focus/product selection.

## Options considered and trade-offs

| Option | Complexity/cost | Benefit | Consequence |
| --- | --- | --- | --- |
| Fixed archival snapshot (selected) | Small dependency-free codec/store; one bounded worker | Safe historical topology with explicit identities and no launch secrets | Only current two-pane equal topology; no live restore or history atomicity |
| Persist complete live owner/launch payload | High and unsafe with current volatile launch data | Tempting automatic reconstruction | Secret persistence, stale identity reuse and process replay violate contracts |
| General workspace tree/catalog plus migrations now | High; unresolved product choices | Supports future hierarchy/ratios | Premature format/API commitments; postpone |
| Save synchronously on owner/Drop | Small code but unbounded IO latency | Straightforward save acknowledgment | Reintroduces known storage stalls; rejected |
| Reuse history payload queue/claim unchanged | Low initial wiring | Existing worker and lock | Queued history can starve latest layout; exclusive claim cannot create second worker; requires a separate reviewed redesign |

No new crate or storage framework is justified. This decision gives a safe
bounded historical association while leaving the full reboot recovery and
representative-workload gates open. Coalescing may lose intermediate topology;
a stale latest durable snapshot is displayed as last saved, not current-at-crash.

## Staged implementation and exact first handoff

1. Pure src/saved_layout.rs codec/model; parent exports module. No runtime,
   disk, CLI, generation allocation or restored-pane constructor in this slice.
2. Separate two-slot fixed-record store + archive-before-current rotation fault
   tests, private layout directory and locked orphan-temp cleanup; no owner IO.
   Preserve invalid destinations, including missing-current/present-previous.
3. Bounded async writer and layout-specific guard + independent delayed-worker,
   failure, coalescing and singleton-retention tests.
4. Owner/driver observation integration with explicit shutdown marker and one
   shared metadata deadline; test every trigger above, including last exited
   retained leaf, retained-unknown launch and same-revision empty transition.
5. Fresh-process layout show/--previous + read-only restart association. Test
   absent server/storage with a configured shell marker: no child, directory,
   identity initialization or generation increment; dispatch before ensure_server.
   Existing-slot reads preserve paths/modes/mtimes/bytes. No automatic startup
   change. Gate broader recovery UI/relaunch separately.

First slice API (frozen in the linked codec task): SavedLayout with private
validated fields, LayoutAxis::{Single,SideBySide,Stacked}, metadata constructor
for nonzero header fields, from_snapshot(metadata, &LayoutSnapshot),
empty(metadata), encode(&SavedLayout)->[u8;160],
decode(&[u8])->Result<SavedLayout,LayoutRecordError> and read-only accessors.
Errors are fixed sanitized variants; Debug redacts all IDs/time. Return a
historical read-only geometry value, not PaneLayout, StateOwner or executable
launch data. Pure code must have no IO/thread/env/clock/launch calls.

Codec acceptance: exact160-byte roundtrips one pane, all four gesture-derived
spatial orders, odd/even roots, valid minima/maxima, zero-pane tombstone;
every short length and oversize/trailing byte; each invalid header/axis/count,
reserved region, nonzero unused slot, zero/scope/conflicting targets, duplicate
pane/session, invalid leaf geometry, malformed snapshot rect/divider/hole;
no wrapping integer arithmetic; redacted Debug/errors. Distinguish sequence
from layout revision; timestamp rollback is allowed. Existing pane_layout tests
must continue passing. Later store/worker/runtime/CLI tests are required before
claiming saved-layout support, and full V-HISTORY requires broader reboot/UI
and representative acceptance.

## Evidence and completion

The queued design phase is complete. One Luna xhigh performed independent
source/design review; parent reconciled its findings before accepting this
scoped contract. See [independent review](P5-SAVED-LAYOUT-REVIEW.md) and the
exact [pure-codec task](P5-SAVED-LAYOUT-CODEC-TASK.md). Review resolved the
fresh-start overwrite, retained-exited leaf, stream-writer ownership,
generation/directory binding, startup-free inspection, final-unsubmitted
candidate and equal-revision transition issues. Later stage acceptance gates
are still mandatory before runtime support is enabled.

Actual checks for the existing source behavior reviewed here:

```text
cargo test --locked --offline --lib pane_layout -- --test-threads=1
17 passed;0 failed;0 ignored;538 filtered (includes2 UI geometry tests)
cargo test --locked --offline --lib server::multi_session -- --test-threads=1
50 passed;0 failed;0 ignored;505 filtered
python3 scripts/check_component_sources.py
vt100 and crossterm-input fingerprints match
```

These67 existing tests do not test an unimplemented saved-layout codec/store.
The preceding full verifier7/8 and31-target integration sweep's known host/socket
failures remain historical evidence on the unchanged tree; they were not rerun
or converted into passes for this docs-only task. New design tests listed above
are proposed, not executed. No saved-layout file, runtime behavior or new CLI
command exists from this document. Full recovery remains unimplemented.

All96 baseline Rust application/test and manifest files and13 prior raw report/
provenance artifacts retain identical hashes. No dependency/vendor change,
commit or push; all ownership is released after final review. The requested
ms-rust skill remains absent. Graph discovery was available; source is unchanged.
Final documentation/diff checks are recorded in current INTEGRATION.md. The
initial review-note check found one trailing-whitespace line, corrected before
the final check; that failed check is not a pass.

Next: implement ONLY the pure saved-layout codec/model under its exact task,
with parent export/review and automated checks. Store/rotation, writer/guards,
owner lifecycle integration, read-only CLI and broader reboot/UI/relaunch
acceptance remain subsequent slices. Parent phases3/5/6 stay open.
