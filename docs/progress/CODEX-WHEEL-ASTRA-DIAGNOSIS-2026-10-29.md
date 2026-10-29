# Codex wheel consultation — Astra, 2026-10-29

Read-only source diagnosis; no fix implemented. Branch `t3/improve-codex-mouse-scroll-speed`, HEAD `daf28a63f062331234ffd63daba577101d3315fa`, existing dirty wheel changes preserved. Only this report was added. No builds, tests, installs, restarts, live-pane input, provider prompts, terminal control or subagents. Prior results below are documentary evidence, not rerun measurements.

## Assessment

The patch fixes burst loss under one synthetic workload, but leaves a costly serial transaction per wheel report. A FIFO can preserve movement while making it arrive late. The highest-value next step is stage timing, followed by reducing the number of durable operations and server round trips per report. Increasing the FIFO/TTL again is not a speed fix. Do not tune sensitivity until report latency and displacement are distinguished.

Current-source facts are distinguished below from hypotheses about their wall-clock contribution. The running old server has not been source-fingerprint-matched to this checkout, so current server source describes the implementation to test, not proof of its exact running instructions.

## Identity evidence

Read-only `~/.local/bin/panelink build-info --json` confirms 0.1.7, revision `daf28a63f062`, dirty fingerprint `a801d78163935d32`. `server-info --json` confirms 0.1.4, revision `10d81c24cdc2`, dirty fingerprint `1edf2c210c2de12d`, generation `15642535525154316809`, negotiated capability mask 511.

`ps -axo pid,ppid,lstart,comm` found client-like PID 55507 starting Oct 29 17:22:01. `lsof -a -p 55507 -d txt -Fni` maps its executable to `~/.local/bin/panelink`, inode 52842083. `stat -f '%i %Sm' -t '%Y-%m-%d %H:%M:%S' ~/.local/bin/panelink` reports the same inode, modified 17:21:06. This is strong noninvasive evidence that a newly started client uses the installed file, stronger than PATH/build-info alone. No in-memory fingerprint RPC or association with the user's specific visible window was established. No pane screen/content was collected. Actual Codex mode, foreground identity/version and configuration remain unverified; a process named codex elsewhere is insufficient.

## Ranked causes and falsifiable measurements

### 1. Six durable full flushes and six sequential RPCs per wheel report

Source fact: `src/ui_client/net.rs:264-295` performs claim, wheel, confirmed release. Expanding this gives:

- Describe ownership, keyed Claim, Describe ownership (`src/ui_client/conn.rs:551-580`).
- Keyed MouseWheelInput (`conn.rs:639-675`).
- Keyed Release, Describe vacancy (`conn.rs:590-610`).

Thus six request/reply exchanges on an already established stream, not six new socket connections. `request` writes then reads synchronously (`conn.rs:394-417`). Every keyed operation reserves durably before transmission (`conn.rs:450-469`), then acknowledges the terminal reply (`conn.rs:514-543`). Reserve and ack both persist (`src/client_context.rs:302-348`); persistence writes the alternating slot and calls `fcntl_fullfsync` (`client_context.rs:388-415`). Healthy successful delivery therefore incurs SIX F_FULLFSYNC calls per report, plus occasional Receipt RPCs every 32 admitted sequences (`conn.rs:59,723-762`). Twenty reports require 120 full flushes and at least 120 request/reply exchanges, before observation traffic.

Inference, highest confidence mechanism: this is inappropriate overhead for a high-frequency pointer stream and a likely major service-time cost. No syscall timings were measured here, so do not claim a percentage or that disk is conclusively dominant. Removing the preliminary history read saves only one RPC; it does not remove this serial chain.

Falsify/quantify: timestamp reserve/flush, each request write/reply, ack/flush, and release completion in an opt-in private fixture. Record durations and counters only. Separate time waiting for the server from local flush time. Compare one-report and sustained report service distributions. Keep durability enabled in all acceptance runs; a special no-flush diagnostic could isolate cost but must never become the safety-preserving solution.

### 2. The FIFO conceals insufficient service rate; rendering still competes with input

Source fact: `state.rs:2783-2808` routes cached alternate screens into the dedicated path and queues at most 32 additional intents. `state.rs:2848-2897` sends one only after prior pending scroll completion, checks per-intent one-second age, and discards stale entries. `apply_scroll_update` rebases queued epochs after success (`state.rs:2477-2492`), not their capture times. `worker.rs:439-461` permits only one scroll command in flight.

