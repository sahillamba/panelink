# Important-feature verification — 2026-10-27

Checkout: `/Users/sahillamba/panelink`, clean `main` at `bd84625cef37cf037e2045ec0cb6a31390ff7db0` before this evidence file. No application changes, commits, or pushes. Existing live panes were not modified by these tests.

## Build identity and scope

Installed PATH launcher resolves to `/Users/sahillamba/.t3/worktrees/panelink/codex-main-local-changes-20261027/target/release/panelink` (0.1.3). These source tests exercise the main checkout (package 0.1.0), not that installed binary. No full product, phase, Apple Terminal, provider identity, or physical reboot acceptance is claimed.

## Canonical verifier

Command: `CARGO_HOME=/private/tmp/panelink-verification-cargo-f8gxrjxv RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py`. Temporary writable Cargo home uses read-only symlinks to the populated standard registry. Initial attempt used an empty temporary cache and failed dependency resolution; it is superseded by the cached run below.

- Library: 675 passed, 0 failed, 4 ignored.
- Binary: 118 passed, 1 failed at socket bind with PermissionDenied / EPERM. Root cargo test stops here, so it does not cover the integration suite.
- Vendored decoder: 39 passed.
- Component fingerprints, fmt, strict all-target Clippy, and build: passed.
- Python: 76 passed, 1 failed (loopback listener). Independent loopback bind probe returned PermissionError / errno 1 / Operation not permitted.
- Documentation checker: 235 files, 0 findings before this evidence file.
- Verifier result: 6 of 8 check groups passed; exit 1.

## Focused integration runs

Each target ran with `cargo test --locked --offline --test TARGET -- --test-threads=1 --nocapture`, same temporary Cargo home. Each had a 55-second process-group deadline. No run timed out and no SKIPPED notice was observed.

| Target | Actual test result |
| --- | --- |
| `p2_pty` | ok. 11 passed; 0 failed; 0 ignored |
| `p3_four_panes` | FAILED. 0 passed; 1 failed; 0 ignored |
| `p3_attach_four` | FAILED. 0 passed; 1 failed; 0 ignored |
| `p3_agent_cli` | FAILED. 0 passed; 2 failed; 0 ignored |
| `p3_socket_detach` | ok. 2 passed; 0 failed; 0 ignored |
| `p3_managed_launch` | ok. 2 passed; 0 failed; 0 ignored |
| `p3b_close_runtime` | FAILED. 0 passed; 5 failed; 0 ignored |
| `p5_history_store` | ok. 10 passed; 0 failed; 0 ignored |
| `p5_history_corruption_cli` | ok. 2 passed; 0 failed; 0 ignored |
| `p5_layout_lifecycle` | ok. 12 passed; 0 failed; 0 ignored |
| `p5_recovery_cli` | ok. 5 passed; 0 failed; 0 ignored |
| `p5_recovery_relaunch` | FAILED. 2 passed; 5 failed; 0 ignored |

Four-pane and agent-CLI scenarios failed before feature acceptance: attach and agent CLI explicitly returned `listener_permission_denied`; four-pane test unwraps a missing startup target. Close-runtime cases report inability to start the server. Recovery relaunch reports pathname socket bind denied. These remain failed/blocked checks, not passes.

Explicit ignored host test `normal_child_exit_restores_termios_and_never_forwards_osc_controls` was attempted: 0 passed, 1 failed; observed child exit 1 instead of expected 7. The existing test documents restricted `/dev/tty` access, but this run alone does not establish the precise failure cause. Terminal restoration/OSC denial are not certified by this attempt.

## Remaining coverage

Full integration sweep, current installed-build isolated acceptance, pointer/UI/clipboard behavior on the actual host, physical reboot, active-agent workload budgets, and provider identity/recovery remain unverified here. Socket permission prevents new isolated server acceptance in this session. No guards or assertions were weakened.

One GPT-6-Luna read-only helper mapped documentation coverage; parent selected, ran, and reviewed checks. No descendant agents were spawned by that helper.

Raw local logs are under `/private/tmp/panelink-feature-*-20261027.log`, `/private/tmp/panelink-feature-verification-cached-20261027.log`, and `/private/tmp/panelink-host-check-20261027.log`; these temporary logs may be removed by the OS. This document preserves the outcomes.
