# Panelink UI/UX design interview

Status: all six question rounds answered; consolidated plan awaiting final
shared-understanding confirmation, 2026-10-27. No implementation authorized.

## Purpose

Stress-test [the proposed UI/UX plan](../UI-UX-PLAN.md) with the user before
promoting its defaults into requirements. Capture settled decisions here and
revise the plan as answers arrive. Create a root GLOSSARY.md when new domain
language is resolved; create ADRs only for consequential trade-offs.

## Confirmed starting points

- Improve Panelink UI/UX with Herdr as inspiration, independently designed.
- Prioritize pane layout, menus and mouse interaction.
- Produce detailed durable planning material future agents can pick up.
- Current product runs inside existing terminal applications.

## Round 1 — confirmed answers

1. The concrete pain points are slow scrolling, difficult dragging and a mouse
   close workflow that appears to require keyboard confirmation. Investigate the
   observed behavior against the selected executable; documented button support
   alone does not establish usability in the user's runtime.
2. Everyday actions must have complete visible mouse routes and equivalent
   keyboard routes. Any close confirmation must be completable with buttons.
3. Four panes are enough. Tabs are more important than adding visible panes;
   elevate tab usability into the plan instead of treating the existing strip
   as sufficient. Exact tab-management scope remains open.

## Round 2 — confirmed answers

4. Improve divider resizing and scrollbar dragging. The initial answer was
   interpreted as whole-pane movement, but Q7 explicitly corrected that:
   whole-pane dragging is not requested; fast, smooth pane-history scrolling is
   the priority. Text selection is satisfactory; preserve it and add an indicator
   after successful copying. Its exact indicator design remains proposed.
5. Visible create, switch, rename and close tab actions are required, with keyboard
   equivalents. Tab close lifecycle was subsequently settled in Q8.
6. Scrolling/scrollbar dragging and layout adjustments must not incidentally change
   typing focus. Make the selected pane visually distinct with stronger contrast.
   Here selected pane means the pane receiving keyboard input, not hovered pane,
   drag destination or text selection.

## Round 3 — confirmed answers

7. No whole-pane dragging. The user clarified that fast, super-smooth scrolling
   through pane history is the intended outcome. Remove the pane-movement branch;
   retain existing explicit Swap without extending it into a new gesture.
8. Closing a tab ends its processes after a dialog with Close tab and Cancel
   buttons. Detach keeps work running. No keyboard step may be required.
9. Focus uses a stronger border/header and explicit focus marker; preserve normal
   terminal text colors in other panes rather than dimming their output.

## Round 4 — confirmed answers

10. New output preserves the history reading position, with a visible New output /
    Back to live control.
11. Scrollbar dragging updates displayed history continuously while held, rather
    than only on release. Bound read work and favor the latest requested position.
12. Clicking + creates and focuses a tab with one ordinary shell. This shell is
    fully managed by Panelink, with the same lifetime, pane controls, history,
    input rules and detach/reattach treatment as other managed panes.
13. Pane Menu and right-click offer the same actions except zoom, which is not
    wanted. Keep split, resize, copy/history and close, with disabled reasons
    and clickable cancellation. Removing existing zoom support is not implied.

## Round 5 — confirmed answers

14. Typing in the focused pane while reading history returns that pane to live
    output and delivers the input normally; retain the clickable Back to live.
15. Overflow uses one tab row with previous/next controls and a searchable All tabs
    chooser. Keep active tab and + accessible without consuming more content rows.
16. Keep top tabs and compact pane headers. No permanent sidebar in this redesign.

## Round 6 — confirmed answers

17. Successful split immediately focuses the new shell pane once committed.
    Cancellation or failure preserves the previous focus.
18. Closing the last tab shows an empty workspace with New tab and Detach;
    no replacement shell starts automatically.

## Consolidation and final checkpoint

The rewritten [implementation plan](../UI-UX-PLAN.md) incorporates all answers
and replaces the original seven packets with eight packets centered on history
responsiveness and full tab lifecycle/UI. No unanswered product question remains
in the interview tree. Final shared-understanding confirmation is pending.
Engineering details such as coalescing bounds, exact label durations, colors and
performance budgets are explicitly proposed for measured/prototype validation.

Source cross-check during consolidation found `WorkspaceIndex::rename_tab`, the
last-active-tab removal refusal and reserved Default/Main CLI removal refusal.
The plan records these as UX-05 compatibility work, not a working empty-state or
in-client rename capability. No implementation, runtime mutation or tests ran.

## Proposal status

The four-pane cap, mouse/keyboard parity, tab create/switch/rename/close actions,
copy success feedback, focus preservation during scrolling/layout adjustment,
tab-close process termination with button confirmation, and stronger focus chrome
without content dimming are confirmed. Stable history during new output,
continuous scrollbar updates, fully managed new shell tabs and consistent menus
are confirmed. Whole-pane dragging and zoom expansion are excluded. Top tabs
without a permanent sidebar, return-to-live on typing, single-row overflow,
focus after split and the final-tab empty state are confirmed.
Existing safety and phase-acceptance contracts remain.
Begin no application implementation during the interview. Before finalizing the
plan, ask the user to confirm the resulting shared understanding.

## Method sources

Local entrypoint: `/Users/sahillamba/.agents/skills/grill-with-docs/SKILL.md`.
Supporting instructions read from upstream on 2026-10-27, without installation:
[grilling](https://github.com/mattpocock/skills/blob/main/skills/productivity/grilling/SKILL.md),
[domain modeling](https://github.com/mattpocock/skills/blob/main/skills/engineering/domain-modeling/SKILL.md).
Use rounds of prerequisite-ready decisions; investigate facts independently;
record resolved language inline and consequential decisions sparingly.
