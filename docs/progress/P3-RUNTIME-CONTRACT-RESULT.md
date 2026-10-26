# Phase 3 runtime contract reconciliation — 2026-10-23

The broad event/watch/launch/layout implementation checklist is superseded by
the delivered bounded contracts. Parent review and two independent Luna High
source/document audits found no missing implementation requirement in that
accepted scope. See the [contract matrix](P3-RUNTIME-CONTRACT-AUDIT.md) and
[independent review](P3-RUNTIME-SCOPE-REVIEW.md).

Finite pane events/reconciliation, workspace invalidations and atomic
snapshot/cursor gap recovery, four-pane split/close/resize lifecycle, and
explicit managed pane run have current source paths and recorded acceptance.
The historical standalone two-session/direct-launch proposal was not adopted;
direct schema6 launch remains disabled while pane run embeds launch in schema7
split. Indefinite subscriptions and large Snapshot queues are future scope,
not unfinished obligations of the bounded implementation. No new runtime
feature, authority, protocol, dependency or product decision was introduced.
Spec revision0.24 and agent-control notes now distinguish current scoped
behavior from older one-child/proposal text. The roadmap replaces the stale
umbrella checkbox with the actual delivered scope; fullPhase3 stays open.

## Actual acceptance performed

The audit found one historical pending acceptance that could be run now: the
expanded pathname CLI fixture. Parent explicitly ran:

```sh
cargo test --locked --offline --test p3_pathname pathname_server_cli_context_reuse -- --exact --ignored --test-threads=1
```

**1 passed/0 failed/0 ignored**,1filtered out, in**1.20s**. The original ignore
attribute remains because restricted environments may deny pathname sockets.
This is separate from the previously recorded standard-suite1088pass/15ignored;
do not add these as a new combined total. No full suite was rerun for this
documentation/evidence-only slice. Source/test/script hashes match the snapshot
at this turn's start; no executable behavior changed.

The fixture checks private socket permissions/singleton refusal, reusable
context and exact full target/owner, atomic screen/cursor reconciliation,
changed metadata watch delivery, explicit empty timeout, observation preserving
context bytes, same-child detach/reconnect, exactly-once token handling and
bounded explicit server shutdown/socket cleanup. See
[pathname acceptance](P3-PATHNAME.md); raw local log is
`/private/tmp/panelink-p3-contract-pathname.log`. This supplies direct current
CLI evidence beyond the older operator-only record.

Final validation:268 Markdown files/0 findings; `git diff --check` passes.
Source/test/script SHA-256 checks all match the turn-start snapshot. All other older test failures and ignored cases retain
their recorded status, including ACK42/history-writer intermittency.

## Remaining scope and next slice

Human four-pane Apple Terminal acceptance remains distinct from automated
proxy evidence; no manual chores are assigned here. In-flight client death
coverage is still narrower than post-ready SIGKILL/socket-loss evidence.
Provider identity, physical reboot, arbitrary descendant cleanup, broader
hierarchy and large-subscription/resource budgets keep their separate scope
and evidence limits. No phase-completion commit is authorized by closing this
implementation checkbox alone.

Next specify a deterministic in-flight input-client death regression against
the existing admission/status/nonreplay contract. First identify an observable
admission/effect barrier and exact owned process boundary; do not infer outcome
from timing, replay input, or turn transport loss into task success. This is a
bounded acceptance task, not a new input/retry policy.

Native GPT-6-Luna High workers owned disjoint audit/review notes; parent owned
scope reconciliation, shared documentation and the actual permitted CLI run.
All ownership is released. Native collaboration was the launcher; ms-rust
remains unavailable. Preserve all accumulated uncommitted work on
codex/p3-scrollback-integration based on9f3d8a0. No commit or push is claimed;
original source checkout untouched.
