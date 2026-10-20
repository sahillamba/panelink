# P3B wave 1 — explicit close/stop core

Status: implemented on 2026-10-20 as a trusted in-process library core. This is
worker output for parent review. There is no socket or runtime route, no CLI and
no client UI yet: `multi_runtime.rs`, `p3_cli.rs`, `ui_protocol.rs` and
`ui_client/*` are unchanged. Nothing here was committed.

## Settled user decisions applied (2026-10-19)

- Any authenticated same-UID client may explicitly stop an individual pane
  session. There is no separate stop grant, Input capability or target pin.
  The key must use the connection's own lease. This does not infer launch
  authority.
- An explicit stop is: HUP to the child process group, a bounded 1 s wait,
  KILL, and another bounded 1 s wait. The existing `TerminalSession::shutdown`
  windows and ordering are unchanged. The result is `Exited`, `Escalated` or
  `Unknown`.
- When one pane's shell exits while its sibling remains, the exited pane is
  removed. The sibling then fills the root and its PTY is resized. When the
  last shell exits, `all_sessions_ended()` becomes true and the serve loop
  closes the workspace through the existing path.
- Closing a pane with a sibling stops it and then collapses the layout.
  Closing the only pane stops it, keeps its tombstone and ends the workspace.
- After a close or collapse, splitting is possible again. Session and pane IDs
  always come from the monotonic counter and are never reused.

## Public API (for runtime/CLI/client)

```rust
// src/close_protocol.rs (schema 9)
pub struct CloseRequest { pub key: OperationKey, pub target: TargetRef, pub expected_layout_revision: u64 }
pub struct CloseStatusQuery { pub key: OperationKey }
pub struct CloseResult { pub key, pub outcome: CloseOutcome, pub target: Option<TargetRef>,
                         pub layout_revision: u64, pub survivor_resize_pending: bool }
pub enum CloseOutcome { Stopped(StopOutcome), Rejected(CloseRejection) }
pub enum StopOutcome { Exited, Escalated, Unknown }
pub enum CloseRejection { NoLayout, StaleTarget, StaleLayout, AlreadyClosed, NotAdmitted }
encode_/decode_close_request, encode_/decode_close_status_query, encode_/decode_close_result
CloseResult::ledger_outcome() -> Outcome

// MultiSessionOwner<E>
pub fn authorize_close(&self, &Connection, &CloseRequest, now) -> Result<(), ServerError>;
pub fn execute_close<RE, Resize>(&mut self, payload: &[u8], now, resize: Resize)
    -> Result<CloseResult, CloseError>       // Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>
pub fn close_status(&self, &Connection, OperationKey, now) -> Result<CloseResult, ServerError>;
pub fn pane_statuses(&self) -> [Option<PaneStatus>; 2];
pub fn pane_view(&self, &Connection, now) -> Result<[Option<PaneStatus>; 2], ServerError>;
pub struct PaneStatus { geometry: PaneGeometry, state: SessionState, live: bool, resize_pending: bool }
pub enum CloseError { InvalidPayload, GenerationMismatch, Ledger, InProgress, Capacity, InternalInvariant }

// EffectSink (default methods; existing fakes unchanged)
fn stop(&mut self) -> StopOutcome;                         // default: Exited if exited, else Unknown
fn apply_size(&mut self, rows, cols) -> Result<(), Outcome>; // default: Err(Failed)
// PtyEffects implements both; TerminalSession::kill_escalated() -> bool
// PaneLayout::collapse(removed, expected_revision) -> Result<LayoutSnapshot, LayoutError>
// ManagedLaunchCoordinator::{retire, is_retired, retired_count}; SessionRegistry::{retire, is_retired, issued, retired}
```

## Wire layout (schema 9, big-endian)

- Request, kind 1, 80 bytes:
  - `0` schema
  - `2` kind
  - `4` key (3×u64)
  - `28` target (5×u64)
  - `68` expected revision (nonzero)
  - `76` 4 reserved zero bytes
- Status query, kind 2, 32 bytes: header, key and 4 reserved zero bytes.
- Result, kind `0x8001`, 96 bytes:
  - `28` outcome: 1 Exited, 2 Escalated, 3 Unknown, 4 Rejected
  - `30` rejection code: 0 for a stop; 1–5 when rejected
  - `32` layout revision after the operation
  - `40` flags: bit0 target present, bit1 survivor resize pending
  - `42` 6 reserved bytes
  - `48` target
  - `88` 8 reserved bytes

