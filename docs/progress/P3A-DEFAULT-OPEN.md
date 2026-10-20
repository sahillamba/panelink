# P3A — zero-configuration `panelink`

Status: implemented by the parent on 2026-10-19 after a user request: "I
should just be able to type `panelink` in a terminal and every feature should
just work." Two delegated attempts produced no changes (empty result, then an
auth-broker timeout), so the parent implemented this slice directly. It was
independently reviewed and the findings fixed.

## Behavior

| Command | Effect |
| --- | --- |
| `panelink` | Open the workspace: start the background server if needed, otherwise reattach |
| Ctrl-X (in the workspace) | Detach; the shells keep running |
| `exit` in the last shell | The server stops and the client exits with `[panelink: workspace closed]` |
| `panelink stop` | Stop the server and its shells |
| `panelink probe` | The old disposable single-shell probe |

The advanced commands (`serve`, `attach`, `control`, `server-probe`,
`pty-probe`) are unchanged. `bin/panelink` now just execs the built binary.

## Design (`src/default_open.rs`)

- **State root.** `$PANELINK_HOME`, or `~/Library/Application Support/Panelink`.
  - The root and each subdirectory (`server/`, `terminfo/`, `contexts/`) must
    be mode 0700, owned by the user, and not symlinks.
  - Every existing ancestor must be a directory owned by root or the user and
    not writable by others. Root-owned sticky directories and root-owned
    system symlinks (`/tmp`) are accepted.
  - The socket path must fit in 103 bytes.
- **Terminfo.** The source is embedded with `include_str!`. It is compiled
  with `/usr/bin/tic` into a per-process staging directory, then the entry and
  a source fingerprint are published by atomic rename. It is recompiled only
  when the fingerprint changes. Concurrent compiles are safe.
