# Wave 2 shared driver shutdown test — 2026-10-22

Status: isolated real-child regression passed on Apple Silicon macOS. This is
one bounded shutdown check; it is not a general shutdown acceptance claim.

The new `tests/w2_server_shutdown.rs` launches two real PTY-backed shell
children through `MultiSessionOwner`, assembles a direct `MultiServerDriver`,
and drops the driver without a pathname listener, `/dev/tty`, or history store.
Both children publish ready and PID markers after installing HUP traps. On
first HUP, each child publishes its marker and waits up to about 400 ms for the
other's marker; it records a peer-missing marker if that HUP does not arrive
within the interval. Both traps then leave their children alive for the shared
escalation. After driver drop, the test requires both HUP markers, no
peer-missing marker, both child PIDs absent from the process table, and broad
cleanup duration between 800 ms and 3 seconds.

Actual command and result:

```text
rustfmt --edition 2021 tests/w2_server_shutdown.rs
exit 0; no output

cargo test --locked --offline --test w2_server_shutdown -- --test-threads=1
running 1 test
test driver_drop_signals_both_children_before_shared_escalation_deadlines ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
finished in 1.10s
```

Two initial runs exposed fixture-construction mistakes, not product failures:
the test had not created the private server directory before
`ServerStorage::initialize`, then its shell `$1` was set to a label instead of
the marker directory. Both were corrected before the passing run. The source
review used the ready `Users-sahillamba-panelink` graph to locate
`MultiServerDriver::drop` and the existing PTY test paths, then inspected source
for the authoritative launch and shutdown behavior.

Parent review strengthened the worker's asymmetric fixture: both children now
check the peer marker, so serialized shutdown fails regardless of iteration
order. Process absence now requires a direct signal-zero syscall to return
exactly ESRCH; an arbitrary subprocess failure is insufficient. The strengthened
parent run passed in1.09s (wave2-evidence/shutdown-parent.log), and the selected
integration run passed as well.

This test proves both HUP handlers ran before either peer-marker deadline,
then checks both direct child processes are absent after shutdown. It does not
exercise an unknown/failed signal outcome, active close-transaction shutdown,
history-drain teardown, server worker/socket shutdown, or every platform.
Ownership is released after this note; production files and unrelated tests were
not changed.
