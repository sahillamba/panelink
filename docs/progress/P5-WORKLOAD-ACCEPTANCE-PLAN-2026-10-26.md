# Combined workload evaluation profile — 2026-10-26

## User choice

The user selected **idle OpenCode plus shell/build output, no model calls**
for the remaining representative performance evaluation. Numeric input/UI
latency, resource, capture lag/loss and error limits remain unset. A successful
measurement run does not by itself certify a performance budget or Phase 5.

## Fixed bounded scenario

Extend the existing `opencode_ten_agent_workload` example with variant C:

- Six independent two-pane workspace/storage domains in one harness process.
- Five workspaces contain two real idle OpenCode TUIs each, for ten agents.
- The sixth contains a paced log producer and a repeated real offline Rust
  compiler build of a small dependency-free fixture. Build files stay private.
- Each pane has a private mode-0700 HOME, XDG directories, temp and project;
  no user credentials or configuration are copied.
- Actual PTY content is 24x100, with outer geometry accounting for margins.
- Start the 60-second measured interval only after all ten branded composer
  views and both shell readiness conditions are observed. Gate active output
  to that measured interval so initial startup is not called sustained load.
- OpenCode probes type one printable character and erase it with Backspace.
  Never send Enter or a prompt. Shell probes use the existing acknowledgment
  canary and are reported separately.
- Keep history/layout capture enabled and inspect actual committed store data
  after bounded writer settlement. Record retained rows/bytes, gaps, end and
  checkpoint availability, and sticky history/drain reasons.
- Sample Panelink process resources separately from OpenCode and shell/build
  process groups. The harness includes drivers and client workers, so its
  resource attribution differs from one independently deployed server.
- Stop and confirm only fixture-owned groups before removing private state;
  unknown cleanup outcomes are failures and preserve evidence.

The launcher order is PATH `opencode`, `~/.opencode/bin/opencode`, then
`tescode workflow custom`. The parent preflight resolved the PATH launcher
and observed **opencode v2.0.32**, with a fresh isolated HOME/XDG and no
credentials. The compiler is the pinned Rust 1.99.1 toolchain's absolute
`rustc` path. No package downloads or model invocations are part of the load.

## Report and checks

Variant C must identify ten OpenCode sessions, two shells, six workspaces,
twelve panes, zero prompt submissions and zero requested model calls. Report
measurement duration, readiness, build cycles, input samples and resources,
actual history/layout observations and cleanup. Preserve schema-1 historical
A/B reports; updated schema-2 reports must not retain the obsolete two-pane
product-limit claim. The runtime currently supports four panes per workspace;
this experiment deliberately uses two.

Before running providers, parent reviews the harness and compiles it offline
with strict Clippy. Two serial release observations are intended if the first
one completes correctly. Preserve failures, blocked readiness and history
loss. Do not weaken composer guards, retry uncertain input, increase memory
quotas or invent a budget pass to complete a run.

Implementation and two serial release observations are recorded in
[the combined workload evidence](P5-COMBINED-WORKLOAD-2026-10-26.md).
They pass their workload correctness checks; numeric acceptance budgets and
the observed RSS variability remain unresolved. Sample counters establish
output progress over the interval; individual probes do not carry independent
output-overlap timestamps. This limitation is explicit in the reports.
