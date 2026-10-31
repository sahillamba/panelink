# Panelink

Panelink is an experimental Rust terminal multiplexer for Apple Silicon Macs.
It runs inside your existing terminal (Apple Terminal first). Your shells live
in a background workspace, so closing the window does not kill them.
Panelink's original application code is intended for the MIT license.

## Button controls in this source build

Controls use filled backgrounds with visible focus, pressed, disabled and pending
states. Press and release inside the same button to activate; dragging away or
pressing Escape cancels. Keyboard shortcuts remain available. Pane Menu and
right-click open one short menu: a Split row (Left/Right/Up/Down), Zoom, Swap,
Copy mode and Close pane. The tab strip has separate select/close targets, a
`+` after the last tab and a `≡` global menu (All tabs, Saved layout, Keys &
help, Detach) pinned right. Select an inactive tab before closing it; Close ends its processes after
confirmation, while Detach leaves them running. Dialog Tab/Shift-Tab moves local
focus; rename text editing stays in the editor.

`NO_COLOR` presence or `TERM=dumb` selects reduced-color controls at setup.
This checkout must be built explicitly; it does not replace the installed
executable or restart its server. Native host/theme acceptance remains separate.
See [the implementation evidence](docs/progress/button-redesign/PROGRESS.md).

## Quick start

Build once and put the launcher on your PATH (see [SETUP.md](docs/SETUP.md)
for installing Rust):

```sh
cargo build --locked
mkdir -p "$HOME/.local/bin"
ln -sfn "$PWD/bin/panelink" "$HOME/.local/bin/panelink"
```

Then, in any terminal:

```sh
panelink
```

That's it. The first run starts your workspace with one shell. Later runs
reattach to it. The client version appears in the bottom-right pane margin
(`v0.1.8` in this local build), matching `panelink --version`.
Use `panelink build-info --json` for the client revision/source fingerprint and
`panelink server-info --json` for the running server's identity and negotiated
capabilities. An older server reports identity unsupported; the client version
does not identify that server.

| Action | How |
| --- | --- |
| Pane actions | Click `Menu` in a pane border, or press Ctrl-B then `s` |
| Split a pane | Choose Split from its Menu, or right-click it and pick Left/Right/Up/Down |
| Navigation mode | Press **Control**-B (the Control key, not Command). A `NAV` bar appears at the bottom |
| Shortcut help | Ctrl-B, then `?`; Esc or `?` closes help |
| ...then split | `r` right, `l` left, `u` up, `d` down (also `%` and `"`), or `s` for the menu |
| ...then switch pane | Arrow keys, Tab or `o` (stays in NAV mode so you can keep moving) |
| ...then close focused pane | `x`, then `y` or click `[Close]` to confirm; `[Cancel]` dismisses; the last pane ends the workspace |
| ...then leave NAV mode | Esc (or wait 4 seconds); Ctrl-X detaches |
| Focus a pane | Click it |
| Scroll a pane | Wheel over it, click the right-margin track to page, or drag its thumb to seek normal history continuously; bottom margin shows History / New output / History truncated plus `[Back to live]`; typing returns to live output |
| Scroll with keys | Ctrl-B then PageUp/PageDown to page, Home for oldest retained output, End for live output |
| Copy text | Drag across pane content and release; copying requires negotiated row-wrap metadata |
| Keyboard copy mode | Ctrl-B then `c`; move with arrows or `h`/`j`/`k`/`l`, press Space to set/clear an anchor, then Enter or `y` to copy a non-empty range |
| Zoom a pane | Ctrl-B then `z`; the selected pane's PTY resizes to fill the current workspace view |
| Swap pane locations | Ctrl-B then `m`; choose another pane with arrows or `h`/`j`/`k`/`l`, then Enter; Esc cancels |
| Resize panes | Drag the divider and release; or Ctrl-B then `e` → `[-]`/`[+]` → Apply |
| Close a pane | Click its `x`, then `[Close]` (Cancel is selected by default; `y` also confirms); `[Cancel]`, `n` or Esc dismisses |
| List/close panes from a shell | `panelink panes`; `panelink close 1` (or `2`) |
| Detach (shells keep running) | Ctrl-X |
| Reattach to the legacy Default/Main workspace | `panelink` |
| Close the workspace | `exit` in every shell, or `panelink stop` |

