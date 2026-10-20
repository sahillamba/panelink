# P3B wave 2b: attach-client pane close

Status: implemented on 2026-10-20 as worker output for parent review. Nothing
is committed. This work covers only the client side. A live end-to-end close
also needs the schema-9 route in `multi_runtime.rs`, which another engineer
is building in parallel. Until that route lands, a real close request gets an
error or rejection reply, and the client shows that as a status message.

## Files

The changes are in these files:

- `src/split_menu.rs`
  - Adds a fifth menu row, "Close pane".
  - Adds `CloseIntent`, `MenuResponse::CloseRequested`, the pure
    `CloseConfirm` prompt (`ConfirmInput` / `ConfirmResponse`) and
    `MenuState::close_selected`.
- `src/ui_client/keys.rs`: adds `confirm_input`.
- `src/ui_client/state.rs`
  - Adds `ClosePlan` and `Effects::close`.
  - Routes prompt input, binds Ctrl-B `x`, renders the prompt, and adds
    result messages (`close_message`).
  - Repairs state when the layout shrinks.
- `src/ui_client/net.rs`: adds `Net::close`, `encode_close` and
  `classify_close_reply`.
- `src/ui_client/conn.rs`: adds `CLOSE_DEADLINE` (4 s).
- `src/ui_client/state_tests.rs`: adds tests.

`host.rs` is unchanged. Its existing endpoint-gone path still prints
"[panelink: all shells exited; workspace closed]".

## UX

- **Menu (right-click, or Ctrl-B `s`)**
  - Rows, in order: Split Left [L], Right [R], Up [U], Down [D], Close pane.
  - The title is now " Pane ".
  - Close is always enabled, including at the 2-pane limit.
  - When Close is selected, the status row reads "Enter: close this pane
    (asks y/n)".
  - Close has no letter shortcut, so a stray `x` typed into the open menu
    cannot start a close. Close is reached by arrows, End, Up-wrap or a click.
- **Ctrl-B, then `x`**: opens the prompt for the focused pane.
  - Detach in NAV mode is now Ctrl-B Ctrl-X, and plain Ctrl-X detaches as
    before.
  - The banner reads: `NAV  s menu | l r u d split | arrows/o focus | x close | ^X detach | Esc`.
- **Prompt**: a bottom row in the selected style.
  - It reads " Close pane N? (y/n) ", where N is the spatial position (1 is
    left/top, 2 is right/bottom).
  - For the only pane it reads " Close pane 1? This is the last pane; the
    workspace ends. (y/n) ".
  - Only `y`/`Y` confirms.
  - `n`, Esc, Enter, Ctrl-C, any other key, a paste, or any mouse press
    cancels and shows "close cancelled".
  - Releases, wheel, focus and resize events are ignored.
  - Every prompt event is consumed. No byte, key, paste or click reaches a
    child, and a click that cancels does not open the menu or move focus.
- **Status messages**:

  | Server result | Status message |
  | --- | --- |
  | Exited | "pane closed" |
  | Escalated | "pane closed (forced)" |
  | Exited or Escalated, survivor resize pending | "… (resize pending)" |
  | Exited or Escalated, last pane | "last pane closed; workspace closing" |
  | Unknown | "close status unknown; the pane may still be running" |
  | Rejected: StaleLayout | "close rejected: layout changed" |
  | Rejected: StaleTarget | "close rejected: pane changed" |
  | Rejected: AlreadyClosed | "pane already closed" |
  | Rejected: NoLayout | "close rejected: no layout" |
  | Rejected: NotAdmitted | "close rejected by the server" |
  | Transport or server error | the existing fixed label |

## Design

- The target and the **server** layout revision are captured when the menu
  or prompt opens. The menu's mirror revision is local, so it is replaced
  with `menu_revision`, as splits already do. Later layout updates do not
  change the captured values; the server rejects stale ones.
