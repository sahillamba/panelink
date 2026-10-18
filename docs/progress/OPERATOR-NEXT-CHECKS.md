# Remaining operator checks — 2026-10-18

These three checks are still pending operator evidence. This checklist is a
handoff, not runtime acceptance. The existing Ctrl-X return-to-shell check is
operator-confirmed; use Ctrl-X below to exit the combined zsh/clipboard run.
Do not repeat the already completed broader manual terminal checklist.

## 1. Default zsh launcher and clipboard denial

Run in an interactive Apple Terminal from the repository root. This rebuilds
the launcher target using the recorded writable offline Cargo cache, then
starts the installed single-word launcher:

The clipboard step replaces your current clipboard with a harmless disposable
sentinel. Save anything you need from it before running these commands.

```sh
cd /Users/sahillamba/panelink
export CARGO_HOME=/private/tmp/panelink-p2-cargo-home
cargo build --locked --offline
printf %s PANELINK-SENTINEL | /usr/bin/pbcopy
panelink
```

In the inner shell, confirm it is zsh with `printf 'ZSH_VERSION=%s\n' "$ZSH_VERSION"`.
Then run each harmless OSC 52 request:

```sh
printf '\033]52;c;UDItWA==\007'
printf '\033]52;c;\007'
printf '\033]52;c;?\007'
```

The first requests a write of `P2-X`, the second a clear, and the third a
read. Confirm none changes the outer clipboard, no OSC 52 is emitted to the
host, and the read does not inject `52;c;` or any other command text at the
shell prompt. Press Ctrl-X to exit the probe. Back in the outer shell, compare
without displaying clipboard contents:

```sh
if [ "$(/usr/bin/pbpaste)" = PANELINK-SENTINEL ]; then
  printf 'CLIPBOARD_SENTINEL=PASS\n'
else
  printf 'CLIPBOARD_SENTINEL=FAIL\n'
fi
```

This intentionally replaces the clipboard with a harmless disposable
sentinel; it does not read or save any prior clipboard value. Report zsh
version presence, prompt behavior, and the comparison result. The expected
comparison is `PASS`.

## 2. Expanded pathname reconcile/watch fixture

From the repository root, in an environment that permits pathname Unix socket
bind/accept, run the actual ignored CLI fixture:

```sh
cd /Users/sahillamba/panelink
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --ignored --exact --nocapture
```

Expected fixture result: exit 0, one test passed. Its assertions cover the
expanded reconcile/watch behavior, including `changed` with an event and a
later cursor, unchanged caller-context bytes, and the zero-wait `timed_out`
response. That zero-wait watch intentionally returns CLI exit 1; the fixture
expects and validates it, so the overall test must still exit 0. A fixture
failure or `listener_unavailable`/`EPERM` is not a pass; report the bounded
failure output and leave the acceptance pending. Earlier sandbox EPERM and
the prior operator pass predate this expanded fixture.

## Report template

Reply with this filled in; include only the failing command's first 40 stdout
and stderr lines if anything fails, and redact any unrelated sensitive text.

```text
1. zsh launcher: PASS / FAIL
   ZSH_VERSION present: yes / no
   Ctrl-X returned to outer shell: yes / no
2. clipboard denial: PASS / FAIL
   OSC 52 write/clear/read had no host effect or injected prompt input: yes / no
   Sentinel compare: PASS / FAIL
3. expanded pathname reconcile/watch fixture: PASS / FAIL
   Exit code: ___
   Failure stdout (up to 40 lines):
   Failure stderr (up to 40 lines):
```

## Resume after disconnect

Use these repository records as the canonical project state before continuing:

```sh
cd /Users/sahillamba/panelink
cat ROADMAP.md
tail -n 80 docs/progress/INTEGRATION.md
```

See [integration evidence](INTEGRATION.md), [clipboard and terminal
acceptance notes](P2-OPERATOR-RUN.md), [pathname fixture evidence](P3-PATHNAME.md),
and [expanded watch fixture evidence](P3-WATCH-PATHNAME.md). Operator reports
update acceptance evidence; this checklist alone does not complete Phase 2 or
Phase 3.
