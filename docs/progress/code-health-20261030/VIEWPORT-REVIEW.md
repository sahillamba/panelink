# Packet02 viewport extraction review

## Result

No material behavior regressions found in the scoped `state.rs` and new `state/viewport.rs` extraction. The extracted transitions match the pre-extraction implementation at `ebbe798` for the reviewed state mutations and ordering.

## Review coverage

- Compared the complete scoped diff against `git show HEAD:src/ui_client/state.rs`; confirmed `HEAD` is `ebbe798beaf5d19191f870c5ebca9731e359718a` and reviewed the viewport owner implementation directly.
- Verified workspace-route and resize invalidation clear pending work and advance each view epoch, while disconnect only discards queued alternate-screen wheel intents. Layout reconciliation preserves target identity and resets history state when a retained target's rectangle changes or the target disappears.
- Verified scroll completion rejects stale target/epoch replies and clears the alternate queue for a stale reply to the same target. The new `finish_extent` result preserves the old early returns: stale replies and invalid row-wrap history snapshots skip `finish_unknown_extent_capture`; accepted result branches run it.
- Checked history checkpoint decode/load and row-wrap failure paths, including status, dirty state, queue clearing, cursor epoch/truncation handling, effective offset arithmetic, and history revision updates against the old branch logic.
- Checked live-screen checkpoint admission order: slot selection happens before loading, but target/revision and associated bookkeeping are committed only after checkpoint and row-wrap loading succeed, preserving the old behavior on errors. Buffer-change cancellation and cursor rollover/RIS handling are retained.
- Checked alternate-wheel FIFO capacity 32, one-second age check, target/epoch/screen validity checks, pop-before-send and failure discard. No queue reinsertion was added. The global pending-history gate and priority order remain: alternate wheel, seek, normal scroll; expiry can remove the front intent and return before trying lower-priority work on that flush.
- Checked immutable production slot access, test-only mutable indexing, dirty propagation, and status application from returned viewport changes.

## Verification and limits

Recorded parent evidence: UI state tests 151/0, full binary tests 323/0, strict all-target Clippy passed, and p3_scroll 3/0. No tests, builds, runtime sessions, or panes were run by this review. The reported p5_workspace_tabs result (4 passed / 1 failed at a missing Close control, partial blank/root shell screenshot, log `/private/tmp/panelink-viewport-pty.log`) remains unresolved; this review did not attribute it to a fixture flake. In the scoped extraction, no moved route, modal, or render transition was found that explains the missing Close control. Parent's focused `track_caller` diagnostic is still needed to identify its origin. This packet is not accepted on this review alone.
