# Pane usability repair result — 2026-10-24

Implementation and focused verification are delivered in worktree
`/Users/sahillamba/.codex/worktrees/7060/panelink`, based on checkpoint
`8ad9bd6`. Final integrated verification passed: **1111 passed, 0 failed,
15 ignored in 164.34 seconds**. The user subsequently approved activation;
the launcher was switched and the workspace restarted as recorded below.
This note does not close Phase 3. All prior dirty work is preserved.
No application dependency or vendor code was added or changed.

## Delivered behavior

- `PANELINK_AGENT_CONTEXT_DIR` explicitly selects an absolute, UID-owned 0700
  caller-context directory without symlinks. Unavailable defaults give an
  actionable diagnostic. The CLI never silently switches context roots, loses
  an uncertain key, or issues replacement input during reconciliation. Local
  socket permission remains a separate requirement.
- Normal-screen wheel events request a bounded, target-pinned history viewport
  using READ capability only. The styled live screen stays authoritative;
  history has a separate cache and hidden cursor. Typing/paste returns live.
  Target replacement, geometry changes and entering an alternate screen fence
  late history replies. One pending wheel request bounds retained checkpoints;
  further ticks are ignored until it completes.
- Alternate-screen apps that enable mouse reporting receive typed wheel input
  through the authenticated owner/CAS, operation-key, admission and dedup path.
  Coordinates use the pane content origin and current child encoding. The
  server rechecks alternate-screen mode at admission. Only an explicit
  preadmission `Unsupported` permits a read-only fallback; uncertain delivery
  is never resent. Alternate screens without mouse reporting stay live and
  show a short limitation notice.
- Each live pane has a one-cell blank margin on all four edges. Layout and hit
  rectangles remain outer geometry; PTYs, snapshots, cursors, split launches
  and child mouse coordinates share the inset rectangle. Content remains at
  least 5x20, outer panes at least 7x22. Equal splits require 45 columns or
  15 rows; maximum root remains 80x240 outer cells.
- Startup launches directly at the content size before the child runs. A
  parent-found resize-after-output race had discarded the newest immediate
  child output; the direct-size launch fixes it. The history-driver regression
  observes the initial `stty` size and preserves both screen buffers.
- Saved-layout schemas retain their historical 5x20 geometry domain and
  legacy migration. Recovery composition uses exact saved checkpoint sizes;
  live padding does not rewrite old records. Old server layout replies remain
  decodable; attach reports the required server upgrade when dimensions lack
  padding, and unsupported wheel schemas receive an actionable status.

## Review and verification

Three bounded native GPT-6 Luna High workers implemented disjoint portions and
cross-reviewed geometry, context privacy/uncertainty, input admission and UI
stale-result handling. The parent reviewed and integrated their changes,
corrected cursor visibility, rapid-tick/epoch and wheel-down behavior, buffer
transition/resize fencing, saved-layout compatibility, startup output loss and
server-side buffer-mode revalidation. The final attach regression was finished
and reviewed by the parent after its worker reached its usage limit. No worker
commits or cost measurement are claimed.

Focused tests pass for the context-root override and exact-context
reconciliation, terminal history/style/wide-cell isolation, mouse encodings,
content geometry/composition, UI state, protocol bounds and compatibility,
split/close/runtime fixtures, saved-layout/store/recovery behavior, and saved
history startup. Real private outer-PTY tests exercise normal wheel history,
wheel-down/typing return to live, alternate-screen up/down delivery, and inset
coordinate translation. They operate only on test-owned processes. They are
not native computer-operated Apple Terminal acceptance.

Raw logs: `/private/tmp/panelink-usability-20261024/`.

Initial broad integration: **1026 passed, 76 failed, 15 ignored**. Failures
exposed old outer/content sizing assumptions, old column-zero text predicates,
historical-layout compatibility, recovery composition, and the startup output
race; they were investigated rather than relabeled as passes. The first strict
Clippy run found two redundant conversions, now removed. The first new attach
mouse test matched single-space hex output against BSD `od` double spaces;
its corrected normalized-byte assertion passes. One focused shutdown fixture
missed its HUP marker once and passed on retry; this is retained as a timing
observation, not a guarantee. The final frozen-source result is recorded below.

