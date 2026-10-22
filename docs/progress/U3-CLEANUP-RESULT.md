# U3 bounded cleanup drain — 2026-10-22

Implemented the standalone `src/terminal/cleanup_discard.rs` helper. It uses
one stack buffer of 4,096 bytes and performs at most 16 reads per call. It
returns `true` only on observed EOF; `WouldBlock`, `Interrupted`, other errors,
and an exhausted budget return `false`. It does not allocate, retry errors,
close the reader, signal a child, or update caller EOF state. Parent integration
will replace the existing unbounded discard loop and preserve its EOF handling.

## Verification

The requested direct standalone command was attempted:

```text
rustc --edition=2021 --test src/terminal/cleanup_discard.rs -o /private/tmp/panelink-cleanup-discard-tests
```

It fails because `pub(super)` has no parent module when this source file is the
crate root (`there are too many leading `super` keywords`). Visibility was
kept as assigned. A temporary harness supplied the intended parent module:

```text
cat > /private/tmp/panelink-cleanup-discard-harness.rs <<'EOF'
mod terminal {
    #[path = "/Users/sahillamba/panelink/src/terminal/cleanup_discard.rs"]
    mod cleanup_discard;
}
EOF
rustc --edition=2021 --test /private/tmp/panelink-cleanup-discard-harness.rs -o /private/tmp/panelink-cleanup-discard-tests
/private/tmp/panelink-cleanup-discard-tests
```

Actual result: 4 tests passed, 0 failed, with no compiler warnings. The
production `terminal/mod.rs` will use the internal helper after parent
integration. The initial fragmented-reader fixture
contained 9,017 bytes at 137 bytes per read, correctly exhausting the 16-read
limit before EOF. It was reduced to 901 bytes so that the test covers
fragmentation and EOF within one call. The rerun passed all four tests with no
compiler warnings. `rustfmt --edition 2021 --check
src/terminal/cleanup_discard.rs` also passed. No Cargo dependency or source
change outside the two assigned files was made.

## Remaining responsiveness design

The helper addresses only the cleanup drain finding. The other synchronous
server and client boundaries need separate changes.

For pane close, keep the owner queue as the sole state authority and represent
close as a transaction advanced on each runtime tick: record the close intent,
send HUP, poll exit while continuing normal work for sibling panes, send KILL
after the existing HUP deadline, then collect status and publish one terminal
result. Launch preparation and durable journal work should run as bounded,
owned jobs returning typed results to the owner. Preserve durable intent before
spawn, deduplication, and a single owner for every acquired child. Once spawn
may have occurred, cancellation must not drop the job or child; a lost result
remains an unknown outcome until reconciliation.

Acceptance should use two panes with one child ignoring HUP and another under
continuous output/input. While close progresses, repeatedly query and exercise
the sibling; verify KILL escalation, one close result, no duplicate launch on a
retry, and bounded owner-queue growth. Inject delayed journal operations and
launch failures before and after spawn to verify intent ordering and unknown
outcomes. Filesystem sync latency has no enforceable wall-clock deadline, so
the owner must not wait on it.

For the interactive client, move socket exchanges and durable context writes to
a dedicated bounded IO owner, or use incremental nonblocking exchanges. Keep
the host event loop responsible for rendering, local navigation, detach-key
recognition, and terminal restoration while requests are pending. Input still
requires its context reservation to finish durably before transmission, and an
uncertain input acknowledgment must never be replayed. Bound queued requests
and replies, coalesce replaceable reads/resizes, and keep ordered mutations and
input reservations explicit.

Acceptance should inject delayed replies for one pane while the other remains
responsive, and independently delay context reservation and acknowledgment
flushes. During each delay, verify rendering, navigation, detach, resize
observation, and terminal restoration remain live. Verify no input is sent
before durable reservation, no uncertain input is resent, ordering is retained,
and bounded queues report overload instead of growing without limit.

Risks include more complex transaction recovery, owner/job shutdown races,
filesystem operations that can outlive a caller deadline, and queue saturation.
These designs need focused review against the existing operation deduplication,
child ownership, transport deadline, and client-context contracts before
implementation. They are proposals and acceptance scenarios, not implemented
behavior or measured latency claims.
