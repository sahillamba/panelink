# P3 agent-facing pane CLI

Status: implementation slice in progress; not a Phase 3 completion claim.

This bounded CLI adds `panelink pane ...`, `panelink wait ...`, and
`panelink events ...` over the running default workspace's existing authenticated
control socket. It is generic terminal control; it does not infer OpenCode
identity or add scheduling. Invoke `panelink` once to start/attach the workspace,
then use these commands from another pane or process.

## Commands

```text
panelink pane list [--as NAME] [--json]
panelink pane read N [--lines K] [--scrollback] [--json]
panelink pane claim N | release N [--json]
panelink pane send N TEXT [--enter] [--claim] [--json]
panelink pane keys N Enter|Tab|Esc|C-c|Up|Down|Left|Right|Backspace... [--json]
panelink pane interrupt N | close N [--json]
panelink wait N --text STRING [--regex] --timeout SECS [--json]
panelink wait N --quiet SECS --timeout SECS [--json]
panelink wait N --exit|--change --timeout SECS [--json]
panelink events [--pane N] [--timeout SECS] --json
```

Pane indices are 1-based in the current spatial layout. Each action first
resolves the selected entry to its complete generation/workspace/pane/session/
occupant target and checks the layout revision again before acting. If the
target/index changed, the CLI reports a stale-target error and does not select
the replacement occupant. JSON output is one object per line with stable action,
target, status, and condition fields. `pane list` samples the direct managed child's current `cwd` and executable
path (`command`, without arguments), separately from the unchanged layout
reply. JSON includes `metadata_source:"managed_child"`, a generation-relative
`sampled_at_mono_ms`, and independent `cwd_status`/`command_status` values.
Failed or unsupported samples remain null with a fixed reason. A shell's `cd`
and direct `exec` are reflected on refresh; a foreground descendant does not
replace its managed shell's identity. This is a point-in-time observation, not
an atomic cwd/executable pair. Paths are private live read metadata and are
never written to launch records, saved layout or diagnostic output. Capability16
and schema12 use a dedicated per-target read-only context; older grants are not
widened. See [the metadata contract](P3-PANE-METADATA-TASK.md).

`pane read` returns bounded sanitized live screen text. `--scrollback` names the
existing live-scrollback read source; the current PTY adapter rejects that
source as unavailable, and the CLI reports the limitation rather than
substituting saved history. Saved checkpoints and live terminal state remain
separate.

The server currently accepts printable text through `pane send`; `--enter`
uses one atomic submit operation. The typed-key command supports the listed
keys. A caller context under `agent-contexts/<name>/` is private (0700 parent
directories, 0600 fixed-format context file), where name is selected by
`--as`, `PANELINK_AGENT`, or `agent`. The context file stores operation
identity/sequence metadata only; input contents are never persisted.

An agent must claim before input, unless `pane send --claim` is used. Claim,
release, input and interrupt use the existing compare-and-set owner revision
and operation key protocol. If a UI or another owner wins that race, the CLI
reports ownership loss/refusal; it does not retry input under a new key.
Uncertain input is reconciled by status and is never retransmitted. Existing
session-info only distinguishes an unowned pane from a client lease; `agent`
does not identify a particular other automation client. Same-macOS-user access
is not per-agent isolation.

`wait` uses atomic screen/metadata reconciliation plus finite event watches.
Text is matched against the sanitized visible screen, regex matching is a
bounded original subset (literal scalars, `.`, `*`, `+`, `?`, and `^`/`$`
anchors). `--quiet` measures unchanged screen revisions; `--exit` reports
`exited (code N)` / `exited (signal N)` from the pane reply's confirmed exit
status (schema 10, L18). It falls back to `exited (status unavailable)` only
when the exited pane collapsed out of the layout before its status was read. Timeout exits nonzero.

`events` streams JSON lines until its timeout or Ctrl-C. A dedicated context
optionally negotiates workspace layout notifications (capability 8/schema 11).
It emits `layout_changed` with generation/workspace/sequence/revision/empty,
and `layout_resync` after a journal gap; it reconciles current topology after
these notifications. Derived pane open/close observations can coalesce
intermediate layouts. Existing pane screen/owner/exit watches continue
independently; their expired cursors require reconciliation before continuing.
Servers and existing grants without capability 8 use bounded layout polling.
`--pane N` pins the initial complete target, even after another pane occupies
that index. Events never replay terminal effects. `pane run` remains a separate
managed-launch extension. See [the workspace contract](P3-LAYOUT-EVENTS-TASK.md).

The dedicated workspace observer proactively resumes the same lease after
25 minutes, before its 30-minute expiry, using only Read and workspace access.
It retains its event cursor across renewal, including after its original pane
retires. Transport-only renewal retries are bounded to three attempts, 15 seconds
apart; expired/unknown grants, generation changes and malformed responses end
the stream. Renewal never creates a replacement lease or replays input. See
[renewal evidence](P3-OBSERVER-RENEWAL-RESULT.md).

## Parent fixes — 2026-10-22

- The regex subset was broken: `+` never advanced, escaped `\.` matched any
  scalar, and `[...]` classes were unsupported. It was rewritten as a bounded
  token NFA that supports classes, negated classes and ranges, with more unit
  cases.
- `pane send` re-resolved the index after an applied send. Input that made
  the child exit (for example `exit`) was therefore reported as
  `pane_not_found`/`stale_target`, even though the server had applied it to
  the exact target. The request already carries the complete target, so the
  post-send re-resolution was removed.
- `wait --exit` now succeeds when its pinned target has left the layout (the
  exited pane collapsed) and the loop reported stale/unavailable. It never
  selects a replacement occupant.
- Two clippy findings were fixed. `p3_agent_cli` now passes 2/2 on three
  consecutive runs; before the fixes it failed consistently.

## Verification record

The isolated real-server integration scenario currently passes: one test covers
private `PANELINK_HOME`, list/read, no-claim refusal, CAS claim/send/release,
text match and timeout, quiet/change/exit waits, close-event observation, JSON
fields, and a stale index after the workspace terminates. This does not verify
same-generation replacement layout races, real UI human takeover, or scrollback
because those capabilities are not exposed by current adjacent contracts.
Full requested build/Clippy/fmt/document checks remain separate gates; see the
worker status/result for actual outputs and counts.
