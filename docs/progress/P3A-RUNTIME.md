# P3A wave 3A — multi-session runtime, schema-8 pane UI and `serve`

Status: implemented 2026-10-19 as worker output for parent review. It
delivers a runnable multi-session server that a future interactive client can
drive. That client can show two panes, type into either pane, and split once
via schema 7, all over the existing authenticated pathname socket. There is
still no interactive UI, mouse, or menu, so Phase 3A is not complete. The
one-child `server-probe` path (`ServerDriver` over `StateOwner`) is unchanged.

## Architecture

- `src/multi_runtime.rs` contains `MultiServerDriver`. It owns one
  `MultiSessionOwner<PtyEffects>`, the `ServerStorage` lock, the explicit
  terminfo directory, a private bounded worker hub and a bounded pending-watch
  list. Field and `Drop` order is: stop workers, settle watches, then drop the
  owner (reaping both children), then drop storage (releasing the singleton
  lock).
- The worker hub has the same bounds as `server_io::WorkerHub`: 8
  connections, an owner queue of 8, the same read/write/reply deadlines, and
  one-slot reply channels. It is separate because the one-child hub only
  admits schemas 1/2/4 and cannot write `Snapshot` frames.
- Workers decode and classify frames and forward exact payload bytes only.
  Every prepare, spawn, resize, key delivery, screen capture and journal
  write runs in `MultiServerDriver::tick` on the driver's thread. This is the
  single owner path that the
  [spawn-descriptor gate closure](P3-SPAWN-DESCRIPTOR-GATE.md) requires.
- `src/multi_startup.rs` contains `StartedMultiServer::start`. It opens the
  ledger and an internal launch lease (lease 1, which has no grant, so no
  client can resume it). It then encodes a schema-6 request from the explicit
  server-start launch context: the invoking CLI's own cwd and environment,
  executable and argv. That request goes through
  `MultiSessionOwner::execute_launch` with `managed_launch_prepare`, using a
  journal ordered intent-before-spawn. The first `Applied` launch establishes
  layout revision 1. The request bytes, which contain environment values, are
  dropped right after the launch. Journal records hold only
  key/session/state.

## Connection model

Grants are pinned to one full target, so a UI uses **one connection per
pane**. The Hello frame payload is an existing SessionHello (28 or 44 bytes),
optionally followed by a 48-byte schema-8 AttachTarget trailer:

| Hello | Attach target |
| --- | --- |
| fresh, with trailer | the named pane |
| fresh, no trailer | first layout pane |
| resume (44 bytes), no trailer | that lease's pinned target |

The one-child runtime still rejects trailers because they change the length.
Read-capable connections get server-side `Snapshot` writes enabled. Clients
opt in to reads with `AuthenticatedStream::enable_snapshot_reads`.

## Routes and capability rules

| Schema | Route | Rule |
| --- | --- | --- |
| 1 | `handle_control` | unchanged schema-1 semantics, pinned scope |
| 2 | `handle_session` | Describe uses the pinned grant; Claim/Release/Handoff by full target |
| 4 | reconcile/watch (legacy) | as schema 5; the cursor session is the target's own session (pinned grants make it unambiguous); replies are encoded in schema 4, so the existing `control reconcile/watch` CLI works against serve |
| 5 | scoped reconcile/watch | READ+EVENTS, pinned scope, bounded pending watches (8) |
| 6 | status query only | own lease, READ; kind 1 launch → error `launch_unsupported` |
| 7 | split | see below; status query → `split_status_unsupported` (resend exact bytes instead) |
| 8 | pane UI | see below |
| other | — | `unsupported_request_schema` |

**Split rule.** The connection must have selected **INPUT**, which requires
`--allow-input`. The split key must use the connection's own lease, and the
connection must be pinned to the exact split target. A split never claims or
requires input ownership. A `Failed` split is returned as an encoded
`SplitResult`; ledger conflict, in-progress and generation errors are fixed
error labels.

**Launch-context rule.** The split child's cwd, environment, executable and
argv come **only** from the embedded schema-6 request. That request holds the
client-captured context, prepared with `prepare_launch` plus the server-owned
terminfo, `TERM` and `PANELINK_*`. The server's ambient environment is never
read. The CLI captures its own process environment (minus `PANELINK_*`,
bounded to 1,024 entries / 48 KiB) and cwd or `--cwd`, and launches
`/bin/zsh` with no arguments.

