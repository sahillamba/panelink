# Codex wheel latency baseline

Date: 2026-10-29. Checkout: `/Users/sahillamba/.t3/worktrees/panelink/t3-ca0a6ac8`.

## Fixture and method

`tests/codex_wheel_latency.rs` now reuses the private fixture, server, attach,
controlling-openpty, bounded screen drain and server-ready helpers from
`tests/p3_scroll_attach.rs`. Each paced/burst case creates its own mode-0700
fixture, runs the built `panelink serve` with a private shell child and connects
a real `panelink attach` process on an openpty controlling terminal. The child
sets raw/no-echo mode, enables alternate-screen and SGR mouse reporting, prints
READY, reads complete 10-byte SGR reports one byte at a time, appends a numeric
counter to its fixture-owned report file and redraws `COUNT-NN` at a fixed row.
The harness injects outer `ESC[<64;3;3M` reports. Counts come from validated
numeric report-file lines; first-visible response comes from the attached
terminal's parsed `COUNT-NN` screen. The burst observation ends after one second
without visible count change, bounded by three seconds. It does not wait for 20.

## Results

Command: `CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target cargo test --locked --offline --test codex_wheel_latency -- --ignored --nocapture --test-threads=1`

Result: pass (1 test). Observed output:

```text
paced: child_report_events=20/20 first_visible_response_ms=96.8 observation_duration_ms=1270.4
burst: child_report_events=1/20 first_visible_response_ms=96.6 observation_duration_ms=1109.1
```

The first-visible values measure wall time from before event injection until
`COUNT-NN` was observed in the attached screen, with helper polling at 20 ms;
they are coarse fixture observations, not exact latency guarantees. Paced
reports were awaited individually. Burst duration is the stable observation
window, and 1/20 is the measured baseline count, not a claim that dropped events
completed.

Deterministic regression command:
`CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target cargo test --locked --offline --bin panelink alternate_screen_wheels_are_not_coalesced_or_replayed -- --test-threads=1`

Result: pass (1 passed, 0 failed, 312 filtered out).

An initial fixture run timed out because the child-side report matcher expected
the wrong coordinate and used shell pattern escaping incorrectly. The child now
counts each complete 10-byte report received in its enabled SGR mouse mode;
the final private-PTY run above passed. No production source was changed. No
Codex application, installed client, Apple Terminal host acceptance, or user
pane was exercised.

## Final-source check

After the client repair, the same topology delivered paced20/20 and burst20/20.
The fixture now requires both counts to equal20, requires a visible count, and
validates the exact expected up-wheel SGR report at child-local(2,2).
An explicit final run during the canonical checks observed first-visible
117.3ms paced /120.1ms burst (20ms polling), and observation durations1593.7ms
/1667.3ms. The burst duration includes a one-second stable observation window;
it is not an exact delivery-completion latency. Earlier final-source run before
exact-report matching observed96.5ms/99.7ms first-visible. No reduced first-response
latency is established; the verified change is preservation of healthy burst
events. Intents still expire at one second or are discarded on invalidation.
