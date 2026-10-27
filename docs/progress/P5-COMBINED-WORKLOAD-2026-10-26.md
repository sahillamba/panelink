# P5 combined OpenCode and shell/build workload — 2026-10-26

## Parent results — current

Two serial release observations completed with **10 idle OpenCode TUIs plus
two shell/build panes**, all 10 composers ready, 60 resource samples, 60
acknowledged OpenCode echo/erase probes and 12 shell acknowledgments per run.
Both runs exited0 and all workload correctness flags passed. Each completed
55 actual fixture builds during its 60-second interval. No prompts or model
calls were requested. Numeric acceptance budgets remain unset; this is not a
performance-budget or full Phase5 pass.

| Observation | OpenCode echo p95 | Echo + erase p95 | Shell ack p95 | Paced lines at interval end | Builds at interval end / after shutdown |
| --- | ---: | ---: | ---: | ---: | ---: |
| [Run6](p5-combined-workload-C-run6.json) | 133.469 ms | 237.911 ms | 18.775 ms | 4,344 | 55 / 55 |
| [Run7](p5-combined-workload-C-run7.json) | 119.883 ms | 228.607 ms | 19.502 ms | 4,304 | 55 / 56 |

One in-flight compiler cycle finished after run7's interval; the separate end
counter preserves that boundary. Producer gates close when the interval ends.
Both reports contain72 acknowledged probes and retain their unmodified raw
resource/input samples. Percentiles use nearest rank on pooled input samples.

| Resource observation | Run6 RSS p50 / sampled max | Run7 RSS p50 / sampled max | Sampled mean CPU run6 / run7 |
| --- | ---: | ---: | ---: |
| Panelink harness | 53.84 / 60.11 MiB | 240.02 / 345.19 MiB | 33.977% / 34.410% |
| Ten OpenCode groups | 1,658.61 / 1,757.30 MiB | 1,600.03 / 1,764.28 MiB | 2.232% / 1.715% |
| Paced output group | 4.89 / 5.00 MiB | 4.86 / 4.98 MiB | 1.165% / 1.270% |
| Build group | 4.86 / 88.66 MiB | 4.84 / 98.17 MiB | 0.238% / 0.170% |

Panelink RSS variability is unresolved. Run6 settled near54MiB; run7 rose
from49MiB at20seconds to338MiB at30seconds and ended near302MiB. Do not select
only the favorable run or call this a stable memory budget. The harness
contains six drivers and their clients in one process, rather than one
deployed server. `/bin/ps` CPU percentages are sampled process-lifetime
estimates, not integrated interval CPU time; 1Hz samples can miss short-lived
compiler/linker work and instantaneous memory peaks. Host scheduling and
background load were uncontrolled.

All12 historical targets were readable, ended and had normal checkpoints;
all10 OpenCode targets also had alternate checkpoints and valid empty normal
row sets. The paced shell retained1024rows/4,997,120bytes with a gap after
retention eviction; build panes retained39/40rows without a gap. Every writer
reported `drained` with no sticky history failure or diagnostic reason.
An explicit gap and no sticky error do not prove complete capture of every
transient frame or establish a history-loss budget.

## Parent corrections and verification

Installed launcher: PATH OpenCode **v2.0.32**. Its default managed background
service attempts the same fixed local port across private HOME/XDG roots.
The private startup logs identified that collision. Parent verified local
CLI help and uses `--standalone` for this exact pinned version, giving each
fixture a private server. No global service configuration was changed.

The first four attempts stopped before measurement (`target_stale`, caused
by the provider exit). The [preliminary run5](p5-combined-workload-preliminary-run5.json)
completed60seconds but exited1 because the new harness incorrectly demanded
positive normal scrollback for idle alternate-screen TUIs. The final guard
requires a present, readable row record and a normal checkpoint; it permits a
valid empty row set and still rejects actual sticky failures. Run5 remains a
failed assertion, not a retrospectively declared pass. Its late total75builds
also included sequential shutdown time; final runs separately record measured
end counts and close output gates.

Independent review's unsafe numeric-PGID cleanup and discarded cleanup-error
findings were corrected with captured PID/kernel-birth/UID checks and explicit
failure reporting/fixture retention. Subsequent review found no new material
cleanup/no-prompt/history issue; it found a leaked version-probe directory and
stale documentation. Parent now removes the probe directory after its process
is reaped, including spawn/exit error paths, and updated this handoff.

Final targeted and all-target strict Clippy, formatting and release example
build pass. Parent independently checked both JSON reports' fixed profile,
counts, monotonic time/output samples, build boundaries, per-target bounds,
history/layout/cleanup consistency and input acknowledgments; it recomputed
resource RSS percentiles and sampled mean CPU from raw samples.

Run6 precedes the version-probe-directory cleanup fix; run7 includes it. Final
source also corrects build-shell input-result labeling. The raw run6/7 input
kind `shell_log` denotes the aggregate shell probe path in those artifacts;
reported shell latency combines both shell panes. Neither post-run metadata
correction changes measured values; no additional full-run pass is invented.

Logs: `/private/tmp/panelink-C-run6.log`, `-run7.log`; blocked/preliminary logs
use `-run1.log` and `-attempt2.log` through `-attempt5.log`. Compile/lint errors
and their corrections are retained under `/private/tmp/panelink-combined-`.
The earlier canonical8/8 verifier predates this example extension; targeted
build/Clippy and the real workload observations are its new executable checks.

