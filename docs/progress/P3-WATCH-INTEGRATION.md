# Bounded watch and atomic reconciliation integration — 2026-10-18

Status: source integration, focused checks and the actual socketpair/private-PTY
scenario pass. Expanded actual CLI pathname acceptance is separately pending;
Phase2 clipboard/zsh retest remains open. This is a one-child experimental
server, not provider readiness/recovery or a completed multiplexer.

## Delivered path

Three GPT-6-Luna xhigh owners implemented disjoint codec, authoritative owner,
and delivery/transport/driver slices under P3-LIVE-WATCH-TASKS.md. Parent reviewed
the source, added CLI parsing/validation/JSON and the PTY metadata accessor,
exported the codec, and integrated the real-PTY scenario. The codec owner also
independently reviewed the CLI without finding a concrete protocol/security bug.
No new dependency was introduced.

`reconcile TARGET` binds one bounded screen read to owner/collected-exit metadata
and a generation-scoped journal cursor in one state-owner turn. The owner checks
live Read|Events authority and the complete immutable target before accessing
the sink/journal. Metadata is synchronized before and after the read without
another poll; returned screen revision must match the authoritative revision.
Complete encoded byte and per-request line limits apply before responding.

`watch TARGET GEN:SEQ LIMIT WAIT_MS` returns at most64 metadata facts and requests
at most1000ms of waiting. Empty positive waits are retained by the owner driver,
at most8, and revalidated on ticks; the state-owner thread never waits on a
subscriber. The connection's sole worker writes Events/requestID0 followed by
one WatchEnd Response with the original requestID, under a single absolute
250ms batch-write budget. At most6200encoded bytes comprise the64Events and
terminal frame. This is separate from6144bytes of journal history and from
future large screen snapshots. No indefinite subscriber/output queue is added.

Transport rejects Events until the caller explicitly enables them after a
validated negotiated Welcome; enabled Events must be valid exact72byte schema3
payloads. Snapshot frames remain disabled. An incoming client Event remains a
forbidden request role. The worker validates the entire owner's proposed batch
before sending its first byte: capability, full target, consecutive cursors,
limit/count/status and final cursor. TimedOut/ResyncRequired have no events.

The CLI uses one absolute2second budget for the whole watch exchange and prints
only after its terminal response validates. It never reserves/ACKs local input
sequences, clears uncertainty, retries or replaces contexts. New contexts ask
for Events optionally; reconcile/watch require it. Existing leases cannot gain
a capability on resume. A separate new context requires explicit initialization.

CLI result policy: Changed exits0; TimedOut and ResyncRequired exit1 with their
explicit status, and protocol/transport/server rejection also exits1 with a
fixed error. Timeout means no observed change for this request, not application
failure or a matched condition. Resync's cursor is only a hint; obtain a fresh
reconciliation before claiming current state. Metadata revision can advance
with adapter output/lifecycle observations even if visible text looks unchanged.

## Real process proof

Parent expanded `tests/p3_socket_detach.rs` with a second scenario. One explicit
private shell disables echo and emits its PID. A negotiated Events client
reconciles readiness, claims input and reconciles the owner revision; the
metadata-only context file stays unchanged across reconciliation.

Two actual authenticated socketpairs resume that same lease. One sends a
positive watch; the owner thread must observe `pending_watches()==1` before a
bounded channel permits the other connection to submit TOKEN. This proves the
watch waited before the observed change, without sleep-only synchronization.
The watch returns ordered screen metadata events and its terminal response.
Reconciliation contains the same ready PID, owner and application GOT marker,
with a cursor/revision consistent with the observed events. An explicit later
CHECK reports exactly1handled TOKEN, proving no watch/reconcile input injection
or resend. A final explicit EXIT yields a collected ChildExited event, then an
atomic reconciliation with child_exited=true and final output. The original
lost-ACK/detach/reconnect scenario still passes unchanged in the same target.

Actual focused parent command:

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_socket_detach pending_live_watch -- --nocapture
```

Exit0:1passed in0.13seconds. Parent focused CLI tests also passed15/15,
including parser boundaries, complete/partial/wrong-identity event batches,
explicit empty outcomes and escaped reconciliation JSON. These counts are
subsets of the full suite, not additional passing tests.

## Combined verification and limits

Parent `cargo test --locked --offline -q` with the writable CARGO_HOME exited0:
**302passed/13ignored** (244library,36binary,4host preflight,11core PTY,
3logical PTY,1notification integration,1pathname-parser regression,
2actual socketpair/private-PTY scenarios; zero doctests). Strict all-target
Clippy and build passed. Final formatting/document checks are recorded in
INTEGRATION.md. The earlier ignored pathname fixture is operator-confirmed;
the extended new CLI watch/reconciliation commands need their own external run.

Owner tracking failures or cursor exhaustion disable notifications without
stopping the owned child, changing input outcomes or claiming effect rollback.
Read and Input remain separately authorized; same-UID trust does not isolate
local agents. Watch timing is subject to scheduling and existing core hard-error
cleanup; the ordinary100ms input drain can add up to two seconds of cleanup.
No hard whole-call watch or host latency guarantee is claimed. Power loss,
server-death recovery, multiple sessions, layout changes, live scrollback,
provider identity/task success and indefinite subscriptions remain unsupported.
All W1/W2/W3 implementation ownership is released; no phase completion commit
or push is claimed while acceptance and Phase3 capabilities remain incomplete.
