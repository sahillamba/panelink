# P4 managed foreign UI probe — 2026-10-25

Status: **blocked before managed UI startup**. This attempt does not establish
foreign-project selection, refusal, or return-to-A acceptance. The Phase 4 gate
remains open.

## Bounded harness change

The private 2.0.32 fixture now binds Ctrl-O to the pinned `open.menu` command
and disables the competing `session.list` chord. The private probe continues
to require its exact, stable fixture row before Enter. It also now requires the
existing independent nonce/sequence observer to acknowledge the exact foreign
project hash, top-level session hash, same TUI instance, and a later sequence
before evaluating refusal. Return-to-A requires a newer observer sequence from
that same TUI plus the fresh managed binding revision. Unit coverage rejects a
wrong project, session, top-level state, instance, or sequence.

This follows the pinned native bundle's command registry: `open.menu` is the
“Open session or project” command with default Ctrl-O at byte offset
113,399,154; its command handler opens the shared picker at offset 117,077,134.
The earlier standalone same-client project fixture used Ctrl-O and required a
nonce-correlated project/session route from the same observer instance
([P4-REMAINING-IDENTITY.md](P4-REMAINING-IDENTITY.md)). These are source and
prior fixture facts; the harness change was not live-accepted in this run.

## Verification

- Launcher resolution: `opencode` was absent from PATH; used
  `/Users/sahillamba/.opencode/bin/opencode`. `tescode` was not used.
- Native pin: OpenCode `2.0.32`, SHA-256
  `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
- Private build command: `CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target cargo build --locked --offline` — passed.
- Panelink binary SHA-256:
  `114eba1800404feb63b82cc9546f5433949d360490e3ed2df15033617d9762a2`.
- `python3 -m unittest -v tests.test_panelink_provider_bridge_probe` — 20
  passed, 1 opt-in native test skipped, 0 failures.
- `python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.
- `git diff --check -- scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.

## Native managed run

Command:

```sh
python3 scripts/probe_panelink_provider_bridge.py \
  --panelink-binary /private/tmp/panelink-p4-bridge-store-target/debug/panelink \
  --opencode "$HOME/.opencode/bin/opencode"
```

The final run exited 2 with `classification=blocked` and
`reason=managed_serve_startup_unverified`. Both managed Panelink children
exited with code 1 before any stdout event; both had `event_count=0`,
`stderr_category=empty`, and no recognized startup error. The pinned version
matched, the loopback-only sandbox was enforced, disposable fixture construction
and cleanup completed, and `prompt_submissions=0`. No attach client or native
picker transition ran. The fixed-schema output is
`/private/tmp/panelink-p4-20261025-managed-provider-bridge-final.json`.

Two earlier executions of the same native managed probe also stopped at server
startup; the final run added only a bounded stderr category to distinguish a
startup failure without retaining raw diagnostics. No further retry was made.
Probe-owned Panelink/OpenCode children were reaped; the separate user service
was not touched. No Rust/application source, user configuration, git index,
commit, or push was changed by this task.

## Gate and safe fallback

The managed foreign UI capability is still **unproven**, rather than classified
as unsupported: no managed UI reached the point where the new route guard could
run. Keep the last-confirmed binding and explicit manual recovery policy in
force; that fallback does not satisfy this UI acceptance gate. Resume only
after the private managed server launch produces a validated startup receipt,
then run the guarded native route and require the recorded same-instance
foreign mapping, refusal with A preserved, and mapped return-to-A revision.
