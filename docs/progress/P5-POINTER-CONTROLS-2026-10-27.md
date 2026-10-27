# Pointer and keyboard controls — 2026-10-27

## Scope and checkout

User requested simpler resizing, draggable dividers, a close button per pane,
clickable/draggable scrollbars and an easily discovered menu, usable with
pointer and keyboard. User selected the installed 7060 worktree:
`/Users/sahillamba/.codex/worktrees/7060/panelink`, branch
`codex/p5-acceptance-continuation`, base `1060668`. Existing dirty scrolling,
workload and acceptance changes are preserved. The older main checkout is not
the application implementation target.

Touch compatibility means gestures translated into mouse presses, motion and
release by the host terminal. This adds no native touch protocol, pixel UI,
multitouch gesture recognition or new host acceptance claim. A project sidebar
is still an optional presentation choice; the pane menu supplies discoverability
without reducing content dimensions.

## Interaction contract

- Each pane uses existing top padding for `[Menu]` and `[x]`. The renderer and
  input router share exact rectangles. Content, cursor and selection coordinates
  keep their existing inset geometry. Recovery's unpadded compositor is unchanged.
- Menu opens actions for the exact clicked pane. Keyboard `Ctrl-B s` opens the
  same actions. Split, zoom, swap, divider adjustment, copy, saved-layout view,
  help, close and detach are discoverable. Unsupported or stale actions refuse
  effects through existing guards. Right-click retains its pane split menu.
- Close captures the complete target and layout revision before confirmation.
  A separate click on Close or keyboard `y` confirms; Cancel dismisses. The
  final-pane warning remains explicit. The opening release never confirms.
- Press a divider, drag to preview and release to apply one absolute position.
  Minimum subtree dimensions constrain preview. No child input or repeated
  resize request occurs during motion. Unchanged drags produce no mutation.
  Escape, focus loss, host resize, route/topology changes or modal takeover
  cancel the captured gesture rather than resolving a replacement target.
- Normal-buffer scrollbar track clicks page by content height; dragging the
  thumb previews locally and seeks on release through retained normal history,
  oldest at top and live at bottom.
  Its geometry is shared with rendering. Pending reads retain cached content;
  further seek requests coalesce within bounded state. Alternate-screen
  application wheel input retains its separate ownership/nonreplay contract.
- Keyboard history navigation is available from NAV. Selection/copy, pane
  gestures, help, menus and confirmation own their input and never leak it to
  a shell. Ctrl-X remains detach while local interactions are active.

## Delivery plan and ownership

1. [x] Inspect current checkout, preserve baseline and specify shared geometry.
2. [x] Renderer: pane controls and shared scrollbar geometry. Luna high owns
   only `src/terminal/compose.rs`.
3. [x] Input and behavior tests: actions, confirmation, divider/scrollbar capture
   and keyboard parity. Luna high owns only `src/ui_client/state.rs` and
   `src/ui_client/state_tests.rs`.
4. [x] Parent integration review, private outer-PTY acceptance, documentation and
   usage-skill maintenance. Parent owns docs and integration fixtures.

Workers do not commit, update release artifacts, activate running workspaces or
spawn descendants. Parent reviews the final changes and independently verifies
claims. No estimated token/cost total is substituted for unavailable telemetry.

## Verification plan

- `cargo test --locked --offline --lib terminal::compose -- --test-threads=1`
- `cargo test --locked --offline --bin panelink ui_client:: -- --test-threads=1`
- Private outer-PTY acceptance: parsed VT100 cells, observable shell readiness,
  pointer divider/close/menu routes, exact live PTY size and process continuity.
- `cargo fmt --check`, strict all-target Clippy, documentation/skill link checks
  and `git diff --check`.

Record actual outcomes below. Native Apple Terminal, real clipboard and touch
hardware acceptance remain separate gates; no phase-completion claim follows
from these bounded checks. No push or live workspace restart is authorized.

## Results

