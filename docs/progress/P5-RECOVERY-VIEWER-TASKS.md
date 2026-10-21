# P5 recovery viewer and relaunch: staged implementation tasks

**Status:** Proposed handoff, 2026-10-21. No task below has been implemented
or tested. Parent assigns one exclusive owner per file per stage, reviews each
slice and records actual test output; do not parallelize overlapping root/CLI
files. Follow [ADR-P5-RECOVERY-02](P5-RECOVERY-VIEWER.md). Resolve marked user
choices before freezing CLI/relaunch behavior; preserve plain `panelink` startup.
No new dependency, unsafe code or credential-bearing fixture.

## Stage 0 — parent gate and fixture contract

**Owned files:** `docs/SPEC.md`, `ROADMAP.md` and this task document (parent
only); no application source. Review exact viewer entry point, buffer display,
small-host treatment, launch selection, live-server behavior and link retention
with user. Record accepted choices distinctly from prototype assumptions. Keep
Phase 4 identity and full reboot acceptance open. Specify a private temporary
test root and synthetic rows/checkpoints only; never use real user history,
config or provider accounts.

## Stage 1 — pure read model and geometry (one worker)

**Owned files:** new `src/recovery_view.rs`, new
`tests/p5_recovery_view_model.rs`; parent alone owns `src/lib.rs` export.

**Proposed APIs:** `RecoverySlot::{Current,Previous}`;
`RecoveryPane { historical_target: TargetRef, rows: HistoryAvailability,
normal: CheckpointAvailability, alternate: CheckpointAvailability }`;
`RecoveryView::from_layout(SavedLayout)`;
`plan_viewport(&RecoveryView, rows:u16, columns:u16, chrome_rows:u16)
-> DisplayPlan`. `DisplayPlan` contains at most two bounded rectangles,
divider, source labels or `TooSmall`, never a `PaneLayout`/live `TargetRef`.
Use `SavedLayout::panes/geometry`, `LayoutAxis` and existing
`pane_layout` bounds; checked arithmetic and 0/1/2-pane fixtures. Keep the
model free of filesystem, PTY, network, commands and provider logic.

**Proposed tests:** both spatial axes, first/second pane order, odd/even
viewport rounding, exact 5x20 leaves, one-cell divider, empty tombstone,
saved dimensions different from host, chrome subtraction, 0x0 and 40/41
columns plus 10/11-row boundary, oversized host clamp and no topology
mutation. Run `cargo test --locked --offline --test p5_recovery_view_model`
with the parent-supplied `CARGO_TARGET_DIR`; record pass/fail counts.

## Stage 2 — exact-target offline read (one worker after stage 1)

**Owned files:** new `src/recovery_reader.rs`, new
`tests/p5_recovery_reader.rs`; parent owns `src/lib.rs` export. Leave
`src/layout_store.rs`, `src/history_store.rs`, existing CLI and writer unchanged
unless a narrowly reviewed missing read-only accessor is demonstrated.

**Proposed API:** `read_recovery(server_dir:&Path, slot:RecoverySlot)
-> Result<RecoveryView, RecoveryReadError>`; open `LayoutStore::open` and
`load(Current/Previous)` explicitly; `HistoryStore::open` and
`load_rows/load_screens` on *each exact* `TargetRef`. Decode rows with
`history_rows::decode` and checkpoints with `screen_checkpoint::decode`,
verify embedded target and validated dimensions, store source timestamp/
sequence, epoch/gap/reset/capture-disabled status. Model independent
`Missing`, `Invalid`, `Available` per source and target. Do not output partial
bytes for an invalid layout or decode only a prefix of an invalid committed
history file; a bad pane does not invalidate a healthy other pane. Reject
unsafe links/FIFOs/oversized files using existing readers. No cleanup,
generation allocation, lock mutation or stdout while loading.

**Proposed tests:** current/previous selection without fallback, invalid
current with valid previous, empty/absent storage, mismatched checkpoint
target, missing/corrupt normal rows and either screen independently, cleared
or pruned history, retained gap and timestamps. Assert root/file bytes,
metadata and mtimes unchanged; absent storage stays absent; no marker shell
starts. Run `cargo test --locked --offline --test p5_recovery_reader`.

## Stage 3 — offline TUI and CLI (one worker after stage 2)

**Owned files:** new `src/ui_client/recovery.rs`, new
`tests/p5_recovery_cli.rs`, existing `src/ui_client/mod.rs`,
`src/default_open.rs`, `src/main.rs`; parent resolves root/export collisions.
Avoid changing live `src/ui_client/state.rs`/`net.rs` initially.

**Proposed APIs:** `default_open::recovery_view(previous:bool)->i32`,
`ui_client::recovery::run(view:RecoveryView)->Result<(), HostError>` and pure
`RecoveryUi::handle_local_key/compose(&mut Compositor, ...)`. Reuse the
existing host guard/terminal restoration and `Compositor::render` with
`ScreenSnapshot::load_checkpoint` only when checkpoint and leaf dimensions
match. Stage a fixed placeholder for mismatch/missing/invalid sources;
render normalized saved rows in a bounded local scroll view via safe cell
presentation rather than emitting stored bytes. Focus none and hide any
historical cursor in the viewer; no live `Net`, input queue, clipboard,
context menu or split. Reflow only display geometry on host resize; leave
saved topology/PTY sizes intact. Dispatch `recovery view [--previous]` before
`default_open::open/prepare` and reject extra/duplicate flags (exit 2).
User-selected command spelling is a Stage 0 decision.

