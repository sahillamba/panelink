# Managed pane run implementation and verification

2026-10-23. Scoped implementation on `codex/p3-scrollback-integration`,
based on `9f3d8a0` plus the previously verified uncommitted live-scrollback
integration. Full Phase3 remains open; no commit or push.

## Delivered behavior

`panelink pane run [--pane N] [--direction left|right|up|down] [--cwd DIR]
[--as NAME] [--json] -- COMMAND [ARG ...]` launches one new managed process
beside an exact captured target. Defaults are spatial pane1/right; relative
cwd resolves from the invoking CLI. Its environment/PATH and raw Unix argv
are explicit inputs; tokens after -- remain literal. No implicit shell or
existing-pane input, UI-focus change, default-workspace auto-start or provider
conversation resume. Four-pane/geometry limits remain.

This is an agent client for existing schema7 split + embedded schema6 launch,
using the existing serialized prepare/journal/spawn worker. No server route,
wire/storage format, dependency, manifest or vendor changes. Existing INPUT
negotiation and exact-grant target checks apply; the operation does not claim
input ownership. Live source paths are never synthesized from stale launch
data. The original uncommitted scrollback code/tests were hash-checked unchanged.

The per-caller/per-full-anchor private run context reserves its operation key
before exactly one send. Server exact-byte deduplication stays authoritative.
Lost or mismatched replies leave uncertainty and cannot produce an automatic
replacement request. A later invocation may reconcile but stops before any
new launch in that invocation. Local bounds and complete encoder validation
precede reservation; refusal never truncates argv or environment. Context and
journal persist identity/status only; no argv/environment values.

Applied returns the exact new target and layout revision, not a permanent
pane index, readiness or task success. Partial/Unknown may retain children.
Post-reservation errors retain the exact operation and unknown outcome; null
target/zero revision mean unobserved. If a valid reply arrived but local ack
failed, its observed child target/revision stay visible while uncertainty
remains. Pre-reservation errors use the existing generic CLI error shape.

## Review and corrections

Three native GPT-6-Luna High workers owned exclusive transport, parser and
acceptance files; parent owned the accepted contract, dispatch/help, docs,
review, stronger tests and final verification. No external OpenCode launcher
was used. Implementation ownership is released; no agent must survive resume.
The requested ms-rust skill remains absent; no guideline compliance claim.

Parent strengthened acceptance so invalid executable/cwd cases exercise a
non-full workspace, quick-exit capture waits for actual output, every split
keeps its sibling live, relative cwd is checked, and fixture teardown requests
workspace stop before forced fallback. Parent added exact uncertainty-reopen
and oversized-argv/no-send regressions.

Independent review found two output gaps: errors after key reservation lost
the key, then valid reply metadata was discarded when local ack failed. Typed
RunPaneFailure now carries only fixed error/operation/optional observed result
metadata. Only observed target/revision are carried in the error, keeping its
size below Clippy's large-error threshold without heap allocation. A
deterministic fake server removes the reserved context pathname
before replying; the local ack fails and the test proves key, child target and
revision survive without clearing uncertainty. Pure JSON tests prove the
corresponding unknown envelope. [Independent review](P3-PANE-RUN-REVIEW.md)
confirms both findings resolved; [accepted task](P3-PANE-RUN-TASK.md) records
the final contract.

Focused parent tests on corrected source pass: transport8/8, parser/JSON9/9.
The real CLI target passed3/3 before the final ack-metadata correction, including
all four directions, invocation-only PATH/env, literal flag tokens/nonUTF8 argv,
relative cwd, fast exit, sibling survival, read-only/capacity refusal, pre-spawn
failures and absence of argument/environment sentinels in state files. Final
frozen-source serial Cargo passed **1084/0/15ignored** in148.24s: library712,
binary140, integration232 across51 targets, plus empty doctests. The real
CLI target passed3/3 in that final run. All seven non-Cargo checks pass
separately: decoder39, Python77, fingerprints, fmt, strict all-target Clippy,
offline build and docs256/0. No source edits followed that run; docs and
`git diff --check` were rechecked after final evidence updates. No Cargo
timeout occurred, and ignored tests remain ignored. Scoped pane-run delivery
is complete; broader Phase3 is not.

Intermediate API changes produced temporary binary-test type mismatches,
corrected before the passing focused runs. An initial fmt check found one
extra blank line in the transport addition; corrected. Prior seven non-Cargo
checks passed before the final ack-metadata correction; they are not final
source evidence. The pre-ack-metadata full serial variant passed1082/0/15ignored
in142.43s. The subsequent non-Cargo run passed6/7: Clippy refused the176-byte
error containing the complete reply. Carrying only validated target/revision
corrected it; strict all-target Clippy and focused8/9 tests then pass. The
pre-compaction full suite also passed1084/0/15ignored in142.95s; this is
separate evidence for the larger error representation. These
intermediate executions remain distinct from the final frozen-source sweep.

## Evidence boundaries and next work

Fake-server lost acknowledgments, context-pathname removal and private automated
PTYs are bounded software checks. They do not establish physical power-loss
durability, human terminal compatibility, general minimum macOS support,
provider identity, an active-agent load budget or full Phase3. No argv/env
persistence does not prevent a child from deliberately printing secrets into
its own retained terminal history.

Next: review richer owner identity for agent discovery under the existing
lease/target authority; wider lifecycle/host gates remain open. Phase4 provider
identity and Phase5 hierarchy/selection/reboot remain separate.

## Verification commands and retained local logs

Full Cargo: `cargo test --locked --offline -- --test-threads=1`, through
`scripts/verify_non_ui.py`'s process-group runner with an explicit600-second
deadline. Non-Cargo checks use that script's seven other commands unchanged
with their existing180-second per-command deadlines. No claim that the
standard default-parallel verifier ran8/8.

- Final frozen-source Cargo: `/private/tmp/panelink-pane-run-cargo-complete.log`.
- Final seven other checks: `/private/tmp/panelink-pane-run-noncargo-complete.log`.
- Focused transport and parser/JSON: `/private/tmp/panelink-pane-run-transport-compact.log`
  and `/private/tmp/panelink-pane-run-parser-compact.log`.
- Real CLI strengthened acceptance: `/private/tmp/panelink-pane-run-cli-parent-final.log`
  (the same target is included in the final full suite).
- Pre-ack-metadata full variant: `/private/tmp/panelink-pane-run-cargo-final.log`.
- Pre-compaction full variant: `/private/tmp/panelink-pane-run-cargo-reviewed.log`.
- Clippy large-error failure: `/private/tmp/panelink-pane-run-noncargo-reviewed.log`;
  corrected strict Clippy: `/private/tmp/panelink-pane-run-clippy-compact.log`.

These temporary logs support the durable results above; no worker-only result
is promoted to full-suite evidence. Graph refreshed fast/nonpersistent after
final source freeze as `Users-sahillamba-.codex-worktrees-7060-panelink`.