- **Lifecycle lock.** `lifecycle.lock` is an exclusive `flock` in the root. It
  serializes these steps for openers and `stop`:
  - the liveness check (the server's own `server/server.lock`);
  - stale-socket removal;
  - storage initialization (`--initialize` only when storage is absent);
  - spawning the server;
  - waiting for the server to be ready.

  A concurrent opener waits, then finds the running server.
- **Stale rule.** The socket is removed only while the lifecycle lock is held,
  the server lock is free, and the entry is a user-owned socket. Any other
  entry is refused.
- **Spawn.** `current_exe() serve --directory … --terminfo … [--initialize]
  --allow-input -- $SHELL` runs in a new session (setsid).
  - `$SHELL` is used only if it is an absolute executable; otherwise
    `/bin/zsh`.
  - stdin comes from `/dev/null`; stdout and stderr go to `server.log`, which
    rotates once it exceeds 1 MiB. The log holds lifecycle JSON only, never
    environment data.
  - The first shell uses the invoking terminal's cwd and environment as its
    explicit launch context (existing `serve` contract, `PANELINK_*`
    stripped).
- **Identity.** `server.pid` stores the pid and its kernel start time (from
  `proc_pidinfo` `PROC_PIDTBSDINFO`). `stop` signals only a non-zombie process
  whose pid, start time and owner match, and only while the server lock is
  held. It waits up to 5 s for that lock to be released.
- **Server end.** `serve` leaves its loop when every retained session has
  exited or failed (`MultiSessionOwner::all_sessions_ended`; false when no
  sessions exist). Every 250 ms the client checks that the control socket
  still exists with the inode it saw at attach. If the socket is missing or
  was replaced (restart), the client restores the terminal and exits 0 with a
  notice. Other metadata errors count as transient.
- **Context creation race.** Two clients attaching at once might pick the same
  context filename. Only an `EEXIST` collision is retried, with the next
  candidate name and the same authenticated stream. Any other create failure
  aborts, so one lease never gets two durable sequence allocators.

## Review

Independent reviewer findings and resolutions:

| Severity | Finding | Resolution |
| --- | --- | --- |
| High | Racing starters could overwrite the pid file; `stop` could signal the wrong process | Lifecycle lock; pid plus start-time identity verified before signalling |
| High | The lock probe unlocked before stale-socket removal; the probe could steal a fresh lock | All checks and removals run under the lifecycle lock |
| High | Retrying any context-create error could duplicate a lease's context | Retry only on `EEXIST` |
| High | Root ancestors and mode were not validated | Ancestor check; strict 0700; symlink leaf rejected |
| Medium | Socket existence is not server identity; restart stranded the client | Socket inode identity; `NotFound` versus other errors distinguished |
| Low | Unneeded `mut` failed strict Clippy | Fixed |

## Tests and verification (parent, 2026-10-19)

- `default_open` unit tests (14):
  - root resolution and socket-path length;
  - 0700, symlink and ancestor checks;
  - endpoint decision table and socket classification;
  - storage-lock probe;
  - terminfo fingerprint, a real compile, and 4 concurrent compiles;
  - identity parsing and process start-time identity (reaped child, pid 1);
  - lifecycle-lock exclusivity;
  - shell selection.
- `tests/p3a_default_open.rs` (outer PTY, isolated `PANELINK_HOME`, zsh with
  an empty `ZDOTDIR`). It passed 5 of 5 tests across 5 consecutive runs, with
  no leftover processes. It covers:
  - start, type, right-click menu, split, type in the new pane, Ctrl-X with
    terminfo/termios/mouse restore, reattach showing both panes' output,
    `stop`, a repeat `stop` reporting nothing running, and a fresh single-pane
    open;
  - last-shell `exit` closing the workspace and client cleanly;
  - SIGKILL of the server leaving a stale socket, with the next open
    recovering it under a new server pid;
  - `stop` plus restart while a client is attached, after which the old client
    exits cleanly;
  - two concurrent opens sharing one workspace.
- Ignored `real_environment_smoke` (the user's real zsh config, isolated
  state root): prompt, echo, menu, split and second-pane echo all pass.
- Full suite: 581 passed / 0 failed / 15 ignored. `verify_non_ui.py` 8/8,
  rustdoc `-D warnings` clean, `git diff --check` clean.

## Limits

- Apple Terminal right-click delivery and feel need the user's manual check.
- The workspace is per user, not per project.
- Plain `panelink` is not a login service; the server lives until `stop` or
  the last shell exits.
- No minimum macOS version is set.

## Navigation mode (Ctrl-B) — 2026-10-19

The user reported that "Ctrl-B doesn't work". The cause was the key: they
pressed Command-B. Apple Terminal handles Command shortcuts itself and sends
nothing to the program. Control-B (byte 0x02) was already working, but it
gave no visible feedback.

What changed in `src/ui_client/state.rs`:
- Control-B enters navigation mode and shows a highlighted bottom bar:
  `NAV  s menu | l r u d split | arrows/Tab/o focus | x detach | Esc cancel`.
- In NAV mode:
  - `l`/`r`/`u`/`d` (and tmux's `%`/`"`) split the focused pane directly. The
    menu's capacity and size rules apply; refusals show a status line.
  - `s` opens the menu.
  - Arrow keys, Tab or `o` move focus. Focus keys keep NAV mode on, so you can
    move several times.
  - `x` or Ctrl-X detaches; Esc or any other key leaves NAV mode.
  - NAV mode ends by itself after 4 s without a key.

Phase 3B supersedes the historical `x` detach binding above: `x` now prompts
to close the focused pane; only `y` confirms. Ctrl-X still detaches from NAV
mode and from the ordinary client. See [P3B-CLOSE-CLIENT.md](P3B-CLOSE-CLIENT.md).
- No key pressed in NAV mode reaches a shell.

Two related client fixes:
- **Splits now claim the new pane immediately.** Previously the new pane was
  claimed on the next poll, so keys typed straight after a split went
  nowhere. `split` now claims at once, and `send_keys` tries one immediate
  claim when the pane is not yet owned.
- **No early-key loss.** An investigation suspected the client was dropping
  keys typed just after a split. Debug tracing showed the keys were applied
  and rendered; the earlier "missing" output came from probe scripts
  searching for whole strings in incremental per-cell output.

Tests:
- 6 new unit tests:
  - the banner is visible, and Esc clears it with no keys sent;
  - NAV mode times out;
  - every direct split key, with the captured target and revision;
  - a split at capacity is refused with no effect;
  - focus moves by arrows, Tab and `o`, NAV mode chains, and typing goes to
    the newly focused pane;
  - `x` detaches.
- New host test `ctrl_b_navigation_mode_banner_split_and_focus_keys`: the
  banner appears and clears, Ctrl-B `r` splits, typed output appears in the
  right half, and Ctrl-B Left moves typing to the left half.
- `p3a_default_open` passed 6 of 6 tests on 4 consecutive runs.
- Full suite: 588 passed / 0 failed / 15 ignored. Verifier 8/8; rustdoc clean.
