# Panelink button design research

Date: 2026-10-30. Status: research complete; the user selected A, filled buttons,
in the [implementation-plan interview](BUTTON-IMPLEMENTATION-PLAN.md).
The complete plan awaits final shared-understanding confirmation.
Checkout: `t3-baecf477`, branch `t3/terminal-button-design-research`.
Scope: source/document inspection and official online references. No live app
inspection, application changes, installation, or terminal compatibility acceptance.
The [interactive comparison](button-design-options.html) is an illustrative HTML
concept, not a screenshot or a cell-accurate terminal renderer.

## Recommendation

Adopt option A: a compact filled-button system. Preserve top tabs and existing
pane margins; give each action its own visible surface, one-cell horizontal
padding, stable location, and explicit interaction state. Use stronger emphasis
sparingly for the next useful action, such as Back to live or New tab.

This addresses the user's reported problem directly: actions should look
clickable before the pointer reaches them. Brackets and colored text alone are
insufficient visual differentiation. This is a design judgment informed by source
inspection, not a measured usability result.

## Current source findings

- `src/terminal/compose.rs:187`: `pane_controls` reserves six columns for Menu,
  three for close, and one header row. It already centralizes their geometry.
- `src/terminal/compose.rs:1224`: `paint_pane_controls` writes `[Menu]` and `[x]`
  with bold cyan foreground for the focused pane and reset/default styling
  otherwise. Neither control gets an independent filled background here.
- `src/workspace_tabs.rs:199`: tab layout uses bracketed project/tab labels and
  reserves right-aligned controls, including compact forms at narrow widths.
- `src/ui_client/state/workspace.rs:1439`: tab dialogs format `[Save] [Cancel]`
  and `[Close tab] [Cancel]` in text rows. These should use the same button
  presentation rules as pane controls.

The installed executable may come from another checkout. These findings describe
this source tree; the user's running build was not identified or inspected.

## References and transferable patterns

