# P3 agent-facing pane CLI

Status: bounded CLI, metadata/events/renewal and live-scrollback slices delivered; not a Phase 3 completion claim.

This bounded CLI adds `panelink pane ...`, `panelink wait ...`, and
`panelink events ...` over the running default workspace's existing authenticated
control socket. It is generic terminal control; it does not infer OpenCode
identity or add scheduling. Invoke `panelink` once to start/attach the workspace,
then use these commands from another pane or process.

## Commands

```text
panelink pane list [--as NAME] [--json]
panelink pane run [--pane N] [--direction left|right|up|down] [--cwd DIR] [--as NAME] [--json] -- COMMAND [ARG ...]
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

`pane read` returns bounded sanitized live screen text. `--scrollback` returns
only the newest retained normal-buffer history rows, in chronological order,
with at most `--lines K` physical rows. It excludes visible screen rows and
alternate-screen output, even while the alternate screen is active. Wrapped
rows remain separate. The response is bounded by the existing 65,536-byte wire
budget (at most 65,446 text bytes); `truncated` reports omitted rows/text or
normal-history eviction in the current emulator epoch. Reset begins a new
epoch; this flag cannot certify continuity across earlier resets. Saved
history/checkpoints are separate sources and are never substituted.
See [live scrollback evidence](P3-LIVE-SCROLLBACK-RESULT.md).

The server currently accepts printable text through `pane send`; `--enter`
uses one atomic submit operation. The typed-key command supports the listed
keys. A caller context under `agent-contexts/<name>/` is private (0700 parent
directories, 0600 fixed-format context file), where name is selected by
`--as`, `PANELINK_AGENT`, or `agent`. The context file stores operation
identity/sequence metadata only; input contents are never persisted.

For an agent sandbox that cannot write the default application-support
location, set `PANELINK_AGENT_CONTEXT_DIR` to one stable absolute directory in
its writable project. For example, from the project root:

```sh
export PANELINK_AGENT_CONTEXT_DIR="$PWD/.panelink-agent-contexts"
panelink pane list --json
panelink pane read 2 --lines 80 --json
```

Panelink creates private0700 directories and rejects relative paths, symlinks,
foreign owners and non-private existing roots. Keep this exact root for future
invocations so uncertain operations remain visible; there is no automatic
fallback to a fresh context root. This setting grants no additional access:
the agent execution tool must also permit connecting to the workspace's local
Unix socket. `control_unavailable` after context setup may indicate that
separate socket permission restriction. No terminal output is stored in these
context files.

An agent must claim before input, unless `pane send --claim` is used. Claim,
release, input and interrupt use the existing compare-and-set owner revision
and operation key protocol. If a UI or another owner wins that race, the CLI
reports ownership loss/refusal; it does not retry input under a new key.
Uncertain input is reconciled by status and is never retransmitted.
`pane list --json` adds `input_owner_info` with kind none/client, a null or
server-issued generation:lease `id`, `is_caller`, `revision`, and nullable
`expires_at_server_secs`. Both this object and the string `input_owner` come
from the same SessionInfo read. None/you strings stay unchanged; other holders
now use client:<lease> instead of the unsupported agent-role label. Text uses
client:<generation>:<lease> and prints revision/server deadline.

The expiry is a raw deadline on the server runtime's monotonic seconds clock,
not wall time or a local countdown. Pending input can retain an owner past
that deadline; only the server's CAS/ownership checks authorize a write.
These are per-pane observations, not one atomic workspace snapshot. Caller
names (`--as`) and process/provider roles are not authenticated owner identity.
Same-macOS-user access is not per-agent isolation. See
[owner discovery contract](P3-OWNER-IDENTITY-TASK.md).

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
that index. Events never replay terminal effects. The managed `pane run` extension is
described below. See [the workspace contract](P3-LAYOUT-EVENTS-TASK.md).

The dedicated workspace observer proactively resumes the same lease after
25 minutes, before its 30-minute expiry, using only Read and workspace access.
It retains its event cursor across renewal, including after its original pane
retires. Transport-only renewal retries are bounded to three attempts, 15 seconds
apart; expired/unknown grants, generation changes and malformed responses end
the stream. Renewal never creates a replacement lease or replays input. See
[renewal evidence](P3-OBSERVER-RENEWAL-RESULT.md).

## Managed process launch

`pane run` creates one new managed process in a split beside the selected
spatial pane (default pane1, right). It needs a running workspace, an existing
input-capable grant and enough room within the four-pane limit. It uses the
invoking CLI's environment/PATH and cwd, with `PANELINK_*` replaced by the
server. Relative `--cwd` paths resolve against the invocation's cwd. Executable
and argv are passed directly; use an explicit shell with `-c` for evaluation.

All Panelink options precede the mandatory `--`. Tokens after it, including
`--json` and `--as`, belong to the child. Raw Unix argv/path bytes are preserved.
The existing 80-token bound, 65,536-byte aggregate argv bound, filtered
1,024-variable/48KiB environment cap and full 65,536-byte encoded request cap
are enforced with refusal rather than truncation. No new wire/schema or server
launch route is added. No input ownership is claimed and client focus stays local.

The JSON result includes `action:"pane_run"`, `outcome`, the full `operation`
key, `existing_target`, new `target` (or null), and `layout_revision`. Exit0
means Applied launch, not process readiness or task success. Failed, Partial
and Unknown exit1; a Partial/Unknown response can still name a retained child.
Post-reservation errors retain these fields and the exact operation key;
`target:null`/`layout_revision:0` mean unobserved, not proof of no child.
If a valid reply arrived before local acknowledgment storage failed, its new
target/revision remain in the uncertain result.
Pre-reservation and syntax errors use the existing generic CLI error shape.
Results do not echo argv, cwd or environment. The returned target remains the
creation identity even if the process exits before the CLI prints its result.

A dedicated private per-anchor run context serializes invocations and reserves
the operation key before send. This client sends once; the server retains
its exact-request deduplication. A lost/malformed acknowledgment leaves the key
uncertain. The next invocation may settle that key by status but stops without
issuing a substitute launch. Inspect current panes before making a fresh
explicit request. No argv/environment is stored in the context or journal.
See [accepted task](P3-PANE-RUN-TASK.md) and
[completed review and verification](P3-PANE-RUN-RESULT.md).

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
fields, and a stale index after the workspace terminates. This original fixture does not verify
same-generation replacement layout races or real UI human takeover. Live-scrollback acceptance is recorded separately in
[P3-LIVE-SCROLLBACK-RESULT.md](P3-LIVE-SCROLLBACK-RESULT.md).
Full requested build/Clippy/fmt/document checks remain separate gates; see the
worker status/result for actual outputs and counts.
