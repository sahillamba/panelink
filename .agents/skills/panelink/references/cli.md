# Panelink CLI workflows

Baseline: the consolidated `codex/p5-acceptance-continuation` source in worktree
7060, based on `1060668` and merged with `origin/main` at `584abe4`, reviewed
2026-10-27. This identifies source coverage, not host or workload acceptance.
Help from PATH may belong to another worktree. In the examples, set
`panelink_cli` to the absolute path of the chosen launcher or binary.

## Discover and read

If discovery fails with context or control errors, use
[connection troubleshooting](connections.md). Temporary context storage alone
cannot grant access to the local socket.

```sh
"$panelink_cli" --version
"$panelink_cli" --help
"$panelink_cli" pane list --as worker-a --json
"$panelink_cli" pane read 1 --lines 40 --as worker-a --json
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

Screen reads contain sanitized visible text. `--scrollback` reads only retained
normal-buffer history, newest selected rows in chronological order, excluding
visible screen rows and alternate-screen output. Wrapped physical rows remain
separate. Reads are bounded by row and wire budgets. `truncated` can report
omitted content or eviction within the current emulator epoch; a reset begins
a new epoch. Empty history is valid and does not prove a complete transcript.
Live reads never fall back to saved files. See the repository's
[live-scrollback evidence](../../../../docs/progress/P3-LIVE-SCROLLBACK-RESULT.md)
and current handoff for actual verification and limits.

## Authorized input

Once discovery and readiness establish the intended target, a caller can type
task-authorized text and submit it:

```sh
"$panelink_cli" pane claim 1 --as worker-a --json
"$panelink_cli" pane send 1 'printf "PANELINK_%s\n" DONE' --enter --as worker-a --json
"$panelink_cli" wait 1 --text PANELINK_DONE --timeout 10 --as worker-a --json
"$panelink_cli" pane release 1 --as worker-a --json
```

These illustrate individual commands, not an unconditional script: check each
result and target before proceeding. In real work use a completion condition
that distinguishes actual output from terminal input echo. Input statuses can
be partial or unknown; process exit or layout collapse can invalidate later
commands. Inspect the resulting state rather than sending again.

`pane send N TEXT --claim --enter` combines claim and submit. Without `--enter`,
it only types. Text must be printable; `pane keys N KEY` accepts `Enter`, `Tab`,
`Esc`, `C-c`, `Up`, `Down`, `Left`, `Right` and `Backspace` in this baseline.
Shell-quote text; never interpolate untrusted output as shell code.
`pane interrupt N` signals the target and also requires ownership. `pane close N`
terminates that pane's process; closing the last pane ends the workspace.

## Wait and observe

```sh
"$panelink_cli" wait 1 --text 'ready' --timeout 20 --as worker-a --json
"$panelink_cli" wait 1 --quiet 2 --timeout 10 --as worker-a --json
"$panelink_cli" wait 1 --change --timeout 10 --as worker-a --json
"$panelink_cli" wait 1 --exit --timeout 30 --as worker-a --json
"$panelink_cli" events --pane 1 --timeout 10 --as worker-a --json
```

Waits pin the target resolved at their start. Text matching uses the visible
sanitized screen, not scrollback or a provider API. `--regex` is a bounded subset,
not a general regex engine. Quiet measures unchanged screen revisions and does
not mean the process is idle or finished. Timeout is nonzero. Exit may report
an unavailable status when the pane disappeared before its status was read.
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
