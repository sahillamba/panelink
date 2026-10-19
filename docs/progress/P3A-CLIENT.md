# P3A wave 3B — interactive `attach` client

Status: implemented on 2026-10-19 as worker output for parent review. An
automated outer-PTY fixture verified it end to end against a real
`panelink serve` and real zsh children. Apple Terminal itself has not been
tested by a person. Right-click delivery in Apple Terminal therefore remains
an operator check. This wave does not complete Phase 3A; the parent decides
phase exit.

## What it does

`panelink attach --directory SERVER_DIR --context-dir DIR` takes over the
host terminal. It shows the server's one or two panes full-screen through
`terminal::compose`, and it routes input as follows:

| Input (menu closed) | Effect |
| --- | --- |
| Printable keys, Enter, Tab, Backspace, Esc, arrows, Home/End, Ctrl-C | Batched typed keys go to the **focused** pane as schema-8 KeyInput. Each batch is up to 64 keys and is flushed after 4 ms idle, on a focus change or on a command. Batches are never resent. |
| Bracketed paste | Sent once with control PASTE to the focused pane. |
| Left click on a pane | Focuses that pane locally (presentation only). |
| Right click (SGR button 2 press) | `MenuState::open_at_pointer` on the pane under the pointer, even if another pane has focus. |
| Ctrl-B, then `s` | `MenuState::open_for_focused`. This is a prototype binding; the spec names the Ctrl+B prefix. Ctrl-B followed by any other key is dropped. A literal Ctrl-B cannot be forwarded because `terminal::Key` cannot express it. |
| Ctrl-X | Detaches. The client releases its claims and exits 0. Children keep running. |
| Other control chords, function keys, other mouse events | Dropped. Mouse events never reach children in this phase. |

While the menu is open, **every** decoded event goes to `MenuState::handle`
and nothing is sent to a pane. Activation produces a `SplitIntent`. The
client then builds a `SplitPlan`:

- the captured target;
- the **server** layout revision recorded when the menu opened (the local
  `PaneLayout` mirror always starts at revision 1, so its own revision is not
  used);
- the direction;
- the `planned_split_size`.

It encodes a schema-7 request that launches `/bin/zsh` with no arguments. The
launch context is the client's own cwd and environment, minus `PANELINK_*`,
capped at 1,024 entries / 48 KiB. The request is sent on the connection
pinned to the captured target. On `Applied`, the client attaches to the new
target, claims it and focuses it (prototype default). Any failure appears as
a bounded, fixed-text status row for 3 s.

## Architecture (`src/ui_client/`, binary-private like `p2_probe`)

- **`host.rs`**: the host terminal side, using the same patterns as
  `p2_probe`'s `HostGuard`.
  - Uses a separately opened nonblocking `/dev/tty`.
  - `ENTER_SEQUENCE` adds `?1000h ?1006h` to the existing modes.
    `RESTORE_SEQUENCE` disables the mouse modes first, then applies
    p2_probe's restore sequence.
  - On restore it flushes input, restores the saved termios and flushes
    again.
  - Restore runs on every exit path: normal return, error, `Drop` during
    unwind, and SIGINT/SIGTERM/SIGHUP through a stop flag.
  - Input uses a fixed 1 KiB chunk, the vendored decoder, a 5 ms work budget,
    a 50 ms escape timeout and a 2 s incomplete-sequence timeout.
  - Output uses a preallocated 4.06 MiB frame. The compositor commits only
    after the whole frame is written. A 1 s output stall is an error.
  - The host size is checked every 100 ms.
  - All buffers are allocated before raw mode starts. `p2_probe` is
    unchanged.
- **`keys.rs`**: pure mapping from `KeyEvent` to `KeyAction`
  (Forward/Quit/Prefix/Ignore), and from any event to `MenuInput`.
- **`state.rs`**: the pure `App`.
  - Holds the layout, a local `PaneLayout` mirror (for hit tests and
    feasibility), local focus, `MenuState`, the key batch, status, and two
    preallocated 80×240 `ScreenSnapshot` caches.
  - `handle_event` performs effects through an `Effects` trait, so tests can
    substitute a recording fake.
  - `render` builds the `ComposeFrame`:
    - The root is `min(host, 80×240)`.
    - Rectangles come from the LayoutReply.
    - A pane shows a placeholder (`connecting...`, `resizing...` or
      `resize pending`) when it has no matching screen.
    - The menu or a one-row status is drawn as an overlay. Pane geometry
      never changes for status.
    - If the host is smaller than the layout root, the client keeps the last
      layout and shows a bounded "terminal too small" notice. It does not
      send a resize below 5×20.
