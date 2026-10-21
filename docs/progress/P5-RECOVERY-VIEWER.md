# ADR-P5-RECOVERY-02: historical viewer and explicit relaunch links

**Status:** Proposed engineering contract for review; no recovery viewer, link
store or relaunch implemented. **Date:** 2026-10-21. **Scope:** the existing
single default workspace, at most two equal-split panes. Product entry points,
launch defaults and hierarchy are pending user choices.

## Context and invariants

The implemented [layout slice](P5-LAYOUT-SLICE.md) writes a validated 160-byte
current/previous record and exposes `panelink layout show [--previous]` without
starting a server. `SavedLayout::geometry()` derives spatial historical targets;
`LayoutStore::load(slot)` validates only the requested slot. History has separate
bounded normalized normal rows and latest normal/alternate screen checkpoints,
loaded by exact target; `history show` is text output, not a frame source.
`ScreenSnapshot::load_checkpoint` can populate the existing compositor's
`PaneContent::Snapshot`, but only at matching dimensions. `default_open::open`
currently prepares/starts a fresh shell if needed, and
`StartedMultiServer::start` takes explicit `StartupLaunch` arguments/context.
The owner has no historical-pane constructor, and client focus is local.

Historical topology is the **last durably saved desired** layout, not the
current-at-crash PTY geometry. Rows may lag or have gaps, screens can be absent
or stale, and two files have no shared commit boundary. Never interpret a saved
target as a live target, terminal input, a launcher or a provider conversation.

## Proposed read-only recovery viewer

Add an explicit terminal viewer selectable for `current` or `previous` (default
only `current`, no silent fallback). Dispatch its CLI route before `prepare`,
`ensure_server`, `ui_client::run`, identity allocation, storage cleanup or any
launch. Open layout and each pane's history read-only, via the existing private
directory/regular-file checks. Validate the selected layout fully before host
output; inspect each exact historical `TargetRef` independently and bind each
decoded checkpoint's embedded target to that pane. Never pick newest history by
time, pane number, project or PID. If the layout is empty, show an explicit
empty-workspace state with no invented panes. If a pane's history is missing,
capture-disabled, pruned, cleared or invalid, keep the valid topology visible
and label that pane's source independently. Preserve a fixed sanitized error
for an invalid requested layout; valid `previous` remains separately readable
if `current` is corrupt. Never repair/remove files in this view.

Display the saved spatial order and orientation with a historical banner:
slot, generation, layout observation time/sequence and "not running". Show
each pane's full historical target, bounded normal scrollback as a separate
scrollable history region, gap/reset/truncation and capture-disabled markers,
and latest normal and alternate checkpoint as *distinct* labeled buffers with
their capture times/output sequences. Never present alternate redraws as
conversation lines or conflate saved rows with screen cells. A screen is a
checkpoint, not a running TUI; historical focus/cursor/input mode is not
restored. Favor a normal checkpoint for initial inspection, with an explicit
buffer toggle when alternate exists; this presentation preference is provisional.
No checkpoint means a placeholder rather than a reconstructed screen from rows.

Derive display rectangles from the saved axis/spatial order using the existing
equal-split geometry rule, recomputed against the *current host content viewport*
after viewer chrome, bounded by the supported 80x240 root and 5x20 leaf limits.
Do not treat the saved root dimensions as a host resize or a requested PTY
resize. A checkpoint may have different dimensions from the display leaf:
only pass a matching-size `ScreenSnapshot` to `Compositor::render`; otherwise
show a labeled fixed placeholder and expose the original checkpoint in a
separate bounded, non-reflowing inspection mode once dimensions fit. Do not
silently resample/crop cells and present them as the saved screen. If the host
cannot fit the saved pane count plus chrome, show one clipped, bounded
"terminal too small" status, keep the loaded topology/history intact, and
retry presentation on host resize. No PTY is created/resized. On zero-sized
hosts render nothing and preserve viewer state. Reuse the host guard and
frame-commit-on-full-write behavior; output only Panelink-generated drawing
controls and sanitized decoded cells. Stored text, ANSI-looking sequences,
OSC 52, query bytes and provider output are never sent raw to a host or child.

