# Scoped wheel implementation and verification

2026-10-29, `t3/improve-codex-mouse-scroll-speed`, uncommitted source based on
`daf28a63f062`. Package0.1.8 identifies this follow-up. Client and server0.1.8 active; scoped capability verified.

## Implemented contract

UI schema8 kind8,80bytes, optional capability1024 `scoped_mouse_wheel` plus
selected Input. Multi-session server only; old grants cannot widen on resume.
One typed keyed event with one durable reserve/ack pair. Before admission the
serialized owner validates mode/target/capacity and vacancy (including refusing
same-caller retained claims), prepares pinned/retained/released owner clones,
and preencodes results. Applied completes delivery and scoped release atomically
to other owner messages; other terminal results retain the holder with existing
TTL semantics. Duplicate/status paths never send input or release later claims.
No disabled flush, batching, enlarged FIFO/TTL, idle claim, or new dependency.

Client negotiation selects the new operation only where supported. Legacy wheel
uses query-first mode detection and vacancy-only Claim CAS, preserving normal
key/paste claim behavior. Unsupported scoped input permits only a read-only mode
refresh, never a fallback input send. Stable no-mouse app feedback is repaired.
The existing pending FIFO remains32/one-second, not a capture-to-PTY deadline.

Parent review repaired early owner-vs-mode ordering, request response-body
matching, manual reserve bypass of uncertainty settlement, missing legacy
query-first/vacancy checks, and stale-target validation on mode refresh. Tests
are evidence for these boundaries, not a provider completion guarantee.

## Focused checks

- Codec23/0 and handshake15/0 independently passed after integration.
- Scoped server/classification/codec9/0: Applied release/revision+2 and no duplicate
  effects; failed/partial/unknown/Accepted normalization retains ownership;
  same/foreign/pending/due owners cannot be stolen; no-mouse validation precedes
  ownership; missing capability/ledger capacity have no effects; preprepared
  release handles revisionMAX-2 and rejects MAX-1/MAX before admission; old grant
  resume cannot widen; single server/input-disabled server do not offer support.
- Client scoped5/0: both capabilities required before reserving, one exact keyed
  RPC and one sequence step, unchanged local owner state, failure release labels,
  wrong-key reply stays uncertain and reconciles by status only, legacy same-caller
  retained claim refused, stable no-mouse returns correct feedback on new and old
  masks without ownership transactions. Underlying PTY failure/drain regressions
  are separate from mock handler outcomes.

A cfg(test)-only owner constructor permits otherwise unreachable revision-edge
fixtures. No production InputOwner state/API change. Missing exhaustive cases
remain explicit: no separate full scoped PTY fault-injection matrix, no complete
capture-to-PTY/p95/p99 sustained-rate profile, busy-four-pane workload, or real
Codex displacement/gesture acceleration acceptance. No timing budgets are
asserted as product guarantees.

## Measurement repair and observations

The initial W4 receiver exited before READY because the server's explicit launch
environment did not pass its test-only variable to the child. Parent put the
variable explicitly in the private launch script. Parent also fixed counting
microsecond samples as u8 and waited for the final bounded sample block.

The receiver is now one integration-test process in raw PTY mode, parses each
exact SGR report without spawning per-report processes, records timestamps in
memory, and writes one sample file after20 reports. Parent records first/last
visible counts and asserts both child and visible final counts equal20.
Twenty-ms outer-screen polling and reparsing captured output remain limitations.
The test is opt-in; timings are observations, not ordinary CI thresholds.

Command for current same-build private server:
`CARGO_TARGET_DIR=/private/tmp/panelink-codex-wheel-target cargo test --locked --offline --test codex_wheel_latency -- --ignored --nocapture --test-threads=1`

Observed: paced20/20, first104.2ms, last1070.2ms; burst20/20, first126.9ms,
last233.4ms, visible final20. Observation1256.6ms includes a one-second stability
window; last-visible233.4ms is measured directly, not inferred by subtraction.

