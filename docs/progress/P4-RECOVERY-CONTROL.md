# Phase 4 recovery-control wire

Status: the scoped v2 transport now has typed exact-session validation and
one-shot navigation messages. This is a transport/API slice; runtime save,
resume UI, plugin integration, selected-route confirmation, and full Phase 4
acceptance remain separate gates. An issued navigation action is never proof
that the provider selected the requested session.

The v2 envelope keeps its existing 24-byte `PLBRSN02` header and adds kinds
6–9. All four use the same strictly increasing challenge sequence as snapshots
on the connection. A request and its response must carry the identical
nonzero challenge. The reader and sender serialize these operations with
snapshot I/O; a malformed packet, wrong role, stale challenge, wrong scope, or
expired exchange poisons the channel.

| Kind | Body | Meaning |
|---|---|---|
| 6 ValidateRequest | `u16be id_len || ASCII session_id` | Ask the provider interface to check one exact ID without navigating. |
| 7 ValidateResponse | `status:u8, reserved:u8=0, id_len:u16be, project_len:u16be, reserved:u16be=0, id, project` | Status 1 Verified requires an exact ID echo and nonempty project bytes equal to the connection grant. Status 2 Missing, 3 ForeignProject, 4 Child, 5 Unsupported, and 6 Uncertain require empty identity fields. |
| 8 NavigateRequest | `revision:u64be, ticket:[u8;16], id_len:u16be, reserved:u16be=0, id` | Request one native route action after exact-ID validation on this connection. Revision and ticket must be nonzero. |
| 9 NavigateResponse | `status:u8, reserved:u8=0, id_len:u16be, revision:u64be, ticket:[u8;16], id` | Status 1 ActionIssued or 2 NotIssued, with exact ID/revision/ticket echoes. This reports the action result, not route selection. |

Session IDs are bounded to 1–128 ASCII bytes from `[A-Za-z0-9_.-]`. Project
identity is bounded to 4,096 opaque bytes. Payload sizes remain below the
existing 4,388-byte message limit. Reserved bytes, lengths, status values,
echoes, and scope are checked before accepting a result. A private-constructor
`ValidatedSession` exists only for a verified response and is deliberately
separate from selected-session `Frame` observations.

The public host APIs are `FreshReader::validate_session` and
`FreshReader::navigate_session`. Navigation is accepted only for the exact ID
validated on the same connection and only once per connection, including a
NotIssued result or a lost acknowledgement. The provider side exposes typed
`FreshSender::respond_validate_once` and `respond_navigate_once` handlers; each
callback receives validated bounded fields, never arbitrary packet bytes.
Foreign-project IDs remain an explicit refusal/manual-fallback case. This wire
does not provision a new project scope or retry an uncertain action.

Both control exchanges cap transport I/O to 300 ms and check the deadline
before callback execution. Synchronous provider SDK work cannot be forcibly
interrupted by the Rust transport; its adapter must independently enforce the
same timeout, discard any late result, and report Uncertain. If navigation
occurred but its response was lost, the host must not retry. A later fresh
snapshot is still required to prove selection.

## Verification

Focused local loopback and codec checks:

```sh
cargo test --lib provider_bridge_v2 -- --test-threads=1
cargo test --lib provider_recovery_control -- --test-threads=1
```

These tests cover shared snapshot/validation/navigation challenges, callback
ordering, malformed requests rejected before callback, exact ID/project/ticket
echoes, one-shot navigation, and fail-closed channel poisoning. They do not
prove a live provider SDK adapter, runtime atomic save, manual resume flow, or
Phase 4 completion.