Screen polling is allowed now (`worker.rs:247-256`), but it is not concurrent: one `worker_loop` executes entire commands serially (`worker.rs:565-585`). The host submits a poll before consuming results and flushing the next wheel (`host.rs:1051-1091`). A queued poll can precede the next wheel; that poll maintains connections, refreshes layout every 250 ms, and iterates all connected panes (`net.rs:599-667`). Each due pane watches and, on change, fetches a screen. Thus both input and screen delivery can wait for other panes or maintenance.

Documentary observation: 20 reports now survive, first-visible response stays roughly 100–120 ms, burst observation lasts roughly 1.7–1.9 seconds including one second stability. Subtracting stability suggests roughly 0.7–0.9 seconds to the last observed change, or roughly 22–29 reports/sec across that short burst. This is an estimate of aggregate completion, NOT measured steady-state service rate or child delivery latency. Nonetheless it explains why 20/20 can still feel slow. At such a service rate, a 60-report/sec stream would accumulate age, overflow and expire even with this patch. A late opposite-direction report waits behind old reports, producing apparent inertia.

Measure open-loop 20/60/120 reports/sec for 5 seconds and bursts of 20/32/64, with one pane and four panes, including a busy sibling. Record capture, enqueue, dequeue, PTY delivery, expiry/overflow/cancellation and queue-age p50/p95/max. Do not require all 64 instantaneous reports to survive the deliberately smaller queue; require exact accounting and bounded age. Trace poll duration and whether a poll is ahead of a wheel. If most delay is after PTY delivery, optimizing input transactions alone will not solve perceived latency.

### 3. Observation cadence, server tick, PTY parsing and render cost compound the delay

Source path:

- Host reads 1024-byte chunks with a 5 ms work budget, decoder dispatches completed mouse events (`host.rs:44-50,857-928`). There is no deliberate 50 ms mouse delay: the 50 ms escape timeout is for incomplete/bare escape handling.
- Server request dispatch reaches `handle_mouse_wheel` (`multi_runtime.rs:2190-2200`). It validates live connection, scope, exact target, owner revision, mode/capacity and ledger admission (`server/multi_session.rs:2668-2725`).
- `PtyEffects::mouse_wheel` validates again and synchronously drains (`server_pty.rs:1408-1448`). Drain's 100 ms value is a maximum, NOT a mandatory sleep. The 2 ms sleep happens only on no-write progress (`server_pty.rs:70-71,922-972`). Applied proves all report bytes written to PTY, not that Codex processed or rendered them.
- `TerminalSession::pump` reads/parses up to 16 KiB before writing input, then refreshes the snapshot even when no child bytes were read (`terminal/mod.rs:1156-1227`). A busy child's parsing and full snapshot work can contribute to input cost and owner tick time.
- Owner tick polls retained PTYs before processing its bounded queue (`multi_runtime.rs:767-787,828-832`). Main serve loop then waits on listener accept with a nominal 1 ms slice (`p3_cli.rs:70,1807,1855`). Listener readiness waits on the listening socket (`transport.rs:346-380,778-815`); existing-connection owner messages do not directly wake that listener. Multiple serial RPCs can each incur scheduling delay. Current transport uses readiness polling, not an assumed fixed 10/100 ms sleep.
- Client events watch is immediate (`conn.rs:828-843`, wait_ms=0), scheduled at 30 ms per pane, or 100 ms screen fallback (`net.rs:36-38,631-659`). A screen change then requires screen retrieval, decode and state application. Rendering is gated at 16 ms; pending output must finish before another frame is prepared; writes are capped at 64 KiB per iteration, followed by a 1 ms sleep (`host.rs:49,57,1257-1287`).

Inference: 30 ms polling plus rendering and serial input/poll work plausibly explains first response around 100 ms. Do not simply add all maximum constants; phases overlap, platform sleeps may overshoot, and no observed 100 ms constant establishes a root cause.

Measure server queue wait, tick/pump parse/snapshot durations, bytes read, write completion, first child output/revision, watch request/reply, screen bytes/decode, render start/commit and outer-PTY observation. Use synthetic multi-row redraws at 24x100 and the user's actual geometry. A receiver that updates one counter cannot characterize full Codex transcript redraw costs. Test the old-server/new-client combination separately from new/new private fixtures.

### 4. Scrolling distance and mode semantics may explain a different kind of “slow”

