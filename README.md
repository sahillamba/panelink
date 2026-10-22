# Panelink

Panelink is an experimental Rust terminal multiplexer for Apple Silicon Macs.
It runs inside your existing terminal (Apple Terminal first). Your shells live
in a background workspace, so closing the window does not kill them.
Panelink's original application code is intended for the MIT license.

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
reattach to it.

| Action | How |
| --- | --- |
| Split a pane | Right-click it, then pick Left/Right/Up/Down (or press `l`/`r`/`u`/`d`) |
| Navigation mode | Press **Control**-B (the Control key, not Command). A `NAV` bar appears at the bottom |
| Shortcut help | Ctrl-B, then `?`; Esc or `?` closes help |
| ...then split | `r` right, `l` left, `u` up, `d` down (also `%` and `"`), or `s` for the menu |
| ...then switch pane | Arrow keys, Tab or `o` (stays in NAV mode so you can keep moving) |
| ...then close focused pane | `x`, then `y` to confirm; the last pane ends the workspace |
| ...then leave NAV mode | Esc (or wait 4 seconds); Ctrl-X detaches |
| Focus a pane | Click it |
| Close pane from the menu | Right-click the pane, choose Close pane, then press `y` to confirm |
| List/close panes from a shell | `panelink panes`; `panelink close 1` (or `2`) |
| Detach (shells keep running) | Ctrl-X |
| Reattach | `panelink` |
| Close the workspace | `exit` in every shell, or `panelink stop` |

Current limits: at most four panes, no tabs. The attach client forwards ordinary
Ctrl-letter keys, Alt/Meta characters when delivered by the host, F1–F12,
Delete/Insert/Page Up/Page Down and Shift-Tab. Ctrl-B remains navigation and
Ctrl-X detaches; Ctrl-C goes to the focused child. Modified function/navigation
keys and application mouse pass-through remain unsupported. Help consumes
typing, paste and mouse actions while open. There is no provider (OpenCode)
session recovery. The older disposable probe retains its narrower input map.

Local help, navigation and Ctrl-X remain available while the attach client
waits for the server. If typing backs up, a busy status appears. Ctrl-X still
detaches; any keys that could not be queued are discarded with a notice after
the terminal is restored. Uncertain input is never automatically resent.

Workspace state lives in `~/Library/Application Support/Panelink` (private,
mode 0700). Set `PANELINK_HOME` to use a different directory. The background
server's output goes to `server.log` there.

The server now saves its latest sanitized visible normal and alternate screen
checkpoints per pane in the private `server/history/` directory (under
`PANELINK_HOME`, if set). They are read-only recovery data, not live-process
state or a complete transcript. An initial `serve` response reports whether
capture started; writes run in a bounded background worker, so saved content
can lag the live screen. A later write or shutdown failure appears in
`server.log` while shells continue running. Normal scrollback rows are saved within a 1,024-row/8 MiB
per-session limit. `panelink history list` lists saved target IDs;
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

**Implementation roadmap:** [ROADMAP.md](ROADMAP.md) tracks phases and
verification evidence. Selected components retain their
[third-party notices](THIRD_PARTY_NOTICES.md).

- [Research and architecture recommendation](docs/PANELINK-RESEARCH.md)
- [Product questions and spec handoff plan](docs/PANELINK-DISCOVERY.md)
- [Working implementation specification](docs/SPEC.md)
- [First prototype preparation and requested Rust skill](docs/PREBUILD.md)

## Alpha release, install and security review

- [docs/RELEASE.md](docs/RELEASE.md): locked local release build with a
  hash manifest, `scripts/install.sh install|upgrade|uninstall` (no network,
  no sudo, keeps state on upgrade, rollback copy), unsigned/not notarized.
- [docs/SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md): SEC-01..SEC-08
  evidence pack for an organization's review (not an approval).

## Automated development checks

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
- The menu's Close pane action and Ctrl-B then `x` require `y` confirmation.
  Closing sends HUP to that pane's process group, then KILL after a bounded
  wait if needed. If its sibling remains, the sibling fills the workspace;
  the last shell exiting ends the workspace. `panelink panes` lists the
  default workspace and `panelink close N` closes its numbered pane.
- Ctrl-X detaches. The shells keep running on the server, and you can run
  `attach` again to reconnect. Ctrl-C in Terminal A stops the server and its
  shells.
- Mouse events are never forwarded to applications in panes.

See [P3A-CLIENT.md](docs/progress/P3A-CLIENT.md) for limits and evidence.