Normal-history scrolling keeps the last screen visible while the next view is
fetched. Normal-history wheel reads do not wait for a separate range probe.
The scrollbar uses the pane's existing right padding; its range is
refreshed when you scroll. Track clicks page through history, and dragging the
thumb updates the displayed normal history continuously while held. An initial click on an
unknown range reads its extent. Alternate-screen apps such as OpenCode
keep their own scrolling behavior and do not show a Panelink history scrollbar.
For mouse-reporting alternate-screen apps, wheel input is sent directly to the
app. A short burst can retain up to 32 unsent wheel events per pane for up to
one second while an earlier event is in flight; typing, other pointer actions,
or a view/target change cancels unsent events. Normal-history scrolling is
unchanged. Clients and servers that negotiate `scoped_mouse_wheel` handle each
wheel event in one scoped transaction. Older servers use the query-first legacy
path; installing a new client alone does not enable the fast path. Server restart
ends its running panes, so keep activation separate from client installation.

The visible pane Menu exposes split (all four directions), zoom, swap, copy and
close; saved layout, shortcut help and detach are in the strip's `≡` menu. Mouse controls use terminal cell
coordinates; touch gestures work when the host terminal translates them into
mouse press, drag and release events. Native touch hardware acceptance remains
open. See the [pointer controls plan and evidence](docs/progress/P5-POINTER-CONTROLS-2026-10-27.md).

Current limits: at most four panes per workspace. The rendered workspace is
bounded to 80 rows by 240 columns; larger terminal windows keep that bounded
workspace in the upper-left area. The attach client forwards ordinary
Ctrl-letter keys, Alt/Meta characters when delivered by the host, F1–F12,
Delete/Insert/Page Up/Page Down and Shift-Tab. Ctrl-B remains navigation and
Ctrl-X detaches; Ctrl-C goes to the focused child. Modified function/navigation
keys and application pointer clicks remain unsupported. Wheel events reach
alternate-screen applications that enable mouse reporting; without that mode,
Panelink reports the limitation. Normal screens scroll through retained live
history. Pane controls and scrollbars use existing one-cell margins. Help consumes
typing, paste and mouse actions while open. An experimental pinned OpenCode
adapter is available only through a dedicated `serve --opencode-bridge` launch,
with explicit prepare/resume commands; see the [provider guide](docs/progress/P4-MANUAL-RECOVERY.md).
Scoped pinned managed acceptance passes; see the [integrated result](docs/progress/P4-INTEGRATED-RESULT.md). The older disposable probe retains its
narrower input map.

Local help, navigation and Ctrl-X remain available while the attach client
waits for the server. If typing backs up, a busy status appears. Ctrl-X still
detaches; any keys that could not be queued are discarded with a notice after
the terminal is restored. Uncertain input is never automatically resent.

Starting a split prepares the new shell and saves its launch records in a
background worker while existing panes keep running. Splits run one at a time;
resize and close may report busy until the pending split finishes. A launch
storage failure leaves existing shells running and prevents further launches
until the workspace is restarted.

Agents and scripts can use `panelink pane list --json` to discover panes and
`panelink events --json --timeout 30` to watch activity and layout changes.
`events --pane N` follows the pane selected when the command starts; closing it
does not switch the stream to a replacement at that index. See the
[agent CLI contract](docs/progress/P3-AGENT-CLI.md) for commands and limits.

If an agent cannot write Panelink's default caller-context directory, give it
a stable writable directory for its private operation metadata:

```sh
export PANELINK_AGENT_CONTEXT_DIR="$PWD/.panelink-agent-contexts"
panelink pane list --json
panelink pane read 2 --lines 80 --json
```

The CLI creates the directory with mode 0700. Keep using the same directory so
uncertain operations can be reconciled. The agent also needs permission to
connect to Panelink's local control socket.

`pane list --json` also reports `input_owner_info`: the server-issued owner
ID, whether it is your caller context, its revision and server-clock expiry.
Other holders are labeled clients; names and agent roles are not inferred.
Ownership changes still require the server's normal claim/release checks.