**Proposed tests:** use synthetic checkpoint with visible styles/wide cells
and apparent OSC 52/control text to assert renderer output contains no raw
stored control sequence/clipboard call; two-pane spatial placement, normal vs
alternate labels/times, row gap/truncation notices and independent errors;
undersized/zero-sized/oversized host, checkpoint mismatch placeholder,
resize redraw, scrolling clamps, quit without effects, short/failed frame
write invalidates baseline and restores termios. Fresh-process CLI fixture
must prove no server PID/socket/identity counter/child/root creation, and
current/previous selection without mutation. Run focused lib filter and
`cargo test --locked --offline --test p5_recovery_cli`; host `/dev/tty`
acceptance must be reported blocked if unavailable, never counted as passed.

## Stage 4 — explicit start/admission gate (parent architecture first)

**Owned files:** `src/multi_startup.rs`, `src/multi_runtime.rs`,
`src/server/multi_session.rs`, `src/default_open.rs`, new
`tests/p5_relaunch_start.rs`; parent exclusively coordinates any shared
`src/p3_cli.rs`, `src/main.rs`, wire schema or server storage edits. Do not
begin until Stage 0 live-server/launcher choices and typed request schema
are approved. Existing `StartedMultiServer::start(StartupLaunch)` launches
one initial child; implement a distinct explicit-start path to obtain a new
generation without implicitly launching that child. Never fake this by
launching then closing a shell. Require held lifecycle/singleton exclusion,
validated saved slot/sequence, fresh client context and explicit per-pane
argv/profile; resolve with `launch.rs` and `managed_launch_prepare` without
reading stored cwd/env/argv. New owner mutation allocates new full targets
and uses current feasible geometry. Expose per-pane admitted key, new target
and `Applied`/`Partial`/`Unknown` status; input/signal/paste cannot be
retried. Refuse existing live server until a user-selected policy exists.

**Proposed tests:** fresh generation/full new targets, same storage workspace
scope, no default extra shell, stale selected sequence and capacity refusal
before effects, two different client env/cwd contexts, secret redaction,
identical key duplicate and changed payload conflict, lost acknowledgment
status query without second spawn, retained first child on second-pane
failure, Unknown without speculative retry, no old target dispatch. Run
`cargo test --locked --offline --test p5_relaunch_start`. No provider
conversation resume in this stage.

## Stage 5 — bounded historical links (separate worker after stage 4)

**Owned files:** new `src/recovery_links.rs`, new
`tests/p5_recovery_links.rs`; parent owns `src/lib.rs`, any
`src/server_storage.rs` stream guard and Stage 4 wiring. Freeze format and
retention only after Stage 0 user/parent review. Propose
`LinkStore::record(bound_generation, source: SavedLayoutKey,
old:TargetRef, new:TargetRef, admitted_key:OperationKey,
outcome:VerifiedOutcome)->io::Result<LinkReceipt>` and read-only
`links_for(source)` with fixed-size/bounded encoding, no secrets, no
authority to dispatch. Require exact old source membership, distinct new
generation, verified owner target, held singleton and separate exclusive
link-stream claim; atomic replacement/sync, corruption refusal and
idempotent same-key/same-bytes retry. Failure after launch reports
`link not saved`; no rollback or fabricated association. Require a reviewed
policy for multiple relaunches and rotation/clear; do not overclaim
cross-stream or spawn atomicity.

**Proposed tests:** mapping for one/two panes, skipped pane, duplicate vs
conflict, link failure before/after rename, unsafe storage/corruption, link
retention cap, generation/target mismatch, retry reconciliation, no
cwd/env/args/provider/raw history in encoded bytes, no dispatch from link.
Run `cargo test --locked --offline --test p5_recovery_links`.

## Stage 6 — opt-in surface and integrated acceptance (parent)

**Owned files:** `src/main.rs`, `src/default_open.rs`,
`src/ui_client/recovery.rs`, `tests/p5_relaunch_cli.rs`, `README.md`,
`docs/SPEC.md`, `ROADMAP.md`, `docs/progress/SESSION.md` and final evidence
file under `docs/progress/`; coordinate Stage 3/4 owners before editing.
Enable `recovery relaunch` or viewer-local confirmation only after Stages 4/5
pass and the user chooses the interface. Present actual per-pane outcomes,
historical links, missing/invalid source labels, and no automatic provider
resume. Verify default `panelink` still attaches/starts fresh normally and
rotation preserves `previous`. Synthetic reboot/restart via isolated private
test root verifies historical topology/history remain readable after new
generation; no claim of physical-machine reboot unless actually exercised.
Run focused lib/integration tests, then
`python3 scripts/verify_non_ui.py` with offline/locked Cargo environment and
`python3 scripts/check_docs.py`; record command output, counts, ignored and
blocked tests separately. Phase 4 live OpenCode identity switching, Apple
Terminal host acceptance and representative workload require independent
evidence before advertising full recovery.

## Design-only verification for this handoff

Only `python3 scripts/check_docs.py` is required for these two Markdown
files. Its actual output belongs in the worker report; tests listed above
are proposed, not executed by writing this document.
