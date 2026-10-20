# Clone, build and test Panelink

This guide starts from a fresh Apple Silicon Mac. Panelink is an experimental
two-pane terminal multiplexer with right-click splits, explicit pane close,
and bounded saved history. A successful build does not certify the remaining
host, OpenCode identity or full reboot/layout recovery gates.

## Requirements

- macOS on Apple Silicon (arm64) is the tested development target.
- A working command line toolchain: `git`, `xcrun`, and `/usr/bin/tic`. If
  `xcrun` is missing, install Apple's Command Line Tools with
  `xcode-select --install` and finish the installer before continuing.
- Rust stable with Cargo, rustfmt and Clippy. The repository's
  `rust-toolchain.toml` selects stable and requests the two Rust components.
  Install Rust with the [official instructions](https://rust-lang.org/tools/install/)
  or the machine's approved package manager. With rustup, you can explicitly
  install the requested components:

  ```sh
  rustup toolchain install stable --profile minimal --component rustfmt --component clippy
  rustup default stable
  ```

The last verified development machine used Rust/Cargo 1.99.1. The repository
tracks stable rather than pinning that exact version; a newer stable toolchain
may be selected on the new machine. Do not install the community `ms-rust`
skill to build the application; it is developer guidance, not a build
dependency.

## Clone and compile

```sh
git clone <repository-url> panelink
cd panelink
rustc --version
cargo --version
cargo build --locked
```

Replace `<repository-url>` with the repository URL you can access. The `--locked`
flag requires Cargo to use the checked-in lockfile without changing dependency
resolution. The first build may download registry dependencies; allow network
access for Cargo or populate its cache first. No application-specific
environment variables are required on a normal developer machine.

To compile and run the current automated checks:

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/verify_non_ui.py
```

For focused saved-history and pane-lifecycle regression checks:

```sh
cargo test --locked --offline --test p3b_close_runtime -- --test-threads=1
cargo test --locked --offline --test p5_history_store
cargo test --locked --offline --test p5_history_rows
cargo test --locked --offline --test p5_scrollback_cursor
```

The outer-PTY close/history fixture is run serially because parallel startup
has previously timed out before its assertions. The fixed verifier passed
8/8 at commit `f82d968`; 15 Rust tests were ignored, not passed. See
[the saved-history evidence](progress/P5-SAVED-HISTORY.md) for the executed
checks and remaining recovery cases.

The final command repeats a bounded offline suite that includes the vendored
decoder, component fingerprints, Rust tests, lint, build and documentation
checks. It needs Python 3 and a populated Cargo cache. Some tests are
intentionally ignored because they require host terminal or pathname socket
access; the verifier reports them as ignored and does not certify those host
gates. Read [the roadmap](../ROADMAP.md) and
[current integration evidence](progress/INTEGRATION.md) before continuing
implementation.

If the first build has already fetched dependencies, the Rust checks can run
offline:

```sh
cargo test --locked --offline
cargo fmt --check
cargo clippy --locked --offline --all-targets -- -D warnings
```

## Environment variables

No environment variables need to be set for a normal clone/build. In
particular, do not set `TERMINFO` globally: the probe and launcher pass their
private terminfo path explicitly to the child.

Cargo honors its usual `CARGO_HOME` and `CARGO_TARGET_DIR` variables. Set them
only if you want a custom writable dependency cache or build-output location.
For example:

```sh
export CARGO_HOME="$HOME/.cargo"
export CARGO_TARGET_DIR="$PWD/target"
```

These are the defaults on a typical rustup setup, so the exports above are
usually unnecessary. In the restricted development environment used for some
Panelink checks, a temporary writable `CARGO_HOME` was needed; that path was
specific to that machine and should not be copied to a new system. `TMPDIR` is
optional and controls where temporary terminfo/runtime directories are made.

Do not put provider keys or other credentials into shell startup files for
Panelink. The current build and tests do not need provider credentials.

## Run Panelink

After building, link the launcher once and open your workspace:

```sh
mkdir -p "$HOME/.local/bin"
ln -sfn "$PWD/bin/panelink" "$HOME/.local/bin/panelink"
panelink
```

Make sure `$HOME/.local/bin` is on your `PATH`. The launcher finds its
checkout from its own location, so recreate the link if you move the
repository. `panelink` compiles its terminfo entry once into
`~/Library/Application Support/Panelink`. It starts the background workspace
server if needed and attaches. Ctrl-X detaches, and `panelink stop` ends the
workspace. `panelink probe` runs the older disposable single-shell probe.
These need access to the host terminal device, which some restricted
environments deny.

If this repository has newer local commits than its remote, clone access alone
will not include them until the repository owner explicitly pushes them. Check
`git log -1 --oneline` and `git status -sb` against the handoff before using
the commit IDs below as evidence on a different Mac.

In the attached client, right-click a pane and select **Close pane**, or press
Control-B then `x` and confirm with `y`. From a shell, `panelink panes` lists
the live panes and `panelink close N` stops pane N. `panelink history list`
prints saved five-number target IDs; `history show TARGET` and `history search
TARGET TEXT` inspect sanitized retained rows and the latest separate screen
checkpoints. After `panelink stop`, `history clear TARGET` deletes that exact
target's Panelink history. Saved content never resumes a live process or an
OpenCode conversation. Full usage is in [README.md](../README.md).

## Continue implementation

The project is paused. On an explicit resume request, start with
[ROADMAP.md](../ROADMAP.md), [SESSION.md](progress/SESSION.md), and the
current Git status/history. The scoped Phase 3B stop slice and P5 saved-history
slice are done at checkpoint commits `c3e207e` and `f82d968`; their parent
Phase 3 and Phase 5 milestones are still open. The next history gates are
the full V-HISTORY reboot/layout and partial-tail acceptance, a deterministic
post-rename/failed-end-sync test, and disk-throughput review. Do the next
implementation directly unless the user explicitly requests an agent. The
user subsequently requested publication on 2026-10-21; check the remote
tracking state rather than assuming a different clone has these commits.
