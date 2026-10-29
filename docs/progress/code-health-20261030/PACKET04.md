# Packet04: pane interaction ownership

Parent implementation against8ba6672. Parent transition review is complete. Parent owns state.rs/state_tests.rs and private copy/interaction children exclusively; the reviewer may write only its report.

## Existing event priority (before extraction)

1. Discard fresh alternate-wheel intents for non-wheel/modal events; consume matching menu release, pane-mode exit and copy exit latches. Focus loss/resize cancels divider mode.
2. Empty-workspace local handling; selection invalidation for modals, ordinary key/paste/wheel, focus loss and resize.
3. Global detach (including route-freeze local detach), then route-freeze consumes all remaining events while observing local resize.
4. Workspace tab dialog, pointer scrollbar/divider capture, pane swap/divider mode, actions menu, help, pane-close confirmation, legacy split menu, workspace picker, header strip, copy mode, ordinary navigation/input/pointer routing, in that order.

The navigation list in ticket04 is illustrative; the above source order is authoritative. Copy-mode pane chrome clicks retain their special escape into pane controls before copy key handling. Repeats/releases from opening/closing gestures remain local. Existing App/effects tests remain the behavioral surface.

## Resulting boundaries

CopyState owns the frozen selection, caret/anchor, copy exit latch, pending text/target/request, per-source attempts and expiring feedback. CopyContext borrows immutable viewport snapshots and receives current geometry/focus/status; it validates frozen identity without cloning screen caches. Capture, Unicode caret movement, extraction, cancellation, clipboard completion filtering and lifecycle resets now live with the state. App applies dirty/status/prefix feedback and retains cross-domain invalidation triggers. Existing selection.rs remains the capture/reanchor adapter.

Interaction owns captured menus, close-confirmation/default selection/opening key, help/NAV state, pane swap/divider mode and exit/release latches. It handles modal events, action-page and confirmation geometry, layout invalidation, target/revision capture, swap/divider choices, preview clamping and divider pointer previews. It returns captured menu/split/close intents or one pane operation. The coordinator retains admission, request-ID allocation, one-way effect submission, pending-operation reconciliation and ordered routing across owners. Scrollbar pointer capture remains in that coordinator because it coordinates viewport extent discovery and read-only seek; it is not duplicated in a child owner.

There are no whole-mutable-App helpers or production mutable owner views. Named lifecycle/capture transitions remain at cross-domain boundaries. Superseded root fields/behavior were removed. Root state.rs5943→4749 lines; copy.rs588 and interaction.rs1267. Total lines increased due to explicit context/feedback interfaces; the gain is ownership and review locality. Rendering remains in root for packet05. The fixed400-byte menu action model stays on the stack with a documented scoped large-enum-variant lint exception, preserving allocation-free activation.

## Parent checks

Isolated target `/private/tmp/panelink-code-health-20261030-baseline-target`, jobs2, RUST_TEST_THREADS=1, Cargo --locked --offline:

- Full binary323/0, including151 state tests. Log `/private/tmp/panelink-interaction-bin.log`.
- Compositor43/0. Log `/private/tmp/panelink-interaction-compose.log`.
- Private PTY: four-pane2/0, attach-host6/0, workspace tabs5/0. Log `/private/tmp/panelink-interaction-pty.log`.
- Strict all-target Clippy, cargo fmt --check, git diff --check and docs434/0 pass. Logs `/private/tmp/panelink-interaction-clippy.log` and `/private/tmp/panelink-interaction-docs.log`.

The existing scenario assertions are preserved; field paths/imports changed, and one divider-clamp call now uses the owning InteractionContext interface after removing its unused production App forwarder. No new mirror tests were added. Delegated review was incomplete; parent completed the [transition review](INTERACTION-REVIEW.md) and accepted the packet. No independent approval is claimed. Combined validation remains packet08. Prior failure evidence is retained. No live panes, installation, provider/native/reboot/clipboard/performance acceptance or publication.
