# Interactive pane controls

Button redesign is implemented in this source checkout, branch
`t3/terminal-button-design-research`; [execution evidence](../../../../docs/progress/button-redesign/PROGRESS.md)
tracks automated checks and remaining acceptance. Resolve the selected executable
before explaining support. Building this checkout does not install it or restart
its server. Native Apple Terminal, real clipboard, touch and host-theme acceptance
remain separate from source/private-PTY verification.

## Buttons and local keyboard focus

- Controls have filled resting backgrounds and distinct focus/pressed/disabled/
  pending states. Informational titles, filters and status text remain plain.
- Press and release in the same valid control to activate once. Drag-away,
  Escape, lost focus or stale target/layout/route/catalog/modal cancels.
- Menus use arrows/Enter and local Tab/Shift-Tab; dialogs cycle local controls
  with Tab/Shift-Tab and Escape cancels. Rename edits text only in its editor.
  Ordinary shell Tab/Space retains its existing meaning.
- Close confirmations initially focus Cancel and identify their process-ending
  consequence. Pending Save/Close prevents another mutation; uncertain outcomes
  retain their existing explicit reconciliation rules.
- NO_COLOR presence (including empty) or TERM=dumb chooses reduced control
  colors once at setup. Reverse fill and attributes distinguish controls without
  a special font; this is not terminal-native theme detection.

