# P5 explicit recovery relaunch — bounded implementation evidence

Date: 2026-10-22. Scope: `panelink recovery relaunch [--previous]`, as a
prototype using the existing fresh `panelink serve` path, the current split
protocol, and historical target links. This is not full reboot recovery.

## Behavior implemented

- The command validates the explicitly selected slot before startup. Missing,
  invalid, and empty layouts fail with fixed messages and no workspace start.
- A live server is refused before effects. The command also takes the existing
  per-root lifecycle lock and repeats the liveness check before startup.
- The server is started through the normal terminfo/server path. Its first
  default shell uses the invoking process's default shell and cwd/environment.
  If two panes were saved, one split is attempted with one durable operation key,
  saved orientation, current client cwd/environment and the invoking default
  shell. The split payload is not retried after an uncertain transport outcome.
- New pane identities are queried after the split and compared to the saved
  spatial order. Successful or uncertain verified occupants receive historical
  links; links contain only the frozen numeric identity/outcome fields.
- Relaunch output describes each old-to-new target, reports `applied`,
  `unknown`, or `failed`, prints `attach with: panelink`, and says provider
  conversations are not resumed and must be selected manually.

## Parent review item: link writer singleton limitation

The frozen `LinkStore::record` contract expects the server's singleton token.
This prototype performs the write in the CLI while holding the existing
per-root lifecycle lock and verifies the server lock/endpoint first. The server
process owns the server singleton; the CLI does not. This lifecycle-lock
coordination narrows startup/stop races but does not prove possession of the
server's singleton token or exclude every direct `serve`/concurrent link writer.
Parent review should decide whether this scoped limitation is acceptable before
relying on concurrent link retention. Link persistence failure prints
`panelink: relaunched, but link not saved`; already launched panes remain alive.

## Executed verification

All commands below use `CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l6`.

Focused relaunch integration command:

```text
CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l6 cargo test --locked --offline --test p5_recovery_relaunch -- --test-threads=1
```

Actual output:

```text
running 5 tests
test missing_invalid_and_empty_slots_refuse_before_any_runtime_effect ... ok
test one_pane_relaunch_uses_fresh_context_and_records_a_new_generation_link ... ok
test previous_slot_is_selected_and_running_workspace_is_refused_untouched ... ok
test two_pane_relaunch_preserves_both_saved_orientations_and_links_in_spatial_order ... ok
test usage_errors_exit_two_without_state_mutation ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The five executed cases prove the selected isolated-fixture contracts, including
both split axes and marker-shell/canary checks. The socket-pathname server was
successfully exercised by these runs; no bind-denial skip is counted as a pass.
This does not prove acceptance on Apple Terminal, provider identity/resume, or
physical reboot recovery.

Actual other check results:

`cargo test --locked --offline --bin panelink -- --test-threads=1`:

```text
running 101 tests
test result: ok. 101 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`cargo test --locked --offline --test p3a_default_open --test p5_recovery_chooser -- --test-threads=1`:

```text
running 7 tests
test result: ok. 6 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`cargo clippy --locked --offline --all-targets -- -D warnings` initially
reported two relaunch-test type-complexity lints and one question-mark lint;
those findings were fixed. The rerun passed:

```text
Checking panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.79s
```

`cargo fmt --check` found formatting differences in the new source and test;
`cargo fmt` then completed successfully. The exact `cargo fmt --check` command
must be rerun after these formatting changes.

Final formatting and documentation checks:

```text
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l6 cargo fmt --check
(no output; exit 0)
$ python3 scripts/check_docs.py
Checked 197 Markdown file(s); 0 finding(s).
$ CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l6 cargo clippy --locked --offline --all-targets -- -D warnings
Checking panelink v0.1.0 (/Users/sahillamba/panelink)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.69s
```

The exact executed command outputs are recorded above. `cargo fmt --check`,
strict Clippy and docs check pass on the final formatted files. All five relaunch
tests and both requested existing test groups were also run after code
formatting; their actual counts are recorded above.

## Parent decision and verification — 2026-10-21

Link-writer singleton limitation: ACCEPTED for this prototype, scoped. The
only link writer is the relaunch CLI, which holds the per-root lifecycle lock
(also taken by plain `panelink` open/stop), and the server never writes
links, so there is no concurrent writer in supported flows. A direct manual
`panelink serve` plus a concurrent relaunch against the same root is
unsupported. A server-owned link stream claim (like the layout writer claim)
is the follow-up if links ever get a second writer.

Split launches use the caller's default `$SHELL` (decision 4), not the
`/bin/zsh` hard-coded in the interactive split path; the split is a single
keyed operation with no retry under a new key.

Parent additions: assertion that the links file contains neither the canary
environment value nor the client cwd; updated the earlier recovery CLI test
that expected `recovery relaunch` to be a usage error (now extra/duplicate
relaunch arguments are).

```text
cargo test --locked --offline --test p5_recovery_relaunch -> 5 passed; 0 failed (3 consecutive runs)
cargo test --locked --offline --test p5_recovery_links    -> 13 passed; 0 failed
python3 scripts/verify_non_ui.py                          -> 8/8; 983 passed/0 failed/15 ignored
```

Earlier verifier runs in this slice: one hit the known intermittent
`server_startup` lock assertion; three consistently failed the outdated
`recovery relaunch` usage-error case before the parent fix. Separately, the
committed baseline reproduced an intermittent `history_writer` timeout test
failure 1/10 runs; it is pre-existing and recorded, not hidden.
