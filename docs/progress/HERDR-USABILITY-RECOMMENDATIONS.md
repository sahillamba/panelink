# Herdr-informed Panelink usability recommendations

Date: 2026-10-22. Status: GPT-6-Luna xhigh study, reviewed and refined by the parent; recommendations only, with no product decision or implementation adopted.

## Scope and evidence

This report uses Herdr’s workflow patterns to identify useful additions for Panelink; it does not recommend copying its layout, visuals, assets, source, keymap, or terminology. Herdr documents workspace → tab → pane, mouse and keyboard navigation, resizing, selection, and agent-state rollups. Its quick start presents clicks as an entry point and keyboard control as optional. [Concepts](https://herdr.dev/docs/concepts/), [Quick start](https://herdr.dev/docs/quick-start/), [Agents](https://herdr.dev/docs/agents/)

Evidence is documentation-only: official pages were read on 2026-10-22 before the site-origin access restriction was relayed, and the official repository is [herdrdev/herdr](https://github.com/herdrdev/herdr). The homepage/repository describe a sidebar and link a demo video, but no screenshot or video frame was inspected; Herdr was not installed, run, or source-inspected. The user also opened herdr.dev in the shared Chrome session: automation could see the tab title and URL, but saved site permission blocked page content, so no visual review was possible. Panelink status comes from its [README](../../README.md), [working spec](../SPEC.md), [roadmap](../../ROADMAP.md), and [Subagents plan](SUBAGENTS-PLAN.md), not a source audit. “Documented” means public docs say it; “inference” means a recommendation based on them.

The older [Herdr UI study](../HERDR-UI-STUDY.md) is useful for interaction evidence, but its Panelink lifecycle paragraph is stale: it describes the disposable probe exiting and stopping its child. The current default Panelink path documents a background workspace, Ctrl-X detach, reattach, and explicit stop. Keep this distinction: live detach/reattach works in the scoped current path; restart layout recovery and provider-conversation recovery remain open.

Parent verification: the GitHub connector confirms `herdrdev/herdr` is a public,
non-fork upstream whose homepage is herdr.dev. Panelink's current
`src/ui_client/keys.rs:28–60` was inspected through the code graph and exact
source: Ctrl-C is forwarded, Ctrl-X detaches, Ctrl-B enters NAV, and the other
Ctrl/Alt/function-key paths are ignored. No runtime or host acceptance was
rerun for this report. The Markdown checker and diff whitespace checks passed.

## Recommended Panelink interaction direction

Build on Panelink’s context-first interactions: click to focus, right-click for pane actions, and Ctrl-B NAV for keyboard operations. Keep focus and NAV state visible. When more destinations exist, add a searchable switcher for workspaces, tabs, panes, or observed workers without requiring a fixed sidebar hierarchy.

This preserves the open workspace/sidebar/tab choice. Herdr shows mouse-first entry and optional prefix control can coexist; Panelink’s direction remains rooted in its split menu and NAV bar. A searchable switcher is a proposal, not an adoption of Herdr’s Goto design.

## Now — make the current two-pane client safe and easy to operate

### 1. Complete ordinary terminal input and mouse routing

**Pain and expected behavior.** Editors, pagers, and shell job control need normal input. Panelink documents that function keys, Alt, and Ctrl keys except Ctrl-C are not sent; its split client does not forward mouse events. Keep Ctrl-B for NAV, route supported keys and paste to the focused pane, and define mouse pass-through while preserving access to Panelink’s menu.

**Status / dependency.** Gap in the [README](../../README.md) and [spec](../SPEC.md). Depends on supported-host input and ownership contracts; modifier delivery needs host acceptance.

**Acceptance.** In supported terminals, test shell editing, Ctrl-D/Z/W, Alt/Meta, function keys, bracketed paste, Ctrl-C, and Ctrl-B. Verify input reaches only the focused/owned pane, menus consume their keys, mouse reports can pass through, and detached input is never replayed.

### 2. Keep interaction responsive during output, storage, and pane lifecycle work

**Pain and expected behavior.** Typing, focus changes, resize, detach, and stop must stay usable while panes produce output or history is being saved. The 2026-10-21 architecture review flags synchronous socket/disk work in interactive rendering/input and owner-thread pane launch/close stalls as high-severity responsiveness risks.

**Status / dependency.** Open risk in the [architecture review](../reviews/2026-10-21-architecture-review.md), not evidence of a completed UI fix. Depends on bounded/nonblocking client and history work and advancing launch/stop as transactions without delaying sibling panes.

**Acceptance.** Under a busy sibling, blocked history writer, and slow control response, verify input, navigation, rendering, detach, and stop remain responsive; report key-to-child and action latency against a numeric budget selected for supported hardware. No input is silently dropped, duplicated, or replayed.

### 3. Make focus and the next action obvious

**Pain and expected behavior.** Make clear whether keys go to a shell or Panelink. Show focused pane plus an unmistakable NAV/menu state. Add NAV help for navigation, split, close, and detach keys, keeping right-click as the mouse path. Show focus after split/close and prevent menu input from reaching a shell.

**Status / dependency.** Partial: click focus, Ctrl-B NAV bar, four split directions, arrow/Tab/`o` switching, `s` menu, and confirmed close are documented. Consolidated help and explicit focus cue are gaps. No hierarchy decision is needed.

**Acceptance.** From the first shell, help reveals split, switch, close, NAV exit, detach, and reattach. Actions identify their target; split/close leave focus predictable; menu input is isolated; ordinary shell keys remain untouched outside NAV.

### 4. Keep detach, pane close, and stop visibly distinct

**Pain and expected behavior.** State effects at the action point: detach leaves shells running, confirmed close ends one pane, and explicit stop ends managed work. Reattach returns to live processes while the server runs; failed attach shows a clear connection state.

**Status / dependency.** Current docs cover Ctrl-X detach, `panelink` reattach, confirmed pane close, `panelink stop`, and Ctrl-C stopping foreground `serve`. These are live-process behaviors. Failure messaging and restart layout recovery remain gaps. See [README](../../README.md), [Phase 3B](P3B-CLOSE-RUNTIME.md), and [saved-layout design](P5-SAVED-LAYOUT.md).

**Acceptance.** Automated scenarios distinguish detach, pane close, last-pane exit, stop, and reconnect. Reattach preserves live child identity/screen; confirmation names the pane; stop names its effect; failed attach reports unknown/unreachable without claiming restoration.

## Next — add navigation, layout control, and reading

### 5. Add tabs and a destination switcher without freezing the hierarchy early

**Pain and expected behavior.** Two panes with no tabs limit separation of logs, review, and active work as the number of destinations grows. After resolving hierarchy, consider a compact collapsible workspace/attention sidebar alongside tab and pane navigation, plus basic tab create/rename/switch/close and a searchable destination switcher. Browsing should show target context without changing focus.

**Status / dependency.** Gap; README says at most two panes and no tabs. The spec leaves hierarchy open. Tabs can be explored in volatile form after the hierarchy decision; persisted tabs need a separately reviewed, versioned format. The queued P5 codec/store/runtime is the roadmap priority and only designs the current two-pane history shape, not a prerequisite for volatile tab UI. See [SPEC](../SPEC.md) and [P5 design](P5-SAVED-LAYOUT.md).

**Acceptance.** Stable targets remain addressable through navigation; focus behavior is defined; switching sends no input to the prior pane. Version and test layout persistence before advertising restore.

### 6. Add resize, zoom, and move with both mouse and keyboard paths

**Pain and expected behavior.** Equal splits can cramp an editor or log view. Add divider drag, temporary zoom, and eventually pane move, with keyboard routes and reset. Panelink keeps its four split directions; Herdr documents resize/zoom/move as workflow patterns, not keymaps to copy. [Herdr Quick start](https://herdr.dev/docs/quick-start/), [Herdr Concepts](https://herdr.dev/docs/concepts/)

**Status / dependency.** Gap beyond splitting. Depends on minimum dimensions, mouse routing, geometry, and saved-layout schema. Resize can improve the two-pane case without approving unlimited panes.

**Acceptance.** Drag and keyboard resize update PTY sizes without losing output; zoom/restore preserve identity and focus; move names its destination and cannot orphan the last view. Tiny-terminal and active-output cases stay responsive.

### 7. Bring selection and saved history into the terminal workflow

**Pain and expected behavior.** Users inspect saved scrollback and screen checkpoints through `panelink history list/show/search`; the live client has no selection/copy workflow. Add selection and copy plus a read-only history view labeled by source and gaps. Never replay stored effects or treat screens as live processes.

**Status / dependency.** Bounded history CLI exists; in-client selection/copy/navigation is unimplemented. Clipboard and mouse-host acceptance remain open. This can proceed independently of provider identity or hierarchy. See [P5 history](P5-SAVED-HISTORY.md) and [selection contract](../SPEC.md).

**Acceptance.** Unicode/wrapped selection copies exactly the selection under the approved policy; search labels source and capture time; missing content is unavailable; recovery views cannot execute OSC, input, or commands.

## Later — surface verified status and Subagents

### 8. Provide attention and activity views without guessing agent state

**Pain and expected behavior.** Help users find exited, failed, or waiting panes. Show only supported states, distinguishing process lifecycle from provider agent state, with freshness and unknown/stale labels. Herdr’s documented states suggest a status surface, but do not prove Panelink can infer them. [Herdr Agents](https://herdr.dev/docs/agents/), [Panelink SPEC](../SPEC.md)

**Status / dependency.** Scoped process lifecycle exists. The Phase 4 live identity gate remains open; design/binding groundwork exists, but OpenCode 2.0.32 live session-switch proof does not. The SA feasibility attempt was blocked before provider readiness. Do not infer work status from output or success from idle.

**Acceptance.** Process and provider states remain distinct. Badges require supported, fresh observations; disconnect, event gaps, and ambiguous identity mark state unknown and offer manual recovery. Status never dispatches input or stops work.

### 9. Deliver the requested Subagents tab after provider proof

**Pain and expected behavior.** The requested Subagents tab should optionally show supported OpenCode background workers without duplicating work. Start read-only, group by confirmed parent, show provider/version/freshness, and ensure opening/closing has no provider effect.

**Status / dependency.** Planned, not implemented. [SA plan](SUBAGENTS-PLAN.md) requires exact ancestry, background lifetime, reconciliation, reconnect, and no duplicate execution. S0 is blocked before provider readiness by local socket permissions; no worker was observed. This does not settle hierarchy or saved-layout format.

**Acceptance.** Per supported version, concurrent/nested workers map to the right parent; missed events reconcile; prompts/results are provider-confirmed; stale data becomes unknown; detach works; hiding the tab has no provider effect; bounded updates do not delay panes. Child TUI attachment needs separate proof and scope.

## Cohesive delivery order

1. **Reliable two-pane operation:** complete key/mouse routing, remove responsiveness stalls, clarify focus/help, and verify detach, close, stop, and reconnect.
2. **Navigation, layout, and reading:** resolve hierarchy, add destination navigation/tabs and resize/zoom/move, and independently deliver selection/history.
3. **Attention and subagents:** expose only verified process/provider status and prove identity/observation before shipping Subagents.

Richer navigation and attention depend on stable targets, focus, and process contracts. Improving input and the current two-pane flow first supports every later phase and avoids promising unproved recovery.
