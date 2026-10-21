# P5 recovery view CLI and TUI — Stage 3

**Status:** Implemented and focused automated checks executed 2026-10-21. The
scope is the explicit `panelink recovery view [--previous]` entry point only;
the startup chooser and live in-app menu item are later parent work. Relaunch is
not implemented. Phase 5 remains open.

## Delivered

- `panelink recovery view` and `panelink recovery view --previous` are
  dispatched before any default/live startup path. Extra, duplicate and
  unsupported `recovery` arguments use exit code 2.
- The CLI resolves the state root with the same root resolver as `layout show`
  and asks the Stage 2 reader for only the selected slot. Missing, invalid and
  storage errors are fixed sanitized stderr labels, returned before TTY setup.
- With stdout piped, the command prints a text summary of slot, generation,
  sequence, saved root size/orientation, exact pane targets and independent row,
  normal-checkpoint and alternate-checkpoint availability; it creates no
  runtime paths and runs no shell.
- On a TTY, an isolated local-only key handler draws with the existing attach
  client's shared `HostGuard` and `Compositor`. The viewer re-plans on resize,
  uses decoded sanitized cells only, labels mismatched or missing checkpoints,
  and uses the newest retained normal rows when that pane has no normal
  checkpoint. Tab changes the
  selected pane, `n` switches its normal/alternate view, and `q`, Escape or
  Ctrl-C exits. The host cursor is never focused into saved data.

## Actual verification

Toolchain/target: Rust/Cargo 1.99.1, macOS aarch64. Worker build directory:
`/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l3`.

`CARGO_TARGET_DIR=.../target-l3 cargo test --locked --offline --bin panelink -- --test-threads=1`
completed with **98 passed, 0 failed, 0 ignored**. This includes 3 new pure
viewer unit tests covering local key handling and bounded Regular/Overview/
TooSmall composition.

`CARGO_TARGET_DIR=.../target-l3 cargo test --locked --offline --test p5_recovery_cli -- --test-threads=1`
completed with **5 passed, 0 failed, 0 ignored**. Fixtures used private
`/private/tmp` roots and synthetic history only. They cover missing/invalid
slots, current/previous summaries for one/two panes and empty tombstones,
per-source history availability, invalid rows, invalid-current/valid-previous,
usage errors, whole-tree non-mutation, absence of `server.lock`, and no marker
shell execution.

The CLI target uses piped stdout, so this does **not** exercise `/dev/tty`,
actual termios restoration, panic restoration on a real terminal, resize delivery
from a host, or Apple Terminal compatibility. Those remain unverified. The
startup chooser and live in-app menu item are explicitly outside this stage.

Strict all-target Clippy completed with no diagnostics, `cargo fmt --check`
completed with exit status 0, and `python3 scripts/check_docs.py` reported:
`Checked 193 Markdown file(s); 0 finding(s).`

After a concurrent parent change exported the attach `HostGuard` and its
bounded control writer from `ui_client::host`, the recovery TUI was switched to
that shared RAII guard. The focused CLI/bin checks, strict Clippy, format check,
and documentation check above were rerun against this integrated source state.

The final combined-source check outputs were:

```text
cargo test --locked --offline --test p5_recovery_cli -- --test-threads=1
test result: ok. 5 passed; 0 failed; 0 ignored

cargo test --locked --offline --bin panelink -- --test-threads=1
test result: ok. 98 passed; 0 failed; 0 ignored

cargo clippy --locked --offline --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s

cargo fmt --check
exit status 0 (no output)

python3 scripts/check_docs.py
Checked 193 Markdown file(s); 0 finding(s).
```
