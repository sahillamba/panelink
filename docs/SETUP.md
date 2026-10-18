# Clone, build and test Panelink

This guide starts from a fresh Apple Silicon Mac. Panelink is an experimental
Rust project; a successful build does not mean the unfinished multiplexer UI,
multi-pane split flow, or every host acceptance gate is implemented.

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

## Run the current disposable probe

From the repository root, compile the checked-in terminfo entry into a private
temporary directory, then launch a child in an interactive terminal:

```sh
probe_terminfo_dir="$(mktemp -d "${TMPDIR:-/tmp}/panelink-terminfo.XXXXXX")"
/usr/bin/tic -x -o "$probe_terminfo_dir" docs/terminfo/panelink-probe.terminfo
./target/debug/panelink pty-probe --terminfo "$probe_terminfo_dir" -- /bin/zsh
status=$?
rm -rf "$probe_terminfo_dir"
exit "$status"
```

The launcher script is also available as `bin/panelink`. Once the binary is
built, running `./bin/panelink` with no arguments opens the disposable zsh
probe; arguments are passed to the Panelink CLI. The launcher finds its
checkout from its own location, so it works after cloning into a different
directory. Ctrl-X stops the probe and its child, returning to the outer shell.
It does not detach or preserve the child. The probe requires access to the
host terminal device and is not supported in every restricted environment.

## Continue implementation

Start with [ROADMAP.md](../ROADMAP.md), then read the named task and evidence
files for the first incomplete item. The right-click split phase is in
`docs/progress/P3A-RIGHT-CLICK-SPLITS.md`; its bounded layout groundwork and
current external-model handoff are in `P3A-LAYOUT.md` and
`P3A-MIMO-MENU-TASK.md`. A previous OpenCode ACP launch was blocked in its
execution environment; do not assume the MiMo task ran. Verify the current
Git worktree and commit state before editing.
