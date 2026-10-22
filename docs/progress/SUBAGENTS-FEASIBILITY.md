# Subagents S0 feasibility attempt — 2026-10-21

Result: **BLOCKED before live session testing**. OpenCode documentation provides
a plausible read-only observation path. Installed-version runtime feasibility
has not passed. This note records execution evidence, not an implemented bridge
or a completed SA/P4 milestone.

## Executable and isolation

- Executable: `/Users/sahillamba/.opencode/bin/opencode`.
- Fresh `--version`: `opencode v2.0.32`.
- SHA-256: `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
- Official tag `v2.0.32` resolves to commit
  `843a6c514e606b436a7fe713422ace0a93ad3d61` via the
  [GitHub tag API](https://api.github.com/repos/anomalyco/opencode/git/ref/tags/v2.0.32).
  Binary-to-source build correspondence was not verified; the tag is not a
  source audit or reproducible-build claim.
- Temporary private fixture: `/private/tmp/panelink-sa-spike-zz7pi_qk`.
  Fresh XDG data/config/cache/state directories and an empty project were used.
  HOME stayed unchanged; the child environment used an allowlist of shell/locale
  basics, without provider credential/config variables. No model prompt was
  sent. No provider configuration was installed or changed.

The first ordinary version/help invocation failed while opening the normal
home log (`EPERM`). The fresh XDG directories allowed CLI preflight to run.
Historical references to another installed version are not current evidence
for this executable; this run explicitly observed 2.0.32.

## Executed checks

| Check | Result | Observation |
| --- | --- | --- |
| `--version` | PASS | Exit0; version2.0.32. |
| `--help`, `acp --help`, `serve --help` | PASS | All exit0; ACP and server commands available. |
| ACP JSON-RPC `initialize`, protocol1 | BLOCKED | Exit1, no stdout/protocol response; private server failed before readiness. |
| `serve --hostname 127.0.0.1 --port 0` | BLOCKED | Exit1 before readiness. |
| `serve --stdio` | BLOCKED | Exit1 before readiness; this mode also attempts a listener. |
| Independent IPv4 bind to `127.0.0.1:0` | BLOCKED | Python socket bind returned errno1, Operation not permitted. |
| Independent Unix socket bind in the writable fixture | BLOCKED | Python socket bind returned errno1, Operation not permitted. |

ACP logs identify its spawned command as `serve --stdio --port 0` and report
`Standalone server exited before reporting readiness`. Server logs use the
generic `Failed to start server. Is port ... in use?` error. Independent bind
results support the environment-permission diagnosis; the generic provider
message does not establish port occupancy. No port guessing or permission
bypass followed those checks. Probe subprocesses exited and were reaped.

Durable evidence:

- [Summary](sa-evidence/bootstrap-summary.json)
- [ACP startup output](sa-evidence/acp-bootstrap.json)
- [Direct server output](sa-evidence/serve-bootstrap.json)
- [Stdio server output](sa-evidence/stdio-bootstrap.json)

The raw startup JSON preserves the observed output; no environment dump or
provider credentials are included. The summary's independent-bind entries
are observations from the separate socket preflight.

## What remains untested

Every required live case was **not run because bootstrap was blocked**:
actual provider-generated child creation; exact ancestry; two-parent isolation;
nested workers; live messages/tool activity; continued observation after the
parent turn ends; missed-event reconciliation; zero duplicate execution; and
wrapper compatibility. No session creation or child launch was observed.
No inference result, latency, or Subagents viewer acceptance is claimed.

## Documentation feasibility assessment

Parent and a separate GPT-6-Luna xhigh read-only reviewer checked the official
V2 docs. These are current documentation capabilities, not proof that this
installed binary or an OpenCode wrapper implements them correctly:

| Need | Candidate interface | Remaining proof |
| --- | --- | --- |
| Discover exact children | Session list supports `parentID` filtering; session records carry parent/project identity. | Show that actual foreground/background subagent invocations expose the expected linked sessions. |
| Read live activity | Experimental per-session event log supports reads after an exclusive aggregate sequence and live following; message reads are also available. | Verify coverage, bounds, ordering and behavior while a child is working. |
| Recover missed activity | Inference: re-list children and resume known child logs from confirmed sequences. | Test listing/log races, pagination, retention and restart/gap behavior on the pinned version. |
| Observe after parent turn | Prefer shared-service/plugin/API observation as a candidate. | Prove worker continuation and observation after parent prompt completion. |
| Avoid extra execution | Read-only observation needs no new prompt or session creation. | Count actual executions before/after observer attachment and reconnect. |

These HTTP surfaces, including the log, are documented as experimental.
The global event stream is volatile and may overflow or miss disconnected
events. The per-session durable log is a more useful recovery candidate, but
does not discover unknown children by itself.
[Official HTTP API](https://dev.opencode.ai/v2/docs/api).

Plugins expose server-event subscription and session reads, making a small
opt-in bridge another candidate transport. Exact scoping, selected root binding
and lifecycle coverage still need testing.
[Official plugin documentation](https://dev.opencode.ai/v2/docs/build/plugins/).

ACP explicitly stops child activity updates after the parent prompt ends,
although permissions/questions can still arrive. It uses a private server and
exits when client stdin closes. Therefore ACP initialization, even if successful,
would not alone prove observation of a separate TUI or continued background
coverage. [Official ACP documentation](https://opencode.ai/v2/docs/cli/acp/).

## Next acceptance run

Run in an execution environment allowing the required local server listeners;
this session cannot grant that permission. First confirm server readiness with
the same recorded executable. Use a scratch project and a deliberately selected
available model/provider, keeping request count and cost bounded; do not choose
an unrequested fallback model or touch ordinary project files.

Then start two parent sessions in that project. Have one invoke a real
background worker that remains active after the parent turn ends. Independently
observe the exact child's messages/activity before and after that boundary,
verify the second parent's workers remain isolated, and reconnect the observer
while work continues. Repeat with foreground and nested workers, checking
exact execution identity and no new prompt/launch caused by observation.
Record unsupported cases rather than weakening the test to manually created
child records. A model-free parentID API test alone is not subagent acceptance.

Keep S0 open until those cases have actual evidence. This bootstrap finding
does not require changing Panelink application code or the queued saved-layout
implementation order. The [phase plan](SUBAGENTS-PLAN.md) remains the design.