Source fact: normal history advances three rows per wheel event and combines deltas (`state.rs:2810-2835`); alternate screen emits one report per accepted event (`terminal/mod.rs:1536-1581`). Decoder recognizes vertical/horizontal wheel and modifiers (`vendor/crossterm-input/src/parse.rs:783-812`), but `ui_client/keys.rs:268-273` retains only vertical direction; outgoing encoding uses bare button 64/65. There is no pixel delta, velocity or acceleration field in this path. Modifier behavior is not transparently passed through. This predates the FIFO patch.

The number of rows Codex moves per report and any timing-sensitive acceleration are unknown. Slowing report cadence could also change application acceleration, even with equal delivered counts. Plain zsh in the native terminal often means the terminal scrolls its own history; zsh in a Panelink pane uses Panelink's three-row history path. Neither is an equivalent application-input baseline. Confirm which comparison the user means.

Measure the same deterministic report sequence against the same Codex version/mode directly and through Panelink in a separately authorized isolated setup, no provider prompt required. Record report counts, rows displaced, first/last motion, direction reversal and report spacing. Separate “each step is too small” from “steps arrive late” and “steps are missing.” Do not assume Codex is in alternate screen: normal/inline mode bypasses the new FIFO entirely. First inspect bounded mode/identity metadata; do not collect transcripts or broad config secrets. No live application/config inspection was needed or done in this consultation.

## Concrete regression and safety limits of the patch

Confirmed by source: cached alternate-screen state alone selects direct delivery (`state.rs:2783`); it does not require mouse reporting. `net.rs:274-284` claims before server mouse-mode rejection and returns “alternate-screen mode changed” on Unsupported. Previously `scroll_inner` required BOTH alternate screen and mouse reporting (`net.rs:335-336`), and `history_result` returned AlternateScreenNoMouse (`net.rs:905`), displaying “use its scroll keys” (`state.rs:2494-2508`). A stable no-mouse alternate application now incurs unnecessary ownership mutations/full flushes and a misleading mode-change message. If another owner holds it, it can misleadingly report ownership conflict for an operation the app cannot support. Fix this routing/status regression without weakening authoritative server mode checks. Reproduce with a private alternate-screen child that never enables mouse; assert zero claim operations and correct feedback.

Another limitation, not a proven contract violation: freshness/cancellation ends at worker admission. `Command::AlternateWheel` carries no capture deadline or cancellation generation (`worker.rs:64-70`). An event can be accepted into the worker queue behind a slow Poll and transmitted much later, even after the UI FIFO is cleared by typing. The documented spec explicitly allows admitted events to be irreversible, so this is not evidence of replay; however “one-second expiry” must not be advertised as a capture-to-PTY deadline. Measure that gap and, in a future change, revalidate fresh unsent work immediately before claim. Once transmission/admission is uncertain, never cancel by retrying or replaying.

No other newly introduced correctness defect was established in this bounded review. This is not exhaustive acceptance or a proof that the remaining cancellation cases are correct.

## Why the existing fixture is insufficient

`tests/codex_wheel_latency.rs:270-324` proves exact repeated SGR delivery to a shell receiver and at least one visible counter. It does not execute Codex. The child forks `dd` via command substitution per report, reads byte-by-byte, opens/appends a log and redraws a single counter. Its overhead is mixed into end-to-end results. Paced mode waits for every visible update before injecting another event, which prevents queue growth by construction. Burst mode sends all 20 identical up reports at once and waits for a one-second stable visible count. It logs neither individual arrival timestamps nor actual last-delivery time; repeated identical reports do not establish mixed-direction/coordinate order. It asserts 20 log lines but not a final visible count of 20.

Capture polling is 20 ms, and `screen()` reparses accumulated output from the beginning (`tests/codex_wheel_latency.rs:206-223`). This makes it a coarse observation fixture, not a latency histogram. Its 24x100 private attach geometry and same-build server do not match every installed configuration. Keep it as a no-loss regression. Add an in-process/private receiver with monotonic per-report timestamps, deterministic distinct directions/coordinates, incremental terminal parsing, final-visible-count assertion and separate first/last PTY and visible times. No provider, disk log per report, or external command per report is required for that receiver.

## Smallest defensible next implementation sequence (proposal only)

