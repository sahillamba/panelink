# Pathname CLI watch and reconciliation acceptance — 2026-10-18

This extends the explicitly ignored actual-binary pathname fixture in
`tests/p3_pathname.rs`. The existing pathname/context/claim/submit/reconnect
scenario remains intact. After the readiness read and successful claim, the
fixture now calls the actual `panelink control reconcile TARGET` command,
checks its target, cursor, screen revision, owner revision/lease and child
status, and compares the caller-context bytes immediately before and after the
query. It performs another reconciliation immediately before the existing
single token submit and uses that returned cursor as the watch starting point.

After the existing bounded Read observes the single GOT marker, the fixture
calls `panelink control watch TARGET GEN:SEQ 64 1000`. It requires a successful
`changed` response with at least one event and a later cursor, then compares
the caller-context bytes immediately before and after watch. A final
reconciliation captures the current cursor after the controlled shell has
stopped printing. A zero-wait watch from that cursor must return an empty
`timed_out` result with exit status 1. The assertion treats that documented
timeout status as the command result, not as a subprocess-helper failure. The
small existing top-level JSON field reader is reused; the fixture adds only a
bounded `generation:sequence` parser and does not add a general JSON parser.

The test still uses the actual binary, explicit environment and working
directory, private fixture paths, bounded output captures and process waits.
The existing child PID, one GOT marker, detach, same-child Read, singleton
refusal and endpoint-cleanup assertions remain in place. No input is retried,
no replacement context is created, and the watch/reconcile queries do not
reserve a caller sequence. The prior corrected pathname scenario is recorded
as operator-confirmed in `P3-PATHNAME.md`; that confirmation predates these
watch/reconciliation assertions and does not cover them.

## Local verification

Using Rust/Cargo 1.99.1, aarch64-apple-darwin, the existing offline Cargo cache
and `/private/tmp/panelink-p3-pathname-target`:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname --no-run
exit 0; integration-test executable built

CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname
exit 0; 1 passed, 0 failed, 1 ignored

CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo clippy --locked --offline --test p3_pathname -- -D warnings
exit 0; Finished dev profile

rustfmt --edition 2021 --check tests/p3_pathname.rs
exit 0

awk 'length($0) && $0 ~ /[[:blank:]]$/ { print FILENAME ":" FNR ": trailing whitespace"; bad=1 } END { exit bad }' tests/p3_pathname.rs docs/progress/P3-WATCH-PATHNAME.md
exit 0; no trailing whitespace
```

The ignored fixture was explicitly attempted once in this restricted
environment:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p3-pathname-target cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --ignored --exact --nocapture
exit 101; 0 passed, 1 failed
```

The actual binary returned `listener_unavailable`; the fixture's independent
pathname bind returned `EPERM` (1, Operation not permitted). This is an actual
failed attempt, not a skip or pass. It stopped before control initialization,
so none of the new pathname reconcile/watch assertions ran here. The zero-wait
`timed_out` assertion is therefore compiled but not runtime-confirmed. Do not
retry in this same restricted environment. Rerun the command above on a host
that permits pathname Unix socket bind/accept, then record that new result
separately. This note makes no terminal-host, provider, or performance claim
and does not complete Phase 2 or Phase 3.
