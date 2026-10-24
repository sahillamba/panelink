# Phase 4 paused handoff — 2026-10-24

Status: **PAUSED by explicit user request**: “pause and handoff at a logic step
ideally before you hit limit”. This supersedes the earlier instruction to keep
slices going until completion. Do not resume implementation or probes until the
user resumes. Phase 4 is not complete. The user subsequently requested a
checkpoint commit of the accumulated work; this checkpoint is not phase
completion. Nothing is pushed.

## Checkout and safety

- Checkout: `/Users/sahillamba/.codex/worktrees/7060/panelink`.
- Branch: `codex/p3-scrollback-integration`; pre-checkpoint HEAD `8ad9bd6`.
- This checkpoint preserves the accumulated Phase3 usability prerequisites,
  Phase4 implementation, tests and evidence together. Phase3 human host
  acceptance and the remaining Phase4 UI gates stay open. No unmerged index
  entries were present before staging.
- All active workers were stopped at pause. Historical agents are not active
  assignments. Resume suitable bounded exclusive work with GPT-6 Luna High,
  parent review and verification.
- Build only with `CARGO_HOME=/private/tmp/panelink-p4-cargo-home` and
  `CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.
- Active `target/debug/panelink` remains SHA-256
  `3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
  Do not overwrite, activate or restart the user's running workspace.
