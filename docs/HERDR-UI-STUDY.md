# Herdr UI reference study

**Purpose:** collect interaction patterns from Herdr’s current public product documentation for later comparison. This is reference material only. It does not select a Panelink UI, settle an open product decision, or add a requirement.

**Evidence date:** 2026-10-18. **Method:** read-only review of official `herdr.dev` pages via web search/open. A visual check was attempted with the browser UI tool; `await cua.createBrowserTab('iab','https://herdr.dev',{visible:false})` returned `Browser is not available: iab`. A Chrome tab attempt also returned `Capability is not available: visibility`. Thus no actual page screenshot could be captured; no pixel-level description below is claimed as visually verified. The homepage content includes product-montage text and advertises a real-layout sidebar, but those remain documented homepage content rather than visual observations. Herdr was not installed, run, or source-cloned. “Documented” means the current official page says this; “inference” is a cautious interpretation; “unknown” means the reviewed pages did not establish it.

## Layout and hierarchy

- **[Documented]** Herdr describes a terminal UI attached to a background server. Its documented product hierarchy is workspace (project container) → tab (layout) → pane (real terminal). Agents are recognized processes inside panes, not a separate terminal layer. A workspace sidebar rolls up agent state. [Concepts](https://herdr.dev/docs/concepts/), [homepage](https://herdr.dev/)
- **[Documented]** The sidebar is called the main dashboard. The configurable expanded sidebar has separate Space/workspace and Agent row layouts. Default agent rows show state icon, machine/workspace/tab, then agent; default workspace rows show state/workspace then branch/Git status. Values can disappear when unavailable. [Configuration](https://herdr.dev/docs/configuration/)
- **[Documented]** The workspace sidebar can be collapsed; tab row defaults to the top and can be moved to the bottom. A configurable right-side tab-row status area can show zoom, hostname, date/time, text, or command output. On narrow rows that area yields to tabs and controls. [Configuration](https://herdr.dev/docs/configuration/), [config reference](https://herdr.dev/docs/config-reference/)
- **[Documented]** Tabs can contain panes; split direction documented as right or down. Split tabs remain visible as a single tab in the Goto picker, while each pane/terminal gets its own row there. [Concepts](https://herdr.dev/docs/concepts/), [Keyboard](https://herdr.dev/docs/keyboard/)
- **[Unknown]** The official pages reviewed do not establish exact proportions, spacing, typography, pane-title chrome, tab naming layout, or the precise visual relationship between sidebar and pane content. No screenshot observation was available.

## Focus, navigation, and structure changes

- **[Documented]** Mouse interaction is described as primary: click workspaces, tabs, panes, and agents; drag split borders; right-click for context menus. A setting disables mouse capture, which also disables Herdr mouse controls. [Quick start](https://herdr.dev/docs/quick-start/), [Concepts](https://herdr.dev/docs/concepts/)
- **[Documented]** Right-click menus include pane splitting and tab creation. Right-clicking a pane frame still opens Herdr’s menu. A per-pane setting can route regular right-click gestures to an application that reports mouse input; a configured modifier can pass through right-click/hold/drag. [Quick start](https://herdr.dev/docs/quick-start/), [CLI reference](https://herdr.dev/docs/cli-reference/)
- **[Documented]** Split borders are draggable to resize. Keyboard resize mode is available; pane swapping and zoom are also documented. Pane move can relocate a pane to another tab or workspace. [Quick start](https://herdr.dev/docs/quick-start/), [Keyboard](https://herdr.dev/docs/keyboard/), [CLI reference](https://herdr.dev/docs/cli-reference/)
- **[Documented]** New tab and split do not focus the new layout by default in the CLI; a focus option is available. The CLI behavior alone does not prove that every UI gesture follows the same default. [CLI reference](https://herdr.dev/docs/cli-reference/)
- **[Documented]** Closing a pane is a listed action. Closing a workspace asks for confirmation by default. [Keyboard](https://herdr.dev/docs/keyboard/), [config reference](https://herdr.dev/docs/config-reference/)
- **[Unknown]** The reviewed material does not establish the full UI close-pane confirmation flow, what happens when the last pane in a tab closes, or all focus-retention rules after a move/close.

## Selection, copy, and discoverability

- **[Documented]** Drag-select copies text; double-click selects a token, and holding the second press while dragging extends selection by whole words. Keyboard copy mode supports history navigation, literal search, and selection. [Quick start](https://herdr.dev/docs/quick-start/), [Keyboard](https://herdr.dev/docs/keyboard/)
- **[Documented]** A configurable prefix (default Ctrl-B) routes the next key to Herdr, preserving ordinary keys for programs running in panes. Prefix-? opens active keybinding help; `/` filters the list. Bindings are configurable, and direct chords are possible. [Keyboard](https://herdr.dev/docs/keyboard/), [Configuration](https://herdr.dev/docs/configuration/)
- **[Documented]** A Goto picker groups terminal/agent destinations by workspace, exposes location/path details for the selected row, supports text search and state filters, and can be navigated by mouse or keyboard. It includes overflow scrolling without selecting a destination. [Keyboard](https://herdr.dev/docs/keyboard/)
- **[Inference]** The picker and help panel make larger workspaces discoverable without requiring users to memorize the entire keymap. Whether this is suitable for Panelink depends on the user’s desired navigation model.

## Agent status, waiting, and failure presentation

- **[Documented]** Agent states are `blocked`, `working`, `done`, `idle`, and `unknown`. Blocked means input/approval/decision is needed; done means finished but not yet seen; idle means finished or waiting and seen; unknown means Herdr cannot confidently classify it. [Concepts](https://herdr.dev/docs/concepts/)
- **[Documented]** Completion-seen state is tracked per client, so one client’s viewing does not clear another client’s Done badge. The docs caution that CLI/API status may therefore differ from a particular client’s badge. [Concepts](https://herdr.dev/docs/concepts/)
- **[Documented]** Sidebar indicators use colored dots by default. A symbols option can distinguish states by shape as well as color; it is static and does not enable spinner animation. Status text is configurable in sidebar rows. [Configuration](https://herdr.dev/docs/configuration/)
- **[Documented]** Invalid configuration falls back to a safe default with a startup warning. A configurable tab status command clears its entry after command failure, empty output, or timeout. [Configuration](https://herdr.dev/docs/configuration/)
- **[Unknown]** The reviewed UI references do not define a general pane-starting/loading screen, server-disconnected error presentation, or a complete set of agent-detection failure messages. The homepage montage contains activity-like terminal examples, but these are not treated as a verified live UI state.

## Persistent server and client lifecycle

- **[Documented]** The Herdr server owns panes and process state; the terminal UI is a client. Detaching with Ctrl-B then Q, closing the terminal window, or losing the connection leaves panes/agents running; running Herdr again reattaches. Explicit `herdr server stop` ends the session and its panes. [Concepts](https://herdr.dev/docs/concepts/), [Persistence and remote access](https://herdr.dev/docs/persistence-remote/), [Quick start](https://herdr.dev/docs/quick-start/)
- **[Documented]** After a full server stop, Herdr restores saved session shape on the next start. On macOS and Linux it ignores SIGHUP; the docs direct operators to explicit server stop or SIGTERM. [Persistence and remote access](https://herdr.dev/docs/persistence-remote/)
- **[Documented]** In remote attach, the remote server owns running panes while the local client renders the UI, sidebar, menus, and theme. The docs describe OS support beyond macOS, including Linux and Windows; some Windows ARM64 host support is best-effort. Direct terminal attach is documented for Linux/macOS, not native Windows. [Persistence and remote access](https://herdr.dev/docs/persistence-remote/)
- **[Panelink context, documented locally]** Panelink currently runs inside existing terminal applications on Apple Silicon Macs; its prototype’s explicit Ctrl-X probe exit stops its child and is not persistent detach. These are product/context distinctions, not a recommendation to copy Herdr’s lifecycle. [README](../README.md), [Discovery](PANELINK-DISCOVERY.md)

## Patterns for later comparison (options only)

These are interaction patterns the user may choose to compare; none is adopted or proposed as a Panelink default here.

- Showing project/workspace context alongside agent state in a compact navigable sidebar.
- Offering both direct mouse operations and a keyboard route that avoids stealing ordinary pane input.
- Keeping a destination picker searchable and making each terminal reachable even inside split layouts.
- Making detach, reattach, and explicit stop visibly distinct actions.
- Showing uncertainty as an explicit `unknown` state, and supporting a non-color-only state cue.
- Keeping session state and the rendering client as separate lifecycle concepts.

## Decisions awaiting user direction

- Which entities should be visible in Panelink’s main navigation surface, and what hierarchy should group them?
- Should the first UI emphasize a persistent sidebar, a picker, tabs, or another structure?
- Which mouse gestures, context menus, keyboard modes, and copy/selection behavior should be supported?
- What status vocabulary and uncertainty/loading/error cues should the UI show?
- Which actions should detach a client, close a view, stop a child, or stop the server—and how should those actions be communicated?
