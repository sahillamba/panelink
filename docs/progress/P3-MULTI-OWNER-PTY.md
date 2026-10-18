# Multi-owner real-PTY assembly evidence

This integration test exercises the trusted public `MultiSessionOwner` with
two real private-PTY shell children. Each launch passes through the actual
launch decoder/preparer, `PreparedLaunch::spawn`, and `PtyEffects`; the owner
retains the resulting effects handles. The fixture directory is created
atomically with mode `0700`, and the storage lock remains alive until after
the owner is dropped.

The test confirms exact launch retry returns the retained result without
calling preparation or spawn again; fresh read-only connections are pinned to
the two distinct targets; polling plus encoded `ControlRequest::Read` messages
return each child's unique READY/PID sentinel without the sibling marker. A
cross-target read through the first connection returns fixed Unauthorized and
global status remains NotAdmitted. A third launch returns the exact registry
capacity error before either callback runs. Dropping the owner reaps both
children, checked with signal zero and `ESRCH`.

## Verification

On the Apple Silicon macOS target, the actual focused command passed:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_multi_owner -- --nocapture
1 passed; 0 failed; 0 ignored
```

`rustfmt --edition 2021 tests/p3_multi_owner.rs` formatted the owned test file.
The repository-wide `cargo fmt --check` and
`cargo clippy --locked --offline --test p3_multi_owner -- -D warnings` both
passed after the core owner completed its formatting and lint fixes. An earlier
Clippy run caught an explicit counter in the concurrently authored owner
module; the owner fixed it before this final rerun. The parent integrated
verifier remains responsible for the full repository check set.

`python3 scripts/check_docs.py` reported 126 Markdown files checked and zero
findings. A whitespace scan of both new owned files found no trailing spaces.

This evidence covers in-process owner dispatch and private-PTY lifecycle. It
does not establish socket pathname/accept behavior, runtime adoption of the
multi-owner type, input ownership/routing, event watches, or non-macOS
portability.