Input ownership is per admitted batch: the UI holds no idle-pane claims, and a
short batch lock does not protect a human's unfinished draft between keystrokes.
Legacy retained CLI claims still block other writers. See the
[CLI ownership workflow](cli.md#authorized-input).

The client version (for example `v0.1.6`) is shown in the bottom-right pane
margin, matching `--version`; it does not identify the running server. The
render area is bounded to 80 rows by 240 columns, so host size and child PTY
size can differ; discover pane dimensions through the matching executable's CLI.
With the project/tab strip (plain `panelink`), the top two host rows are the
strip and a blank spacer, so the pane root is two rows shorter than the host.
From 120 columns the strip uses wider margins and gaps; panes at least 40
columns wide inset their Menu and close controls two cells from the edges.

## Focus chrome

- The focused pane gets a **bold/cyan header** and an ASCII **`>`** focus marker after the filled Menu control.
- A **contrasting padding rail** around the pane border distinguishes it from unfocused panes.
- Child terminal text is **not dimmed** in unfocused panes; only the header/border chrome changes.

## History and scrollbar

- **Wheel** and **scrollbar** gestures never move typing focus.
- **Thumb drag** updates the visible history **continuously** while held (one read in flight; latest position wins).
- While a pane shows history, new output does not move the reading position.
- The bottom margin shows one of:
  - `History`
  - `New output`
  - `History truncated`
- A clickable **`[Back to live]`** is shown pane-local for each pane in history.
- **Typing** in the focused pane returns it to live and sends the key once.
- Track clicks page; the thumb seeks. Until a seek discovers the full range the
  thumb uses a conservative extent (no oldest-row probe before every wheel read).
- Normal-buffer history is separate from saved history/recovery and from
  alternate-screen application scrolling, which keeps its mode/ownership checks.
- For a known alternate-screen view, wheel events go through a direct typed
  server request, with no preliminary history read. While one is in flight,
  up to 32 fresh unsent events per pane are kept FIFO for at most one second;
  direction and coordinates are preserved, and overflow drops new events.
  Typing/paste, non-wheel mouse input, navigation/overlays, resize, target,
  layout or mode changes, disconnect, and failed/stale results cancel unsent
  events. The server rechecks target, mode and ownership on every delivery.
  When capability1024 (`scoped_mouse_wheel`) is negotiated, every admitted
  event uses one durable keyed vacancy-scoped operation. Applied confirms PTY
  delivery and scoped release; other outcomes do not confirm release. A retained
  same-caller claim is refused too. Old servers use a fresh mode query and a
  vacancy-only claim/send/release sequence; no-mouse views do not claim. New
  server support is required for the fast path; a client update alone is not
  enough. Server restart is separate because it terminates existing panes. Unknown, partial or
  uncertain delivery is never retried, and an uncertain claim is not released.
  A delivered PTY event does not prove the application acted on it. Screen polls
  may continue during these events; normal-history poll suppression is unchanged.

## Shared pane menu

`Menu`, **right-click**, and **Ctrl-B `s`** all show the same `PaneActionModel` with identical:

- captured target/revision,
- action **order** and **labels**,
- **enabled/disabled** state and reasons.

The menu is one page (no Back/More/Cancel). It opens at the right-click cell,
below the pane's Menu button, or centred for Ctrl-B `s`. Rows: **Split** with
Left/Right/Up/Down on one row (one-letter L/R/U/D on narrow views), Zoom pane,
Swap pane, Copy mode (Back to live in history), a separator, and Close pane.
Up/Down move between rows, Left/Right between split directions, Tab through
every item; `l`/`r`/`u`/`d` split directly and `v` still opens the saved
layout. An unavailable item stays visible and dimmed; selecting or clicking it
shows its reason in the bottom border (or bottom row on small views).
Resize divider, Saved layout, Shortcut help and Detach are not pane-menu rows:
use divider drag or Ctrl-B `e`, and the strip's `≡` menu.

- Opening the menu (press or click) **never activates** an item.
- Click **outside** dismisses and is **consumed**; it cannot affect content behind it.
- **Esc**, focus loss, host resize, or layout/route change cancel the menu.

## Divider resize

- A divider drag **previews** locally.
- Release applies **one absolute resize**; if the position is unchanged, **zero** effect is sent.
- Host/layout/route changes, Esc and focus loss cancel the gesture; a later
  Enter cannot commit a cancelled value. `Ctrl-B e` gives keyboard and
  `[-]`/`[+]`/Apply pointer adjustment with the same limits. Never resend an uncertain
  mutation with a new key.

## Tabs (UX-05c + UX-06)

`panelink` with no arguments opens the **selected visible tab** with the project/tab row at the top.

- **Zero visible tabs** shows an interactive **empty state** with clickable `[ New tab (N) ]` and `[ Detach (Ctrl-X) ]`. No process is started. On a non-TTY it prints a notice and exits.
- **` + `** (right after the last visible tab) or Ctrl-B `+` creates one managed **"Shell N"** tab and switches to it.
- **`≡`** (pinned right) opens the global menu: All tabs, Saved layout, Keys & help, Detach.
- **Capacity**: eight tabs per project.
- **Tab row**: project left, filled tabs with separate close targets in the
  middle, `+` after them and `≡` pinned right. Names shorten before controls
  disappear. Previous/next buttons appear only when tabs overflow (30 or more
  columns); All tabs in `≡` and Ctrl-B `w`/`n`/`p` also reach hidden tabs.

- **Scrollbar**: the right-margin column stays blank until the pane has retained history; then a dim track and `#` thumb appear.
- **Tab right-click** or Ctrl-B `t`: New tab, Rename, Close tab.
- **Rename**: right-click or Ctrl-B `t` → Rename opens a text field with Save/Cancel. Enter saves, Esc cancels. On conflict the edit is preserved and an error is shown.
- **Close tab**: confirmation with **Cancel** as the default. Close ends the tab's processes. The last tab closed returns to the empty state.
- Closing a tab requires it to be **open** (the active route). An inactive tab shows: "open this tab first to close it".
- **Key bindings** (from `keys.rs`):
  - `Ctrl-B +` — create new tab
  - `Ctrl-B t` — tab actions (rename/close) for the active tab
  - `Ctrl-B w` — open All tabs chooser
  - `Ctrl-B n/p` — next/previous tab
  - `Ctrl-B [/]` — next/previous project

## CLI workspace tab commands

```sh
panelink workspace tab rename <project-id> <tab-id> "New name"
panelink workspace tab close <project-id> <tab-id>
```

`rename` changes display metadata only (revision-checked). `close` stops the route's processes, verifies the stop, then removes the tab from the open list.

## Copy, close and status feedback (UX-07)

- Selection is unchanged (drag-release or keyboard copy mode). After the
  clipboard confirms success, the source pane shows `Copied` for about 1.5 s.
  Failures show `Copy failed`, `Clipboard unavailable` or `Copy too large`,
  never `Copied`. Copied text is never shown in labels, status or logs.
- Pane close (`x`, menu, `Ctrl-B x`) opens a confirmation naming the captured
  pane with "Ends this process" (the last pane explains the workspace ends) and
  clickable `[Close]` `[Cancel]`. On views of at least 11 rows and 44 columns
  it is a centred, bordered, padded box (up to 52 columns) with centred buttons
  and a key hint; smaller views use a compact two-row prompt at the bottom. Cancel is selected by default: Enter on the
  default cancels; Left/Right/Tab move the selection; `y` confirms explicitly;
  `n`/Esc cancel. The opening click/key and key repeats cannot confirm. A layout
  or occupant change cancels the prompt; it never closes a replacement.
- Persistent states: `Applying…` while a mutation is pending, `Outcome unknown`
  after an uncertain close/operation, and `Disconnected` (mutations disabled)
  until authoritative state reconciles. Transient success never hides them. A
  confirmed server end still detaches the client.

## General notes

- Pointer gestures require the host terminal to report mouse press/motion/release.
- Touch works only through that translation; native multitouch compatibility is not established.
- Keep host acceptance, real clipboard acceptance, and private-PTY evidence separate from source claims.

## Separate tab-close targets

The project selector is left, tabs occupy the middle with `+` after them, and
the `≡` global menu stays right. Each visible tab has a disjoint close target. Clicking close never selects
a different tab. An inactive tab reports `open this tab first to close it` until
its runtime generation can be bound by selecting it. Rename is in Tab actions;
active-tab click and right-click retain menu access. The final tab leaves an empty
workspace with New tab/Detach and no automatically spawned replacement shell.

Help's filled close row follows the same release/cancel rule. The startup
saved-layout prompt remains line mode, and the offline recovery viewer remains
key-only; key hints on those surfaces are not newly clickable buttons.
