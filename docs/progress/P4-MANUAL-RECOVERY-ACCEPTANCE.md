# P4 manual-recovery owner acceptance fixtures

`tests/p4_manual_recovery.rs` exercises the owner workflow across the real
`ManualRecovery`, `ProviderBridgeRuntime`, prepared-choice writer, managed
binding writer, Unix source socket, and recovery endpoint. The test source
implements the bounded v2 outer packet fixture so it can respond to snapshots,
validation, and navigation on one connection. It is a cooperating protocol
fixture, not OpenCode and not evidence of provider behavior or identity
attestation.

The explicit workflow test starts from a Home snapshot, then submits exact
choices that the fixture classifies as missing, foreign-project, and child
sessions. Those choices must return their fixed manual-recovery reasons without
creating `prepared-resume.record` or issuing navigation. A verified explicit
choice returns a Prepared ticket only after the prepared file exists in its
Prepared state; the test also checks that no managed binding row exists at that
point. A distinct Resume request must persist Spent before the fixture observes
the navigation request. The fixture then changes its selected top-level ID,
and the owner must wait for a new snapshot and durable managed-binding
publication before returning Confirmed. The test checks the combined record
file only after that reply.

The same case disconnects a resume client before the owner dispatches it and
verifies that the ticket remains Prepared with no navigation. It then retries
the still-unspent explicit request successfully, checks duplicate resume
rejection, reopens the prepared store, and verifies that its Spent record still
cannot issue a second action. A second case drops the client after the source
observes the one-shot navigation request. It verifies that the selected route
and managed binding can become durable despite a lost acknowledgment, and that
reopening the Spent record never replays the action.

The fixture reads the prepared record's bounded fixed state byte only after the
source receives the navigation request; that ordering checks that the Spent
publication happened before the external action. It checks the combined
managed-record magic after Confirmed, while the production writer retains its
exclusive store lock. Tests do not open a second store handle or infer a
provider identity from the fixture's session choice.

## Verification

Command:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --test p4_manual_recovery -- --test-threads=1
```

The default sandbox attempt is blocked before runtime startup because
`UnixListener::bind` for the private source pathname returns `EPERM`
(`Operation not permitted`). With the authorized local IPC/disk test run outside
that sandbox boundary, both integration cases passed: **2 passed, 0 failed**.
The initial permitted run exposed and fixed a fixture path error before the
passing result. No skipped test is counted as a pass.

Strict test-target Clippy passed with `-D warnings`. No provider executable,
live model, network service, or automatic resume is used. No dependency or
commit was added.