To start a command in a new pane of an already running workspace:

```sh
panelink pane run --pane 1 --direction down --json -- /bin/zsh -f
```

The command uses your invoking shell's environment and working directory; add
`--cwd DIR` before `--` to choose another directory. Everything after `--` is
literal child argv. The result gives the new full pane/session target, rather
than a permanent pane number. A successful launch does not mean the command
has finished. The workspace still supports at most four panes.

Workspace state lives in `~/Library/Application Support/Panelink` (private,
mode 0700). Set `PANELINK_HOME` to use a different directory. The background
server's output goes to `server.log` there.

Projects group separately running workspace tabs. The interactive client
(plain `panelink` on a TTY) shows a top project/tab row and supports in-client
create, rename, close and switch. With no visible tabs it shows an empty state
(New tab / Detach) and starts no process. On a non-TTY it prints a notice.

```sh
panelink workspace list
panelink workspace project create Editors Main
panelink workspace tab create 2 Review
panelink workspace tab rename 2 4 "New name"
panelink workspace tab close 2 4
panelink workspace select 2 4
panelink workspace open 2 4
panelink workspace stop 2 4
panelink workspace remove 2 4
```

Use the IDs printed by `workspace list`; the example assumes a fresh registry.
`workspace open` without IDs uses the saved selection. Explicit IDs open that
exact workspace. Each workspace has its own server, panes, history and client
contexts; detaching leaves its processes running. Remove requires a stopped
workspace and removes its registry entry while retaining saved files. Legacy
commands such as `stop`, `panes` and `history` keep addressing Default/Main.
Registry initialization adds that workspace in place; its entry cannot be
removed while legacy commands remain supported. Projects are named groups;
no project-root discovery is included.

Tab controls in the interactive client:
- `+` or `Ctrl-B +` creates a managed "Shell N" tab and switches to it.
- `Ctrl-B t` (or right-click a tab) opens tab actions (New tab / Rename / Close).
- `Ctrl-B w` opens the searchable All tabs chooser.
- `Ctrl-B n/p` switches next/previous tab; `[`/`]` switches project.
- At most eight tabs per project; `+` follows the last tab, `≡` (All tabs, Saved layout, Keys & help, Detach) is pinned right, and `[<] [>]` appear only when tabs overflow.
- Rename uses a text field with Save/Cancel (Enter/Esc).
- Close asks confirmation with Cancel as the default; closing the last tab
  returns to the empty state.
- Closing a tab requires it to be the active route; inactive tabs show
  "open this tab first to close it".

Plain `panelink` and raw `attach` both use the workspace system; plain
`panelink` attaches with the tab strip, while raw `attach` remains headerless.
See the [routing result](docs/progress/P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md)
and [tab integration evidence](docs/progress/P5-TAB-INTEGRATION-2026-10-25.md).

Drag copying freezes the source at mouse press, preserving it while output
continues. Resizing, switching buffers or history views, and opening a modal
cancel the drag. Copy runs asynchronously in the attached client and reports
fixed status messages. A peer without row-wrap metadata disables drag copying.
Automated checks use fake clipboard sinks; real Apple Terminal/clipboard
acceptance remains open.

Keyboard copy mode (`Ctrl-B c`) is a bounded prototype for the pane's currently
visible live or history view. It moves a local caret and copies the anchored
range from a frozen snapshot, so ordinary live output does not replace the
view being selected. Enter or `y` with no anchor exits without copying; Esc
cancels, and Ctrl-X detaches. Other keys, paste, and mouse input stay local
while the mode is active. Resize, focus, buffer/view, route, modal, or wrap
identity changes cancel it. The existing attached-client clipboard queue is
used. `Ctrl-B c` remains a prototype shortcut assumption. Typed-protocol
Repeat events for Enter/`y` are guarded; classic streams that report repeats
as indistinguishable Press events cannot provide the same repeat guarantee.
Automated client, compositor and clipboard-free PTY checks pass. Real
Apple Terminal/clipboard acceptance remains open. See the [keyboard copy
result](docs/progress/P5-KEYBOARD-COPY-RESULT-2026-10-25.md).

