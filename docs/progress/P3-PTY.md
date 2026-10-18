# Private PTY effect adapter — 2026-10-18

Status: the bounded `PtyEffects` adapter and focused private-PTY lifetime tests
are implemented and pass on the tested Apple Silicon macOS host. This is a
same-process prototype proof of server-owned PTY state across logical
detach/resume. It does not establish production server lifetime, socket
compatibility, Apple Terminal behavior, Phase 2 completion, or M1 acceptance.

## Scope delivered

[`src/server_pty.rs`](../../src/server_pty.rs) implements `PtyEffects::new(TerminalSession)` as the synchronous `server::EffectSink`. The adapter owns the only
`TerminalSession`; it has no client, socket, host terminal, queue worker, or
separate text copy. Its screen revision starts at 1, advances with checked
arithmetic when a pump reads output or observes a lifecycle change, and fails
closed if progress fails or the revision is exhausted.

Screen reads use only the engine-owned sanitized visible cells. Live scrollback
is rejected as unsupported. The adapter bounds display text to 65,446 bytes
(65,536 minus the response codec's 90-byte metadata), respects the requested
1–1,024 line limit, uses spaces for blank cells, skips wide-character
continuations, and puts LF only between rows. It clips only at UTF-8 scalar
boundaries and reports omitted cells/rows through `truncated`. Any control
scalar encountered in cell text is replaced with U+FFFD; raw PTY output and
clipboard payloads are never returned.

Input validation rejects direct calls over 32 KiB, unsupported commands,
non-live sessions, and a nonempty pending queue before admission. Text plus
optional submit is queued atomically by the existing core; paste uses its
bounded core path; interrupt calls the core's child-group SIGINT operation
once. Text and paste drain through nonblocking pump calls until the 100 ms
absolute deadline, counting only the PTY write bytes against the request length.
The adapter clears any remaining queue bytes before returning, so no pending
suffix can pass to a later owner. A complete known write or OS-accepted signal
returns `Applied`; known partial input returns `Partial`; no known delivered
input returns `Failed`; progress/revision uncertainty returns `Unknown`. These
outcomes say nothing about application-level task success.

The real-PTY tests in [`tests/p3_detach.rs`](../../tests/p3_detach.rs) cover:

- A ready shell marker, a typed Submit, its application-observed `GOT` marker,
  logical Detach, same-generation/same-lease resume, and a post-detach marker
  with the same shell PID.
- A dropped Submit acknowledgement with a durable uncertain `ContextFile`, a
  status query after resume, and `ClientContext` reconciliation without a
  client resend. A fault-injected repeated wire payload returns the saved
  status and does not produce a second application marker.
- Stale target and stale owner rejection, plus a fresh read after resume whose
  revision advances and whose text includes output produced while detached.
- A ready-marker SIGINT fixture that observes the shell's trap; the operation
  is `Applied` on accepted signal delivery, and child exit is checked on a
  later bounded poll.
- Sanitized Unicode/clipboard output, a UTF-8-boundary read budget, line
  bounds, direct 32 KiB input validation, and LiveScrollback rejection.

These tests keep the PTY inside one `StateOwner` in the test process. They do
not prove survival after that server process exits. Parent-owned socketpair
drop/reconnect tests remain a separate transport boundary; pathname socket
and host-terminal checks remain pending.

## Graph and source inspection

The code graph was queried before direct source inspection. Its project status
was `ready`, with 2,109 nodes and 6,763 edges. Search located
`TerminalSession::pump`, `snapshot`, `shutdown`, `pending_input_bytes`,
`discard_pending_input`, and the existing `InputOwner` state. The exact
`EffectSink`/`ReadResult` API was confirmed with the owner of `src/server.rs`;
the implementation uses those signatures without editing that file. No graph
refresh was run during parallel edits.

The P2 restore-order evidence remains limited to private PTYs and a synthetic
host slave; it makes no Apple Terminal claim. See
[`P2-RESTORE-ORDER.md`](P2-RESTORE-ORDER.md) and
[`P2-CORE-RESULT.md`](P2-CORE-RESULT.md) for the core lifecycle and cleanup
limits used here.

## Actual verification

The host reported Rust/Cargo 1.99.1 and Darwin 25.5.0 arm64. No dependency,
manifest, or lockfile was changed for this task.

The initial temporary harness imported the root modules by path from
`/private/tmp/panelink-pty-harness` and ran:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-pty-harness/Cargo.toml --test p3_detach -- --nocapture
```

Actual first result: 2 passed and 1 failed. The SIGINT trap marker was visible;
the test checked `child_exited()` before a subsequent pump had collected the
exit status. The test was corrected to poll for exit with a three-second
deadline. The first strict Clippy attempt also found a collapsible conditional
in `server_pty.rs` and the temporary crate omitted `protocol_wire`; both were
corrected before the final run.

Final repository integration command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_detach -- --nocapture
```

Actual result: exit 0, 3 passed, 0 failed, 0 ignored, finished in 0.32 s:

```text
test pty_reads_are_sanitized_utf8_bounded_and_scrollback_is_rejected ... ok
test interrupt_delivers_sigint_after_a_ready_marker ... ok
test logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
```

The temporary module-import harness then passed:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --offline --manifest-path /private/tmp/panelink-pty-harness/Cargo.toml --all-targets -- -D warnings
```

Actual result: exit 0. These file-scoped checks also exited 0:

```text
rustfmt --check --edition 2021 src/server_pty.rs tests/p3_detach.rs
awk '/[[:blank:]]+$/ { print FILENAME ":" FNR ": trailing whitespace"; bad=1 } END { exit bad }' src/server_pty.rs tests/p3_detach.rs docs/progress/P3-PTY.md
git diff --check
```

All produced no output. The three owned files were untracked during this run,
so `git diff --check` covered only the tracked working-tree diff; the explicit
`awk` scan checked trailing whitespace in the new owned files.

## Remaining limits and handoff

The 100 ms drain deadline bounds the ordinary write loop. It is not a hard
whole-call bound: a core pump error can enter the existing HUP/KILL cleanup
path, with up to two additional seconds of nominal cleanup waits. Pending
terminal-query replies behind a discarded input suffix are also discarded;
this remains a prototype compatibility limitation. A successful input outcome means the PTY accepted the
request bytes, not that a child application handled them; the fixture separately
observes its `GOT` marker. A signal result means the OS accepted SIGINT, not
that the child exited; the test observes the trap and exit separately. This
adapter itself adds no live scrollback accessor, durable generation/journal, launch API,
layout/event behavior, reconnect after server death, pathname socket support,
or host acceptance evidence. The user-confirmed Ctrl-X follow-up does not
complete the remaining clipboard or Apple Terminal gates.

Files owned for this task are released: `src/server_pty.rs`,
`tests/p3_detach.rs`, and this evidence note. No commit or push was made.
