# Agent feedback corrections — 2026-10-27

Scope: implement the five important improvements selected from the user's
`feedback from andgent.md` in the separate root checkout. Work occurs in
`codex/main-local-changes-20261027`, based on `10d81c24cdc2`, preserving all
previous dirty work. Package version is 0.1.4. No commit or push is requested.

## Resulting contracts

- Submit uses bracketed paste when the child has enabled that mode. Text,
  closing marker and one Enter form one bounded admission and nonreplayable
  operation. Plain text typing and non-bracketed children retain their existing
  semantics. PTY Applied is still distinct from application acceptance;
  scoped receipts expose `application_submission:unverified`.
- Finite recognized-agent waits retain one authenticated connection/context
  rather than performing a new handshake for every 50 ms observation. Every query
  still checks process identity; failures are reported without automatic
  reconnect or input replay.
- Connection and mutation diagnostics expose fixed cause, logical stage and
  transport-stage labels. Pending keys and reconciliation status carry no
  input payload, credentials or paths. Input failures include available
  before-operation ownership observations, explicitly unverified as current
  authority. No hidden retry, takeover, identity switch or input replay follows.
- `pane read` and `agent read` support `--bottom --lines K`. The default is still
  top-oriented. Bottom selects physical visible rows including blanks from one
  complete bounded snapshot. A byte-truncated prefix fails as
  `bottom_read_truncated`; scrollback and bottom are mutually exclusive.
- Codex 0.164.0's observed dim `Ask Codex to do anything` placeholder and visible
  cursor identify its empty composer. Bare markers require the input cursor
  and blank rows below; a marker followed by multiline draft content is unknown.
  Typed placeholder text is unknown. Activity/approval markers retain priority
  across the bounded screen. Unknown remains a refusal to guarded prompt;
  neither Idle nor a screen marker proves task completion.
- `build-info --json` reports client package/revision, source-dirty state and
  FNV-1a source fingerprint. The fingerprint includes application/vendored
  sources, manifests/lockfile and embedded assets; it is not cryptographic or
  reproducible-build evidence. Actual worktree Git HEAD/index/ref paths and
  source directories trigger rebuilding metadata. Archive builds hash their
  source tree and report unknown revision/unverified dirty state.
- `server-info --json` uses a dedicated reusable caller context and an
  authenticated, live, capability-256-gated schema-15 read-only query. Its
  strict bounded response carries server build identity, generation and
  connection-selected capabilities, separately from client identity. Older
  servers omit the optional capability and return identity unsupported without
  receiving the new query. Legacy single-session masks exclude these routes.

## Evidence

The installed Codex 0.164.0 was probed only in disposable private PTYs/homes,
using a dummy credential and a fixture provider at loopback port 9. No model
prompt was submitted and no user pane received input. Raw `/status` plus Enter
remained a draft; in a fresh process bracketed `/status` plus Enter produced the
local status response with zero total tokens. Raw captures are private temporary
artifacts. This establishes the reported submit behavior for that version and
local command; it is not a claim of arbitrary provider/task acceptance.