OpenCode model calls remain prohibited. No native Terminal/clipboard,
physical reboot, large real-project build or active-agent measurement is
claimed. Input latency is observed through socketpair queries and emulator
snapshots, without the native host renderer. The two examples' A/B topologies
remain available, but padding and paced-shell rate changes mean older A/B
reports must retain their original workload descriptions.

## Original implementation-owner handoff (historical)

This note records a source-only extension to the existing idle real-OpenCode
workload example. It defines Variant C for the remaining representative
combined-output workload while keeping the earlier A/B profiles available.
It does not claim a run, acceptance, a numeric budget, or Phase 5 completion.
The parent must review the example before any real run; executable checks and
runtime measurement remain parent-serialized.

## Workload profiles

- **A** remains five two-pane workspaces, each with one idle real OpenCode TUI
  and one paced shell.
- **B** remains five two-pane workspaces with ten idle real OpenCode TUIs.
- **C** uses six two-pane workspaces. The first five contain ten idle real
  OpenCode TUIs. The sixth contains one ordinary shell running a paced log
  producer and one ordinary shell running a repeated, dependency-free Rust
  fixture build. The build invokes the locally resolved absolute `rustc`
  with `--edition=2021`; it does not use Cargo or network access. Its source
  and executable output remain inside that pane's private fixture.

The shell panes announce readiness before the timed interval. Their workloads
wait for the shared measurement gate, which opens after all ten OpenCode brand
and composer guards and both shell-ready markers are observed. The log producer
writes eight terminal lines per 100 ms pacing interval. The build shell emits
`BUILD_OK N` and atomically advances its build counter only after `rustc`
returns success; a later compiler failure is separately recorded even if an earlier cycle passed. Shell probes continue during output. OpenCode probes type one
printable character and erase it with Backspace; they never press Enter.
Shell probes use the existing `q` plus Enter acknowledgment path.

Every pane receives private 0700 HOME, XDG, temporary and project directories.
No authentication or existing config is copied. Launcher selection follows
PATH `opencode`, `~/.opencode/bin/opencode`, then `tescode workflow custom`;
the chosen launcher and its version are included in the report. OpenCode is
idle: prompt submissions and requested model calls are both zero.

## Geometry, history, and resource evidence

The root is 53 rows by 102 columns, producing 24 by 100 content cells after
pane padding. The split launch size comes from `planned_split_size` applied to
a decoded layout reply. The example retains the socket-pair client and
`MultiServerDriver` execution path; no runtime quota, timeout, or application
behavior was changed.

Schema 2 reports dynamic workspace and pane counts, static profile metadata,
per-second active paced-line/build counters, and resource summaries separated
into the Panelink process, OpenCode child groups, shell-output groups, and
shell/build groups. Each workspace reports saved target references, retained
row count/bytes, gap, ended state/time, normal/alternate screen presence,
capture-disabled state, sanitized fixed history reason labels, writer drain
outcome, layout matching, and process-group cleanup. History is inspected only
after driver/writer settlement while holding the existing maintenance lock.
Sticky history failures remain explicit correctness failures in the emitted
report; they are not converted to a pass. The report sets
`spec_r03_complete` to false and `acceptance_budget_status` to `unset`.

The fixture is removed only after all test-owned process groups are confirmed
stopped. On an early or uncertain failure, the private fixture is retained for
inspection rather than being removed beneath a possibly live child.

## Initial pending parent review (superseded above)

Parent formatting, targeted strict Clippy and isolated release build passed.
The first C invocation used PATH OpenCode v2.0.32 and created all six private
workspaces, but stopped at `startup screen query failed` before opening the
measurement gate. Its blocked record is
`/private/tmp/panelink-opencode-workload-C-run1.json`; stderr is
`/private/tmp/panelink-C-run1.log`. It is not a 60-second measurement or pass.

Parent removed the hardcoded per-probe output-overlap claim: schema2 now says
`probe_output_overlap_instrumented: false`. Counter samples establish output
progress over the interval, without independently correlated per-probe
timestamps. The original directory-uniqueness check is named
`private_pane_directories_distinct`; it does not prove content isolation.
Sample timestamps are correctly labeled `elapsed_ms`.

Independent review found that numeric group IDs need captured kernel birth
identity before signalling, and error-path cleanup outcomes must be reported.
A bounded implementation follow-up owns only the example to fix those issues,
add fixed per-pane startup diagnostics, and read the actual sticky history
failure signal after driver shutdown rather than infer failure from any
diagnostic reason. No second provider attempt runs before those fixes are
reviewed and compiled.

No Cargo, build, test, provider, model, manual UI, clipboard, or performance run
was performed by the implementation owner. No timing, resource, row-retention,
build-cycle, or budget result is available yet.

After parent source review, the exact proposed C run is:

```sh
CARGO_TARGET_DIR=/private/tmp/panelink-phase-acceptance-target \
  cargo run --locked --offline --release \
  --example opencode_ten_agent_workload -- \
  --variant C --run 1 \
  --output /private/tmp/panelink-opencode-workload-C-run1.json
```

This is a proposed command, not evidence that the example builds or runs.
Run it only in the serialized parent verification slot. Preserve any failed
attempt and its report; do not infer a performance-budget pass from workload
completion or report generation. OpenCode model calls remain prohibited for
this workload.