Follow-up wide-host repair: the user's64x269terminal triggered a legacy attach
panic in the absent-header staging slice (`269` columns into a240cell cache).
Private PTYs reproduced it above240columns. Legacy render now uses the same
80x240bounded viewport as the tabbed client. A unit regression reproduces the
exact panic against the old code and passes for initial/incremental/resize
frames after the fix. Client147/0 and default-open outer-PTY7/0/1ignored pass;
the new64x269scenario verifies startup, detach, reattach and identical live
target. First PTY run failed because the new read-only test context lacked
`init`; fixed fixture and complete target rerun pass. Strict all-target Clippy,
debug build, fmt and diff checks pass. Logs: `/private/tmp/panelink-wide-client.log`,
`/private/tmp/panelink-wide-pty-r2.log`, `/private/tmp/panelink-wide-clippy.log`,
`/private/tmp/panelink-wide-build.log`. Full suite above/below is prior evidence;
it was not rerun for this repair. Installed7060debug binary rebuilt; running
user server and its shell untouched. Native host retry is still pending.

Renderer worker and parent independently ran the focused compositor suite:
36 passed, 0 failed. Shared control/scrollbar geometry, parsed-cell rendering,
modal damage restoration and partial-write behavior are covered. State routing
is still in progress; these are component checks, not integrated acceptance.

Parent review identified an alternate-screen race in the existing wheel API:
a queued history gesture could query a newly changed buffer and deliver a child
wheel event. Pointer/key seeks now use a distinct read-only worker command and
`Net::seek_history`; a server-reported alternate screen cannot trigger ownership
claim or input. The same bounded pending-result/budget path is retained. A
socketpair regression extends the history test with alternate+mouse reporting.


Independent Luna source review of compositor, read-only seek backend and worker
accounting found no material issue; this was read-only source review, not a
runtime check. Parent added a worker admission/result-release regression and a
private outer-PTY scenario covering actual sizes/process continuity, scrollbar
thumb endpoint seeking, and per-pane close confirmation. Their runs are pending.

The maintained usage skill is still installed from the older main repository
copy. Its new interactive reference explicitly identifies this 7060 build and
preserves the older CLI baseline. Four skill Markdown files/links and unchanged
frontmatter structure pass stdlib checks. The bundled quick validator could not
run because PyYAML is absent; no package was installed for this task.

## Parent integration — current

The state implementation worker hit its usage limit before verification and
released incomplete edits. Parent continued directly; no replacement agent was
launched. An independent state review found that a buffer change cleared pointer
divider capture but retained its paired keyboard preview. Parent clears both,
with a regression proving a later Enter cannot commit the canceled gesture.
Parent also added scrollbar Escape cancellation, a clickable Help close strip,
and a pointer exit from keyboard copy via existing pane controls.

Menu pages now share one viewport-derived slice for rendering and hit testing,
including minimum 7-row roots. Keyboard Cancel has its own selectable sentinel;
it cannot alias Detach. Tests traverse every action and Cancel with pointer
input at 7/8/24 rows, 22/81 columns and with/without the workspace header. The
saved-layout availability query uses the same page mapping. Pointer close takes
the confirmation before submitting, matching keyboard one-shot semantics.

Divider motion clamps to feasible subtree dimensions. Parent replaced a full
span scan per motion with a bounded binary search of the contiguous valid
interval, grounded in the two subtrees' minimum extents. Tests cover both axes,
nested dividers, unchanged drags, sparse motion, cancellation and real PTY size.
Scrollbar hit testing refuses the same pending-resize placeholders as rendering.

Actual checks so far:

- Focused parent compositor: 36 passed, 0 failed.
- Parent client runs r1 136/0, r2/r3 141/0; r4 141/2 exposed the menu's action
  count/cancel-sentinel mismatch after viewport paging, then r5 143/0 after
  correction. A final resize-placeholder guard/test is included in the full run.