Same receiver/new client with historical server command adds
`PANELINK_WHEEL_BENCH_SERVER=/Users/sahillamba/.local/bin/panelink.before-0.1.7-20261029-172159`.
That0.1.6 backup has no scoped capability: paced20/20 first111.8ms/last1248.9ms;
burst20/20 first106.0ms/last588.9ms, visible final20. Different server builds and
coarse polling preclude attributing all timing difference to RPC reduction or
claiming first-response improvement. No native host or Codex rendering measured.

Context-only benchmark now measures1 and3 durable pairs per logical report on
fresh private contexts and includes whole-report distributions. During broader
checks the one-pair20/100 cases observed146.03/148.36 reports/sec (100-report
median6.294ms, p958.435ms). This excludes transport/server/child/render overhead.
Log: `/private/tmp/scoped-wheel-context-comparison.log`. It is not an end-to-end
rate. Prior three-pair measurements around52-54 reports/sec remain in the linked
[original transaction report](CODEX-WHEEL-TRANSACTION-COST-2026-10-29.md).

## Live observations and activation boundary

Read-only discovery recognized pane2 as Codex. A temporary helper using existing
public authenticated transport and only READ capability queried its exact
HistoryQuery target and printed flags only: alternate_screen=true,
mouse_reporting=true. It collected no persisted screen/transcript or input,
claimed no owner, and detached its temporary read connection. This confirms
relevance to that discovered pane, not the user's visible window association or
scroll acceptance. Installed client remains0.1.7 and server0.1.4; no restart/input.

New-server activation is separate because restarting terminates current panes.
A new client alone cannot enable scoped delivery on that old running server.

## Broad verification

Canonical non-UI verifier is running to
`/private/tmp/panelink-scoped-wheel-verification.log`. Final result and independent
review will be recorded here before activation is requested.

## Final verification, review and installation

Canonical runner exited0, all8 checks passed. Root Rust1595passed/0failed/20ignored;
decoder39/0; component sources match; fmt/strictClippy/build pass; Python233run
with1skip; docs412files/0findings. Explicit private receiver and durable context
measurements were run separately despite being ignored in ordinary CI. Log:
`/private/tmp/panelink-scoped-wheel-verification.log`.
[Astra review](SCOPED-WHEEL-REVIEW-2026-10-29.md) found no material actionable
correctness/safety regressions; this is bounded source review, not host acceptance.

Installed standalone `/Users/sahillamba/.local/bin/panelink` reports0.1.8,
revision`daf28a63f062`, dirty fingerprint`f565cab6c33f3b53`, SHA256
`0de732e355163a6f1e8a2b6a0c308389542545d93cc55025d1efcd16e244a254`.
Previous0.1.7 backup: `~/.local/bin/panelink.before-0.1.8-20261029-181325`.
Atomic copy matched the isolated built artifact. --version/build-info verified.
Server-info confirms unchanged0.1.4 generation15642535525154316809, capability
mask511 (no scoped bit). Therefore client installation alone does NOT activate
this performance path. Live restart remains pending explicit authorization
because it ends running panes. No stop/restart, pane input, commit or push.

## Authorized live server activation

User explicitly approved restart with termination of running panes. `panelink
stop` returned workspace stopped, then `panelink recovery relaunch` returned
Applied for both saved pane positions. Fresh shells only; provider conversations
were not resumed and no terminal input was sent. `server-info --json` verifies
client/server0.1.8 share fingerprint`f565cab6c33f3b53`, new server generation
15642535525154316810, selected mask2047 including`scoped_mouse_wheel`1024.
`pane list` verifies two live zsh panes, vacant owners/revision1. Both fresh shells
start in the caller checkout; saved geometry is restored at initial startup size
and will resize when the user attaches. `agent list` shows no recognized running
Codex after restart, as expected. Actual Codex scrolling acceptance still requires
reattaching and reopening/resuming Codex. No prompts, wheel input or provider calls
were sent by this task. No source change, rebuild, commit or push for activation.