- Pinned native OpenCode2.0.32 SHA-256:
  `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
  Launcher used is native opencode, not Tesco wrapper. Keep the documented
  PATH → ~/.opencode/bin/opencode → tescode discovery fallback.
- No dependencies/lockfile changes; ms-rust remains uninstalled.
- Codebase-memory requests failed with `Transport closed`; direct source reads
  were used. Try the required graph tools first on resume.

## Verified implementation

The opt-in `serve --opencode-bridge` path provisions the original native TUI
plugin through a private per-launch config, scoped one-use grant and same-user
Unix transport. Source snapshots are challenge-bound, requested every500ms,
with300ms IO deadlines and3000ms receipt lease. Exact managed target death,
source failure, expiry and storage ambiguity revoke eligibility. The PTY owner
never waits on provider/storage work. Cooperative exec gates protect new
listener/accepted descriptors; endpoint Drop detaches instead of joining.

The binding writer now saves a single atomic joint managed/provider record.
Reopened data is stale/historical. `provider-binding show` is exact-target
historical inspection after the writer releases its lifetime lock; it does not
work as a concurrent live inspector. `status` is the live explicit-operation
surface. The fixture's bounded own-record decoder is test-only, not a new public
inspection API.

`provider-binding prepare` validates the exact top-level ID/project using the
pinned native SDK and durably saves a separate scoped Prepared choice.
A separate `resume` revalidates, durably marks its ticket Spent, issues at most
one public-router action, then waits for a later fresh selected snapshot and
joint record save. Prepared is not selected; lost acknowledgments remain
uncertain. A spent operation is never replayed. One choice/attempt per launch.
The spec now explicitly distinguishes prepared candidate from selected binding.

Missing/deleted native SDK failures are classified only by the exact generated
SessionNotFoundError or a structured SDK response404; arbitrary error status
fields/timeouts remain unverified. Root parentID may be absent/null; linked
children are refused. Fork roots have a separate fork-origin link.

## Latest verification and evidence

- **Final full serial Rust:1231 passed,0 failed,15 ignored**,63 result groups,
  exit0. `/private/tmp/panelink-p4-20261024/full-freeze-rust.log`.
- Strict all-target Clippy passed after final Rust source/test changes.
- Earlier seven supporting checks all passed: decoder39, source fingerprint,
  fmt, all-target Clippy, isolated build, Python196(one intentional opt-in live
  skip), docs316/0. This supporting snapshot predates the newest probe/test/docs
  additions; rerun supporting checks after remaining work/source freeze.
- An intermediate full run had1230pass/1fail/15ignored at the existing
  `w2_server_shutdown` HUP marker. The fixture used external touch/sleep in
  shell traps. It now uses builtin printf/timed-read with unchanged peer-window,
  reaping and duration assertions. Five focused passes plus the final full pass
  are recorded in [shutdown evidence](P4-SHUTDOWN-REGRESSION.md).
- At the user-requested checkpoint, the latest full Python suite passes:
  200 tests, one intentional opt-in live skip,14.130s; log
  `/private/tmp/panelink-p4-20261024/checkpoint-python.log`. Docs319/0 and
  diff whitespace checks pass. Final probe files compile with py_compile.
- SIGKILL/reopen joint-record and Prepared/Spent tests:3pass. These establish
  process-kill boundaries, not power-loss durability.
- Endpoint6pass, manual actual IPC/disk2pass, runtime7pass. The initial runtime
  confirmation test deliberately passes a caller clock sampled before worker
  publication, guarding the dequeue/receipt race.
- Native manual recovery: Luna **and independent parent** pass with embedded
  binary SHA `1497ac732020573d633df85d38fdd594a92b31d7f1b8ee29d06e0087e52ec9ee`.
  Missing/deleted, foreign project, linked child, Prepared/source preservation,
  explicit resume, fresh selected save, duplicate refusal and exact historical
  linkage pass. A second managed target accepts/resumes a real nonempty
  imported-history fork after history digest/top-level/origin/boundary checks.
  See [native recovery](P4-NATIVE-MANUAL-RECOVERY.md).
- Native bridge failure: Luna **and independent parent** pass with the same
  binary. Verified direct child PID + canonical pinned image before SIGSTOP;
  owner unavailable, PTY/serve alive, A remainsrev1; prepare refused before/after
  SIGCONT, no implicit regrant; historicalA retained. All cleanup/no-prompt/
  no-user-message gates pass. See [native failure](P4-NATIVE-BRIDGE-FAILURE.md).
- Parent reports are `/private/tmp/panelink-p4-20261024/native-manual-parent-final.json`
  and `native-failure-parent-final.json`. Both classify proven and cleanup true.

## Exact stopping point / next slice

**Normal in-TUI transitions through the actual managed adapter remain open.**
Do not count the earlier managed probe's apparent A1→B2→A3 as safe keyboard
acceptance. Its generic “sessions” substring guard matched background text,
not a verified modal. The stricter current probe withholds title and Enter.
Standalone native new/fork/project acceptance and explicit manual recovery are
separate evidence from this remaining managed-UI gate.

Root cause found immediately before pause:

1. Pinned2.0.32's proven session-list shortcut is **Ctrl-X then l**; Home/new
   shortcut is Ctrl-X then n. See `probe_opencode_v2_tui_identity.py:796`.
   Bare Ctrl-O/P were wrong for the session-list diagnostic. Ctrl-O is used
   separately for project switching by the remaining-identity fixture.
2. Panelink attach reserves Ctrl-X to detach. Sending the native leader through
   the outer PTY therefore exits attach. Raw shell checks already proved normal
   Ctrl-O byte15 and complete title bytes reach the child, so this is not a
   general forwarding failure.
3. The existing `control keys TARGET OWNER_REV TEXT` helper accepts printable
   characters/Enter and rejects control characters. Do not assume it can inject
   a native leader. Schema8 internally supports typed Control(u8).

Recommended bounded next assignment after user resumes: establish a safe native
session-list route in a **private disposable fixture**, keeping shipped behavior
unchanged. Candidate A: verify pinned public `tui.json` / keybind configuration
and map session_list to a noncolliding shortcut only in fixture-owned config.
Candidate B: a typed raw UI-protocol fixture helper sends Control-X+l after a
fresh bounded read and explicit exact-target claim/owner revision. Label it as
server-injected native input, not physical attach keyboard acceptance. Do not
blindly extend the product CLI or change the user's OpenCode config/keymap.
The exact private-config API still needs verification before use.

Then require actual current-modal heading/rows, unique exact target before
Enter, and exact live binding revisions A→B→A while passive B stays unchanged.
Add native Home/new/fork and project transitions through the managed source,
including visible foreign-scope refusal/manual fallback. Reuse the standalone
nonempty-fork helpers; no prompt/model call. Do not replace these gates with
an explicit manual-resume-only proof or weaken screen guards to obtain a pass.

After those pass: parent review, full supporting checks (and Rust again only
if source changes), reconcile ROADMAP/SPEC/SESSION and component evidence, then
commit fully complete phase code plus required dependencies/documentation.
No push. Preserve the separate Phase3 human four-pane Apple Terminal gate and
Phase5 reboot/history/layout gates. Current-at-crash and automatic resume remain
explicitly disabled; do not invent a provider selection/durability barrier.