Schema 9 was unused. The runtime classifier must add a `[0, 9]` arm, which
is owned by the next task.

## Transaction ordering

The transaction runs in this order:

1. Decode.
2. Exact-payload duplicate lookup in the global ledger. A duplicate returns
   the retained result before any target or layout check. A changed payload
   under the same key returns `OperationConflict`.
3. Check the bounded result store (`MAX_RETAINED_CLOSE_RESULTS = 4096`).
4. `admit`.
5. Validate the target and revision. A rejection is recorded as `Failed` and
   has no effect.
6. Call `EffectSink::stop` exactly once.
7. For `Exited` or `Escalated`, the remaining steps run:
   1. `note_exited`.
   2. If a sibling exists, collapse the layout and drop the stopped pane's
      size entry.
   3. Retire the slot. This drops the handle; its child is already reaped.
   4. Apply one absolute resize to the survivor.
8. Complete the ledger:
   - `Applied`
   - `Partial` if the survivor resize failed; the pane stays pending and is
     retried by `resize_layout`
   - `Unknown`
   - `Failed`

For `Unknown`, the layout and record are unchanged and a later observed exit
collapses the pane in `poll`. No input is sent or replayed and nothing is
respawned.

`poll()` now ends with `collapse_exited_panes()`. The survivor is resized
through `EffectSink::apply_size`, because `poll` has no caller resize port. If
both shells exit in the same poll, the survivor is not resized and
`all_sessions_ended()` becomes true.

## Identity, dedup and journal bounds

- Registry slots are freed only for `Exited` or `Failed` records. A
  `Failed` record outside the layout is reclaimed before a split. IDs are
  never reused, and `is_retired` recognizes spent targets through the
  counter and synthetic mapping.
- Launch results are now a bounded `Vec` (`MAX_RETAINED_LAUNCH_RESULTS =
  4096`) instead of 2 entries. An exact retry of a retired launch still
  returns its result. Split results are kept in `committed_splits` for
  exact retries after a close and re-split.
- `LaunchJournal` rejects a reused session ID or key. Fresh IDs pass its
  preflight, which a fake journal test enforces and the real-PTY test
  exercises with the real journal.
- Total launches per server generation are bounded by the smallest limit:
  - The journal allows 4096 keys and 1 MiB / 56 B = 18 724 records. A
    3-record launch therefore allows about 6 241 launches, so the 4096-key
    cap applies first. The journal is shared across generations in one
    storage directory.
  - The coordinator retains 4096 results; when full, `next_target` returns
    `IdentifierExhausted`.
  - The ledger allows 4096 operations per lease.
  - Two sessions can be live at once.

  In practice, at most about 4096 launches/splits are possible per storage
  journal before an explicit repair.

## Verification (2026-10-20, run locally)

| Command | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | pass |
| `cargo test --locked --offline` | 619 passed / 0 failed / 15 ignored (baseline 588/0/15) |
| `cargo test --locked --offline --test p3b_close` | 2 passed (zsh, sh real PTYs) |
| `python3 scripts/verify_non_ui.py` | exit 0, all checks passed |

The new tests are:

- codec: 7
- layout: 3
- registry: 2
- coordinator: 2
- terminal escalation (real PTY): 1
- owner close/collapse: 14
- `tests/p3b_close.rs`: 2 real-PTY scenarios

The real-PTY scenarios check that:

- A live sibling closes as `Exited` in under 1.5 s and its PID is reaped.
- The survivor PTY reports `24x81`.
- A re-split gets session 3.
- A HUP-trapping child closes as `Escalated` in 1–2.5 s and is reaped.
- A SIGTERMed shell auto-collapses in `poll`.
- Closing the last pane sets `all_sessions_ended()`.

## Limitations / open items

- No runtime, CLI or client wiring yet. That work also needs a schema-9 route
  and a confirmation UX.
- Close results and the layout are volatile. A server restart loses them, as
  it does for splits.
- `Unknown` stop results leave a possibly-live child retained. Nothing kills
  it again automatically.
- `pane_statuses` reports an exited but uncollapsed pane until the next
  `poll`.
- The ms-rust guideline skill is still not installed. No compliance claim is
  made.
