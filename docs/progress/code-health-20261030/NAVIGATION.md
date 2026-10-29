# Maintainability navigation

Use current source and the [execution state](PROGRESS.md); dated baseline/review reports remain historical evidence.

| Change | Owner | Coordinator boundary |
| --- | --- | --- |
| History reply, cursor epoch, fresh wheel FIFO or seek scheduling | src/ui_client/state/viewport.rs | App supplies modal/route admission and applies notices/selection invalidation |
| Tab metadata completion or unknown-create reconciliation | src/ui_client/state/workspace.rs | Existing routes/metadata workers perform I/O; App freezes cross-domain work |
| Pane confirmation, action menu or swap/divider input | src/ui_client/state/interaction.rs | App allocates/adopts one operation and dispatches the captured intent |
| Frozen selection, Unicode caret or clipboard completion | src/ui_client/state/copy.rs | Immutable CopyContext validates current view; App supplies invalidation triggers |
| Frame assembly and overlay priority | src/ui_client/state/render.rs | Immutable RenderView; host retains output acceptance and compositor commit |
| Pane chrome/scrollbar geometry and frame damage | src/terminal/compose.rs | Same pure geometry remains shared with hit testing |
| Host event processing and intent drain order | src/ui_client/host.rs | One dispatcher, existing host budgets/restoration |
| Server ownership/split/runtime helper scenarios | src/server/multi_session/{tests,split_tests,runtime_helper_tests}.rs | Production authority remains in multi_session.rs |

The App/effects scenarios remain in src/ui_client/state_tests.rs. No duplicate replacement suites or arbitrary file-size limits were added. The server core and compositor remain coherent larger modules; further production separation needs an ownership design, not a line-count target.

Structural extraction changes source navigation, not supported commands or wire/storage contracts. Build a selected checkout explicitly into an isolated target; this work does not replace an installed client or restart its server. Keep native/provider/performance acceptance distinct from the canonical non-UI gate.
