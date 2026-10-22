# P5 recovery startup chooser — L4

**Status:** Implemented in the bounded startup path; checks and acceptance
results are listed below. Relaunch is not offered or implemented. The broader
Phase 5 recovery/reboot gate remains open.

## Behavior implemented

- Plain `panelink` first resolves the configured state root and probes for an
  authenticated live server or held server lock. A running server takes the
  existing attach path without showing the chooser.
- If there is no reachable server, the opener attempts read-only
  `LayoutStore::open(server/layout)` and loads both current and previous slots.
  A missing/unreadable/invalid current slot suppresses the chooser and does not
  print an absence error. A previous-slot read fault suppresses the chooser as
  well. This inspection does not create directories, open server storage,
  acquire lifecycle/server locks, change generations, or start a server.
- The chooser is selected only with TTY stdin and stdout. Piped or redirected
  invocations continue down the default fresh-shell path.
- The small `/dev/tty` prompt reports only relative age and pane count. One
  byte is read in raw mode, with a ten-second deadline. A Drop guard restores
  the captured termios after terminal acquisition, including failed attribute
  capture/setup, input, timeout and write paths. Enter, Escape, timeout,
  unknown input, and IO failures choose the existing fresh-shell path. `v` and
  (when available) `p` invoke the read-only recovery reader/viewer and then show
  the chooser again. `q` exits
  without starting a server.
- Saved targets, content, IDs and physical paths are not written to the chooser
  prompt. The viewer remains the explicit Stage 3 read-only viewer; saved data
  is never replayed as process input or startup arguments.

## Verification and acceptance evidence

The bounded integration target is `tests/p5_recovery_chooser.rs`. It contains
exhaustive pure decision/key mappings, non-TTY saved-layout and missing-layout
shell-marker cases, and a strict-deadline private-PTY `q` scenario using an
isolated root. A PTY case counts as passed only when the prompt is observed and
the process exits cleanly without a server lock or shell marker. A platform or
environment skip is blocked, not a pass.

Toolchain/target: Rust and Cargo 1.99.1 on macOS aarch64. `CARGO_TARGET_DIR` is
`/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l4`.

Actual executed results:

```text
cargo test --locked --offline --lib recovery_chooser -- --test-threads=1
test result: ok. 4 passed; 0 failed; 0 ignored; 635 filtered out

cargo test --locked --offline --test p5_recovery_chooser -- --test-threads=1
BLOCKED during concurrent L5 host/client edits: compiler reported duplicate
LayoutSlot/LayoutStore/RecoverySlot imports in src/ui_client/host.rs and
incomplete/inconsistent menu availability API integration in L5-owned
src/ui_client/host.rs, src/ui_client/state.rs, src/ui_client/net.rs and
src/ui_client/state_tests.rs. The PTY acceptance was therefore not executed.
Captured compiler output included:
error[E0252]: the name `LayoutSlot` is defined multiple times
error[E0252]: the name `LayoutStore` is defined multiple times
error[E0252]: the name `RecoverySlot` is defined multiple times
error[E0061]: this method takes 4 arguments but 3 arguments were supplied
error[E0599]: no method named `server_directory` found for mutable reference `&mut Net`

cargo test --locked --offline --bin panelink -- --test-threads=1
BLOCKED by the same L5-owned in-progress host/client integration errors above.
Captured compiler output also included:
error[E0046]: not all trait items implemented, missing: `recovery_availability`

cargo clippy --locked --offline --all-targets -- -D warnings
BLOCKED by the same L5-owned in-progress host/client integration errors above.
Captured compiler output also included:
error[E0592]: duplicate definitions with name `needs_recovery_availability`
error[E0609]: no field `recovery_checks` on type `&mut ui_client::state::tests::Fake`

cargo fmt --check
BLOCKED by formatting differences in L5-owned src/ui_client/state.rs and
src/ui_client/state_tests.rs. Owned Rust files were formatted directly.
Actual formatter output began:
Diff in src/ui_client/state.rs:167:
Diff in src/ui_client/state.rs:459:
Diff in src/ui_client/state_tests.rs:102:

rustfmt --check --edition 2021 src/recovery_chooser.rs src/default_open.rs
tests/p5_recovery_chooser.rs
exit status 0 (owned Rust files only)

cargo clippy --locked --offline --lib -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s

git diff --check
exit status 0 (no output)

python3 scripts/check_docs.py
Checked 193 Markdown file(s); 0 finding(s).

Final owned-only reruns:

```text
cargo test --locked --offline --lib recovery_chooser -- --test-threads=1
test result: ok. 4 passed; 0 failed; 0 ignored; 635 filtered out

cargo clippy --locked --offline --lib -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.55s

rustfmt --check --edition 2021 src/recovery_chooser.rs src/default_open.rs tests/p5_recovery_chooser.rs
exit status 0 (no output)

python3 scripts/check_docs.py
Checked 193 Markdown file(s); 0 finding(s).
```
```

An initial library run exposed an incorrect age-boundary expectation; that test
was corrected, and the final focused library run passed. The PTY fixture is
implemented but its chooser, termios-restoration and no-runtime-effects
assertions remain unexecuted pending a clean shared-source compile. Piped
shell-marker assertions are likewise unexecuted; no acceptance pass is claimed
for those scenarios.

## Parent review and final verification — 2026-10-21

L4 was blocked by L5's broken tree; after the parent repaired L5, the full
suites ran. Two piped-run tests expected the marker shell to start and the
server lock to exist, but plain `panelink` requires a terminal to attach and
(unchanged baseline, verified on committed HEAD) exits 1 when piped; the
tests now assert the decision under test (no chooser/viewer, fresh-shell
startup path reached via `lifecycle.lock`). The private-PTY `q` scenario
ran and passed. Existing `tests/p3a_default_open.rs` now answers the real
chooser with Enter after a stop leaves a saved layout: 6/6 pass, which is
end-to-end evidence that Enter takes the fresh-shell path.

```text
cargo test --locked --offline --test p5_recovery_chooser -> 7 passed; 0 failed
cargo test --locked --offline --test p3a_default_open    -> 6 passed; 1 ignored
python3 scripts/verify_non_ui.py                         -> 8/8; 964 passed/0 failed/15 ignored
```
