# P5 saved-layout driver lifecycle acceptance — bounded task — 2026-10-21

Status: assigned (stage 4 acceptance of [P5-SAVED-LAYOUT.md](P5-SAVED-LAYOUT.md),
"Owner observation and shutdown semantics"). The PARENT already implemented
the integration; this task is acceptance testing only.

Parent integration (read, do not edit): `src/multi_runtime.rs`
(`LayoutObserver`, `MultiServerDriver::enable_layout`, `layout_status`,
`observe_layout` at the end of `tick`, Drop freezes the observer before
`prepare_driver_shutdown` and shares one 250 ms deadline with history),
`src/server/multi_session.rs` (`layout_terminal_empty`), `src/layout_writer.rs`
(`begin_shutdown`/`PendingLayoutShutdown`), `src/p3_cli.rs` serve startup.

## Exclusive ownership

ONLY new `tests/p5_layout_lifecycle.rs` and new
`docs/progress/P5-LAYOUT-LIFECYCLE.md`. Do not edit src/, manifests or other
tests. If you find a production defect, write a failing test, mark it
`#[ignore = "parent defect: ..."]` and describe it precisely in the note; the
parent fixes production code. No commits.

## Harness

Copy the style of `tests/p5_live_history_driver.rs` (StartedMultiServer::start
with `/bin/sh -c SCRIPT`, private /private/tmp roots, `ServerStorage::initialize`,
pump `driver.tick(|| 1)` with deadlines) and `tests/p3a_runtime.rs` for real
splits over socketpairs (`connect`, `handshake`, `split_payload`, close
requests). Enable with `LayoutStore::prepare(&server_dir)` then
`driver.enable_layout(store)`. Read results with a separate
`LayoutStore::open(server/layout)` and `load(LayoutSlot::Current|Previous)`,
and `driver.layout_status().unwrap().status()` for observed/durable keys.
Wait for durability by polling status `last_durable` with a deadline.

## Required scenarios (each a separate #[test])

1. Startup one pane: current = one pane, sequence 1, revision 1, axis Single,
   target == startup target; no previous.
2. Split (real second child via the schema-7 split route): current becomes
   two panes with the correct axis/spatial order, sequence 2, revision > 1.
3. Duplicate/unchanged ticks: many ticks with no topology change never
   advance sequence (status last_observed stays).
4. Root resize (UI schema-8 RootResize) changes rows/columns -> new
   sequence with the new desired geometry.
5. Close one pane of two (schema-9 close) or let one child exit naturally:
   current becomes one pane (survivor), new sequence.
6. Natural exit of the LAST pane (child script exits): current becomes the
   empty tombstone (pane_count 0) with revision == last live revision and a
   greater sequence; afterwards no further sequence is emitted.
7. Driver Drop with live panes (`drop(driver)`): current stays the last
   live topology (NOT empty) even though children are stopped.
8. Restart rotation: run scenario 1 in generation G, drop, reopen storage
   with `ServerStorage::open` (generation G+1), start again and enable:
   previous == exact prior current bytes (load_bytes), current == new
   generation one-pane record.
9. Shared deadline: with history AND layout enabled, `drop(driver)` returns
   within child-stop time + ~250 ms + slack (assert < 3 s total; record
   measured timing in the note, do not claim a budget).
10. Read-only CLI after the run: `panelink layout show` with
    PANELINK_HOME pointing at the root (server dir must be `ROOT/server`)
    prints the saved record and does not change the generation file bytes.

Run and paste ACTUAL output into the note:

```text
cargo test --locked --offline --test p5_layout_lifecycle -- --test-threads=1
cargo clippy --locked --offline --all-targets -- -D warnings
cargo fmt --check
```

Record exact counts, any ignored-defect tests, timing observations and limits
(no machine reboot, no live restore/relaunch, socketpairs not pathname
sockets).
