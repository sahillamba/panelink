# Panelink CLI workflows

Baseline: current source in the selected worktree, reviewed 2026-10-27. The
recognized-agent and scoped-submit interfaces below have bounded private-runtime
checks, including a local Codex `/status` check; broader provider/host acceptance
remains open. The client and server must both support
capability 128 / schema 14. An older running server may need a restart and an
attached UI may need detach/reattach after updating. This reference is not proof
that the installed executable or live server supports those commands.
Help from PATH may belong to another worktree. In the examples, set
`panelink_cli` to the absolute path of the chosen launcher or binary.

## Discover and read

If discovery fails with context or control errors, use
[connection troubleshooting](connections.md). Temporary context storage alone
cannot grant access to the local socket.

```sh
"$panelink_cli" --version
"$panelink_cli" build-info --json
"$panelink_cli" server-info --json
"$panelink_cli" --help
"$panelink_cli" pane list --as worker-a --json
"$panelink_cli" pane read 1 --lines 40 --as worker-a --json
"$panelink_cli" pane read 1 --bottom --lines 12 --as worker-a --json
"$panelink_cli" pane read 1 --scrollback --lines 100 --as worker-a --json
```

Select pane `1` only after discovery confirms the intended process. Resolve a
spatial request such as "on the right" from the returned `size.x`/`size.y` and
dimensions, rather than guessing a pane number. Read that pane before input;
the managed executable can be a shell hosting a different foreground TUI. Use the
same caller name on all commands belonging to that client. `cwd` and `command`
sample the directly managed child's directory and executable without arguments;
inspect their independent status fields. A foreground descendant does not
replace its managed shell's identity. `input_owner_info` identifies the observed
owner and whether it matches this caller; it grants no input authority. Pane
numbers, process metadata and screen titles cannot identify a provider conversation.

Screen reads contain sanitized visible text. Without `--bottom`, a short
`--lines K` selects the top K physical visible rows. `pane read N --bottom
--lines K` and `agent read REF --bottom --lines K` select the last K visible
physical rows, including blanks, and return `row_selection:bottom`. Selection
uses one complete bounded snapshot; an over-budget full screen fails with
`bottom_read_truncated`. `--bottom` and `--scrollback` cannot combine. `--scrollback` reads only retained
normal-buffer history, newest selected rows in chronological order, excluding
visible screen rows and alternate-screen output. Wrapped physical rows remain
separate. Reads are bounded by row and wire budgets. `truncated` can report
omitted content or eviction within the current emulator epoch; a reset begins
a new epoch. Empty history is valid and does not prove a complete transcript.
Live reads never fall back to saved files. See the repository's
[live-scrollback evidence](../../../../docs/progress/P3-LIVE-SCROLLBACK-RESULT.md)
and current handoff for actual verification and limits.

## Authorized input

For one complete text plus Enter submission, use the additive scoped command:

```sh
"$panelink_cli" pane submit 1 'printf "PANELINK_%s\n" DONE' --json --as worker-a
```

The command claims, submits the whole text plus Enter, and releases using three
separately keyed operations. It accepts nonempty printable text up to the
existing input byte limit; shell arguments after TEXT are joined with spaces.
Check the returned target and delivery/release status. When bracketed paste is
enabled by the child, text is framed as paste with Enter outside the closing
marker. `application_submission:unverified` means PTY delivery has not proved
application acceptance; read an observable response. Release occurs only when
the submit result is known `Applied`. Partial, failed, or uncertain input is
never replayed or released. If release fails after `Applied`, delivery remains
confirmed and release is reported unconfirmed. A retained claim by the same
caller makes scoped submit fail with `input_already_claimed`.

Legacy retained ownership remains available:

```sh
"$panelink_cli" pane claim 1 --as worker-a --json
"$panelink_cli" pane send 1 'printf "PANELINK_%s\n" DONE' --enter --as worker-a --json
"$panelink_cli" pane release 1 --as worker-a --json
```

These are separate operations. A retained claim blocks other writers until a
known release. No command steals it. The attached UI no longer holds idle-pane
claims; each UI input batch uses short per-batch ownership. That lock does not
protect a human's unfinished draft between input batches. Check every outcome
and inspect the resulting state rather than sending uncertain input again.