| Reference | Documented pattern | Suggested use in Panelink |
| --- | --- | --- |
| [Warp tabs](https://docs.warp.dev/terminal/windows/tabs) | Explicit new-tab action, tab context actions, mouse and keyboard access | Keep tab creation discoverable and related tab actions together |
| [Warp toolbar](https://docs.warp.dev/terminal/windows/configurable-toolbar) | Distinct header buttons; configurable visibility and placement | Consistent grouping and surfaces; customization is not needed for this first redesign |
| [iTerm2 appearance](https://iterm2.com/documentation-preferences-appearance.html) | Themes for chrome outside terminal content, high-contrast variants, configurable tab close buttons | Separate controls from output visually; preserve child terminal colors |
| [Zellij themes](https://zellij.dev/documentation/themes.html) | Separate text/ribbon/list/frame roles and selected states | Strong semantic style system within a character-cell interface |
| [Windows Terminal palette](https://learn.microsoft.com/en-us/windows/terminal/command-palette) | Discoverable action list with nested command groups | Group secondary actions; a searchable action palette is an optional later feature |
| [Charm Huh](https://github.com/charmbracelet/huh) | Styled terminal forms, confirmation fields, multiple themes, accessible prompt mode | Coherent dialog/button presentation and readable confirmation choices |

These references were researched through official documentation/source examples,
not hands-on comparative testing. Charm Huh is a TUI library reference, not a
terminal emulator. No library or dependency adoption is proposed.

## Three directions

| Direction | Appearance | Strength | Tradeoff |
| --- | --- | --- | --- |
| A — Filled controls, recommended | Rectangular background runs, padded labels, accented primary action | Strong clickable appearance; fits existing single-row controls | Needs careful contrast against light/dark host backgrounds |
| B — Outlined controls | Padded bracket edges in headers; box-drawn outlines in larger dialogs | Quiet terminal aesthetic; structural distinction survives reduced color | More visual punctuation; weaker filled surface cue |
| C — Visible global action bar | A plus one global row for Split, Resize, Copy, Close | Everyday actions are immediately discoverable | Costs one terminal row; focused target must be explicit |

Option B's browser pixel outlines approximate a visual direction. A one-cell
terminal header cannot draw a full three-row box around each button without
changing layout. Use `[ Menu v ]` there instead.

Option C is an alternative requiring reconsideration of the existing quiet-canvas
layout decision. No option adds a permanent sidebar, whole-pane dragging, or new
zoom functionality. Current confirmed requirements remain authoritative until a
new direction is selected.

## Apply the system to every control family

| Area | Proposed treatment |
| --- | --- |
| Project selector | Neutral filled button with label and menu indicator; distinguish project from tab |
| Tabs | Selected tab gets a distinct surface/marker; tab close has its own bounded click target |
| New tab and overflow | Labelled New tab when room allows; `+` compact fallback; All tabs remains reachable |
| Tab rename/close | Shared action menu, clear Save/Cancel and Close tab/Cancel buttons |
| Pane headers | Neutral Menu button and separate close button; focused marker belongs to pane chrome, not button selection |
| Split actions | Clear Left/Right/Up/Down choices; unavailable directions explain capacity or size limits |
| Resize | Separate decrement/increment controls, displayed size/position, Apply and Cancel |
| Copy/history | Copy is an action; Copied is status. Back to live is a prominent action; New output is status |
| Menus and choosers | Full-row targets, selected marker, grouped actions, visible cancel/close route; keep context and target visible |
| Close dialogs | Name the exact target and process consequence; Cancel initially focused; explicit destructive label |
| Saved layout/recovery | Reuse the same button and menu system; keep saved views and process relaunch distinct |
| Detach/help | Secondary action treatment; explain that Detach leaves processes running |
| Last-tab empty state | Prominent New tab and secondary Detach; preserve no-auto-spawn behavior |
| Notices/version/mode | Plain status text unless actionable; do not give informational labels button surfaces |

Keep supported legacy zoom/swap actions reachable in existing menus, with the
same styling, without adding new prominent controls for them.

## Interaction rules

1. Rest: distinct surface and one blank cell on both sides of the label. The
   whole visible button, including padding, is the hit target.
2. Keyboard focus: extra marker or reverse-video/border treatment; distinct
   from the currently active tab and the pane receiving terminal input.
3. Pressed: immediate local visual feedback; activate once after a valid release
   in the same target. Cancel a press that leaves or loses its target.
4. Unavailable: readable subdued style plus a discoverable reason; never rely
   on dim text alone or allow activation.
5. Pending: truthful Saving/Closing label and duplicate activation prevention.
   Preserve uncertain-result reporting; do not automatically retry input.
6. Destructive: color plus explicit wording. A quiet pane-header x opens a
   confirmation; a dialog uses Close pane or Close tab, never generic OK.
7. Hover: optional enhancement when host mouse motion is available. Rest and
   keyboard states must work without hover, hand cursors, special fonts, or emoji.

Narrow panes truncate optional labels before losing Menu/close targets. Choose
compact forms from shared layout geometry. Do not hardcode a browser-style
44-pixel target requirement into a terminal cell layout. Larger targets require
an explicit row/column budget and host testing.

## Proposed implementation order and acceptance

First prove a shared styled button in Menu/close, Back to live and a close
dialog. Once the direction is selected, apply it to all families above rather
than restyling each independently. Keep render and hit geometry identical.
Do not change shell/provider state or input ownership during styling work.

Evaluate at normal and minimum pane sizes, two/four panes, tab overflow, dark and
light themes, reduced colors, ASCII fallback, keyboard-only operation and hosts
without hover. Confirm press cancellation, exact destructive target, pending
state, and that controls do not forward events into a child application.
Inspect cell-accurate captures and run relevant compositor/UI checks during
implementation. No Rust/build/runtime checks were run for this research task.
Product spec and usage-skill behavior should change only with an accepted and
implemented design; these proposals do not advertise new runtime support.