## Schema-8 wire format (big-endian)

Requests:

| Kind | Name | Bytes | Class | Layout |
| ---: | --- | ---: | --- | --- |
| 1 | LayoutQuery | 8 | Query | schema, kind, 4 zero |
| 2 | ScreenQuery | 48 | Query | schema, kind, 4 zero, target (40) |
| 3 | KeyInput | 88+8n | Input | see below |
| 4 | RootResize | 40 | AbsoluteResize | schema, kind, key (24), expected layout rev u64, rows u16, cols u16 |
| 5 | AttachTarget | 48 | Hello trailer | schema, kind, 4 zero, target (40) |

KeyInput layout:

- 0–3: schema and kind.
- 4: key (24 bytes).
- 28: target (40 bytes).
- 68: expected owner revision, u64, nonzero.
- 76: count, u16, 1..=64.
- 78: 10 zero bytes.
- 88: entries of 8 bytes each: code u16, zero u16, scalar u32.

Key codes: 1 Char (non-control scalar), 2 Enter, 3 Tab, 4 Backspace, 5 Escape,
6 Interrupt, 7 Up, 8 Down, 9 Left, 10 Right, 11 Home, 12 End. Non-Char keys
must carry scalar 0.

Replies:

| Kind | Name | Bytes | Frame |
| ---: | --- | ---: | --- |
| `0x8001` | LayoutReply | 184 | Response |
| `0x8002` | ScreenReply | 64 + checkpoint (≤ 921,760) | **Snapshot** |
| `0x8003` | ResizeReply | 48 | Response |

LayoutReply fields:

- 4: revision, u64.
- 12: rows; 14: columns.
- 16: pane count (1 or 2).
- 18: divider flag; 20: divider rect (x, y, rows, cols, u16 each).
- 28: 4 zero bytes.
- 32: pane 0 (72 bytes); 104: pane 1 (72 bytes, zero when absent).
- 176: 8 zero bytes.

Each pane entry holds: target (40), rect (8), pending flag u16, zero u16,
confirmed rows u16, confirmed cols u16, and 16 zero bytes.

ScreenReply holds: target at 4, screen revision at 44, checkpoint length (u32)
at 52, 8 zero bytes, then one complete `screen_checkpoint` record for the
same target and revision.

ResizeReply holds: key at 4, status at 28 (1 Applied, 2 ResizePending),
pending pane bits at 30, revision at 36, rows at 40, cols at 42, and 4 zero
bytes.

Every encoder validates before writing. The decoders check size bounds before
parsing, plus reserved bytes, nonzero IDs, generation consistency and layout
consistency. Errors are static, data-free `UiCodecError` values. `Debug`
redacts targets, keys and scalars. `planned_split_size` previews the new
pane's size with the same `PaneLayout` geometry the server uses.

**Schema-8 semantics.**

- **LayoutQuery** needs only READ. Geometry is shared across both panes'
  connections. It reports the pending-resize flag and confirmed size per pane.
- **ScreenQuery** needs READ and the pinned target; another pane returns
  `unauthorized`. The checkpoint is encoded on the owner thread from
  `PtyEffects::screen()` at `PtyEffects::revision()`. It is at most one
  80×240 checkpoint (≤ 921,760 bytes), carried in one bounded Snapshot frame
  with a 2 s write deadline.
- **KeyInput** follows the same order as control TEXT:
  1. Live connection, own lease and INPUT.
  2. Exact-key duplicate returns the recorded status and is never
     re-delivered.
  3. Pinned scope, then stale target.
  4. Input-owner pin at the expected revision.
  5. `PtyEffects::validate_keys`, then admission.
  6. Delivery via `PtyEffects::keys` (typed `TerminalSession::key`, the same
     bounded drain as text; any unwritten suffix is discarded, giving
     `Partial`/`Failed`).
  7. Completion.
- **RootResize** needs INPUT and the caller's own lease; input ownership is
  not required. Duplicates return the recorded status. It calls
  `resize_layout` with `PtyEffects::resize`. The ledger records `Applied`,
  `Partial` (some pane pending) or `Failed` (layout rejection; no PTY effect;
  error labels `stale_layout`, `layout_rejected` or `no_layout`). Same-size
  requests retry only pending panes, as before.

## Other changes outside the new files