`pane send N TEXT --claim --enter` combines claim and submit. Without `--enter`,
it only types. Text must be printable; `pane keys N KEY` accepts `Enter`, `Tab`,
`Esc`, `C-c`, `Up`, `Down`, `Left`, `Right` and `Backspace` in this baseline.
Shell-quote text; never interpolate untrusted output as shell code.
`pane interrupt N` signals the target and also requires ownership. `pane close N`
terminates that pane's process; closing the last pane ends the workspace.

## Recognized foreground agents

These commands require the client and server to negotiate capability 128 and
schema 14. They address the default workspace, preserve the caller context, and
have only the bounded acceptance recorded in the repository handoff:

```sh
"$panelink_cli" agent list --json --as worker-a
"$panelink_cli" agent get 1 --json --as worker-a
"$panelink_cli" agent read 1 --lines 80 --json --as worker-a
"$panelink_cli" agent prompt 'TARGET_REF' Please inspect this error --wait --json --as worker-a
"$panelink_cli" agent wait 'TARGET_REF' --until idle --timeout 30 --json --as worker-a
```

`agent list` reports each current pane, including unrecognized occupants. `get`
and `read` accept a pane number or returned `agent_ref`; read is screen-only,
defaults to 80 lines and permits 1 through 1024. Recognized references pin the
full target and process tuple, formatted as
`GEN:WORKSPACE:PANE:SESSION:OCCUPANT@PID:START_SECONDS:START_MICROSECONDS:KIND`.
A reference is stale if the pane occupant or foreground process changes.
`prompt` requires a returned process-bound reference and an observed `idle` screen; it refuses
`blocked`, `working`, and `unknown`. Text arguments are joined with spaces and
use the same bounded printable-text limit as pane submit. Optional `--wait`
waits for observed activity, then the requested state; it does not prove that
this prompt completed. `wait` defaults to `idle` or `blocked`, a 30-second
timeout, and supports `--until idle|working|blocked|unknown`; timeout range is
1 through 300 seconds. Neither state nor wait output establishes provider
conversation identity or per-prompt completion.

Process recognition uses the native PTY foreground-group leader PID, kernel
birth identity and executable basename `codex` or `opencode`. Wrappers are
`unknown`. State is a bounded terminal-screen heuristic over at most the last
80 rows and first 240 Unicode scalar values per row; it is not an agent
readiness guarantee. An older server reports this optional feature unsupported.

## Wait and observe

```sh
"$panelink_cli" wait 1 --text 'ready' --timeout 20 --as worker-a --json
"$panelink_cli" wait 1 --quiet 2 --timeout 10 --as worker-a --json
"$panelink_cli" wait 1 --change --timeout 10 --as worker-a --json
"$panelink_cli" wait 1 --exit --timeout 30 --as worker-a --json
"$panelink_cli" events --pane 1 --timeout 10 --as worker-a --json
```

Waits pin the full target resolved at their start. A geometry-only layout
change (host resize, divider move) that keeps that target at the same index does
not end the wait. A replaced occupant, closed pane or renumbered index returns
`stale_target`; run `pane list`, compare the target and re-issue deliberately.
Text matching uses the whole visible sanitized screen (bounded by 64 KiB/1,024
rows), not scrollback or a provider API. `--regex` is a bounded subset, not a
general regex engine. Quiet measures unchanged screen revisions and does not
mean the process is idle or finished. Timeout is nonzero. Exit may report an
unavailable status when the pane disappeared before its status was read.
Builds before this change matched only the top 24 rows and ended every wait on
any layout revision; check the executable you are using.

To detect completion of a long task in an unrecognized pane, ask it to print a
marker that the prompt does not contain literally (for example describe it as
"DONE followed by a hyphen and 7Q4"), confirm start with a short
`wait --change`, then make one blocking `wait --text MARKER --timeout N`. A
marker never prints while an agent waits on an approval screen, so choose a
bounded timeout and read the pane when it expires.
Events are bounded metadata observations, including negotiated layout events.
`events --pane N` pins the initial full target rather than following a replacement
at that index. After a gap, expiry or restart, obtain fresh authoritative state.

