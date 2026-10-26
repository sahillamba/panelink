# Phase 5 selection client integration — 2026-10-25

## Scope delivered

Resumed the attached-client selection slice from the shared dirty checkout.
The existing state integration now has explicit left press, held-button drag,
and release handling; captures a frozen cell snapshot only from a confirmed
pane content rectangle with authoritative row-wrap metadata; clamps movement
to its origin pane; and validates target, geometry, buffer, and view epoch at
release. Ordinary live screen revisions do not replace the frozen selection.
Collapsed/blank selections do not enqueue clipboard data. Selection events
remain local and issue no pane key, paste, or scroll effects.

The host already enables button-motion mode 1002 with SGR 1006 and restores
both modes. The bounded `ClipboardWriter` starts before the host is opened,
nonblockingly submits one copied string, and reports fixed redacted result
classes. On detach, it receives a nonblocking cancellation request, then the
host terminal is restored before the writer is joined or network cleanup
runs. Declaration order also restores the host before either worker is
dropped during unwind. Writer startup failure is surfaced as the static
`clipboard unavailable` status. The worker uses its existing production
`/usr/bin/pbcopy` sink; tests remain injected or use private fake executables
and do not touch the host clipboard.

Added a state lifecycle test covering press/drag/release, preservation across
an ordinary screen revision, empty-copy suppression, absence of pane effects,
and cancellation after pane geometry changes. Added an alternate-screen
revision regression proving that ordinary output in the same buffer preserves
the frozen selection and `view_epoch`; buffer changes still reset history view
state and invalidate selection.

## Verification

Parent integration follow-up: the first r3 compile found simultaneous mutable
captures of the clipboard writer and a removed restore helper still imported
by the worker tests. The parent passed one mutable cleanup context through
the callback helper and retained the shared restore-before-detach helper.
It removed the unused initial network-handle assignment while preserving
declaration order for unwind cleanup. The focused binary client run passed
**74 tests,0 failed,0 ignored** in1.38s; log
`/private/tmp/panelink-p5-parent-client-r4.log`. This includes the ordinary
alternate-screen revision and restore/cleanup-order regressions. It does not
constitute real clipboard or human host acceptance.

`rustfmt --edition 2021 --config skip_children=true` was run on the owned Rust
files, and scoped `git diff --check` passed for the owned client files. Per the
resumption instruction, no Cargo, build, or test command was run by this
implementation sub-agent. After this correction, the parent should run the
binary-targeted client tests serially with its existing target directory:

```text
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --bin panelink ui_client:: -- --test-threads=1

CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo fmt --all -- --check
```

No commit, push, host activation, user configuration change, dependency
addition, clipboard read, or real clipboard write was performed. Apple
Terminal mouse-motion behavior and real clipboard acceptance remain unverified;
Phase 5 remains open.