- `src/transport.rs` (outside the original ownership; the user approved this
  edit): adds opt-in `enable_snapshot_reads` and `enable_snapshot_writes`.
  Only opted-in directions accept or send `Snapshot` frames, up to the
  existing 2 MiB wire cap; the receive buffer is reserved fallibly from the
  validated header. Defaults and all existing tests are unchanged.
- `src/server_pty.rs`: adds `screen()`, `validate_keys()` and
  `keys(iter) -> Outcome`. The drain was refactored into `drain_request`
  (same behaviour).
- `src/server/multi_session.rs`: adds `lease_target`, `first_pane_target`,
  `layout_view`, `observe_target`, `handle_key_input`, `handle_root_resize`
  (with `RootResizeReply`), `authorize_split`, `launch_status`, and the
  `LayoutView` alias. `src/server.rs` is unchanged; the existing private
  helpers are reachable from the child module.
- `src/p3_cli.rs` / `src/main.rs`: add the `serve` subcommand (same flags,
  singleton and endpoint semantics as `server-probe`; Ctrl-C, SIGTERM and
  SIGHUP stop it; the root size is the stdout TTY size clamped to 5–80 ×
  20–240, else 24×80). They also add control `init [TARGET]`, `layout`,
  `split TARGET LAYOUT_REV left|right|up|down [--cwd DIR]` and
  `keys TARGET OWNER_REV TEXT` (≤ 63 printable scalars plus Enter). Split
  sends the layout query and the split on one connection, with request IDs 1
  and 2.

## Tests

- `ui_protocol` (11): golden request and reply bytes, round trips for every
  key, zero IDs and revisions, key-count bounds (0, 64, 65, size cap),
  unknown codes, and control/surrogate/out-of-range scalars. Also
  reserved/trailing/truncated input, inconsistent layout/resize/screen
  replies, the AttachTarget trailer, `planned_split_size`, and redaction.
- `transport` (+2): opted-in Snapshot round trip above 64 KiB, and the
  read-only/write-only direction rules. An oversized Snapshot header is
  rejected before allocation.
- `multi_session::runtime_helper_tests` (5): key input needs INPUT, own
  lease, ownership and pinned scope, and never replays. Root resize covers
  Applied, duplicate, stale and Partial/pending outcomes. Split and resize
  need INPUT and the caller's own lease. Observation and launch status are
  scoped, and the internal lease has no grant.
- `multi_startup` (2): first launch establishes the layout, journals
  Intent/Spawned/Applied with no env or argv bytes, and holds the lock.
  Invalid context or failed preparation spawns nothing, journals nothing and
  releases the lock.
- `p3_cli` parser (+1): serve and the new control forms, plus malformed
  variants.
- `tests/p3a_runtime.rs` (1, real `/bin/zsh -f`, explicit env, empty private
  HOME/ZDOTDIR, socketpairs):
  - Pane 1: layout shows 1 pane; ScreenQuery checkpoint shows READY; schema-6
    launch → `launch_unsupported`; keys before claim are rejected; claim +
    KeyInput `echo ACK1` Enter → ACK1; an exact replay is not re-typed.
  - Splits: a stale-revision split is `Failed` with no effect; Split Right →
    2 panes (40|40 + divider).
  - Pane 2: a second connection pinned to pane 2 reads its own READY, is
    `unauthorized` for pane 1's screen, claims, and its ACK2 appears only in
    pane 2.
  - Lost-ack split replay gives the identical result, still 2 panes; a third
    split is `Failed` at capacity.
  - RootResize to 30×101 → both `stty size` show 30x50; a stale RootResize →
    `stale_layout`.
  - Disconnecting every client keeps both PIDs alive; dropping the driver
    reaps both (ESRCH) and releases the lock.
  - Passed 5/5 consecutive runs.
- `tests/p3a_serve_pathname.rs` (1, not ignored): runs the real
  `panelink serve --initialize --allow-input -- /bin/zsh -f -i` on a
  pathname socket.
  - A second serve is refused (`server_storage_unavailable`).
  - `control init`, `layout` (1 pane), `split FIRST 1 right` (applied, rev 2),
    `layout` (2 panes); a further split is refused locally
    (`split_unavailable`).
  - Pane 1 claim + `keys` → `PANE1-42`.
  - `init PANE2` context, claim + `keys` → `PANE2-43`, each only in its own
    pane.
  - SIGTERM → exit 0, `stopped` line, socket removed.
  - Passed 3/3. If the bind fails with `listener_unavailable`, the test
    prints `SKIPPED:` with the reason; that did not happen on this host.

