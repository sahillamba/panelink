# Interactive pane controls

Version gate: these controls are implemented in the user-selected
`/Users/sahillamba/.codex/worktrees/7060/panelink` checkout on 2026-10-27.
Check its `docs/progress/P5-POINTER-CONTROLS-2026-10-27.md` for final verification
and build availability. The development client must be rebuilt/reattached to
show them; this does not replace a separately packaged release binary.
[CLI workflows](cli.md) describes the same consolidated source baseline.
Resolve the selected binary before explaining support.

Input ownership is per admitted batch: the UI holds no idle-pane claims, and a
short batch lock does not protect a human's unfinished draft between keystrokes.
Legacy retained CLI claims still block other writers. See the
[CLI ownership workflow](cli.md#authorized-input) for the confirmed source
contract and its pending runtime acceptance.

Local build 0.1.2 displays `v0.1.2` in the bottom-right pane margin. This is
the client version, matching `--version`; it does not identify the running
server. Modal notices can temporarily cover it. Unpadded saved-layout views
do not carry a badge. Detach and reattach to display a rebuilt client.

Normal-history wheel reads return the requested viewport without first probing
the oldest row solely to size the scrollbar. Until an explicit seek discovers
the full range, the thumb uses a conservative extent. A first track interaction
with an unknown extent discovers that range. Background polls are not newly
queued while a scroll is outstanding; a poll already in flight can still delay
it. Alternate-screen app scrolling retains its fresh mode/ownership checks and
is not accelerated by replaying or batching uncertain input.

Its render area is bounded to 80 rows by 240 columns, including in larger host
windows. Host size and child PTY size can therefore differ; discover the current
pane dimensions through the matching executable's CLI.

When the selected client renders `[Menu]` and `[x]` in pane top margins:

- Menu exposes pane actions with mouse and keyboard routes. `Ctrl-B s` opens
  the actions menu; Back/More/Cancel keep actions reachable in short viewports.
  Right-click retains the existing split context menu.
- `[x]` requests close for that exact pane. A separate Close click or `y`
  confirms; Cancel dismisses. Closing terminates work, while detach preserves it.
- A divider drag previews locally and applies one absolute resize on release.
  Menu → Resize divider also offers minus/plus and Apply/Cancel controls;
  `Ctrl-B e` retains keyboard adjustment. Host/layout/route changes cancel
  stale gestures. Never resend an uncertain mutation with a new key.
- The right-margin scrollbar navigates retained normal-buffer history; clicks
  page and thumb drags seek. This is separate from saved history/recovery and
  alternate-screen application scrolling. Typing returns to live output.
- Pointer gestures require the host terminal to report mouse press/motion/
  release. Touch works only through that translation; native multitouch and
  broad touch-hardware compatibility are not established.

Use CLI/API discovery and exact targets for automation. UI pane position or a
local focus change does not authorize input and is not a permanent target ID.
Keep host acceptance, real clipboard acceptance and private-PTY evidence
separate. Reattaching a newly built client changes presentation while live
processes remain server-owned.