The first repaired integration run completed with **1111 passed, 0 failed,
15 ignored** in 209.67 seconds. The subsequent server-side alternate-to-normal
guard passed its focused regression. The next full run recorded **1110
passed, 1 failed, 15 ignored** in 202.43 seconds: the existing layout-rotation
fixture reopened storage immediately after a deliberately nonblocking writer
Drop. The parent changed that fixture to require bounded
`LayoutShutdownOutcome::Drained` before opening the next generation; production
shutdown semantics are unchanged. All five layout-writer integration tests pass
with that correction. Final complete locked/offline serial verification
(`full-accepted.log` / `full-accepted.json`) passes: **1111 passed, 0 failed,
15 ignored in 164.34 seconds**, with library722/binary148/integration241
across55 integration targets (58 result groups including doc tests). Seven supporting checks pass: input decoder 39, Python 77,
component sources, formatting, strict all-target Clippy, build and docs
(281 Markdown files, zero findings). Ignored tests remain separate evidence.
The final code graph was refreshed in fast, nonpersistent mode: 12166 nodes,
54486 edges; graph discovery was followed by current source inspection.

## Pre-activation workspace observations

The tested binary is
`/Users/sahillamba/.codex/worktrees/7060/panelink/target/debug/panelink`.
Its SHA-256 is
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
The PATH launcher still resolves to the separate main checkout
`/Users/sahillamba/panelink/bin/panelink`; process executable inspection likewise
showed that main-checkout binary. No user server or pane process was stopped,
restarted or sent input to install these changes.

A permitted read-only check with the worktree binary and an explicit private
context root first listed three live panes and read pane 2 successfully.
A later refresh listed one live pane; pane 2 correctly returned `pane_not_found`,
and pane 1 read succeeded (240 text bytes, contents not printed). The changing
pane count is live workspace state. Restricted-sandbox calls could not connect
before escalation; the directory override does not bypass that restriction.

For an agent with socket access, use the new build with a stable writable root:

```sh
export PANELINK_AGENT_CONTEXT_DIR="$PWD/.panelink-agent-contexts"
/Users/sahillamba/.codex/worktrees/7060/panelink/target/debug/panelink pane list --json
/Users/sahillamba/.codex/worktrees/7060/panelink/target/debug/panelink pane read 1 --lines 80 --json
```

Activating new server geometry/protocol in the existing workspace requires a
controlled switch to this build. Stopping that workspace ends its pane
processes, so no such restart was inferred from a request for usability fixes.
Full human four-pane host acceptance remains open; the supplied screenshot is
partial real-host feedback. No commit or push was performed.

## Activation completed after explicit approval

The user selected **Activate and restart workspace** in the restart-choice
prompt. The parent rechecked the binary SHA-256 above, then atomically changed
`/Users/sahillamba/.local/bin/panelink` from the main-checkout launcher to
`/Users/sahillamba/.codex/worktrees/7060/panelink/bin/panelink`.
The old target is retained in
`/private/tmp/panelink-usability-20261024/activation-launcher.json`.

The supported `panelink stop` returned `workspace stopped` with exit0.
The new launcher then started a fresh default workspace. Its noninteractive
open command returned1 at the UI attachment boundary; it was not counted as an
interactive attach pass. Independent `pane list --json` succeeded and reported
one live pane. `pane read 1 --lines 1 --json` succeeded with78 text bytes;
contents were not printed. The listed outer rectangle is24x80. Process
executable inspection showed PID95331 running the worktree binary above and
no old main-checkout Panelink executable in that inspection.

The checks used the explicit stable private root
`/Users/sahillamba/.codex/worktrees/7060/panelink/.panelink-agent-contexts` and
permitted local socket access. They sent no pane input. Agent commands still
need that metadata root (or another stable writable private root) and socket
permission in restricted execution environments. No shell profile was changed.

Activation is complete. Run `panelink` in a terminal to attach to the fresh
workspace. The launcher now depends on this worktree and its built binary;
keep both in place until an explicit installation/migration replaces them.
Saved history was preserved, and no provider conversation or ended process
was resumed. Human four-pane Apple Terminal acceptance remains open. Source
changes remain uncommitted and nothing was pushed.
