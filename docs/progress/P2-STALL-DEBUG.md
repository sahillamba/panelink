# P2 host output-stall diagnosis

The `run_loop` output-stall path reproduces correctly on an unread outer PTY without `/dev/tty`. I added a private binary unit test that constructs an `openpty` pair, keeps its master unread, starts a real `TerminalSession` running a slow continuous-newline child, waits until the child's HUP-ignore trap is installed, enters raw and visual modes on the host slave, then invokes the production `run_loop` with its decoder, fixed input chunk, and fixed frame.

The loop returns `OutputStalled` after 1001 ms. The test also confirms the host slave remains `O_NONBLOCK` after raw-mode setup. Its bounded cleanup reports the expected `HostRestore` because the visual reset cannot fit in the unread output queue, but restores every saved termios field and collects the child's exit status. The whole focused test takes 2.74 seconds, including cleanup. No production semantics changed.

## Source review

`Frame::write_once` keeps a cursor across partial writes. A positive write advances the cursor and resets the no-progress timestamp; `WouldBlock` or `Interrupted` reports `OutputStalled` after one second without a successful write. `run_loop` makes at most one such write attempt per turn and sleeps for one millisecond. It does not clear the pending-frame timer while the frame is blocked. This behavior matches the direct PTY reproduction.

The separate Rustix PTY diagnostic measured an unread slave output capacity of 1,024 bytes: one nonblocking write accepted exactly that prefix and the next attempt returned `WouldBlock` immediately. Its process used `setsid` and `TIOCSCTTY`. While that session leader was alive, `tcgetattr` and `tcgetsid` succeeded. After it exited, the retained slave still supported `fcntl`, while `tcgetattr` failed with Darwin errno 25 (`ENOTTY`) and `tcgetsid` failed with errno 2 (`ENOENT`). This confirms the test supervisor must remain the controlling-session leader until termios assertions finish; retaining only a slave descriptor is insufficient.

The previous subprocess no-drain scenario reported `probe exceeded no-drain output-stall deadline` after its five-second test deadline. That timeout remains real and unresolved: the direct test removes the subprocess supervisor and separate `/dev/tty` open, so it establishes the bounded loop and cleanup behavior but does not localize the process-level difference. The current evidence does not identify a production defect or justify a production change. Re-run the no-drain subprocess scenario with the supervised PTY fixture in an environment that permits the probe's independent `/dev/tty` open before changing production output behavior.

## Verification

Focused command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --bin panelink p2_probe::tests::unread_outer_pty_returns_output_stalled_before_bounded_cleanup -- --nocapture
```

Actual output:

```text
Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.32s
Running unittests src/main.rs (target/debug/deps/panelink-2b20bfb7af1b202e)

running 1 test
unread_outer_pty_loop_result=host terminal output made no progress; elapsed_ms=1001
test p2_probe::tests::unread_outer_pty_returns_output_stalled_before_bounded_cleanup ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 16 filtered out; finished in 2.74s
```

The temporary PTY diagnostic was run with:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-pty-diagnosis/target cargo run --offline --manifest-path /private/tmp/panelink-pty-diagnosis/Cargo.toml
```

Its actual output was:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.01s
Running `/private/tmp/panelink-pty-diagnosis/target/debug/panelink-pty-diagnosis`
output_capacity_bytes=1024 elapsed_ms=0 would_block=true
while_child_live tcgetattr_ok=true tcgetsid_ok=true
child_exited_within_2s=true
after_child_exit tcgetattr=Err(Os { code: 25, kind: Uncategorized, message: "Inappropriate ioctl for device" }) tcgetsid=Err(Os { code: 2, kind: NotFound, message: "No such file or directory" })
retained_slave_fcntl=Ok(OFlags(RDWR)) master_held=true
```

Final `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` exited 0 with no output. Final `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings` output:

```text
Checking panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s
```

The first format and Clippy attempts ran while `tests/p2_host.rs` was being edited: formatting was incomplete and Clippy saw a transient reference to the removed `ProbeHarness.child` field. The host file was left untouched; both checks passed after its owner finished the concurrent changes. The codebase-memory `search_graph` call also failed with `Transport closed`; source review used exact file reads instead.

This test does not exercise the separate `/dev/tty` open, subprocess supervisor, host diagnostic routing, or Apple Terminal. The operator-only outer-PTY scenarios remain necessary for those behaviors.

## Parent source observation

Apple's published [controlling-TTY driver](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty_tty.c) describes retaining the underlying vnode until the controlling session leader exits and the vnode is revoked. The [PTY close path](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty_dev.c) calls the line discipline's close; [ttylclose](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty.c) may wait for queued output when its close flags lack FNONBLOCK. This suggests session teardown as a possible explanation for the old whole-process no-drain timeout despite the passing one-second loop check. It is an inference from published source, not a verified stack trace from the operator's process or a claim about the exact installed kernel revision. The supervisor keeps that session alive until the process-exit assertions complete; only subsequent fixture teardown may drain the private PTY.

## Follow-up bounded exit reproduction

A disposable safe-Rustix probe compared three isolated subprocess cases with exactly 256 output bytes and the master unread. Parent polled for 500 ms, drained the master, then polled to a two-second overall deadline:

```text
case=nonleader_no_devtty initial_exit_before_drain=true initial_exit_ms=Some(3) post_drain_exit=true exit_code=Some(0) elapsed_ms=3 drained_bytes=256
case=leader_no_devtty initial_exit_before_drain=false initial_exit_ms=None post_drain_exit=true exit_code=Some(0) elapsed_ms=502 drained_bytes=256
case=leader_readonly_nonblock_devtty initial_exit_before_drain=false initial_exit_ms=None post_drain_exit=true exit_code=Some(0) elapsed_ms=501 drained_bytes=256
```

The third case successfully opened `/dev/tty` read-only/nonblocking/CLOEXEC; it never attempted to write through that alias. Writes used only the subprocess's existing private PTY stdout. Both session-leader cases delayed exit until the master was drained, while the nonleader exited promptly. Thus controlling-session teardown reproduces a whole-process delay without depending on a `/dev/tty` alias open. This supports keeping the probe a nonleader under the test supervisor. It does not identify an exact installed-kernel stack or prove the operator's original timeout has passed after correction; that complete scenario still needs its external rerun. The production application's one-second output-stall limit is unchanged.