## Verification (2026-10-19, macOS arm64, Rust 1.99.1)

| Command | Result |
| --- | --- |
| `cargo test --locked --offline` | 535 passed, 0 failed, 14 ignored (pre-existing `#[ignore]`) |
| `cargo fmt --check` | clean |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | clean |
| `RUSTDOCFLAGS="-D warnings" cargo doc --locked --offline --no-deps` | clean |
| `CARGO_HOME="$HOME/.cargo" python3 scripts/verify_non_ui.py` | all 8 checks passed, 3/3 runs |
| `python3 scripts/verify_non_ui.py` (default) | cargo checks fail: the script's fallback `/private/tmp/panelink-p2-cargo-home` registry lacks the pre-existing `libc` dependency (environment issue; `Cargo.toml`/`Cargo.lock` unchanged) |

One intermittent failure was found and fixed during verification. Under
parallel `cargo test`, a sibling test's fork can briefly share an
`flock`ed storage descriptor until its exec closes it (close-on-exec). An
immediate re-`open` after drop could therefore see `WouldBlock`. The new
startup and runtime tests now wait up to 3 s for the lock release, instead of
asserting the first attempt.

## Review fixes (2026-10-19)

1. **Inherited `PANELINK_*`.** `serve` now captures one filtered context
   first (inherited `PANELINK_*` dropped) and reuses it for the terminfo and
   executable preflight (`main.rs::preflight_serve_launch`) and for startup.
   Previously the preflight captured the unfiltered environment, so starting
   `serve` from inside a pane failed with `launch_context_invalid`. Tests: a
   unit test (`filtered_launch_context`), and the pathname test, which runs
   every process with inherited
   `PANELINK_SESSION`/`PANELINK_GENERATION` canaries. Both children echo only
   the server-owned values (`ENV1-1-…-<generation>`, `ENV2-2-…`); the
   canaries never appear. With the old preflight swapped back in, the
   pathname test fails.
2. **Schema-4 reconcile/watch** are accepted by `multi_runtime`, mapped to
   the scoped form for the request's own target, and answered in schema 4.
   `server-probe` is unchanged. Tests: a `multi_runtime` unit test
   (mapping/classification). The pathname test runs `control reconcile`,
   `watch` with no change (`timed_out`), watch after output (`changed`), and
   reconcile of the other pane, which is refused by pinned scope.
3. **Two-pane `LayoutReply`** must be exactly one horizontal or vertical
   partition: ordered, full cross-axis span, a one-cell divider directly
   between the panes, and exact coverage of the root. Negative tests cover
   overlap, gap, reversed order, short divider, misplaced divider, short or
   narrow coverage, a two-cell divider, and the decoder path.
4. **Pathname skip.** The test now fails on any startup error. It skips only
   when `PANELINK_ALLOW_PATHNAME_SKIP=1` is set **and** serve reports
   `listener_permission_denied`, a new label emitted only for a bind error of
   kind `PermissionDenied` (EPERM/EACCES). The skip prints
   `SKIPPED (NOT A PASS)`.

## Limits and open items

- **Not done:**
  - No interactive client, right-click menu, compositor wiring or host mouse
    proof.
  - No split status query. Recover a lost split by resending the exact bytes,
    which returns the retained result without effects.
  - No client-initiated schema-6 launch.
- **CLI behaviour:**
  - The CLI's `split` previews the size from the current layout and refuses
    locally once 2 panes exist. The server still independently rejects
    stale, capacity-full or mismatched requests (proven over socketpairs).
  - `keys` sends text plus Enter only. Other keys exist in the wire format
    but are not exposed in the CLI.
- **Runtime behaviour:**
  - The serve loop ignores per-session poll errors so that one exited child
    does not stop its sibling. A storage-identity failure or listener error
    still stops the server.
  - ScreenQuery allocates one checkpoint-sized buffer per request on the
    owner thread (≤ 0.9 MB, fallible reserve). It is bounded by the 8-request
    queue drain per tick; it is not preallocated.
  - Layout, ledger and the split result are volatile. A restart starts a new
    generation with one fresh child; nothing is re-executed from the journal.
- **Out-of-scope edit:** the `src/transport.rs` opt-in, approved by the user.
  It needs parent review.
