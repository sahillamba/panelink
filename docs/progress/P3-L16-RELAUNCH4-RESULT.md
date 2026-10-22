# L16 result: four-pane saved-layout relaunch

Date: 2026-10-22. Worker: GPT-6-Luna (xhigh) via
`~/.opencode/bin/opencode run -m tesco-litellm/gpt-6-luna --variant xhigh`.
The parent stopped the worker at its 60-minute timebox, while it was stuck on
a test fixture the codec rejected, and finished the slice. The worker wrote no
result file; this one is parent-written.

## Delivered (worker + parent)

- Worker: `RecoveryView` exposes saved pane rects. A pure
  `RelaunchTopology` is rebuilt from rects and dividers.
  `relaunch_split_plan` emits Right/Down steps that preserve spatial order.
  `recovery_relaunch` loops over the steps: committed revision, re-query,
  fresh key per split, stop on the first failed or uncertain step, with no
  retry. Matching and links are generalised to N panes, and the plan has unit
  tests for 1-4 panes including a nested T-junction.
- Parent fix 1 (codec): `saved_layout::encode` wrote axis byte 0 for 3- and
  4-pane records, which `decode` rejects (it requires 3). Every 3- and 4-pane
  saved layout therefore failed to round-trip. Encode now writes 3 when there
  are more than two panes.
- Parent fix 2 (relaunch): the server scopes each new lease to one pane, and a
  split may only name the caller's own pane. Splitting a pane created by an
  earlier step was therefore rejected (`server_rejected`). Relaunch now opens a
  connection scoped to that source pane through the Hello attach-target
  trailer (`RelaunchPeer::connect_scoped`).
- Parent test fixes: the 3- and 4-pane fixtures are built by replaying real
  splits through `PaneLayout`, and expected spatial orders were corrected
  (7, 9, 8 and 7, 9, 8, 10).

## Checks

`cargo test --test p5_recovery_relaunch -- --test-threads=1`: 7 passed,
0 failed, in two consecutive runs.

## Limits

- Split ratios are always 1:1 because no ratio operation exists, so a saved
  tree with non-equal proportions is matched by topology, not exact rects.
- The relaunched root uses the server's default size, not the saved
  rows/columns.
- The CLI-side link writer still holds only the lifecycle lock (existing
  limitation).
