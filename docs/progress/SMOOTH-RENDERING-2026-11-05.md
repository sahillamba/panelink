# Continuous terminal rendering — 2026-11-05

Checkout: `t3-eb955439`, branch `t3/smooth-terminal-rendering`; clean before task.
No installed executable replacement, live pane input, server restart, commit or push.

## Target and feedback loop

Aim for 60 changed-screen updates/s under continuous activity. Initial opt-in
regression floor: 50 updates/s, p95 inter-update gap at most 40 ms. These are
engineering tolerances for this fixture, not guaranteed physical display FPS.
Idle screens must remain change-driven. Input latency is a separate open budget.

```sh
cargo test --locked --test codex_wheel_latency attached_continuous_rendering_budget -- --ignored --nocapture
```

The test drives a synthetic child writing a counter every 4 ms through its real
server-owned PTY, socket, attach compositor and outer PTY. It parses terminal
cells incrementally, timestamps changed markers for two seconds, and excludes
startup. The outer capture samples every 2 ms and the observer every 1 ms, so
reported gaps include observation jitter. It coalesces superseded child frames;
no input is replayed. The explicit test is ignored in ordinary CI because
performance assertions depend on host scheduling. No provider is invoked.
Keep PANELINK_WHEEL_BENCH_SERVER unset to test the checkout server.

## Diagnosis and result

Before production changes the test failed: 32.5 updates/s, p95 gap 32.5 ms,
maximum 34.5 ms (65 samples). The host already allows a frame every 16 ms, but
its network worker watches for screen changes only every 30 ms.

Changed event watches schedule their next observation after 8 ms, leaving room
for transport and the compositor. An unchanged watch returns to the existing
30 ms cadence; legacy no-event servers retain 100 ms fallback. No protocol,
ownership, input retry, output buffer or compositor baseline rules changed.

First run after the change: 60.1 updates/s, p95 19.2 ms, maximum 19.5 ms
(121 samples). Final serial rendering/wheel regression run: 57.6 updates/s,
p95 20.2 ms, maximum 49.1 ms (116 samples). Both pass the documented floor;
the isolated long gap shows why this is not a minimum-60-FPS guarantee.
The final run overlapped an all-target Clippy check. CPU/power overhead of
faster active watches is unmeasured; quiet panes return to their old cadence.

## Verification and remaining gates

- UI-client tests: 244 passed, zero failed, serial.
- Explicit real-PTY rendering and wheel tests: 2 passed. Paced and burst wheel
  delivery each remained 20/20; this is not a wheel-latency improvement claim.
- Strict all-target Clippy passed.
- Documentation/skill links, formatting and whitespace checks recorded in SESSION.md.

Single pane, 24x100, small counter updates, debug build, synthetic high-rate
output only. Full-screen redraws, large terminals, four busy panes, the ten-agent
workload, resource overhead, sustained tail latency, typing/scroll response and
physical Apple Terminal presentation still need representative measurement.
No full canonical verifier or native/provider acceptance is claimed.