1. Instrument the existing path under an opt-in private measurement mode in `tests/codex_wheel_latency.rs`, `ui_client/{host,state,worker,net,conn}.rs`, `client_context.rs`, `multi_runtime.rs`, and `server_pty.rs`. Keep only timestamps/counters, no input payloads or pane text. Fix no-mouse routing/status and add its deterministic regression. Establish which stage dominates before changing concurrency or safety policy.
2. If the serial transaction cost is confirmed, add one capability-negotiated, keyed **scoped wheel** operation in `ui_protocol.rs`, `handshake_protocol.rs`, `multi_runtime.rs`, `server/multi_session.rs`, and the owner model as needed. Client `conn.rs`/`net.rs` sends one exact-target typed event with one durable reserve/ack pair. On the serialized owner, require vacancy (including refusing a retained same-caller claim), validate live target, mode, coordinates and capacity, acquire scoped ownership, deliver once with bounded suffix discard, release only on known Applied. Return delivery and release dispositions plus authoritative ownership revision in the same keyed result. On partial/failed/unknown outcomes preserve unresolved ownership and existing reconciliation; lost replies never trigger resend. This can reduce six RPCs/six full flushes to one RPC/two full flushes without disabling durability. It needs careful ledger/ownership/crash tests; merely skipping Describe or keeping an idle claim is not equivalent.
3. Start with single-event semantics. Only if still needed, extend to a tightly bounded ordered vector of fresh wheel reports, one claim per immediate batch, no netting/repetition multiplier, per-report coordinates/direction preserved, capacity checked up front and ambiguous suffix never replayed. Do not start with an unbounded “gesture session” or larger TTL. Separate mode generations may be needed if supporting delayed batches across mode transitions.
4. If measurement identifies polling as dominant, make background polls yield between panes, prioritize active-pane observation after known input, and process completions before scheduling another general poll (`host.rs`, `worker.rs`, `net.rs`). Keep regular screen updates during sustained input; no starvation. Consider owner-queue readiness wakeups (`p3_cli.rs`, `multi_runtime.rs`, `transport.rs`) only if queue-wait measurements justify them. Avoid a blanket faster timer or unrelated core rewrite.

Old servers retain the existing fallback and its limitations; new scoped protocol performance requires an updated server and separately authorized activation. Do not restart the user's server as part of a client-only install. Update SPEC, agent usage skill, compatibility guidance and evidence together if implemented.

## Proposed acceptance, not existing product guarantees

Private benchmark targets on a stated idle Apple Silicon host: sustained 60 reports/sec for five seconds with no loss, duplicates or growing backlog; 20-report burst last-visible response within 250 ms; input-to-PTY p95 below 20 ms and input-to-visible p95 below 50 ms/p99 below 100 ms. Report failures rather than relaxing TTL to pass. These provisional budgets must be reviewed against the direct receiver/application baseline and host geometry. Under load, publish distributions and cancellation/drop counts instead of averaging away tails. Keep timing thresholds out of ordinary shared CI; deterministic no-replay, ownership, mode, exact-order, bounds and final-visible-count tests belong there.

Real application acceptance separately requires same version/mode/geometry, direct versus Panelink rows-per-report and latency, including sustained scrolling and reversal. Native host gesture acceleration must be checked with authorized real-host observations; injected SGR reports cannot prove it. Canonical 8/8 is regression evidence, not this acceptance.

## Commands, sources and limitations

Executed only read-only shell/source operations plus writing this file: `pwd`; `git status --short --branch`; `git status --short`; `git rev-parse HEAD`; `git log -1 --format='%h %s'`; `git diff -- src/ui_client/state.rs src/ui_client/worker.rs src/ui_client/net.rs`; `cat`, `sed -n`, `nl -ba`, and `rg -n` on the onboarding documents, skill/reference, wheel reports and source files cited above; `rg --files docs/progress`; the identity commands quoted above. Read AGENTS, README, SPEC, DISCOVERY, PREBUILD, ROADMAP and current SESSION guidance; large combined reads were truncated, followed by targeted rereads of relevant sections. Graph-first orientation used codebase-memory `search_code`, `search_graph`, `index_status` for project `Users-sahillamba-.t3-worktrees-panelink-t3-ca0a6ac8`; ready does not imply freshness, so line evidence comes from current files. No reindex performed. An initial lookup of nonexistent `src/agent_context.rs` and an unmatched `components/input-decoder/src/*` glob failed; corrected to `client_context.rs` and `vendor/crossterm-input`.

A quick historical memory pass informed the distinction between normal-history coalescing and safety-sensitive application input; current source and current task's bounded-FIFO authorization supersede older no-queue guidance. No new timing run was performed: repeating the same coarse fixture would not isolate the missing stages. All proposed performance numbers and implementation changes remain unverified.