Viewer input is local navigation (select pane, scroll rows, switch normal/
alternate checkpoint, exit) only. Use a separate viewer state/handler instead
of the live `App`'s `Net`, key batching, split/close actions, menu or paste path.
No socket is contacted, no user input is dispatched to a historical target, no
approval/input/clipboard requests are replayed. An explicit local Copy action,
if eventually offered, needs its own reviewed policy; the initial viewer does
not access the system clipboard. Terminal restoration applies on error/exit.
Bound display memory to two snapshot caches, up to two saved pane histories
(current per-target limits: 1,024 rows/8 MiB plus bounded checkpoints) and a
fixed frame buffer; cap scrolling/indexing and check all arithmetic.

## Explicit relaunch and historical link contract

Relaunch is a **new user-initiated operation**, separate from inspection and
from the plain `panelink` opener. It must begin with the user selecting a slot
and exact saved snapshot sequence/targets and explicitly selecting a fresh
launcher for each requested pane (or explicitly opting to skip a pane). The
invoking client supplies a freshly captured absolute cwd, volatile environment,
executable and arguments/profile selection at that invocation. No historical
cwd, env, argv, command text, provider ID or terminal content exists in the
layout and none may be recovered from history, title, journal or daemon ambient
environment. Apply `launch.rs` precedence and `managed_launch_prepare` policy;
reject unavailable/oversized contexts before effects with redacted errors.
Shell defaults, per-pane prompts and reuse of one launcher are pending choices;
the prototype must not silently infer a replacement OpenCode command.

Before implementing the operation, add a dedicated **explicit-start mode**
that can establish a new generation without `default_open`'s automatic first
shell. Under lifecycle/server singleton exclusion, refuse an already-running
workspace unless the user-selected live-server policy is separately designed;
never stop a live workspace to make room for recovery. Preflight selected
layout revision/sequence, geometry, launch choices and resource budget. Allocate
new pane/session IDs and complete `TargetRef`s in the new generation through
the owner, not by copying the saved IDs; spatial order and desired topology are
new owner mutations, not restored PTY state. Request dimensions computed for
the current valid host/root, not an unsupported saved PTY size. Each launch is
a durable mutation with one operation key, admission before effects and the
existing `Applied`/`Partial`/`Unknown` distinction. Same-key/same-payload retry
returns recorded results; conflicting payload refuses. On lost acknowledgment,
query status/reconcile without dispatching again or minting a fresh retry key.
No keystrokes, paste, shell command text, prompts or relative resize are
replayed; disconnected input is discarded. If outcome is Unknown, show it and
require reconciliation before another user action, not automatic replacement.

Persist an **old -> new historical link** only after a new target exists as an
owner-acknowledged occupant. Proposed bounded, versioned, separate link stream
key: (storage domain, source slot generation, workspace, snapshot sequence,
full old target, new generation, full new target, operation key). Store only
identities, explicit outcome (`applied`, `partial`, `unknown` when an allocated
target is verified), link verification time and no cwd/env/argv/provider IDs,
input or display text. Link write requires the held generation-bound singleton
and its own stream exclusion/atomic sync; never claim atomicity across a PTY
spawn, layout/history writes and link persistence. A missing or failed link
write leaves the launched PTY alive and reports "link not saved"; a link never
authorizes input or proves a process still lives. On restart, show a link only
as historical association; never follow it to dispatch. On uncertain spawn,
do not invent a link to an unverified occupant. Fix duplicate/idempotent link
policy, retention, source-slot rotation and failure recovery before a format is
frozen; repeated relaunches may produce distinct new-generation links only from
distinct explicit user actions. Do not write a speculative mapping before
owner admission/identity is known.

Provider conversation recovery is a **separate Phase 4 gate**. A fresh generic
OpenCode process is not a conversation resume. Until exact authoritative
top-level identity, in-TUI switches, acknowledgment and ordering are proven,
show at most a time-stamped last-confirmed identity when one exists, otherwise
manual selection. Never pass `--continue`, guess latest session or automatically
resume on relaunch. A future explicit provider-owned picker/ID + project path
must verify and durably save the selected binding before an independent resume
operation; uncertainty remains visible. Saved rows and checkpoints do not
establish provider identity or contain the provider's conversation store.