Pane controls are bounded prototypes. Zoom is server-authoritative per
workspace and resizes the selected pane's actual PTY; other panes and their
processes keep running at their existing PTY sizes. Unzoom restores the exact
underlying split geometry for the current root size. Swapping changes two
panes' locations in the same workspace without moving or replacing their
processes. Divider preview is local; the chosen absolute position is sent once
on Enter, Apply, or release of a divider drag. Split/layout changes are refused
while zoom is active; unzoom first.
If a captured pane target or layout revision becomes stale, the local chooser
cancels. Results are asynchronous and an uncertain operation is reported with
a fixed status; it is never replayed automatically. These operation ratios and
zoom state do not establish physical-reboot restoration and do not change the
saved-observation codec. The bindings are prototype choices, not a final
shortcut configuration. Typed Enter/Esc repeat and release events are handled
locally; classic streams that expose repeats as indistinguishable press events
cannot provide the same repeat guard. See the [pane operations contract](docs/progress/P5-PANE-OPERATIONS-CONTRACT-2026-10-25.md)
and [specification](docs/SPEC.md#bounded-pane-operations-prototype).
Automated verification includes the full serial Rust suite (1,399 passed,
15 ignored), final client123/0 and outer-terminal3/0 checks, plus real four-PTY
process continuity. Human Apple Terminal, real clipboard, physical reboot and
representative workload acceptance remain open; see
[the integration evidence](docs/progress/P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md).

After an abrupt server kill, a later `panelink` invocation can recover its stale
socket and start a new workspace generation. This starts fresh processes;
saved history is separate. An uncatchable kill cannot run graceful cleanup or
flush pending captures, and arbitrary descendants may survive it. Use
`panelink stop` for orderly workspace shutdown.

The server now saves its latest sanitized visible normal and alternate screen
checkpoints per pane in the private `server/history/` directory (under
`PANELINK_HOME`, if set). They are read-only recovery data, not live-process
state or a complete transcript. An initial `serve` response reports whether
capture started; writes run in a bounded background worker, so saved content
can lag the live screen. A later write or shutdown failure appears in
`server.log` while shells continue running. Normal scrollback rows are saved within a 1,024-row/8 MiB
per-session limit. For a running pane, `panelink pane read 1 --scrollback --lines 100 --json`
reads the live normal-buffer history tail, excluding visible screen rows.
It reports truncation within the current emulator epoch and does not read
saved files. `panelink history list` lists saved target IDs;
`panelink history show TARGET` reads retained rows and latest normal/alternate
screens; `panelink history search TARGET TEXT` searches that saved content,
labeled by source. `TARGET` is the five-number ID printed by `history list`.
Stop the workspace before `panelink history clear TARGET`; this deletes only
Panelink history, not provider conversations. Ended sessions expire after
30 days at the next server startup. History cannot restore a live process or
content overwritten before it was captured.

After an interrupted history write, the next server startup discards recognized
uncommitted temporary files before capture starts. You can also run
`panelink history cleanup` while the workspace is stopped. This preserves saved
rows and checkpoints; it does not recover unfinished writes. Unsafe leftovers
cause cleanup to fail, and a startup cleanup failure leaves shells running with
history capture unavailable.

Malformed or truncated saved rows/checkpoints are refused as whole files.
`history show` and `search` return an error without partial content if any saved
component for that target is invalid. The files are preserved; cleanup and clear
do not repair them. Healthy targets can still be read by their explicit IDs.
The current format validates structure and safe text, but has no checksum to
detect every content change. See the [recovery policy](docs/progress/P5-COMMITTED-CORRUPTION.md).

Panelink also saves the latest layout in two bounded 280-byte records under
`server/layout/`. `panelink layout show` reads the latest saved layout;
`panelink layout show --previous` reads the preceding saved generation.
These commands work without starting a workspace. Layouts contain historical
pane identities and geometry; saved rows and screens remain separate sources.
Legacy 160-byte layouts remain readable through the supported migration.
Background saves can lag live changes, and malformed records are refused and
preserved. This storage slice does not establish full reboot recovery.

**Implementation roadmap:** [ROADMAP.md](ROADMAP.md) tracks phases and
verification evidence. Selected components retain their
[third-party notices](THIRD_PARTY_NOTICES.md).

- [Research and architecture recommendation](docs/PANELINK-RESEARCH.md)
- [Product questions and spec handoff plan](docs/PANELINK-DISCOVERY.md)
- [Working implementation specification](docs/SPEC.md)
- [First prototype preparation and requested Rust skill](docs/PREBUILD.md)
- [Panelink usage skill for agents](.agents/skills/panelink/SKILL.md): when to
  use Panelink, supported CLI workflows, targeting, ownership and recovery limits.
  Maintained alongside feature changes under [AGENTS.md](AGENTS.md).

## Alpha release, install and security review

- [docs/RELEASE.md](docs/RELEASE.md): locked local release build with a
  hash manifest, `scripts/install.sh install|upgrade|uninstall` (no network,
  no sudo, keeps state on upgrade, rollback copy), unsigned/not notarized.
- [docs/SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md): SEC-01..SEC-08
  evidence pack for an organization's review (not an approval).

## Automated development checks

GitHub Actions runs these checks on standard Apple Silicon macOS runners for
pull requests and changes to `main`. See [CI.md](docs/CI.md) for caching,
failure evidence, local reproduction, and acceptance limits.

Run `python3 scripts/verify_non_ui.py` for the delivered Rust/Python tests,
including the vendored input decoder suite, formatting, strict Clippy, build,
component source fingerprints, and documentation checks. It uses offline
Cargo commands and reports failures automatically. Ignored host/pathname tests
and live provider acceptance remain separate, uncompleted gates; a successful
component run does not certify the five non-UI milestones.

Run `cargo run --locked --offline --release --example live_history_bench` for
the bounded synthetic two-pane history workload. It writes only private
temporary fixtures and emits a JSON report. See
[measured results and limits](docs/progress/P5-LIVE-HISTORY-BENCH.md).

## Advanced commands

The commands below are lower-level tools for development and testing. You do
not need them for normal use. Run `panelink --help` for the list. `panelink probe`
opens the old disposable single-shell probe. Rebuild after source changes; the
launcher runs `target/debug/panelink` from this checkout.

`doctor` reports the target and the limits of the current prototype, and
exits with status 1. Unknown commands or extra arguments exit with status 2.

## Experimental terminal probe

On Apple Silicon macOS, compile the checked-in terminfo entry into a private temporary directory, then launch one disposable child from an interactive terminal:

```sh
probe_terminfo_dir="$(mktemp -d "${TMPDIR:-/tmp}/panelink-terminfo.XXXXXX")"
/usr/bin/tic -x -o "$probe_terminfo_dir" docs/terminfo/panelink-probe.terminfo
cargo run --locked -- pty-probe --terminfo "$probe_terminfo_dir" -- /bin/sh
```

Use Ctrl-X (hold Control and press X) only when finished: it quits the entire disposable probe and stops its child, returning to the outer shell. It does not detach or leave the child running. To leave `vi` and continue checking the inner shell, use the editor's own quit command (`Esc`, then `:q!`, then Enter). Ctrl-C is forwarded to the child. The initial input profile supports printable characters, Enter, Tab, Backspace, Escape, arrows, Home/End and bracketed paste. Ctrl-D, Alt and function-key shortcuts are not forwarded yet. Child output goes through the terminal engine and snapshot renderer; child clipboard requests cannot reach the host clipboard.

Ordinary updates now repaint changed cells; wide-character footprint changes can repaint the affected row. Startup and resize establish a full frame. The host retains a bounded baseline only after all of that frame's bytes are accepted, so partial writes cannot advance it prematurely.

This is an unvalidated feasibility probe. It requires permission to open `/dev/tty`; the current agent sandbox may prevent that. Cleanup attempts to restore saved termios exactly and resets visual modes to conservative defaults, then stops/reaps the child within bounded cleanup windows. SIGKILL, abort and unhandled fatal signals cannot run cleanup. See [probe evidence](docs/progress/P2-PROBE-RESULT.md) and the [manual checklist](docs/progress/P2-EXPERIMENT.md) before treating Apple Terminal behavior as verified. The probe has no persistent history, server or provider identity recovery.

## Experimental single-child server and control

Build the binary and terminfo entry on Apple Silicon macOS. These commands
create private temporary directories; the physical paths are used because the
server accepts only an unambiguous absolute runtime path:

```sh
cargo build --locked
panelink_bin="$PWD/target/debug/panelink"
probe_terminfo_dir="$(mktemp -d "${TMPDIR:-/tmp}/panelink-terminfo.XXXXXX")"
probe_terminfo_dir="$(cd "$probe_terminfo_dir" && pwd -P)"
server_dir="$(mktemp -d "${TMPDIR:-/tmp}/panelink-server.XXXXXX")"
server_dir="$(cd "$server_dir" && pwd -P)"
context_dir="$(mktemp -d "${TMPDIR:-/tmp}/panelink-context.XXXXXX")"
context_dir="$(cd "$context_dir" && pwd -P)"
chmod 700 "$server_dir" "$context_dir"
/usr/bin/tic -x -o "$probe_terminfo_dir" docs/terminfo/panelink-probe.terminfo
printf 'panelink_bin=%s\nserver_dir=%s\ncontext_dir=%s\n' "$panelink_bin" "$server_dir" "$context_dir"
```

In one terminal, start a single child. `--initialize` is required for a new,
empty dedicated server directory; only use it once for that identity domain.
The example child prints a marker and then stays alive for read-only inspection:

```sh
"$panelink_bin" server-probe --directory "$server_dir" --terminfo "$probe_terminfo_dir" --initialize -- /bin/sh -c 'printf "Panelink child ready\\n"; exec /bin/sleep 300'
```

In another terminal, first assign the three absolute paths printed in Terminal
A to the shell variables below. Then create a caller context, inspect the
current target, and read that exact target. Copy the five-number `target`
string from `describe`; every operation names its target explicitly:

```sh
panelink_bin='/copy the panelink_bin value printed in Terminal A'
server_dir='/copy the server_dir value printed in Terminal A'
context_dir='/copy the context_dir value printed in Terminal A'
context_file="$context_dir/caller.ctx"
"$panelink_bin" control --directory "$server_dir" --context "$context_file" init
"$panelink_bin" control --directory "$server_dir" --context "$context_file" describe
"$panelink_bin" control --directory "$server_dir" --context "$context_file" read 'GEN:WORKSPACE:PANE:SESSION:OCCUPANT_REV'
```

`describe` returns the five-field `TARGET` and current numeric `owner_revision`.
Ownership commands such as `claim TARGET OWNER_REV` require that revision from
the caller; inspect again after an ownership change to get the new revision.
Text/paste/submit/interrupt input requires starting the server with
`--allow-input` and successfully claiming the target first. This is an explicit
prototype capability grant; do not add the flag unless that child should accept
agent input. `control init` creates a durable metadata-only context. If a
mutation or read loses its acknowledgment, the CLI does not resend it; use
`control ... resync` to check only that uncertain operation before reserving
another. Use `control ... receipt THROUGH_SEQUENCE` to explicitly retire
completed server results through a sequence whose result you have handled.
Receipts refuse unresolved context uncertainty and leave the local sequence
unchanged; retired results are no longer available. This avoids exhausting the
1024-result per-lease limit during repeated use. Detaching or closing a client
does not stop the child. Press Ctrl-C in
the server terminal to explicitly stop the foreground server and its child.

New client contexts optionally request Events and Input alongside Read.
Reconcile/watch require negotiated Read|Events; existing leases cannot silently
gain a new capability. The current API handles one child and one explicit target at a time:
there is no pane layout/multiple-session UI, provider identity/recovery, or
general multiplexer yet. The corrected pathname CLI scenario is operator-confirmed passing; see [socket gate evidence](docs/progress/P3-SOCKET-GATE.md)
and the [CLI implementation evidence](docs/progress/P3-CLI.md).

## Bounded live watches

On the experimental one-child server, use the full target from `describe`:

```sh
panelink control --directory "$server_dir" --context "$context_file" reconcile 'GEN:WORKSPACE:PANE:SESSION:OCCUPANT_REV'
panelink control --directory "$server_dir" --context "$context_file" watch 'GEN:WORKSPACE:PANE:SESSION:OCCUPANT_REV' 'GEN:CURSOR' 64 1000
```

Use the cursor returned by `reconcile` (or the previous completed watch).
Reconciliation binds one bounded screen, owner/exit metadata and cursor under
one state-owner turn. Watch returns at most64 metadata events and waits for
0–1000ms; it is a finite request, not an indefinite subscription. Changed exits0;
`timed_out` or `resync_required` exits1 with its explicit status. A timeout means
no change was observed for this watch, not child task failure. On
`resync_required`, obtain a fresh reconciliation before treating state as current.
Neither command reserves input sequences nor clears an uncertain input result.
Events contain metadata only; inspect the reconciled screen for visible output.

A lease created without Events cannot be upgraded by reconnect. Use a separately
initialized caller context explicitly if Events are needed; the CLI never
replaces an old context silently. Input remains separately opt-in on the server.
Socketpair/private-PTY live-watch checks pass locally; the expanded actual CLI
pathname fixture remains a separate external acceptance check.

`panelink pane list --json` includes current managed-child cwd and executable
path (`command` excludes arguments), sample time and per-field availability.
Refresh reflects shell `cd` and direct `exec`; foreground descendants are not
inferred. Workspace event observers renew their existing lease before expiry
and keep their event cursor. See [the agent CLI notes](docs/progress/P3-AGENT-CLI.md).

## Low-level split panes (`serve` + `attach`)

Plain `panelink` runs these two pieces for you. Use them directly only when
you need a server in a specific directory. `panelink serve` is a foreground
server for up to four panes. `panelink attach` is a full-screen client for it
that you run inside a terminal.

Reuse the build and terminfo setup above. Then, in Terminal A:

```sh
"$panelink_bin" serve --directory "$server_dir" --terminfo "$probe_terminfo_dir" --initialize --allow-input -- /bin/zsh -i
```

In Terminal B, using the same three paths:

```sh
"$panelink_bin" attach --directory "$server_dir" --context-dir "$context_dir/attach"
```

- Typing goes to the focused pane. The client claims input ownership on each
  pane. If another client owns a pane, a status line says so and no keys are
  sent.
- A left-click focuses a pane. This changes only local presentation.
- To open the split menu, right-click a pane, or press Control-B (navigation
  mode) and then `s`. Choose an item with the arrow keys and Enter, with
  `l`/`r`/`u`/`d`, or with a click. Esc closes the menu. While the menu is
  open, it receives all input and the panes receive none.
- A split launches one new `/bin/zsh`. It uses the attach client's own
  working directory and environment, minus `PANELINK_*`. The new pane gets
  focus.
- The pane's `x`, menu Close action and Ctrl-B then `x` require a separate
  `[Close]` click, Enter with Close selected, or `y` confirmation; Cancel is
  the default selection and the opening click/key cannot confirm.
  Closing sends HUP to that pane's process group, then KILL after a bounded
  wait if needed. If its sibling remains, the sibling fills the workspace;
  the last shell exiting ends the workspace. `panelink panes` lists the
  default workspace and `panelink close N` closes its numbered pane.
- Ctrl-X detaches. The shells keep running on the server, and you can run
  `attach` again to reconnect. Ctrl-C in Terminal A stops the server and its
  shells.
- Mouse events are never forwarded to applications in panes.

See [P3A-CLIENT.md](docs/progress/P3A-CLIENT.md) for limits and evidence.

Tab action, rename and tab-close dialogs are centered and capped at 28, 56
and 44 terminal columns respectively. All tabs is capped at 64 columns and
12 rows (including its search header); keyboard navigation scrolls the list.
Pane-close confirmation is capped at 48 columns at the bottom of the view.
Widths shrink to fit smaller viewports. Pointer targets follow the visible
dialog bounds; clicks beside a dialog do not activate its actions.

Tab actions has two columns of inner padding, blank rows around its content,
a separate heading and a keyboard hint footer. Small viewports use the compact
four-row layout.