The upstream [Codex composer state-machine description](https://github.com/openai/codex/blob/531748a080a5772fe3dc55face2261a3cfdea9da/docs/tui-chat-composer.md)
explains the non-bracketed paste-burst Enter suppression path. The private
reproduction, rather than that older document alone, supports the correction.

A complete private Panelink server then recognized real Codex as idle, accepted
one guarded `/status`, released ownership, observed the local response with zero
tokens, returned independent server identity/capabilities and read the bottom
four rows. Initial debug evidence:
`/private/tmp/panelink-feedback-runtime-arosjy2e/result.json`.
The maintained [private probe](../../scripts/probe_codex_agent_feedback.py)
accepts explicit Panelink/Codex executable paths, uses `--no-daemon`, preserves
evidence and stops its own workspace. Its compatibility cleanup uses Codex's
supported daemon stop with its private home if a daemon was created.
All earlier private probe daemons were stopped through their private homes;
remaining updater processes were stopped only after matching their recorded
start time and executable under the corresponding private package directory.
User Codex homes and workspaces were not stopped.

Verification and installed-artifact results are appended below.

## Failure history and review

Initial integration caught an immutable iterator in an intermediate classifier,
a Vec/slice comparison in identity dispatch, and test-only helper/import errors.
These were corrected before final checks. The first real server-identity query
failed local handshake validation because Read appeared in both required and
optional bits; the masks are now disjoint. That private workspace was stopped.

One serial private recognized-agent run failed the wait-timeout output assertion.
The focused rerun passed unchanged in behavior, with diagnostic output added to
that assertion. No timeout guard or assertion was weakened, and no cause is
claimed from that isolated pass.

A final repeated real-Codex fixture exited with explicit `No space left on
device` in its saved terminal checkpoint, after disposable Codex homes had made
separate daemon package copies. Only this task's nine private package trees
were removed, preserving configs/logs/results/checkpoints and user installations;
available space recovered from about 429 MiB to 3.1 GiB. The maintained probe now
uses `--no-daemon` to avoid those copies and background updater lifetime.

The first no-daemon attempt observed startup output changing between the exact
screen observation and guarded submit. The server refused it as stale; no input
was replayed. This exposed overly conservative reporting of a keyed explicit
rejection as possible delivery. A validated exact-key `Rejected` reply now records
`NotAdmitted` and clears only its acknowledged pending key, retaining the current
claim under the existing failed-input contract. A regression distinguishes this
known refusal from a lost acknowledgment/reconciliation failure. The private
probe now establishes idle with a stable observed screen revision for 500 ms
before its single local command. This is a readiness check, not task completion
or permission to retry an uncertain write.

Initial review found that a bare marker could authorize over a multiline draft.
The parent added complete lower-row/cursor checks and regression coverage.
The reviewer also questioned single-session capability exposure; current source
shows that its legacy mask excludes both new bits. Round 2 confirmed that mask
reasoning and found no new material defect in the corrected classifier.

Delegation was bounded to exclusive classifier and build-identity files, plus
read-only submit diagnosis and review. Classifier round 1 was revised after
parent review; round 2 was cancelled in favor of actual captured style evidence
in round 3. Parent integrated, corrected and verified all source changes. Workers
ran no Cargo/build/tests and left commits to the parent. All tasks are terminal.

## Final build and verification

The full library/client serial checks pass: library 907 passed/0 failed/4 ignored;
client 252 passed/0 failed. The same final command passes agent CLI 7/0,
recognized agents 2/0, shared input 2/0 and input PTY 3/0: 1,173 passing tests
in total, with four ignored library tests. This includes multiline-draft/style/cursor recognition,
strict identity codec/capability tests, bottom-row selection and a socketpair
regression distinguishing explicit rejection from lost ACK/reconciliation failure.
Logs: `/private/tmp/panelink-feedback-final-verification.log`. Earlier failed runs
remain separately preserved in the failure history.

Strict lib/binary Clippy, Cargo formatting and whitespace checks pass. Eight
affected Markdown files have zero link/hygiene findings; maintained skill
frontmatter is valid. The optimized locked/offline release build passes. Final Clippy/release logs
are `/private/tmp/panelink-feedback-final-clippy.log` and
`/private/tmp/panelink-feedback-final-release.log`. No
crate was added; build identity uses a standard-library-only build script.

The final optimized artifact passed the private real Codex 0.164.0 `/status`
workflow using `--no-daemon`, with independent server identity, a repeated
context-reusing identity query, one guarded submission, confirmed release,
observable application response, zero tokens and bottom-row reads. No private
Codex daemon was started. Evidence:
`/private/tmp/panelink-feedback-runtime-ks8eijqi/result.json`, log
`/private/tmp/panelink-feedback-installed-acceptance.log`.

Installed `~/.local/bin/panelink` still points to this worktree's optimized
`target/release/panelink`, now 0.1.4. Prior 0.1.3 is retained at
`~/.local/bin/panelink.before-0.1.4-20261027` with its original SHA256
`dd0a9c064d6a77bb0afe996bc73f8fb9d7ddf280475581a685adc6364`.
New source fingerprint: `1edf2c210c2de12d`, revision`10d81c24cdc2`, source dirty.
New binary SHA256:
`878f5e9595675cc7bf6426ed5fc1adf5b6fd4a74677c064c01b2669cbac8b070`.
The installed client's read-only `server-info --json` reports
`server_identity_unsupported` for the existing user server, preserving its
unknown build identity. That server and its panes were not restarted or sent
input. Root checkout feedback/evidence files remain untouched.

A later full command passed both unit targets but failed the first agent CLI
fixture at control init with `transport_failed`; its six sibling scenarios
passed. This is a startup failure, not an input assertion, and is retained in
the log. No cause is claimed and no deadline/assertion was weakened. Another recognized-agent wait failure was isolated to a handshake request
write. Source showed a new connection every 50 ms; the bounded wait now retains
one connection. Its focused recognized/shared/input-PTY rerun passes 2+2+3 tests.
This removes repeated connection-attempt churn by construction; it is not a
claim that every historical transport failure has the same cause. Final serial
verification results and the updated artifact identity are recorded below.

## Limits and activation

This is a bounded feedback correction, not completion of broader roadmap,
provider identity/recovery, Apple Terminal/clipboard, reboot, workload or
asynchronous task-notification gates. No task scheduler or result-notification
API was added. The installed existing server remains its previous process;
server-side corrections require a future explicit restart, which ends its panes.

## Live activation — 2026-10-27

User approved the restart and termination of current pane processes. Installed
0.1.4 stop succeeded, ending two panes in generation15642535525154316808. Default
open chose a fresh shell, advancing to generation15642535525154316809 with one
live zsh shell. The saved layout/input was not replayed. UI displayed v0.1.4.
Live server-info matches the installed client: version0.1.4, revision10d81c24cdc2,
fingerprint1edf2c210c2de12d, selected mask511 including agent/build identity.
Agent discovery succeeds and the plain shell remains correctly unrecognized;
input owner is none while attached. The temporary verification UI was detached
and server-info succeeded afterward. No child input or provider launch occurred.
The earlier pending-activation boundary is now closed for this installed server.
