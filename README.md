# Panelink

Panelink is a planned Rust terminal multiplexer and persistent workspace runtime for Apple Silicon Macs. Alongside the one-shot terminal probe, the executable now includes an experimental foreground server/control CLI for one PTY child. Short-lived control commands can reuse a durable caller context while that server is running. This is an integrated prototype, not a general multiplexer, daemon, or provider-recovery system. Panelink's original application code is intended for the MIT license.

**Implementation roadmap:** [ROADMAP.md](ROADMAP.md) tracks the current phase, ownership, and verification steps.

Current work: Phase 3 integration continues while Phase 2 host acceptance remains pending. The integrated prototype runs one child under a foreground server and exposes authenticated control requests for explicit target reads, bounded reconciliation/watches, status, ownership, and optional input. The server persists monotonically advancing generations within its initialized storage domain; a private client context reuses one lease and records uncertain operations for explicit resync. This does not provide multiple panes/sessions, layout operations, runtime event subscriptions, or OpenCode/provider recovery. Input is opt-in with `--allow-input`; the default server does not advertise the input capability. The corrected pathname CLI acceptance scenario is operator-confirmed passing outside the sandbox; local bind remains restricted by EPERM. Current integrated test totals and ignored cases are recorded in [INTEGRATION.md](docs/progress/INTEGRATION.md), not inferred from component-only checks. The bounded decoder, replacement terminal core, host IO loop, restoration guard, and one-shot probe are described in their linked progress notes below; Apple Terminal acceptance remains incomplete. Selected components retain their [third-party notices](THIRD_PARTY_NOTICES.md). The interface is planned as a terminal UI inside existing terminal applications; the earlier desktop GUI hypothesis has been superseded. Some workflow and integration choices remain open.

- [Research and architecture recommendation](docs/PANELINK-RESEARCH.md)
- [Product questions and spec handoff plan](docs/PANELINK-DISCOVERY.md)
- [Working implementation specification](docs/SPEC.md)
- [First prototype preparation and requested Rust skill](docs/PREBUILD.md)

## CLI

For this development checkout, the launcher at `bin/panelink` opens the current
disposable zsh probe (`/bin/zsh`) when called without arguments and forwards explicit
subcommands to the built binary. Build once with `cargo build --locked`, then
link it into the existing user PATH directory:

```sh
ln -s /Users/sahillamba/panelink/bin/panelink "$HOME/.local/bin/panelink"
rehash
panelink
```

The launcher compiles private temporary terminfo and removes it after the probe
exits. Ctrl-X quits the probe and stops its child; this default is not persistent
server detach. Explicit commands such as `panelink --help`, `panelink control …`
and `panelink server-probe …` retain their normal behavior. Rebuild after source
changes; the launcher uses `target/debug/panelink` from this checkout.

After installing a Rust toolchain, run these commands from the repository root:

```sh
cargo run -- --help
cargo run -- --version
cargo run -- doctor
```

With no arguments, Panelink prints help. `doctor` reports the target and the limits of the current prototype; it exits with status 1 because the persistent multiplexer and host/pathname acceptance are still unsupported. Unknown commands or extra arguments exit with status 2. The `server-probe` command below launches one child; `control` attaches short-lived clients to that foreground server.

Future implementers should read these documents before starting. Confirmed user requirements, engineering proposals, and unresolved decisions are distinguished. Dependencies require a concrete justification for saved effort and maintenance; there is no blanket approval to add libraries.

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
