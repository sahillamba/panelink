# P3 N2 bounded metadata event journal

Status: implementation and focused local verification complete. This is a
volatile pure-Rust journal for validated N1 metadata events; it does not add
live subscriptions, dispatch, capability advertisement, automatic resync, or
server runtime behavior.

## Contract implemented

`src/event_journal.rs` exports `EventCursor`, `EventJournal`, `JournalError`,
`MAX_RETAINED_EVENTS = 64`, and `MAX_RETAINED_ENCODED_BYTES = 6144`.
`EventJournal::new` rejects generation zero and starts at sequence zero.
`push` checks target generation, obtains the next sequence with `checked_add`,
validates the exact N1 `Event`, then mutates the queue. Invalid pushes and
sequence exhaustion leave the cursor and retained events unchanged.

The journal preallocates a 64-entry `VecDeque`. On event 65 and later, it evicts
the oldest event before appending, keeping the length at or below 64. Each
retained event accounts for its fixed 72-byte payload plus the 24-byte wire
frame header, so the logical total is at most `64 * 96 = 6144` bytes. This is
wire-size accounting, not an RSS bound.

`read_after` accepts sequence zero, requires the same generation, rejects a
future sequence and limits outside 1–64, and returns the earliest retained
events strictly after the supplied cursor. If sequence `n` is the last evicted
event, cursor `n` is valid and cursor `n-1` returns `CursorExpired`. The caller
must reconcile authoritative state after expiry. Results are pages; the
separate `current_cursor` reports the latest fact and does not imply delivery or
completion. Errors and custom `Debug` implementations expose no cursor or
target identifiers. The journal has no clock, storage, lock, sink, dispatch,
coalescing, snapshot, or generated identity source.

## Verification

The owned source passed:

```text
$ rustfmt --edition 2021 --check src/event_journal.rs
exit 0
```

Before the parent added the `lib.rs` exports, focused tests ran in a temporary
harness outside the repository. The harness imported the real `protocol.rs`,
`protocol_wire.rs`, `control_protocol.rs`, `event_protocol.rs`, and this journal
module. Its focused command reported:

```text
$ /private/tmp/panelink-event-journal-tests event_journal::tests
running 8 tests
8 passed; 0 failed; 0 ignored; 53 filtered out
exit 0
```

The complete temporary harness also ran the existing protocol, wire, control,
and N1 event-codec tests:

```text
$ /private/tmp/panelink-event-journal-tests
running 61 tests
61 passed; 0 failed; 0 ignored
exit 0
```

After module export, the focused root library and cross-component tests passed:

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib event_journal::tests
exit 0: 8 passed; 0 failed; 0 ignored
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_notifications
exit 0: 1 passed; 0 failed; 0 ignored
```

The integration test exercises cursor expiry and pagination with actual 96-byte
encoded frames. Full-workspace tests and Clippy remain with the parent after
parallel work integration.

The first harness compile failed because the temporary crate omitted
`protocol_wire`, which `control_protocol.rs`'s own tests import. Adding that
module to the harness fixed the compile; no application source change was
needed. The first journal format check also reported one rustfmt line wrap;
`rustfmt --edition 2021 src/event_journal.rs` corrected it, and the owned-file
check then passed. Full-workspace tests and Clippy are left to the parent after
parallel work integration.

## Ownership release

Work is limited to `src/event_journal.rs` and this evidence file. No module
export, dependency, manifest, roadmap, shared specification, commit, or push was
changed. Ownership is released to the parent for independent review and
integration. This component does not complete Phase 3 or establish live event
or watch capability.
