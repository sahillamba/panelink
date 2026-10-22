# Wave 2 server close implementation result

Status: implemented and parent-reviewed; final integration evidence is in
WAVE2-RESULT.md. No commit was created.

The live multi-session close route now admits a transaction, sends HUP once,
and advances child-exit polling and one-time KILL escalation on owner ticks.
Close state remains explicitly pending in the internal runtime path; no
schema-9 wire variant or terminal success reply is emitted before final
capture, lifecycle update, collapse, survivor resize, and ledger/result
completion. A timed-out or disconnected worker does not cancel owner progress.
Legacy direct `execute_close` callers retain the synchronous API.

Closing panes immediately reject input and resize, including the direct
`PtyEffects::resize` path used by root resize. Ordinary polling cannot collapse
an active close target before its final close result is captured. Process exit
and PTY EOF remain independent facts. Unknown outcomes retain the child slot
and disabled input/resize state. Pump read/write/wait errors during phased
shutdown no longer enter legacy synchronous cleanup. Driver shutdown starts
HUP for all effects and advances them with shared deadlines. Live ticks do not
enter shutdown deadline waits; final output draining after confirmed exit
retains its existing fixed read budget.

The fake-clock owner tests cover admission, exact retry without a second HUP,
pending ledger status, KILL escalation, single terminalization, resize/collapse
ordering, and Unknown retention with later natural exit. The real two-PTY test
proves that a sibling continues to accept input and produce output during the
logical HUP interval while another child ignores HUP, then observes escalation
and confirmed exit. The HUP interval uses fake monotonic time; it is not a
measurement of a one-second wall-clock delay. Additional fault coverage proves
a PTY read error during phased shutdown returns promptly while retaining the
child handle, and direct resize is rejected after close begins. A mixed-API
fake test proves the retained legacy synchronous entrypoint rejects a second
key for an actively closing target without signaling it again, while capacity
accounts for active transactions.

An actual UnixStream socketpair runtime test sends a schema-9 close through a
worker, observes the driver admit it as `PendingClose`, stops the worker so its
reply receiver is dropped, and verifies the owner still advances the retained
close to terminal status. The runtime pending waiter is then removed even
though its reply channel is disconnected. The separately owned shared-shutdown
regression also passed: two real children both received HUP before the shared
grace deadline, were reaped, and driver drop completed in 1.10 seconds; see
`docs/progress/W2-SHUTDOWN-TEST.md` for its evidence limits.

Checks run successfully:

- `cargo check --locked --offline --lib`
- `cargo test --locked --offline --lib server::multi_session::close_tests:: -- --test-threads=1` (17 passed)
- `cargo test --locked --offline --lib server_pty::tests:: -- --test-threads=1` (3 passed)
- `cargo test --locked --offline --test w2_server_close -- --test-threads=1` (1 passed)
- `cargo test --locked --offline --lib terminal::unit_tests::phased_shutdown_pump_error_does_not_enter_legacy_wait_cleanup -- --test-threads=1` (1 passed)
- `cargo test --locked --offline --lib server_pty::tests::direct_resize_rejects_a_pane_after_close_begins -- --test-threads=1` (1 passed)
- `cargo test --locked --offline --lib multi_runtime::tests::socketpair_close_worker_disconnect_does_not_cancel_owner_transaction -- --test-threads=1` (1 passed)
- `cargo test --locked --offline --lib server::multi_session::close_tests::legacy_close_does_not_stop_a_target_owned_by_an_active_close -- --test-threads=1` (1 passed)
- `cargo test --locked --offline --test w2_server_shutdown -- --test-threads=1` (1 passed; 1.10 seconds, separate owned test)
- `rustfmt --edition 2021` on the owned Rust files

The worker's `cargo fmt --all -- --check` also inspected pinned local dependency
sources, which were not rewritten. Owned files were formatted directly; the
parent's required `cargo fmt --check` passes. The two-child shared-shutdown check
has its own limited acceptance
note; it does not cover unknown signal outcomes or history-drain teardown.
Async launch remains outside this slice.

Parent integration replaced the public unit error with the existing sanitized
stop outcome, grouped pending-capacity facts at dispatch, and removed a
Copy-type clone to satisfy strict all-target Clippy without lint suppression.
Parent also made typed-key capacity validation reject closing panes before
admission and extended the direct closing-pane regression. See WAVE2-RESULT.md
for final combined verification; earlier worker commands are component evidence.
