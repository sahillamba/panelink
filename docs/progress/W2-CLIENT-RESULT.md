# Wave 2 client responsiveness result

## Outcome

The active attach loop now owns only host-terminal work and presentation
state. A single client IO worker owns `Net`, `PaneConn`, socket exchanges, and
context durability operations. It serializes both panes, so a delayed exchange
can still delay the sibling pane; the host remains responsive while it waits.
Initial attach connection is still synchronous before raw mode.

`Net::poll` returns typed layout, screen, and status updates for the host to
apply. The host rejects older layout revisions within the same generation.
Split completions carry their admitted request and captured focus generation;
the host applies the focus intent only after a newer validated layout contains
the returned full target. A user focus move cancels that intent.

The worker uses 32 command slots, 32 result slots, and a shared 4 MiB budget
for admitted command ownership plus reserved results, including in-flight
work. Poll capacity is reserved before polling and remains accounted for until
the host consumes its result. Protocol maxima are checked before copying key
or paste data. One host-owned key batch stays pinned when admission fails;
later paste or mutations cannot pass it. Accepted input is never replayed
after an unknown result. Ctrl-X always exits, including under queue saturation.
Only an unadmitted retained key batch is discarded at detach, with a fixed
notice after terminal restoration. Work already admitted has an uncertain
outcome and is not retried.

Drop closes channels without joining. A blocked result sender exits when its
receiver is dropped. Explicit post-restore detach observes a fixed deadline
and joins only a worker already marked finished; context filesystem sync may
outlive that deadline, and its worker retains sole ownership until it exits.

## Verification

Passed:

- `cargo test --locked --offline --bin panelink ui_client:: -- --test-threads=1`
  (50 client tests, including deterministic queue saturation, blocked fake
  backend, stale split/focus, and Unix socketpair stalled-exchange coverage).
- `cargo check --locked --offline --bin panelink`.

The socketpair test proves command admission stays responsive while a transport
exchange is stalled. A WorkerHandle-backed test observes that terminal restore
precedes the actual worker detach command. The durable reserve-before-send and
validate-before-ack ordering remains implemented in `PaneConn::keyed`; this
slice does not add a delayed-fsync gate at that seam or claim an end-to-end
delayed durability test. This worker slice did not run a host-terminal PTY
acceptance test; that remains an integration follow-up. No sibling-pane IO
isolation is claimed.

## Limits and risks

- `Net::connect` can still delay startup before raw mode.
- The 4 MiB figure covers admitted command payloads and reserved results, not
  the complete process footprint. One bounded transport frame and current
  decode/copy buffers, plus two preallocated host screen snapshots, are
  additional bounded memory.
- A context `fsync`/`F_FULLFSYNC` has no cancellation deadline. UI responsiveness
  is preserved, while worker shutdown may complete later.
- Detach cleanup is best effort if the worker remains in an uninterruptible
  synchronous operation after the bounded post-restore wait.