- `Net::close` sends one keyed request with one fresh key:
  - It uses the target's own connection when connected, otherwise any
    connected pane. Any same-UID lease may close, and the key uses that
    connection's lease.
  - A lost reply goes through the existing `keyed` path: the key is marked
    uncertain and settled by status. It is never resent with a new key, and
    the client never retries automatically.
  - A schema-9 result for this key gets `ack_admitted`. A schema-1 response
    for this key gets `ack`. Any other reply is "close reply invalid", and
    the key is not acknowledged.
- On Exited or Escalated:
  - `Net` drops the closed pane's `PaneConn` and polls the layout
    immediately.
  - `App` moves focus to the sibling and discards batched keys typed for the
    closed pane. They are never redirected to another pane.
- Shrink for any reason, whether an explicit close or the server collapsing
  a pane whose shell exited, goes through `set_layout`:
  - `local_layout` rebuilds a one-pane mirror, so split items are enabled
    again.
  - Focus moves to the remaining pane.
  - Batched keys for the removed pane are discarded.
  - An open prompt for the removed pane is cancelled with "pane already
    closed".
  - The removed pane's screen cache is released.
  - `Net::sync_panes` drops its connection.
  - The compositor repaints fully because the pane rectangles and divider
    change.
- After a shrink, splitting works as before (`Net::split` attaches and claims
  the new pane).
- When the last pane closes, the server ends the workspace. The host's
  existing endpoint check prints the closed-workspace notice.

## Tests (actual results)

There are 17 new tests:

- `split_menu`: 4
  - The close item is always enabled, has no letter, and returns the
    captured values.
  - Mouse presses on the close row work in both normal and compact modes.
  - Only `y` confirms in the prompt model.
  - Prompt text, including the last-pane warning and clipping.
  - The redaction test was extended to `CloseIntent` and `CloseConfirm`.
- `keys`: 1 (`confirm_input` mapping).
- `net`: 2
  - The encoded request carries the exact key, target and revision, and a
    zero revision is refused locally.
  - Replies are matched to the exact key (result, rejection result, schema-1
    refusal, wrong key, truncated).
- `state`: 10
  - Ctrl-B `x` then `y`.
  - Nine cancel inputs, with no bytes reaching the child.
  - Releases do not cancel the prompt.
  - The menu Close item uses the captured revision.
  - Last-pane warning.
  - Every result message, with exactly one request and no retry.
  - The resize-pending message.
  - Shrink repair.
  - A prompt is cancelled by auto-collapse.
  - Re-split after a shrink.

Existing tests changed to match the new geometry (no checks were weakened):

- Menu geometry tests: the normal overlay is 8 rows; the compact overlay is
  6 rows, or 5 at the 5-row minimum, where the status row is omitted. The
  updated tests assert more.
- `navigation_mode_x_detaches` became `navigation_mode_ctrl_x_still_detaches`.

| Command | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | pass |
| `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --lib` | pass |
| `cargo test --locked --offline --lib` | 497 passed / 0 failed / 4 ignored |
| `cargo test --locked --offline` | 636 passed / 0 failed / 15 ignored (baseline 619/0/15) |

No live end-to-end close was run, because the runtime route is not there yet.

## Limitations / open items

- The `p3a_*` host tests never exercise Ctrl-B `x`. They only use Ctrl-B `s`,
  `r` and arrows. `P3A-DEFAULT-OPEN.md` still says NAV `x` detaches; the
  parent should update it.
- Add a host-level close test (PTY: Ctrl-B `x` `y`, collapse, re-split) once
  the runtime route lands.
- The client blocks for up to 4 s (`CLOSE_DEADLINE`) while the server stops a
  child. Input typed during that time is read afterwards; none of it is lost
  or replayed.
- After an `Unknown` result the pane stays; the user can try again with a new
  confirmation and a fresh key.
- The ms-rust guideline skill is not installed, so no compliance claim is made.
