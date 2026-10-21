# P5 saved-layout independent review

**Date:** 2026-10-21
**Review scope:** bounded archival layout metadata and the staged read-only restart view in `P5-SAVED-LAYOUT.md`. This review does not authorize or claim implementation, live recovery, or full-reboot acceptance.

## Assessment

The revised design is coherent for the current two-pane prototype and is safe to advance to the pure codec slice. It keeps old targets historical, leaves ordinary `panelink` startup as a fresh-shell launch, and makes the saved view explicitly read-only. The earlier architecture findings are resolved in the revised decision: a higher-generation fresh-shell save archives the prior current record before replacement; an empty record requires positive owner lifecycle evidence; and the storage contract now allows one writer per disjoint metadata stream while keeping a single owner per stream.

The codec handoff is appropriately isolated from storage, async work, CLI dispatch, and runtime construction. The store, writer, owner/driver integration, and CLI stages remain future acceptance gates. The store revision-ordering rule was tightened during this review; no design-text blocker remains for the codec slice.

## Source facts used in the review

- `src/pane_layout.rs` defines a fixed two-pane `LayoutSnapshot` with full `TargetRef` values and spatial rectangles. New layouts start at revision 1; accepted splits, changed root dimensions, and two-to-one collapse advance the revision. A resize at the current dimensions leaves it unchanged (`PaneLayout::new`, `preview_split`, `resize`, `collapse`; lines 77–90, 115–134, 141–156, 220–245, 247–288).
- `src/server/multi_session.rs` commits desired root geometry before applying PTY sizes, so `ResizePending` can leave accepted layout geometry while one or more PTYs still await resize (`resize_layout`, lines 686–720). `finish_pane_exit` records `SessionState::Exited`; it retains the final exited pane as a tombstone and collapses a two-pane layout when one pane exits (lines 1899–1947). This supports the design's distinction between desired saved geometry and confirmed PTY sizes, and between an empty record and the owner's retained final leaf.
- `src/multi_runtime.rs` stops children in `MultiServerDriver::drop` before draining history capture (lines 324–360); `PtyEffects::prepare_driver_shutdown` calls `stop` (src/server_pty.rs:764–774). Therefore shutdown-caused stops cannot be treated as an ordinary last-pane close.
- `ServerStorage::open` advances the generation while holding the server singleton before returning storage (`src/server_storage.rs:252–285`), and `StartedMultiServer::start` uses that generation to create a fresh ledger and launch the supplied startup command (`src/multi_startup.rs:107–149`). No saved layout is currently restored there.
- Current default invocation routes to `default_open::open` (`src/main.rs:88–102`); `ensure_server` starts a server when one is absent (`src/default_open.rs:500–519`). The proposed `layout show` route therefore must stay ahead of default startup, as the revised ADR now requires.
- `src/server_storage.rs` currently has an exclusive history-writer claim and retains a duplicate of the singleton lock for worker lifetime (`:298–333`). The revised ADR and `docs/SPEC.md` now state one exclusive writer per disjoint stream, with separate directory ownership and no cross-stream transaction.

These are source observations only. They do not establish any saved-layout codec, storage, CLI, or reboot behavior.

## Resolved findings

1. **Fresh-shell startup would otherwise erase the only prior layout.** The revised ADR keeps ordinary startup unchanged and adds `current.layout` plus `previous.layout`. Before a higher-generation current replacement, it preserves the validated current bytes in previous and syncs the directory. Exact duplicate archive retry is idempotent; invalid slots fail closed; previous is selected only by an explicit view. Retention is explicitly bounded to the most recent preceding saved generation.
2. **A driver stop could be mistaken for intentional last-pane removal.** The revised ADR requires an owner-serialized observation with `SessionState::Exited` for every target in the last accepted layout, and sets a shutdown marker before stop effects. Unknown, missing, or failed lifecycle evidence preserves the last good record.
3. **The prior single-writer wording conflicted with independent history and layout workers.** `docs/SPEC.md` now makes the stream boundary explicit: one writer per independently owned stream; history and layout have separate directories and claims, and no cross-stream atomicity is claimed.
4. **A full queue could strand the final owner-deferred snapshot ambiguously.** The revised ADR specifies one nonblocking final try-submit before closing admission. If it cannot be accepted, the owner records an incomplete/unsubmitted result and last observed key, without waiting for queue space or calling the candidate submitted.
5. **A read-only command could accidentally autostart a fresh shell.** The revised ADR dispatches `layout show` before default startup and requires a fresh-process test with absent storage/server that proves there is no child launch, directory creation, identity initialization, or generation increment.

## Store revision invariant (resolved)

The revised ADR now states that a changed nonempty candidate at a higher same-generation sequence requires a strictly greater layout revision. Equal revision is limited to the single canonical nonempty-to-empty transition; the empty record may have a greater revision when earlier coalesced changes preceded the final exit. Once empty is durable, another changed candidate in that generation is refused. Byte-identical same-key retry remains idempotent. This matches the inspected code, which advances revision for accepted nonempty geometry/topology changes while final-pane exit retains the last leaf and its revision. It protects against stale or buggy candidates without conflating sequence and revision counters.

The ADR also binds incoming records to the held `ServerStorage::generation`, expected workspace, and exact layout directory. The store slice should test those bindings alongside its previous-before-current fault cases. No design-text blocker remains for the pure codec slice.

## Future acceptance gates

- **Codec:** round-trip canonical empty, single-pane, and split records; reject malformed lengths, versions, reserved bytes, scope/target mismatches, malformed geometry, and any snapshot that would otherwise be normalized. Exercise all four split gestures, spatial orders, odd/even sizes and pane-boundary sizes. The result must claim codec acceptance only.
- **Store and rotation:** test each existing-slot combination, current corruption/unsupported versions, previous corruption, wrong workspace, slots newer than the held generation, missing current with present previous, same-key conflicts, lower keys, same-generation revision rules above, and retry after each archive/current rename and sync boundary. Verify the old current remains intact if archive publication fails and that archive-only success is never reported as the incoming save.
- **Async writer and shutdown:** test coalescing and sequence gaps, one queued plus one in-flight plus one owner-deferred limit, final try-submit success and failure, sticky incomplete state, delayed/failing storage, singleton retention after timeout, and a single shared 250 ms metadata-drain budget. Verify no owner-thread filesystem sync is introduced.
- **Owner observations:** test accepted desired geometry with partial PTY resize, rejected/duplicate operations, final explicit close, natural exits in either pane order, unknown or missing lifecycle state, retained-unknown launch, last exited leaf, empty tombstone at the unchanged layout revision, and shutdown marker ordering. Confirm uncertain lifecycle state never clears the saved nonempty layout.
- **Read-only CLI:** test default/current and explicit previous selection, malformed/unsupported data with no partial stdout, no silent slot substitution, exact historical target display, and dispatch before any autostart/storage initialization path. Verify reading leaves existing file paths, modes, mtimes, and bytes unchanged.
- **Recovery boundary:** the two slots preserve only saved topology, not unsaved last-moment changes, history atomicity, shell processes, terminal emulator state, or provider conversations. `current` means the latest durable saved record, not a guarantee of the layout at the instant of a crash. Full power-loss, host/filesystem durability, UI recovery, explicit fresh-context relaunch, and automatic recovery remain separate work and acceptance.

## Review evidence and limits

This was a read-only source and design review using graph-first symbol discovery followed by direct source inspection. No files other than this new review note were edited by this reviewer. No tests or runtime acceptance were run as part of this review, and no storage, writer, CLI, or restart behavior is claimed as implemented. Parent-reported pane-layout and owner test results remain parent evidence, not review execution.
