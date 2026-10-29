# Codex wheel transaction cost — 2026-10-29

## Scope and method

This private, opt-in measurement times only the durable local client-context component. Each synthetic logical wheel runs three sequential `ContextFile::reserve` / `ContextFile::ack` pairs against one locked context. The real implementation persists each operation and fully flushes it; no durability behavior is disabled. Samples are retained in memory until each case completes. The finite cases are 1, 20 and 100 logical wheels (3, 60 and 300 reserve/ack samples apiece). The fixture is created under `/private/tmp` with mode 0700 and removed on drop. It uses a unique process-scoped context path and fixed nonzero identity; operation keys come from `reserve` and are passed unchanged to `ack`.

The requested `ClientContextStore` type does not exist in this checkout (`rg` found no declaration); the current public API is `panelink::client_context::ContextFile`. The test uses that actual durable API rather than a mock or pipe.

## Host and build identity

- Checkout: `/Users/sahillamba/.t3/worktrees/panelink/t3-ca0a6ac8`
- Branch / HEAD: `t3/improve-codex-mouse-scroll-speed`, `daf28a63f062331234ffd63daba577101d3315fa`
- Package: `panelink 0.1.7`
- Host: `Darwin HyperMac.local 25.5.0`, arm64, Apple Silicon `T8132`
- Rust: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `aarch64-apple-darwin`
- Profile: Cargo test profile, optimized+debuginfo disabled (debug test profile)
- Target directory: `/private/tmp/panelink-codex-wheel-target`

## Context-only results

Command:

```sh
CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target cargo test --locked --offline --test wheel_transaction_cost -- --ignored --nocapture --test-threads=1
```

Result: passed, 1 test; total test runtime 2.28 s. The test prints reserve and ack latency in microseconds; p50/p95/max; summed reserve and ack wall time; case wall time; and `context_only_reports_per_sec` (logical wheels divided by context-only case wall time).

| Logical wheels | Reserve / ack samples | Reserve p50 / p95 / max (µs) | Ack p50 / p95 / max (µs) | Summed reserve / ack (ms) | Case wall (ms) | Context-only reports/s |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 3 / 3 | 4021.2 / 11951.9 / 11951.9 | 3027.7 / 3979.1 / 3979.1 | 19.076 / 9.971 | 29.052 | 34.42 |
| 20 | 60 / 60 | 3003.9 / 3895.2 / 4141.8 | 3004.1 / 3531.4 / 3946.2 | 184.305 / 182.857 | 367.215 | 54.46 |
| 100 | 300 / 300 | 3025.7 / 3648.0 / 4927.2 | 3006.0 / 3823.8 / 9916.0 | 926.657 / 927.593 | 1854.488 | 53.92 |

These timings include only reserve/ack persistence and local call overhead. They do not include transport, server handling, PTY delivery, child parsing, screen observation, rendering, or host event capture. The one-wheel sample is especially noisy because it has only three observations per operation. These are measurements, not thresholds or a claim about complete wheel latency.

## Existing exact-report fixture, separately

Command:

```sh
CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target cargo test --locked --offline --test codex_wheel_latency -- --ignored --nocapture --test-threads=1
```

Result: passed, 1 test. Paced: 20/20 child report events; first visible response 102.5 ms; observation duration 1117.3 ms. Burst: 20/20 child report events; first visible response 131.4 ms; observation duration 1646.1 ms. This is a separate coarse private fixture result. It is not paired per-operation stage timing and cannot be subtracted from the context-only result to prove causation.

## Interpretation and next measurements

The 20- and 100-wheel cases measured about 54 context-only reports/s, consistent with six local durable transitions per logical report being a material cost worth addressing. This alone does not show what fraction of perceived scroll delay comes from context persistence. A one-RPC design should be evaluated only after separate measures of each request/reply wait, server queue/tick and PTY write completion, child first-output, observation/watch and screen retrieval/decode, render commit, and outer terminal visibility. Capture-to-worker queue age, poll interference, expiry/drop counts, and direct-versus-Panelink application displacement also remain needed to explain perceived responsiveness. Existing end-to-end results are recorded above without treating them as proof of this component's contribution.
