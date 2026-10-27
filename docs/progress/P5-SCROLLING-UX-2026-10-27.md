# Scrolling presentation repair — 2026-10-27

## Scope

User testing reported a flashing loading view while scrolling and requested
scrollbars. Work is on `codex/p5-acceptance-continuation`, based on `1060668`,
with earlier acceptance/workload changes preserved. This is a usability repair,
not completion of Phase 3/5 or the memory/capture-loss investigation.

## Behavior

- A pending history read preserves the last valid live/history snapshot. It
  no longer replaces the pane with `loading scrollback...`.
- While a request is in flight, known normal-buffer wheel gestures accumulate
  a bounded row delta. Once its result is released, a single follow-up request
  starts from the returned effective offset. Opposite directions cancel. View
  invalidation, typing, resize, buffer changes and failures discard queued
  movement. Alternate-screen child wheel input is not queued or replayed.
- Normal-buffer panes have a vertical position indicator in existing right
  padding. It never consumes PTY content or selection/copy cells. Menus cover
  the indicator and closing/moving menus restores it. Its size/position follow
  the last observed history extent and effective offset; zero/unknown extent
  has a full-height thumb. The range refreshes while scrolling, not continuously
  while live output arrives. Wheel navigation works across the pane including
  its margin. Track clicks and thumb dragging are not implemented.
- A bounded read-only query at the maximum history offset discovers the extent
  without changing the wire format. Known interior extents are cached per exact
  target/revision. The optional probe is best effort: errors, malformed replies,
  alternate-screen changes and revision mismatches preserve the valid requested
  viewport and use a conservative extent. Probe failures are not cached.
- Chrome changes paint only the margin; unchanged frames emit no repaint.
  Staged/committed baseline and partial-write invalidation remain authoritative.
  The unpadded recovery compositor retains its original dimensions/content.

## Review and verification

Parent review corrected the renderer to skip unpadded and absent scrollbar
state. An independent source review found the optional extent probe could
reject a valid viewport; parent made it best effort and added socketpair cases.
Follow-up review found no material issue in that correction.

Initial renderer tests used unsupported tiny terminal dimensions; corrected to
accepted minimum dimensions, plus a direct one-cell thumb arithmetic check.
Subsequent test failures at a wrong thumb row and padded geometry in the
unpadded fixture were corrected without changing product bounds. Initial
socketpair test import names and explicit snapshot-write permission were also
corrected. Earlier failures remain in `/private/tmp/panelink-scroll-*` logs.

Parent verification:

- Library: 880 passed, 0 failed, 4 ignored at
  `/private/tmp/panelink-scroll-tests-r3.log`. This predates the final equivalent
  scrollbar-loop lint cleanup; final compositor rerun: 32 passed, 0 failed at
  `/private/tmp/panelink-scroll-compose-final.log`.
- Binary: 221 passed, 0 failed, 0 ignored; history cursor integration: 6 passed,
  0 failed at `/private/tmp/panelink-scroll-final-tests.log`.
- Outer-PTY workspace acceptance: 3 passed, 0 failed at
  `/private/tmp/panelink-scroll-ui-final-r2.log`. Its earlier whole-host-row
  marker checks included new chrome; repaired to match exact parsed child
  content. Divider detection now requires the top border to distinguish tracks.
  These runs also precede the equivalent renderer-loop lint cleanup.
- Strict all-target Clippy and final formatting passed, at
  `/private/tmp/panelink-scroll-clippy-final.log` and `-fmt-final.log`.
- Documentation validation and `git diff --check` passed before final build.

This is targeted/unit/integration verification, not a fresh full canonical
verifier run. Initial test-fixture compile, transport opt-in and checkpoint
revision-field errors were corrected; snapshot/read guards remain unchanged.
The initial UI integration result was 1 passed / 2 failed before content-only
marker correction; the successful rerun above supersedes it without hiding it.

Refreshed local release: two fresh locked/offline optimized builds produced
identical SHA-256 `c0af2dd88dce4c4dc3e14d16ed46a85dbba715aab3218f0dda81eedf1c893576`.
The arm64 artifact is `release/panelink-release`; its manifest hash was checked
independently and `--help` exited 0. Build record:
`/private/tmp/panelink-scroll-release.log`. It is an unsigned dirty-checkout
local testing build. Existing running workspaces and installed launchers were
left alone; restart the testing workspace with this binary to load both the
updated client and server. No native Apple Terminal or real clipboard
acceptance is claimed. No commit, push, installation,
or running-workspace activation is part of this repair.