## Entry points and failure policy

Candidate CLI: `panelink recovery view [--previous]` for offline inspection;
`panelink recovery relaunch [--previous]` only after a separate typed schema
and interactive/explicit launcher-input design are reviewed. Consider a
viewer-local "Relaunch..." confirmation/menu that leaves read-only mode and
invokes that separate command with fresh client context. Neither is enabled
by this ADR. `panelink layout show [--previous]` and `history show TARGET`
remain direct text inspection. Plain `panelink` continues to attach to a live
server or start a fresh default shell when none is running; that fresh save may
rotate current into previous. No automatic recovery chooser, provider resume
or changed default startup without an explicit product decision.

Reject absent/unsupported/unsafe layout and storage with fixed labels; do not
fall back silently. Handle history file errors per pane/source without hiding
healthy data; refuse invalid individual committed files as whole units (no
salvage), even if another source exists. Check source target, epoch, dimensions,
timestamps and bounded file sizes; identify known gaps and never assert
layout/history cross-file consistency. A failed renderer/host setup restores
terminal modes and leaves storage untouched. Relaunch pre-effect failure leaves
old history and any live workspace untouched; partial launches retain acquired
new occupants and show precise per-pane outcomes, with no automatic rollback,
retry or duplicate child. Storage errors never terminate otherwise running PTYs.
Diagnostic messages and persisted links contain no secrets, sensitive paths,
environment values, launch arguments or stored history content.

## Options and pending user questions

| Choice (all pending) | Benefit | Trade-off / question for user |
| --- | --- | --- |
| Explicit CLI viewer only vs in-client Recovery menu | Scriptable/offline vs discoverable | Should the menu be available during a live workspace, and what should it open? |
| Opt-in startup chooser vs unchanged plain startup | Earlier discovery vs predictable fresh shell | Should `panelink` ever offer recovery automatically, or only with a flag? Proposed current default stays unchanged. |
| Two-pane topology view vs one-pane-at-a-time history view | Spatial context vs simpler small-terminal access | Should users prioritize saved geometry or reading one pane when the host is too small? No implicit merge is proposed. |
| Per-pane fresh launcher selection vs single explicit launcher for all panes | Correct per-pane intent vs faster shell recovery | What should the explicit relaunch prompt/CLI require; can users skip individual panes? |
| Relaunch only when server stopped vs attach/create in an existing live generation | Clean fresh generation vs faster reuse | Is creating another workspace/merging desired? Current two-pane owner lacks that policy. |
| Bounded links (last generation) vs longer retention | Small/private store vs historical traceability | How long should links survive layout slot rotation/history pruning and how are they cleared? |
| Manual provider selection vs future verified exact-ID resume | Safe now vs convenience after Phase 4 proof | Which supported provider versions/projects should be eligible, and should any last-confirmed candidate be displayed by default? |

Workspace/tab hierarchy, per-pane launcher defaults, retention, clipboard
policy and firm review criteria remain open in SPEC/discovery. The table records
options, not approved requirements.

## Canonical text proposed for parent review

Proposed `docs/SPEC.md` addition after the scoped saved-layout section:

> A proposed opt-in terminal recovery viewer reads a selected saved layout and
> its exact-target retained histories without creating a server or replaying
> terminal data. Missing or corrupt history is labeled per pane; saved geometry
> and checkpoints are historical, not live PTYs. Explicit relaunch requires
> fresh client-provided launch context and new-generation target identities;
> durable old-to-new links are associations only. Provider conversation resume
> remains gated on authoritative identity, with last-confirmed/manual fallback.
> Entry-point, launcher, retention and hierarchy choices remain pending.

Proposed `ROADMAP.md` next gate: review user choices and this ADR; implement
viewer first, then a separately gated explicit-start/relaunch/link slice with
automated evidence. Keep full Phase 5, reboot and Phase 4 identity OPEN.

## Evidence

This is design only; all scenarios in the [staged task list](P5-RECOVERY-VIEWER-TASKS.md)
are proposed, not executed. No source, runtime, manifest or canonical spec was
changed here. The preceding layout slice's test counts are historical evidence,
not verification of this viewer or relaunch.
