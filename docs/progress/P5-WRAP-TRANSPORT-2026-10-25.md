# P5 authoritative row-wrap transport — 2026-10-25

Status: negotiated row-wrap metadata transport is implemented and its focused
automated checks pass. This is a transport/cache component only; Phase 5 remains
open. It adds no selection gesture or clipboard lifecycle behavior.

## Transport behavior

- Added optional handshake capability bit 32, `CAPABILITY_SCREEN_ROW_WRAPS`.
  The single-session owner does not advertise it. The multi-session owner may
  grant it to fresh leases; resuming an older grant cannot add it.
- New attach handshakes request the bit as optional. `PaneConn` tracks the bit
  returned by each successful handshake, including reconnects.
- The multi-session runtime reads row flags from the same `ScreenSnapshot` that
  it encodes. History replies use the `history_snapshot` returned for the
  requested offset. Legacy peers still receive the legacy payload.
- Runtime reply validation and client reply decoding use the explicit
  negotiated decoders. An extension without a selected capability is rejected.
- `ScreenUpdate` carries validated wrap flags beside its checkpoint. A history
  result carries the reply revision and flags beside its checkpoint and view
  epoch. Client cache code applies flags only after loading that exact
  checkpoint. An absent bitmap leaves wrap state unknown; checkpoint loading
  clears stale volatile flags.
- The existing codec component adds up to ten bytes to the bounded screen and
  history payload limits. Snapshot frame admission already has a larger fixed
  bound; worker result reservations use the extended reply constants.

## Focused verification

Environment: `CARGO_HOME=/private/tmp/panelink-p4-cargo-home`,
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`; commands used
`--locked --offline` and one test thread.

- Handshake protocol: 14 passed, 0 failed.
- UI protocol: 21 passed, 0 failed. Includes legacy-byte equality, marker and
  trailer strictness, forged/unnegotiated extension rejection, bitmap bounds,
  and maximum extended reply size.
- Single-session capability refusal: 1 passed. Multi-session resume
  immutability and fresh optional grant: 1 passed.
- Multi-runtime focused group: 17 passed, 0 failed. The socketpair runtime test
  negotiates bit 32, observes actual wrapped output, and validates an offset-1
  history snapshot with its own wrap metadata.
- Attach client connection tests: 4 passed, including a socketpair handshake
  proving the client offers the capability optionally.
- Attach client state tests: 39 passed, including exact live/history cache
  application of validated flags.
- Existing snapshot-wrap integration target: 3 passed.
- `cargo fmt --check` reports only the active workspace-index module ordering
  in `src/lib.rs`; the transport-owned files have no remaining formatting
  differences.

The binary test build still reports the existing unused-constructor warning in
the concurrently added clipboard worker. That worker is not constructed by the
host event loop in this transport slice. No full suite or phase verifier was
run.

## Remaining work and evidence limits

The client selection gesture, compositor overlay, host mouse mode 1002
restoration, and clipboard writer lifecycle remain separate integration work.
No host clipboard command was invoked. Apple Terminal drag delivery and
clipboard behavior remain unverified. No phase-completion or user-acceptance
claim follows from these component tests.
