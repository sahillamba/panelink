# P6 R-03 workload with real OpenCode (idle agents) — 2026-10-22

User decision (2026-10-22): real OpenCode TUIs, **idle, no model calls, no
prompts**. Worker GPT-6-Luna (xhigh) built `examples/opencode_ten_agent_workload.rs`
but its runs hung; the parent stopped it and fixed the harness (see below),
then executed all runs.

## Method

- Five independent two-pane workspaces/storage domains in one process (the
  prototype caps a workspace at two panes), history and saved layout enabled.
- Variant A (mixed): each workspace = 1 OpenCode TUI + 1 `/bin/sh` paced
  output shell → 5 OpenCode + 5 shells.
- Variant B (R-03 ten-agent case): both panes OpenCode → **10 OpenCode
  sessions**. Shells cannot be added on top in this prototype (2-pane
  limit); that is recorded as a limit, not hidden.
- OpenCode 1.19.0 via `opencode` on PATH. Each TUI: fresh 0700 HOME/XDG,
  fresh project dir, `OPENCODE_DISABLE_MODELS_FETCH=1`, autoupdate off, no
  auth, minimal env. Prompt submissions recorded as 0 in every run.
- Measurement starts only after every TUI shows its brand and composer.
  60 s interval. Input round trip for OpenCode = type one character into the
  composer, observe echo, erase with Backspace (never Enter). Shells use the
  ack-marker probe. `ps` sampled every second, Panelink process separated
  from OpenCode children and shells (R-03 overhead attribution).
- Every child process group is stopped and verified non-live afterwards.

## Results (two runs per variant, sequential)

| Run | OpenCode / shells | OpenCode TUI startup | Owner tick p95 (per-ws range) / max | OpenCode keystroke echo+erase p50 (per-ws) / p95 max | Shell ack p50 | Panelink process RSS p50 / max | Panelink CPU mean / max | OpenCode children RSS p50 / max | History / layout | Shutdown per ws | Leaks |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A1 | 5 / 5 | 2.9–3.1 s | 175–201 µs / 9.8 ms | 112–115 ms / 188 ms | 26–30 ms | 11.4 / 16.8 MiB | 23.0% / 29.2% | 4.27 / 5.30 GiB | 5/5 ok | 177–208 ms | none |
| A2 | 5 / 5 | 3.0–3.2 s | 168–191 µs / 22.1 ms | 111–118 ms / 191 ms | 15–16 ms | 11.7 / 17.6 MiB | 23.8% / 31.2% | 5.16 / 5.44 GiB | 5/5 ok | 183–221 ms | none |
| B1 | 10 / 0 | 3.5–4.7 s | 177–209 µs / 17.0 ms | 117–121 ms / 229 ms | n/a | 9.2 / 15.2 MiB | 20.3% / 27.6% | 3.79 / 6.70 GiB | 5/5 ok | 204–249 ms | none |
| B2 | 10 / 0 | 3.2–4.5 s | 196–243 µs / 21.7 ms | 173–178 ms / 195 ms | n/a | 10.2 / 12.9 MiB | 22.2% / 31.6% | 5.37 / 7.91 GiB | 5/5 ok | 249–405 ms | none |

Raw data: [A1](p6-raw/opencode-agents-A-run1.json), [A2](p6-raw/opencode-agents-A-run2.json),
[B1](p6-raw/opencode-agents-B-run1.json), [B2](p6-raw/opencode-agents-B-run2.json).
Host: Apple M4 Pro (Mac16,8), 12 cores, 24 GiB, macOS 26.6.1. The
"Panelink process" here is the single benchmark process hosting five
server drivers plus their control clients, so it overstates one real
server's overhead. Idle OpenCode TUIs dominate memory (~0.4–0.8 GiB each)
and show bursty CPU (B1 OpenCode children mean 189%, p95 618%).

## Classification against R-03

**10 OpenCode sessions usable with idle agents: observed** (B1, B2: all 10
TUIs rendered and accepted/erased typed input; owner tick p95 ≤ 0.25 ms;
keystroke round trip p50 ≤ 0.18 s). Panelink overhead is recorded
separately. **Not established:** active-agent load (no model calls by user
decision), "10 OpenCode sessions plus representative shells" in one view
(2-pane-per-workspace prototype limit), a performance budget, or other
hosts. These remain explicit limits.

## Parent harness fixes (example only, no product code)

- Startup readiness loop issued a new blocking screen query every pass and
  deadlocked a size-1 command channel (11+ min at 0% CPU); now at most one
  in-flight query per pane.
- Keepalives were not sent during the up-to-60 s readiness wait, and the
  first measured keepalive was postponed a full interval; the server drops
  idle clients after ~1 s (broken pipe). Fixed both.
- The 30 s input-owner lease (MAX_OWNER_TTL_SECS) expired during slow TUI
  startup, so input was rejected `StaleOwner`; the client now re-claims with
  a fresh operation key before a probe when ownership lapsed. Input itself is
  never resent.
- Leak check counted zombie/exiting processes awaiting reap by the PTY owner;
  now only live processes count. Probe deadlines widened (8 s echo/erase,
  20 s whole probe) for ten concurrent TUIs.
- First B attempts (before the deadline widening) failed with
  `input round trip failed` / `control client stopped`; recorded, then
  rerun. All four reported runs used the final binary.
