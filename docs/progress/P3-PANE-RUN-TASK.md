# Managed pane run — accepted bounded implementation contract

2026-10-23. Parent design/review under the user's implementation request.
The existing schema7 split + embedded schema6 launch already carries all
required fields. Reuse that authenticated serialized worker path; do not
enable direct schema6 launches, add dependencies or alter wire/storage formats.
Full Phase3 remains open. Preserve the prior uncommitted scrollback work.

## CLI and scope

`panelink pane run [--pane N] [--direction left|right|up|down] [--cwd DIR]
[--as NAME] [--json] -- COMMAND [ARG ...]`

Defaults: anchor spatial pane1, direction right, invoking process cwd and
environment. This requires an already running default workspace, an INPUT
grant under existing --allow-input policy, available geometry and at most
four panes. It does not require/claim the anchor's input owner or change UI
focus. A new pane/session is created; nothing is typed into an existing shell.
The anchor index resolves once to a full target and layout revision; check
it again before sending. The server validates that exact target/revision.
Never follow a replacement, recompute a new target after failure or retry
under a new key. No empty-workspace/server auto-start.

Parse raw OsString argv before the legacy UTF8 agent dispatcher. All CLI
options occur before the mandatory -- delimiter. After --, every token is
literal command argv (including --as, --json, --cwd and further -- tokens).
Reject missing command, unknown/duplicate options, invalid indices/directions/
caller names, empty executable and excessive arguments before opening contexts.
Use the existing 80-token entrypoint bound and 65,536-byte aggregate argv cap.
Support Unix non-UTF8 executable/arguments/paths without lossy conversion.
Resolve relative --cwd against the invocation's absolute current directory.
No implicit shell expansion. An unqualified command uses the effective client
PATH through existing prepare_launch; an explicit shell -c is shell evaluation.

## Client transport API

Add WorkspaceLink::run_pane(&mut self, target: TargetRef, layout_revision: u64,
direction: SplitDirection, cwd: Option<PathBuf>, executable: &OsStr,
arguments: &[OsString]) -> Result<SplitResult, RunPaneFailure>.
RunPaneFailure contains a fixed code, an optional reserved OperationKey and
optional validated terminal metadata (new target and committed revision).
Pre-reservation failures have no operation; every post-reservation failure
retains that exact key, including after read-only rejection reconciliation.

Use a dedicated per-anchor run context in the existing private caller
directory, separate from read/input contexts. open_for_target reuses this
lease/sequence across short CLI calls and serializes concurrent invocations.
Selected INPUT, matching generation and absence of prior unresolved/discarded/
just-settled uncertainty are preconditions for a new launch.
Capture invoking-client environment with the existing PANELINK_* filtering,
1,024-variable/48KiB limits. Resolve cwd locally as above. Encode the entire
request using the existing split encoder and planned_split_size before
reserving a key; total payload <=65,536 bytes, never truncate. Keep all argv/
environment in volatile memory and drop request payload before output.

Before send, reserve the context's durable key. Send exactly once with a
monotonic request ID. There is no automatic retransmission (the server still
deduplicates exact eligible requests). On transport/bad response/local ack
failure preserve uncertainty; do not ack a reply for another key/target.
Server Error may be reconciled with read-only OperationStatus; it does not
authorize a substitute launch. Validate result key AND existing_target against
the request, and require a terminal outcome before context ack.
A subsequent invocation may settle the old uncertain operation but must stop
without issuing its requested launch on that invocation. Unknown/partial
results must never be represented as no child or as launch readiness.

## Output and acceptance

JSON: ok, action pane_run, outcome applied|failed|partial|unknown, operation
(gen:lease:sequence), existing_target, target (new full five-part ID or null),
layout_revision. No argv, cwd or environment in result/diagnostic strings.
Post-reservation errors also use these JSON fields: outcome unknown (failed
only for authoritative NotAdmitted), exact operation and existing_target,
target null and layout_revision0 when unobserved, plus fixed error/message.
If a valid terminal reply was observed before local ack failure, retain its
new target and committed revision while reporting outcome unknown. Null target
here means identity unobserved, never proof that no child exists.
Pre-reservation/syntax errors use the existing generic CLI error shape.
Exit0 only Applied (launch ownership + journal complete; not task success).
Other outcomes exit1; invalid syntax exit2. Text output gives outcome and new
full target when present. Return the captured result even if the child already
exited; do not re-resolve after successful launch or imply a stable pane index.

Tests: argv delimiter and options/invalid/UTF8 byte bounds; client wire bytes,
exact target and key validation, local refusal before reserve, no resend after
lost ack and no fresh operation on uncertainty reconciliation. Real CLI:
explicit cwd and argv literal boundaries, invocation env/PATH versus stale
server env, sanitized context/journal content, original sibling still alive,
all directions, capacity/pre-spawn failures, readonly refusal and fast exit.
Existing runtime gated split tests cover duplicate/disconnect, retained child
on Unknown, capacity and owner progress; rerun them plus source-specific tests.
Run full locked/offline serial suite and seven other verifier checks separately
with bounded fixture deadlines. Retain failed runs; ignored != passed.

## Ownership

Parent: shared docs/exports, agent_cli dispatch/help, review and verification.
Luna transport: src/p3_cli.rs only (method + focused unit tests).
Luna CLI: new src/agent_cli/pane_run.rs only (parser/action/unit tests).
Luna acceptance: new tests/p3_pane_run.rs only (real CLI fixture/tests).
Workers may not commit or change other files or run broad checks concurrently.
Native collaboration is the launcher; no external provider invocation needed.