## Launch a managed process

```sh
"$panelink_cli" pane run --pane 1 --direction down --cwd /absolute/project --as worker-a --json -- /bin/zsh -f
```

This creates a fresh process in a new split of the already running default
workspace; it does not submit text to the existing pane. Everything after `--`
is literal child argv. The invoking CLI supplies its cwd and filtered environment.
The result contains the operation and new full target; Applied does not prove
readiness or task completion. At most four panes are supported. A partial or
unknown launch may have created a child; preserve the context and operation key.

## Projects and workspace tabs

```sh
"$panelink_cli" workspace list
"$panelink_cli" workspace project create Editors Main
"$panelink_cli" workspace tab create PROJECT_ID Review
"$panelink_cli" workspace open PROJECT_ID TAB_ID
```

Use actual IDs from `workspace list`. Creation and opening may start processes
and require task authorization. Each workspace has its own server, panes and
contexts. Legacy `pane`, `wait`, `events`, `history` and bare `panelink` commands
continue to address Default/Main, even while another tab is visible. There is no
implicit active-tab routing for these commands. For another routed server, use
its discovered endpoint with the matching low-level `control` interface; do not
invent high-level workspace selectors. `workspace stop` ends its processes;
`workspace remove` requires it stopped and retains its saved files.

Provider binding is an opt-in pinned adapter behind a dedicated
`serve --opencode-bridge` workspace. Explicit prepare/resume and verified provider
identity are required. Read the matching
[recovery guide](../../../../docs/progress/P4-MANUAL-RECOVERY.md) before acting;
ordinary history reads do not resume conversations.

## Saved state and lifetime

| Need | Command or action | Meaning |
| --- | --- | --- |
| Open/reattach | `panelink` in an interactive terminal | Starts a workspace if needed |
| Split/focus | Ctrl-B then `r/l/u/d`; arrows or Tab | Fresh shell split; baseline at most four panes |
| Help/detach | Ctrl-B then `?`; Ctrl-X | Detach leaves server-owned processes running |
| Inspect saved output | `history list`, `history show TARGET`, `history search TARGET TEXT` | Retained rows and screen checkpoints, separate from live reads |
| Inspect saved geometry | `layout show [--previous]` | Historical identities and geometry |
| Browse recovery | `recovery view [--previous]` | Offline read-only viewer |
| Relaunch layout | `recovery relaunch [--previous]` | Starts fresh default shells; a mutation |
| End workspace | `stop` | Stops all of that workspace's processes |
| Delete history | `history clear TARGET` | Requires stopped workspace; use the ID returned by history list |
| Discard temporary history writes | `history cleanup` | Requires stopped workspace; preserves committed history |

Prefix command fragments in the table with the chosen executable. Saved history
is bounded (1,024 rows/8 MiB per session) and may lag live output. It cannot
restore a process, recover every overwritten screen, or resume an agent's
conversation. Provider identity/resume and full reboot acceptance need their
own current evidence. The older disposable `probe` has different lifetime and
input behavior; its quit action stops its child.

## Build and control diagnostics

`build-info --json` is local. `server-info --json` makes a read-only schema-15
query after capability 256 is negotiated, returning separate client/server
revision, source fingerprint, source-dirty flag and connection-selected
capabilities. The FNV-1a fingerprint distinguishes source contents; it is not
an artifact checksum, security attestation or reproducible-build proof. Missing
server support returns `server_identity_unsupported`, never client identity.

Connection/input errors include fixed `cause`, `stage`, `transport_stage` and
reconciliation information where observed. Pending operation identifiers contain
no input payload. Input refusals can include `input_owner_info`, labeled
`owner_observation:before_operation`; `current_ownership:unverified` prevents
mistaking a prior lease for current authority. Retain the same context and
inspect/reconcile the pending key. No error authorizes replay.

A finite `agent wait` holds one authenticated connection and its private context
lock, checking the exact foreground process on every observation. Concurrent
clients need distinct stable caller names. Connection loss is reported, without
reconnecting, changing caller identity or replaying a prompt.
