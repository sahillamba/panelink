# BTN-02e parent tab-strip capture

Parent retained worker BTN02d keyboard subset after source review. It adds local
Tab/ShiftTab editor/Save/Cancel focus and Press-only action shortcuts; worker
state161/0 and full private target5/0 remain recorded in BTN02d, not claimed as
pointer acceptance.

Parent implemented StripPress and capture_strip_event in App. It captures the
exact TabAction, entire bounded TabsLayout, catalog revision, route, host and
focus generation; Down has no action and Up validates the same semantic target
once. Drag-away/wrong-button/Escape/catalog/layout/route/focus/host invalidation
cancels. It runs in both normal root routing and empty host entrypoint without
requiring a live layout/network worker. Existing modal/header captures preserved.

Actual parent gates: full state suite **162 passed/0 failed**, strict all-target
Clippy passed, fmt/diff checks passed. Logs under logs/btn02e-parent-*. New raw
strip regression passes. Initial state159/2 failures retained: two old fixtures
assumed Down activation; explicit Up added, preserving all assertions. Empty
header-plus fixture now asserts no create request before release.

Not complete BTN02: strip pressed presentation, Back to live/empty action body
capture, tab dialogs and chooser rows still need work. No private PTY run for
this parent slice yet; next owner must run full p5_workspace_tabs. No install,
user-pane operations, server restart, commits or push.
