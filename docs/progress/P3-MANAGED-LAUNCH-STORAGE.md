# Launch journal capacity and fault checks — 2026-10-18

The parent added `LaunchJournal::preflight_launch` in
[src/server_storage.rs](../../src/server_storage.rs). It validates the new
operation/session identity, rejects a spent session within the same generation,
checks the cumulative 4,096-key limit and requires all 168 bytes for
Intent/Spawned/Applied before the first Intent. Checked addition rejects byte
counter overflow. Poisoned handles reject preflight. The check writes nothing
and reserves nothing; the single owner must serialize the ensuing transaction.
It does not guarantee a later write, flush or filesystem operation succeeds.

The existing one-child [startup](../../src/server_startup.rs) now performs this
check before ledger admission and child spawn. The record path also rejects a
new operation that would reuse a session identity in its generation. Identical
records for the existing operation remain idempotent; later generations may
use their own session 1. No persistent record format changed, and no new launch
endpoint or authority is enabled.

The parent ran the locked/offline `server_storage` library tests: **11 passed,
0 failed**. Three new tests establish read-only preflight, invalid/spent
identities, exact three-record headroom and overflow, global historical-key
capacity, poisoned admission, and genuine production-path write failures after
Intent or Spawned. Reopening those incomplete rows reports Unknown in a new
generation. Capacity boundary counters and historical entries are injected in
memory rather than creating thousands of fully synced records. Write failures
use a read-only file handle; these are not simulated power loss or flush-fault
proofs. No test launches a child from recovered metadata.

This is a storage prerequisite. Multi-session registration, global admission,
real-child post-spawn retention and API launch acceptance remain separate.

The parent also reran all four `server_startup` tests successfully. The former
full-journal case now exercises zero, one and two records of remaining space:
each rejects startup without appending even Intent or creating the child marker.
Existing real-child startup and post-spawn commit-failure cases still pass.