- **`conn.rs`**: one `PaneConn` per pane, holding an authenticated stream
  pinned by the AttachTarget trailer and a `ContextFile`. See
  [Connections and contexts](#connections-and-contexts).
- **`net.rs`**: `Net` implements `Effects`.
  - Polls the layout every 250 ms and attaches to newly appeared panes.
  - When the host size differs from the root, sends one RootResize with the
    expected revision, at most once per (revision, size).
  - Polls each pane every 30 ms with a finite `wait_ms=0` schema-5 watch.
    ScreenQuery is sent only after ScreenChanged or ChildExited, or on
    resync. If events are unavailable, it falls back to a 100 ms screen poll.
  - Skips loading a checkpoint whose revision is unchanged.
  - Reconnects a dropped pane every 1 s.
- **`src/terminal/checkpoint_load.rs`**: `ScreenSnapshot::load_checkpoint`
  copies a decoded checkpoint into the snapshot's reserved capacity, so it
  never allocates. It reapplies the host-unsafe scalar filter.
  Application-cursor and bracketed-paste modes are not in checkpoints, so
  they are reported as off.

## Connections and contexts

- **Context files.** Each pane has a context file at
  `DIR/pane-G-W-P-S-O.ctx`.
  - `DIR` is created with mode 0700 if missing. An existing `DIR` must be a
    0700 directory owned by the user.
  - An existing context file is resumed and never overwritten. The resumed
    grant must still be pinned to that exact target, or the file is skipped.
  - If a context file is locked by another client, the client tries up to 7
    numbered alternatives. It never silently replaces a foreign context.
  - Any uncertain operation is settled by OperationStatus before new work.
  - **Dead contexts.** A resume Hello can be rejected with
    `unknown_context`, which happens when the lease expired or the server
    restarted into a new generation. That proves the lease is dead.
    - If the context holds no unresolved operation, the client removes the
      file while still holding its lock and reuses the name for a fresh
      lease.
    - If an unresolved operation remains, the file is kept and its name is
      skipped.
- **Discovery never leaks a lease (review fix 5).**
  - On start, the client first tries to resume each canonical
    `pane-*.ctx` (at most 64) whose pane is still in the layout. Every later
    `attach` with the same `DIR` therefore reuses the same lease.
  - Only when none resumes does it open a fresh Hello with no trailer. That
    discovery lease is then adopted as the first pane's new context; it is
    not abandoned.
  - Proof: 70 sequential attach/detach cycles succeed and leave exactly one
    context file. A new `control init` still gets a lease afterwards. With
    discovery reverted to "always fresh", the same test fails partway through
    the 70 cycles with `server rejected the attachment`, which is lease
    exhaustion. The exact cycle was not recorded.
- **Lease renewal (review fix 1).**
  - The server lease is 1,800 s, and only a resume Hello renews it.
  - Each pane keeps a `LeaseSchedule` based on when its last Hello
    *started*. That time is never later than the server's base, so the local
    estimate cannot outlive the server's lease.
  - At 1,500 s (TTL minus a 300 s margin) the pane opens a new stream with a
    resume Hello. The old stream is replaced only after the new handshake
    succeeds. Uncertainty is then settled by status; no input is resent.
  - A failed renewal is retried every 15 s.
  - If the estimate passes the TTL, or the server answers `unknown_context`
    on a request or a resume, the pane is marked dead. It is then
    re-attached: a settled context is replaced, and an unsettled one is
    reported.
  - A lost stream is reconnected with backoff (1 s doubling to 8 s), and
    each reconnect also renews the lease.
- **Keyed operations.** Each one reserves its key durably, sends once,
  validates the exact key, then acks. A server Error frame is settled through
  status.
- **Receipts (review fix 4).** Receipt state counts only what this process
  validated.
  - `last_admitted` is the highest key with a validated terminal admitted
    result: a completed status, a resize or split result, or a status settled
    by `resync`. Pre-admission rejections are not counted.
  - `receipted` is set only after the server's exact receipt success
    response, which is `Completed(Applied)` for that key.
  - During use, a receipt is sent once 32 more results have been admitted.
    At detach, the client sends a receipt through `last_admitted`.
  - On resume, after settling any uncertainty, the client walks down from
    `next_sequence - 1`. It sends at most 4 exact OperationStatus queries and
    receipts through the first key that is `Completed`.
  - Receipts are refused while an operation is uncertain.
  - Proof: after 70 cycles, `control status N` is rejected as retired for
    every sequence the client used. With receipts disabled, sequence 1 shows
    `completed`, so the check is discriminating.
- **Claims.** The client claims at attach (`Describe`, then `Claim` at the
  current owner revision with a 30 s TTL) and renews every 10 s. If another
  lease owns the pane, no keys are sent and a status row is shown. On
  detach, the client releases its claims (best effort).
- **Diagnostics.** These are fixed labels. They never include targets, keys,
  environment values, paste text or screen text. Split and paste payloads
  are dropped right after sending.
  - Diagnostics never use inherited stderr (review fix 3). After
    restoration they are written to the client's nonblocking `/dev/tty`
    with a 100 ms write deadline. Before takeover, they go to a separately
    opened nonblocking `/dev/tty`, like `p2_probe::report_error`.
  - The default panic hook writes to stderr *before* unwinding, so attach
    installs a silent hook and catches the unwind. The unwind runs
    `HostGuard::drop`, which restores the terminal, and only then is a fixed
    message reported.

## Bounded exit and stalled servers (review fix 2)

- **Restore first.** Every exit path restores the host terminal before any
  network work: mouse-off and restore bytes, input flush, the saved termios,
  then another flush. This ordering is the `restore_then_detach` function,
  and a unit test asserts it. The network work that follows is release,
  status settle and receipt. Signals and errors take the same path, and
  panics restore through `Drop`.
- **Bounded network work.**
  - Inside the interactive loop each exchange has a 400 ms deadline
    (`OP_DEADLINE`). A split and its new-pane attach get 2 s.
  - A timed-out stream is poisoned, so it is dropped and later
    reconnected.
  - The stop flag is checked between panes, so SIGINT/SIGTERM/SIGHUP wait at
    most about one exchange per pane.
  - Detach exchanges are 500 ms each, within a 1.5 s total budget, and stop
    at the first stall.
- **Proof.** The serve process is frozen with SIGSTOP, then Ctrl-X is
  pressed.
  - The restore bytes appear within the asserted 1 s; the exact latency was
    not recorded.
  - termios matches the saved state and the client exits 0 within 4 s.
  - With `OP_DEADLINE` temporarily set to 2 s, the same test fails
    ("restored after 1.82 s").

## Server-facing finding

The first connection is a bootstrap stream: a fresh Hello with no trailer,
used to learn the first pane and its layout. The first version reused that
stream with request ID 1 again. The server requires strictly increasing
request IDs per stream and closes the stream silently when they repeat. The
client now carries the bootstrap's last-used ID forward. This was a client
bug; the server behaviour is as documented, and no server file was changed.

## Tests

- **Review-fix unit tests (binary, +3)**:
  - **Lease schedule**: renewal at TTL−margin, retry after a failure, a
    conservative expiry estimate, and a simulated five-lifetime run with an
    injected clock in which the lease never lapses.
  - **Context names**: the name round-trips and foreign names are rejected.
  - **Exit ordering**: the terminal is restored before detach.
- **Unit tests (binary, 14)**:
  - **Key mapping (2)**: real decoder bytes to keys and commands; SGR press
    and release to `MenuInput`.
  - **State routing (8)**:
    - batching and flush on focus change;
    - divider click ignored;
    - right-click captures the pane under the pointer while another pane has
      focus;
    - every menu-open input, including Ctrl-X, Ctrl-C, paste and Enter on a
      disabled item, produces no `send_keys`, `send_paste` or `split`;
    - an outside click closes the menu without refocusing;
    - the split plan uses the captured target and the server revision
      recorded at open (5, not the later 6), with planned size 24×40, and
      focuses the new pane;
    - keyboard fallback, including Esc;
    - split failure shows status;
    - Ctrl-X flushes, then quits;
    - frames for one and two panes, placeholders and the menu overlay;
    - the too-small notice.
  - **Split bytes (2)**: request bytes decode to the captured target,
    revision 42, direction, 11×80, `/bin/zsh`, cwd, no args and one env
    entry; the environment excludes `PANELINK_*`.
  - **Restore order (1)**: the sequence disables every enabled mode, mouse
    first.
  - **p3_cli attach parser (1)**.
- **Library (2)**: checkpoint round trip (styles, RGB, wide glyph, hidden
  cursor); capacity reuse across 80×240, 5×20 and 24×100.
- **`tests/p3a_attach_host.rs` (1, not ignored; runs here)**: the real
  `serve --initialize --allow-input -- /bin/zsh -f -c 'printf PANE1-READY; exec zsh -f -i'`
  with an explicit environment and an empty HOME/ZDOTDIR. `attach` runs as a
  session leader on a 24×100 outer PTY under a supervisor shell. The test
  checks, in order:
  1. Raw mode starts and the 1000/1006/1049/2004 enables are present.
  2. READY is rendered.
  3. The root is resized to 100 columns and pane 1 is claimed.
  4. Typed `echo ACK$((40+1))` gives `ACK41` both on screen and in
     `control read`.
  5. An SGR right-click `\e[<2;10;5M`/`m` shows "Split Right"/"Split Down".
  6. `r` gives a 2-pane layout with pane 1 on the left. The new pane is
     claimed and shows its prompt.
  7. The menu letter did not reach pane 1.
  8. `echo ACK$((40+2))` appears **only** in pane 2.
  9. Ctrl-B `s` shows the menu ("Pane limit"). Esc closes it, and pane 2's
     text is byte-identical before and after.
  10. Ctrl-X: exit 0, mouse/alt-screen/paste disables are present, termios
      matches the saved state, the layout still has 2 panes and both children
      are alive.
  11. Re-attach resumes both contexts and renders ACK41 and ACK42. Ctrl-X
      exits 0.
  12. SIGTERM stops the server: exit 0 and the socket is removed.

  If the pathname bind is refused, the test prints `SKIPPED:`; that did not
  happen here.
- **Host tests added by the review fixes (4)**:
  - **Stalled server**: SIGSTOP the server, press Ctrl-X. The terminal is
    restored within 1 s and the client exits 0 within 4 s.
  - **Full stderr pipe**: stderr is a blocking pipe that is already full.
    - With no server, attach exits 1 within 5 s and its error appears on the
      terminal.
    - With a server, a normal attach and Ctrl-X exit 0 and restore termios.
  - **70 attach/detach cycles**: exactly one context remains, a new
    `control init` still succeeds, and every used sequence is retired.
  - **Server restart (new generation)**: the old context is detected as
    dead and replaced, typing works, and only the new generation's context
    remains.

## Verification (2026-10-19, macOS arm64, Rust 1.99.1)

| Command | Result |
| --- | --- |
| `cargo test --locked --offline --bin panelink -- ui_client attach_parser` | 17 passed (after review fixes) |
| `cargo test --locked --offline --test p3a_attach_host` | 5 tests, passed 3/3 consecutive runs (≈14.7 s each) |
| `cargo test --locked --offline` | all passed (lib 464, bin 55, all integration suites); pre-existing ignores only |
| `cargo fmt --check` | clean |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps` | clean |
| `python3 scripts/verify_non_ui.py` | all 8 checks passed |

## Limits and open items

- **Not tested by a person in Apple Terminal.** Apple Terminal may consume
  right-click; for example, Ctrl-click may show its own menu. Ctrl-B `s` is
  the equivalent route.
- **Input**
  - Typed keys are limited to the schema-8 key set.
  - Alt/Meta, function keys and control chords other than Ctrl-C are
    dropped.
  - Pastes are capped at the control text limit (32 KiB); a larger paste is
    refused locally.
  - Typing is not echoed locally. Latency is about one batch flush (4 ms),
    one KeyInput round trip and one 30 ms watch tick.
- **Rendering**
  - Rendering is not exact for application-cursor programs, because the
    checkpoint lacks that mode. Keys still go through the server's typed
    encoder.
  - Each changed screen costs one full checkpoint transfer (up to 0.9 MB) per
    pane. There are no diff transfers.
- **Server**
  - A per-pane connection that drops is retried every 1 s. Its claim is
    re-acquired, which can fail if another client claimed the pane meanwhile.
  - Split recovery after a lost acknowledgment (resending the exact bytes) is
    not automated. The client shows the failure and leaves the key uncertain
    in that pane's context. The next operation on that pane settles it
    through status.
  - The menu's disabled reasons come from the local mirror. The server
    revalidates everything.
- **Product decisions still open**
  - The prefix and menu bindings (R-08).
  - Whether to focus the new pane.
