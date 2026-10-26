# Pane usability fixes — 2026-10-24

The user confirms a real Apple Terminal workspace opens, and reports three
actual usability problems in the attached screenshot: its agent cannot discover
other panes (`context_directory_unavailable`), wheel scrolling does nothing,
and pane text touches dividers. This is partial host feedback, not an itemized
four-pane acceptance pass. It reopens the usability work below.

## Accepted repair scope

- Explicit `PANELINK_AGENT_CONTEXT_DIR` selects a stable private writable
  metadata context root for sandboxed agent invocations. Keep absolute path,
  UID ownership, mode0700 and no-symlink validation. Never silently switch roots
  or hide an uncertain input operation. Provide actionable diagnostics. This
  does not bypass OS restrictions on connecting to the server socket.
- Wheel over an ordinary pane shows its retained normal-buffer history through
  read-only, target-pinned queries. Keep the parser's live state unchanged,
  retain separate live/view caches, hide the history cursor, bound offsets,
  and reject stale asynchronous results after typing or target replacement.
- Wheel over an alternate-screen application requesting mouse reporting is typed input
  routed to that pane with content-local coordinates and the application's
  current mouse encoding. Preserve capability, ownership, admission,
  deduplication and uncertainty rules. Discover mode through a read query;
  ordinary history scrolling must not claim input ownership. Alternate-screen
  apps with mouse disabled must not be shown unrelated hidden shell history.
- Keep tiled outer pane rectangles and hit regions, but inset content one cell
  on all sides. Share content geometry across rendering, cursor positioning,
  PTY resize, split launch size and mouse coordinate conversion. Preserve the
  minimum5x20 PTY content by requiring7x22 outer panes. Tiny terminals and
  minimum split refusal must remain safe.

## Ownership and evidence

Native GPT-6 Luna High workers own disjoint sets: agent CLI/context regression;
terminal history/mouse core; geometry/compositor; UI/protocol/runtime scrolling
integration (as slots are released). The parent owns canonical docs, review,
integration verification and deployment decisions. Preserve all prior dirty
work, no worker commits and no push.

Use targeted meaningful tests, real private-PTY wheel/history acceptance where
possible, then complete serial offline Cargo plus seven supporting checks on
frozen source. Do not count sandbox socket-denied starts as passing tests.
No native macOS computer-use controls are available in this session; do not
claim a computer-operated Apple Terminal test.

The user's PATH launcher currently resolves to the separate main checkout
`/Users/sahillamba/panelink/bin/panelink`, while fixes are developed in worktree
7060. Verification must identify the exact built binary. Existing live server
processes must not be terminated just to deploy these fixes; any necessary
restart of live user processes is a separate consequential action.
