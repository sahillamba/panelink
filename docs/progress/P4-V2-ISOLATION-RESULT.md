# Phase 4 linked-child and project isolation — 2026-10-24

Status: **both scoped development-fixture slices pass 3/3 fresh live runs**.
Two GPT-6 Luna High owners implemented exclusive helper/test/note files; the
parent integrated, reviewed, corrected and independently verified the results.
This extends the [prior evidence](P4-V2-EXTENDED-RESULT.md). Full Phase 4,
production bridge integration and durable recovery remain open.

## Slice 1: selected linked child and return to parent

The owned server creates a linked child through its documented `parentID`
API without a prompt. A third private TUI selects that exact child and reports
`session_top_level=false`; it never substitutes the parent's identity. A bounded
top-level listing excludes the child and contains the original two roots.
Keyboard selection then returns the third TUI to the exact top-level parent.
Both original observer streams advance beyond hook-entry watermarks, and all
new rows preserve their original identities. Cleanup and final empty-message
reads must succeed before the case remains proven.

An actual fork request on the empty parent returns HTTP400 in every final run.
The exact pinned executable documents a copied-history fork and maps its
empty-history error to `empty_session`; the HTTP helper does not retain error
bodies, so that error kind is source evidence rather than an observed response
claim. The passing selection case uses **linked-child creation**, not copied
conversation history. A nonempty copied-history fork remains untested.

## Slice 2: separate-project identity isolation

The owned server creates an empty root session in a second private directory.
A third TUI selects it with exact session and canonical directory hashes.
Applying those observations to the first directory's scope fails closed.
The two original streams advance and preserve their identities, including the
post-cleanup check. The extra session's user-message reads are empty before
and after its TUI cleanup. This proves separate-client directory/project scope;
it does not prove a project switch inside one existing client or separate Git
repository identities.

Both cases reject invalid/capped observations, wrong scope and missing fresh
rows. Unknown startup cache fields cannot establish identity; after identity
is established, unknown/Home/child or foreign transitions fail the applicable
identity gate. Every authenticated helper request checks exact listener
ownership first. Additional clients drain alongside both originals under the
existing byte caps and bounded deadlines.

## Frozen live evidence

Combined receipt:
`/private/tmp/panelink-p4-20261024/v2-isolation-final-repetitions.json`.
All six runs exit0 with the exact native OpenCode2.0.32 SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
Launcher: `opencode` on PATH. Each run has fresh private HOME/XDG/projects,
fixture-only authentication and enforced loopback-only sandboxing. Original
sessions and the added session have successful empty user-message reads.
All three TUI groups/PTYs and the owned server receive cleanup receipts.
Reports contain no raw IDs, paths, credentials, bodies or terminal text.

| Run | Child wall ms | Child case ms | Project wall ms |
| --- | ---: | ---: | ---: |
| 1 | 12890.428 | 4677.675 | 11518.108 |
| 2 | 13163.448 | 5041.584 | 11360.313 |
| 3 | 12439.698 | 4521.607 | 11349.747 |

Every run also passes the original A-B-A gate with zero missed, extra or
rejected observations. The child case reports linked creation, child selected,
non-top-level status, parent return and original observers unchanged. The
project case reports exact second-directory selection, foreign-scope rejection
and original observers fresh. No prompts/model work, user accounts, installed
configuration, live Panelink panes or launchers are changed.

## Parent review and failed diagnostics

Review corrected missing original-TUI draining, observation watermarks,
ignored rejected records, warm-up handling, ownership checks before later
requests, malformed listing metadata and failures after provisional success.
The child helper now receives an explicit pinned version from the bootstrap.
Late message-read or cleanup failures revoke proof. Behavioral tests cover
these failure paths and the full mocked lifecycles.

Two child diagnostics failed before selection at the top-level list check:
`v2-isolation-child-diagnostic.json` (7795.709ms) and
`v2-isolation-child-list-diagnostic.json` (8306.990ms). The latter showed
HTTP200 with zero rows. The helper had canonicalized the directory in the API
query, while the provider stored its original path spelling. Queries now retain
the fixture's original spelling; identity comparisons still use canonical
paths. A symlink-alias lifecycle regression covers this distinction. The
corrected diagnostic passed in12921.034ms before the final repetitions.
The project diagnostic passed in10969.614ms. All diagnostic receipts are in
the same private temporary evidence directory; failed runs remain failed.

## Verification and next step

Parent full Python discovery: **160 passed, no failures/skips**, in8.269s.
The new child/project focused suites pass8/8 and7/7 respectively. All eight
changed Python modules compile. Tracked and explicit untracked whitespace
checks pass. No Rust/build input changed; the previous separate Rust baseline
remains1111 passed/0 failed/15 ignored and was not rerun for this tooling work.
Documentation check:291 Markdown files, zero findings.
An initial restricted discovery ran147 tests with one failure because the
sandbox refused the existing loopback-listener bind. The final permitted run
above exercised that check successfully; the restricted failure is not a pass.

Next: version the opt-in bridge contract and connect supported top-level
observations to the existing binding core, with source-instance ordering,
heartbeat/expiry and explicit reconciliation. Then implement the durable
last-confirmed store/acknowledgment boundary and pre/post-ack crash tests using
the existing codec. Unknown/Home, child and unsupported version/project states
must remain ineligible. New-conversation top-level ID, nonempty fork and
same-client project-switch gates remain open; no latest-session heuristic,
automatic provider resume or current-at-crash claim is enabled. The separate
Phase 3 human host gate remains open. No commit or push.