- Private outer-PTY r1 failed at a first-page assumption; r2 failed because the
  fixture's rendered-label search collapsed unpainted blank VT100 cells and
  clicked the wrong border position. Parent preserved cell columns rather than
  weakening the close assertion. r3 passed the pointer scenario 1/0. r4 had
  3 pass/1 fail at the menu action-count mismatch; r5 all four scenarios pass
  (4/0, 35.45 s). Runs use private fixtures and readiness barriers.
- Strict all-target Clippy r1 passed before final menu/cancellation refinements.
  Final full serial Rust and supporting checks are running; no final pass yet.

Logs: `/private/tmp/panelink-pointer-{compose-parent,client-r1,client-r2,client-r3,client-r4,client-r5,pty-r1,pty-r2,pty-r3,pty-r4,pty-r5,clippy-r1}.log`.
These are local paths, not portable artifacts or Apple Terminal acceptance.

Final parent refinements: Resize divider has visible minus/plus controls and
Apply/Cancel, so its menu route is usable with pointer input. Duplicate bracketed
button names were removed from instructional titles so rendered-label clicks
identify the actual button rows. Copy mode can return to the pane Menu without
keyboard input. Final client suite: 146 passed, 0 failed at
`/private/tmp/panelink-pointer-client-final-r2.log`; final strict Clippy passed
at `/private/tmp/panelink-pointer-clippy-final-r2.log`. Final four workspace
outer-PTY scenarios: 4 passed, 0 failed in 38.19 s at
`/private/tmp/panelink-pointer-pty-final.log`.

The initial full run passed library884/0/4ignored but rejected the parent's
inconsistent pending-resize fixture in binary235/1. The fixture now includes
both the confirmed size and the matching pending-size field; production codec
guards were preserved, and its focused rerun passes1/0. The next full run hit an
older four-pane fixture expecting Ctrl-B s to open Split directly. Parent updated
that fixture and the analogous two-pane host fixture to observe actions then
Enter Split; their pane-limit and no-child-input assertions remain intact.
The final no-fail-fast serial full run is pending.

The older raw-attach/default-open tests also matched whole host rows as exact
shell output, so the pre-existing new right-margin scrollbar made their
ACK41/RE43 assertions fail despite the correct child output being visible.
Parent changed those readiness assertions to exact parsed VT100 cells within
the root PTY content rectangle, excluding only known border cells. Command
echo cannot satisfy the exact marker test. Existing routing/ownership/cleanup
assertions remain. The legacy rerun is recorded separately in
`/private/tmp/panelink-pointer-legacy-final.log`.

## Final verification and availability

The final locked/offline no-fail-fast serial Rust command exited 0:
`cargo test --locked --offline --no-fail-fast -- --test-threads=1`.
All 77 completed result groups total **1,437 passed, 0 failed, 15 ignored** at
`/private/tmp/panelink-pointer-full-final.log`. This includes library884/0/4ignored,
binary238/0 and all integration/doc groups. Legacy corrected fixture reruns also
pass separately: raw attach5/0 and default-open6/0/1ignored at
`/private/tmp/panelink-pointer-legacy-final.log`. The test-only fixture updates
landed while the full command was running; the final full log contains their
successful target results. Application source was unchanged during that final
run. This is not one of the earlier stopped/failing full runs.

Final strict all-target Clippy and locked/offline debug build exited 0 at
`/private/tmp/panelink-pointer-clippy-final-r3.log` and
`/private/tmp/panelink-pointer-build-final.log`. Formatting, component fingerprints
(vt100/crossterm-input), documentation383/0 and diff whitespace checks pass.
The usage skill's four Markdown files/links and unchanged frontmatter structure
pass; the optional PyYAML-based validator limitation remains recorded above.
The code graph was refreshed after source freeze.

The installed development symlink still targets this checkout's `bin/panelink`,
and its `target/debug/panelink` is rebuilt. A new client attachment loads these
controls without restarting server-owned shells. No running workspace/client
was killed or replaced, no release/MANIFEST artifact was updated by this task,
and no commit/push was made. Phase3/5 host, real clipboard, physical reboot and
representative-budget gates remain open; native touch hardware was not tested.
