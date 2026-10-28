# Interactive pane controls

Implemented in source on branch `t3/panelink-ui-ux-plan` (UX-02..UX-07,
uncommitted, parent-reviewed, private-PTY tested). Host acceptance (Apple
Terminal, real clipboard, touch) has not been run for this redesign. Rebuild
and reattach to display updated controls; [CLI workflows](cli.md) describes the
same source baseline. Resolve the selected binary before explaining support.

Input ownership is per admitted batch: the UI holds no idle-pane claims, and a
short batch lock does not protect a human's unfinished draft between keystrokes.
Legacy retained CLI claims still block other writers. See the
[CLI ownership workflow](cli.md#authorized-input).

The client version (for example `v0.1.5`) is shown in the bottom-right pane
margin, matching `--version`; it does not identify the running server. The
render area is bounded to 80 rows by 240 columns, so host size and child PTY
size can differ; discover pane dimensions through the matching executable's CLI.

## Focus chrome

- The focused pane gets a **bold/cyan header** and an ASCII **`>`** focus marker after `[Menu]`.
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

## Shared pane menu

`[Menu]`, **right-click**, and **Ctrl-B `s`** all show the same `PaneActionModel` with identical:

- captured target/revision,
- action **order** and **labels**,
- **enabled/disabled** state and reasons.

The four split directions are separate rows; an unavailable direction stays
visible with its reason (size, four-pane capacity, disconnected). Order:
Split Right/Left/Up/Down → Resize divider → Copy mode / Back to live → Close
pane → Zoom/Unzoom → Swap pane → Saved layout → Shortcut help → Detach. Short
viewports page with Back/More; Cancel stays reachable. Clicking a disabled
item selects it and shows its reason.

- Opening the menu (press or click) **never activates** an item.
- Click **outside** dismisses and is **consumed**; it cannot affect content behind it.
- **Esc**, focus loss, host resize, or layout/route change cancel the menu.

## Divider resize

- A divider drag **previews** locally.
- Release applies **one absolute resize**; if the position is unchanged, **zero** effect is sent.
- Host/layout/route changes, Esc and focus loss cancel the gesture; a later
  Enter cannot commit a cancelled value. Menu → Resize divider and `Ctrl-B e`
  give keyboard adjustment with the same limits. Never resend an uncertain
  mutation with a new key.

## Tabs (UX-05c + UX-06)

`panelink` with no arguments opens the **selected visible tab** with the project/tab row at the top.

- **Zero visible tabs** shows an interactive **empty state** with clickable `[ New tab (N) ]` and `[ Detach (Ctrl-X) ]`. No process is started. On a non-TTY it prints a notice and exits.
- **`+`** (header button or Ctrl-B `+`) creates one managed **"Shell N"** tab and switches to it.
- **Capacity**: eight tabs per project.
- **Overflow**: if tabs do not fit, `<>` previous/next controls and an **All** button open a searchable chooser. Active tab and `+` remain reachable.
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
- Pane close (`[x]`, menu, `Ctrl-B x`) opens a confirmation naming the captured
  pane with "Ends this process" (the last pane explains the workspace ends) and
  clickable `[Close]` `[Cancel]`. Cancel is selected by default: Enter on the
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
