# Panelink implementation roadmap

Code-health maintenance (2026-10-30): user authorized the nine-packet plan in
isolated `refactor/code-health-20261030`, based on `a1134ec`. Parent orchestration
uses GPT-6-Luna for bounded tasks. Fresh repaired baseline passes8/8; packets01–04/06/07 are verified;05 and combined08 remain.
Original checkout and installed runtime are untouched. See the
[execution state](docs/progress/code-health-20261030/PROGRESS.md). Product phase
gates remain unchanged; no push or install is authorized.

Scoped wheel follow-up (2026-10-29): source0.1.8 implements optional capability1024
for one keyed vacancy-scoped wheel operation. Parent codec/handshake/server/client
focused tests pass. Controlled private receiver observes final20-event count at
233.4ms on the new server; first-response improvement is not established. Full
verifier passes8/8 (rootRust1595/0/20ignored) and Astra review has no material
findings. Client/server0.1.8 activated after explicit restart approval; new
generation15642535525154316810 advertises scoped capability1024 (mask2047).
Two fresh shell panes restored; user Codex scrolling acceptance remains open. See
[result and limits](docs/progress/SCOPED-WHEEL-RESULT-2026-10-29.md).

Codex wheel repair (2026-10-29): bounded fresh alternate-screen wheel FIFO and
direct typed delivery are implemented on `t3/improve-codex-mouse-scroll-speed`.
Parent binary317/0, strict all-target Clippy and scoped docs checks pass; corrected
private-PTY fixture now requires and observes paced20/20 and burst20/20. Canonical
verification passes8/8 (Rust1580/0/18ignored; decoder39; Python233run/1skip;
docs407/0). User-authorized client0.1.7 is installed; detach/reattach to load it.
Server unchanged; real Codex/host acceptance remains open. See [plan and evidence](docs/progress/CODEX-WHEEL-2026-10-29.md).

UI/UX planning (2026-10-27): six interview rounds confirm smooth history scrolling,
continuous scrollbar updates, reliable dividers, full tab controls, button-only
confirmations and stronger focus chrome. Four panes; no whole-pane drag, sidebar
or zoom expansion. [Consolidated outcome and eight packets](docs/UI-UX-PLAN.md)
were confirmed by the user on 2026-10-27 with an implementation request; see the
[interview](docs/design/UI-UX-INTERVIEW.md). UX-01..UX-07 are implemented in the
uncommitted working tree of `t3/panelink-ui-ux-plan` (2026-10-28); UX-08
automated verification: full serial Rust 1568/0/17 ignored, strict Clippy, fmt,
build, decoder, fingerprints and docs pass; Python bridge-plugin suite 13 errors
(environmental, also on the unchanged base). Not committed, installed or pushed.
Apple Terminal, real clipboard, touch and UX-01 before/after timing re-measure
remain open; see docs/progress/SESSION.md.

CI repair in isolated checkout (2026-10-27): run40511639904 failed a pane-list
handshake; the reproduced post-accept deadline race now retains one dequeued
connection for the next slice without relaxing timeouts or authentication.
Canonical verifier passes8/8; final transport40/0/2ignored. New hosted
confirmation remains pending.
See [evidence and checkout identity](docs/progress/CI-ACCEPT-DEADLINE-2026-10-27.md).
No product phase gate is changed.

Active user-selected work (2026-10-27): Herdr-style scoped pane input
and recognized-agent list/prompt/wait. See [implementation plan](docs/progress/HERDR-WORKFLOW-IMPLEMENTATION.md). Shared input does not protect an unfinished human draft;
exact-target, per-operation owner and no-replay checks remain.

Local repair (2026-10-27): optimized client 0.1.2 removes eager wheel range probes
and avoids queuing background polls during scrolling. Scoped Codex socket/context
permissions verify discovery and reads. This earlier repair preceded the accepted
shared-input design above. See [current evidence](docs/progress/SESSION.md).

Local install (2026-10-27): client 0.1.1 adds a bottom-right version badge in
existing pane padding. Latest main plus local documentation additions is installed
from `codex/main-local-changes-20261027`; focused compositor/client checks pass.
See [current install evidence](docs/progress/SESSION.md). Phase gates remain as below.

CI baseline (2026-10-26): standard GitHub-hosted Apple Silicon validation
passed on PR #2 head `1060668`; weekly dependency-update proposals are
configured. Required-check activation remains a repository setting; see
[CI.md](docs/CI.md). This does not change the product phase gates below.

```text
current-phase: Phase3/Phase5 resumed; bounded tabs/copy/pane operations automated verified. FullRust1399/0/15ignored, finalclient123/0, outerPTY3/0 and all7support checks. Phase4 scoped completion retained; host/clipboard/reboot/workload gates remain open.
last-updated: 2026-10-27
wide-attach-repair: 2026-10-27 host64x269panic fixed by bounding legacy render viewport; client147/0 and default-openPTY7/0/1ignored, Clippy/fmt/build pass. Existing server untouched; native host retry remains pending. See SESSION.md.
pointer-controls: 2026-10-27 user-selected7060; pane Menu/x, pointer close confirmation, divider drag/menu adjustment, scrollbar click/drag and NAV history implemented. Parentclient146/0, compositor36/0 and workspaceouterPTY4/0; final broad verification recorded in P5-POINTER-CONTROLS-2026-10-27.md. Touch requires host mouse translation; native host/touch/clipboard gates stay open.
scrolling-ux: 2026-10-27 user feedback repair: keep valid content during history fetch, coalesce bounded normal wheel motion, right-padding position indicators; optional extent read best effort. Library880/0/4ignored, binary221/0, history6/0, outerPTY3/0; finalcompose32/0 and strictClippy/fmt pass. See P5-SCROLLING-UX-2026-10-27.md. Host acceptance remains separate.
workload-profile: User selected ten idle OpenCode sessions plus shell/build output, no model calls (2026-10-26). Combined variantC measured twice60sec:10TUIs+2shells,72acknowledgedprobes/run,55measuredbuilds/run, no stickyhistoryfailure, allgroupsstopped. RSS sampledmax60.11/345.19MiB remains variable. Numeric loss/lag/error/latency budgets unset. See P5-COMBINED-WORKLOAD-2026-10-26.md.
last-verified-acceptance: Resume from1060668 on codex/p5-acceptance-continuation. Canonical verifier8/8 (root result totals1402pass/0fail/15ignored; decoder39; Python233run1skip). Final crash-fixture cleanup then separately1/0 and independent review clear. Schema3 live-history benchmark restored for padding,48/48overlap probes; paced failurefalse, burst/shutdown final-payload byte-pressure failuretrue. See P3-P5-RESUME-2026-10-26.md.
last-verified-p4: Parent serial Rust1231pass/0fail/15ignored,63groups; all seven supporting checks pass decoder39/Python232run231pass1opt-in skip/docs332zero/fmt/Clippy/build/fingerprints. Luna+independent parent full managed native pass incl foreign135/postrefusal138/returnA152 and exact A rev7->8, passiveB1; zero prompts/cleanup true. Active binary unchanged.
checkpoint: User-authorized verified Phase3 checkpoint: scrollback, pane run, owner discovery and client/server/killed-input-client acceptance on codex/p3-scrollback-integration based on9f3d8a0; see Git history for checkpoint ID. FullPhase3 remains open; no push. Original source checkout preserved.
last-verified: Pointer-controls final locked/offline serial no-fail-fast Rust1437pass/0fail/15ignored across77resultgroups; finalclient146/0, compositor36/0, workspaceouterPTY4/0. StrictClippy/fmt/debugbuild/fingerprints/docs383zero pass. See P5-POINTER-CONTROLS-2026-10-27.md; host/touch/clipboard/reboot gates remain open.
prior-evidence-pane-operations: Full locked/offline serial Rust1399pass/0fail/15ignored across75groups. After strictClippy payload boxing/lint cleanup, finalclient123/0 and outerPTY3/0 pass; actual4PTYprotocol1/0. All7support checks pass decoder39/fingerprints/fmt/strictClippy/build/Python232run1skip/finaldocs. See P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md. No clipboard or activation.
prior-evidence-orchestration: Serial1091pass/0fail/15ignored in182.72s plus seven supporting checks before user-reported usability repairs; see P3-ORCHESTRATION-RESULT.md.
prior-evidence-scrollback: Final serial Cargo1064pass/0fail/15ignored in122.73s (lib712/binary123/integration229 across50 targets); seven non-Cargo checks passed separately, decoder39/Python77/docs253/0. First full serial run1063pass/1fail/15ignored at shutdown HUP marker; isolated and full rerun pass without code changes. See P3-LIVE-SCROLLBACK-RESULT.md.
prior-evidence-metadata: Final serial Cargo1059pass/0fail/15ignored in152.92s (lib709/binary123/integration227 across50 targets); all7 non-Cargo verifier checks pass separately, decoder39/Python77/docs251/0. Initial parallel verifier7/8 had2 library timing/observation failures; preserved in P3-PANE-METADATA-RESULT.md, not relabeled. No final Cargo timeout.
prior-evidence-upstream: verifier 8/8; full suite 993 passed/0 failed/15 ignored after origin/main merge (095bf45) and four-pane, agent CLI, L16 relaunch, L17 attach, L18 exit status, before stash integration; not combined-tree verification.
prior-evidence-stash: live-split slice library 641 passed/4 ignored, focused integration 73 passed/0 failed, verifier 7/8 with binary 115 passed/1 failed at pathname-bind EPERM; not combined-tree verification. See docs/progress/ASYNC-LAUNCH-RESULT.md.
next-action: Investigate combined-workload RSS variability and set workload loss/lag/error/latency/resource criteria using the two recorded idle10+shell/build observations. Complete Phase3 human four-pane host and Phase5 Apple Terminal/real clipboard/physical reboot/layout recovery gates. Native Terminal control explicitly denied this session. Wide burst benchmark still records final-payload loss. Do not relabel process/PTY automation or measurements as human/reboot or performance-budget acceptance. No push or activation.
history-checkpoint-store: live visible-screen capture integrated 2026-10-20, not full history — see docs/progress/P5-LIVE-CAPTURE.md
history-engine-cursor: local vt100 0.16.2-panelink.2 exposes bounded borrowed normal-history rows, gap and RIS epoch; now consumed by the saved-row writer — see docs/progress/P5-SCROLLBACK-CURSOR.md
history-saved-rows: scoped P5 saved-history slice DONE at f82d968; retry/read acceptance, bounded orphan-temp discard and synthetic throughput delivered in current working tree; 1,024 rows/8 MiB, 30-day ended retention, stopped-workspace clear/cleanup. Full V-HISTORY reboot/layout and representative workload remain open — see docs/progress/P5-SAVED-HISTORY.md
blocked-on: No remaining scoped Phase4 gate. Phase3 human four-pane Apple Terminal proof and Phase5 distinct human host/clipboard/physical reboot/layout recovery/workload gates stay open; bounded M2 pane operations automated pass.
merged-origin: 2026-10-22 merged origin/main (Wave1 usability, Wave2 client/server close, Herdr study, Subagents SA plan); see docs/progress/WAVE2-RESULT.md, USABILITY-WAVE1-RESULT.md, SUBAGENTS-PLAN.md
setup: see docs/SETUP.md for a fresh Apple Silicon Mac; Rust 1.99.1. Current source/status supersedes the historical index block in STASH-MERGE-RESULT.md. Worktree graph project: Users-sahillamba-.codex-worktrees-7060-panelink.
phase-4: DONE — 2026-10-25 — pinned managed UI/foreign refusal/return plus explicit recovery, parent native verified; see docs/progress/P4-INTEGRATED-RESULT.md
phase-1: DONE — 2026-10-17 — see docs/progress/INTEGRATION.md
phase-3a: DONE — 2026-10-19 — see docs/progress/P3A-DEFAULT-OPEN.md and INTEGRATION.md
phase-3b-stop: DONE as scoped two-pane stop slice — 2026-10-20 — checkpoint c3e207e; broader Phase 3 remains open
phase-p5-saved-history: DONE as scoped saved-row/checkpoint/CLI slice — 2026-10-20 — checkpoint f82d968; full V-HISTORY and Phase 5 remain open
phase-p5-history-writer: DONE as component and scoped live adoption — 2026-10-21 — see docs/progress/P5-HISTORY-WRITER.md and P5-LIVE-WRITER-RESULT.md; full Phase5 remains OPEN
phase-p5-live-history-bench: DONE as bounded synthetic observations — 2026-10-21 — see docs/progress/P5-LIVE-HISTORY-BENCH.md; no representative workload budget or full phase completion
phase-p5-history-diagnostics: DONE as fixed bounded observations — 2026-10-21 — see docs/progress/P5-HISTORY-DIAGNOSTICS.md; run2 burst final payload rejected by byte limit with drained writer queue; no full phase completion
phase-p5-final-tail: DONE as bounded suffix admission — 2026-10-21 — see docs/progress/P5-FINAL-TAIL.md; subsequent timeout-only metadata slice below preserves canceled receipts
phase-p5-drain-metadata: DONE as timeout-only accepted-end finalization — 2026-10-21 — see docs/progress/P5-DRAIN-METADATA.md; payload loss and full Phase5 remain OPEN
phase-p5-committed-corruption: DONE as whole-file refusal/preservation policy and acceptance — 2026-10-21 — see docs/progress/P5-COMMITTED-CORRUPTION.md; no salvage, checksum or full reboot claim
phase-p5-saved-layout-design: DONE as reviewed engineering contract — see docs/progress/P5-SAVED-LAYOUT.md and P5-SAVED-LAYOUT-REVIEW.md
phase-p5-saved-layout-impl: DONE as bounded codec/store/writer/observation/read-only view; post-integration shutdown hardening and evidence in docs/progress/P5-LAYOUT-COMPLETION.md; no reboot or full history claim
phase-p3-async-launch: DONE as serialized live-split prepare/journal/spawn worker with owner adoption and bounded shutdown; initial startup and legacy direct-core calls remain synchronous. See docs/progress/ASYNC-LAUNCH-RESULT.md.
phase-p3-workspace-events: DONE as scoped schema11/capability8 journal, atomic snapshot/cursor, finite watches and CLI gap/pinned-target acceptance — 2026-10-23; full Phase3 remains OPEN. See docs/progress/P3-LAYOUT-EVENTS-RESULT.md.
phase-p3-metadata-renewal: DONE as scoped schema12/capability16 managed-child sample query, owner isolation/deadline, CLI status and same-lease observer renewal — 2026-10-23; full Phase3 remains OPEN. See docs/progress/P3-PANE-METADATA-RESULT.md.
phase-p3-live-scrollback: DONE as bounded live normal-history read integration — 2026-10-23; final serial1064pass/0fail/15ignored, prior intermittent shutdown-marker failure preserved. Full Phase3 remains OPEN. See docs/progress/P3-LIVE-SCROLLBACK-RESULT.md.
phase-p3-pane-run: DONE as bounded explicit managed process CLI over the existing serialized split launch path — 2026-10-23; exact operation/target output and nonreplay uncertainty verified, final1084pass/0fail/15ignored. Full Phase3 remains OPEN. See docs/progress/P3-PANE-RUN-RESULT.md.
phase-p5-recovery: prototype view, chooser, in-workspace menu, explicit relaunch and 30-day old-to-new links implemented; full reboot remains open
phase-sa-feasibility: ATTEMPTED/BLOCKED — OpenCode2.0.32 ACP/server readiness blocked by local socket permissions; OpenCode1.19.0 probe did not prove selected-session switching. See SUBAGENTS-FEASIBILITY.md and P4-OPENCODE-TUI-IDENTITY.md.
phase-usability-wave1: scoped implementation delivered 2026-10-22; typed keys/help/cleanup and automated evidence. Full acceptance remains open; see USABILITY-WAVE1-RESULT.md.
```

## PR 3 hosted CI follow-up — 2026-10-27

Hosted CI at `ec09eb0` failed in the wide-terminal regression with connection
loss; seven supporting checks passed. The socket readiness repair preserves
all deadlines and has an old-fail/new-pass background-server reproduction.
Final combined verifier8/8: rootRust1441/0/15ignored, decoder39/0,
Python233run/1skip and all supporting checks pass. New hosted result pending. See
[the CI repair evidence](docs/progress/PR3-CI-SOCKET-READINESS-2026-10-27.md).

## Consolidation checkpoint — 2026-10-27

The user authorized combining main's dirty work into 7060 and opening a PR.
Main's live-scrollback source already exists here; its unique skill, research
and handoff docs have been imported. Combined-tree verifier passed8/8,
root Rust1439/0/15ignored, decoder39/0, Python233run/1skip and supporting
checks; imported skill validation passes. This is a checkpoint for PR.
See [the consolidation record](docs/progress/WORKTREE-CONSOLIDATION-2026-10-27.md).
Phase 3/5 acceptance gates remain open.

## Pointer controls work — 2026-10-27

User selected this installed 7060 checkout for simpler pointer and keyboard
operation. [Plan and current evidence](docs/progress/P5-POINTER-CONTROLS-2026-10-27.md)
tracks draggable dividers, pane Menu/close controls, clickable confirmation,
scrollbar seeking and input isolation. Existing dirty scrolling/workload work
is preserved. Phase 3/5 host, clipboard and reboot gates stay open.

## Control commands

- `continue Panelink roadmap`: read this file, inspect working tree and evidence, execute next action.
- `roadmap status`: report saved state without building.
- `pause`: complete current edits, checkpoint agent reports and next action, stop new work.
- `verify`: run available verification commands for completed phases; record failures honestly.

## Model routing

| Work | Model | Effort |
| --- | --- | --- |
| Precise bounded implementation and targeted tests | gpt-6-luna | high (current user request 2026-10-24) |
| Documentation, tooling and bounded maintenance tasks | gpt-6-luna | high (explicit user routing 2026-10-24) |
| Architecture, ambiguity, security review, integration | Parent orchestrator | inherited |
| Escalation after two failed attempts at same defect | Parent orchestrator | inherited |

The user renewed this routing on 2026-10-21, explicitly requesting parent
orchestration with GPT-6-Luna where suitable. Use exclusive file ownership and
parent review/verification. No token-cost telemetry is available.

Historical non-UI work authorization (2026-10-18): finish the five milestones
in [NON-UI-EXECUTION.md](docs/progress/NON-UI-EXECUTION.md) with automated tests.
Do not hand the user manual verification tasks or mark environment-blocked
checks as passed. Continue independent implementation/testing while a specific
acceptance gate is blocked; do not bypass that gate in enabled behavior.
The user's new 2026-10-21 instruction ended the pause; the automated-test and
no-manual-handoff requirements remain in force.

## Architecture and constraints

Read AGENTS.md, docs/SPEC.md and docs/PREBUILD.md. Apple Terminal on Apple Silicon is the first host. Use original Rust application logic. Start with the standard allocator. Phase 1 has zero third-party crates; this does not commit later PTY/emulator work to hand-written terminal parsing or handwritten OS bindings. Later dependencies require exact versions/features and justification. Preserve existing uncommitted planning changes. Parent must commit each fully completed phase after its required checks pass, per the user's2026-10-17 authorization. Workers do not commit. No pushes without a separate request; do not mix incomplete phase work into completion commits.

Phase 1 is a library/CLI foundation, not yet a multiplexer. No raw terminal input/output relay may be presented as terminal emulation. No provider auto-resume or conversation identity claim without M1 evidence. Secrets must stay out of debug output and persisted metadata. Future leases are server-issued, generations unique per server start; foundation takes explicit caller-supplied IDs and a monotonic logical time, without claiming runtime identity issuance.

Prototype assumptions, not confirmed product choices: local only, persistent server eventually, explicit stop, launch/monitor/resume without autonomous scheduling. Workspace hierarchy remains unresolved and is outside phase 1.

Gotchas: Rust was initially absent, then became available during implementation; Rust/Cargo 1.99.1 now tested and recorded. Cargo registry access now works; restricted home-cache writes require a writable CARGO_HOME (review used /private/tmp/panelink-p2-cargo-home). Sources in /private/tmp are disposable; durable candidate lock/tree/inventory are in docs/progress/p2-review/. Alacritty/vte's selected std path has uncapped combining and OSC storage; small feed chunks do not bound retained state. See docs/progress/P2-SOURCE-GATE.md. Host input is a separate allocation boundary: incomplete paste buffering, UTF-8 conversion, filtered queues and bounded EOF/error return must all be reviewed before enabling a UI. Explicit core shutdown closes the PTY and forbids further IO; natural child exit still drains final output. HUP tests must handshake after trap installation before claiming KILL escalation. The requested ms-rust skill is absent; no guideline compliance stamp. Graph index project: Users-sahillamba-panelink; refresh after source changes. Status observations never authorize dispatch. Path::components normalizes trailing separators; reject slash bytes before basename resolution. Global lease/operation/payload limits and expiry reclamation are now implemented; a real server must schedule maintenance and preserve non-reusable tombstones.

Graph update2026-10-21: connector recovered during final-tail work; parent
rebuilt and refreshed a fast/nonpersistent index after source edits. Earlier
Transport closed observations below are historical. Verify freshness again on
resume rather than assuming it persists.

Graph freshness gotcha: before checkpoint b6ee66b, application sources were
untracked relative to the initial documentation commit, so Git-diff-based
detect_changes missed application edits. The checkpoint now tracks the sources.
Refresh after changes and verify the indexed revision; use direct source/tests
as authority and a full refresh when index freshness is uncertain.
Earlier continuations saw Transport closed. During the current metadata slice
the connector is available and the parent refreshed a fast/nonpersistent index
after source freeze. Verify freshness on resume; source/tests remain authoritative.

## Phase 1 — offline foundation

Goal: dependency-free, inspectable crate with protocol admission logic, explicit launch environment resolution, and a truthful CLI.

- [x] P1A: Cargo package, library root, CLI (`--help`, `--version`, `doctor`), README usage.
- [x] P1B: src/protocol.rs implementing operation admission, outcome status, receipts, expiry and retry classification; unit scenarios.
- [x] P1C: src/launch.rs implementing explicit environment precedence and executable lookup; unit scenarios.
- [x] P1D: orchestrator reviews integration, records actual verification and unresolved issues.

### P1A exact interface

Single crate `panelink`, edition 2021, no dependencies, `publish=false`, license MIT (license text copyright pending). src/lib.rs exports protocol and launch. CLI has no sockets, PTYs, daemon, clipboard or launch side effects. Help identifies experimental foundation. `doctor` prints target OS/arch and truthful unsupported runtime status; exits 1 because runtime is not implemented. Unknown args exit 2, help/version exit 0. No environment values printed. Do not pin an unverified toolchain version.

### P1B exact interface and owned files

Exclusive files: src/protocol.rs, docs/progress/P1B.md. Agent may NOT edit roadmap, Cargo.toml or module roots. Report phase completion in its own evidence file; orchestrator updates shared STATE (exclusive ownership exception to append rule).

Implement public, documented types OperationKey { generation: u64, lease: u64, sequence: u64 }, OperationClass (Query, DurableMutation, Input, Signal, Clipboard, AbsoluteResize), Outcome (Accepted, Applied, Failed, Partial, Unknown), Admission (Execute, InProgress, Completed(Outcome)), ProtocolError enum, Ledger. No serialization or IO. Ledger new(generation), open_lease(id, now_secs), renew_lease(id, now_secs), admit(key, payload: &[u8], now_secs), complete(key, outcome), status(key, now_secs), receipt(lease, through_sequence, now_secs). Use HashMap/BTreeMap and exact payload bytes in volatile memory, no debug of payload. Lease TTL 1800sec, 1024 unreceipted entries. Reject zero IDs/sequences. Expired IDs cannot reopen. Expiry rejects further admissions even for duplicates. Expiration does not cancel accepted work: complete may record completion after expiry but status remains expired. Generation mismatch rejects old requests; duplicate same payload returns cached admission before sequence checks; changed payload conflicts. New sequences strictly greater than high watermark; reject old unknown sequence conservatively. Retired entries never execute. Cumulative receipt only through completed known entries: reject if any admitted entry at/below receipt is Accepted; remove terminal results, advance retirement watermark. No receipt beyond highest admitted sequence. Capacity error before mutation. complete only transitions Accepted to a terminal outcome, repeated identical completion allowed; cannot replace outcome. No command execution, crash persistence, exactly-once claim, network framing. Retry classification: queries reissue, durable same-key-only, input/signal/clipboard never, resize reconcile. Tests must exercise duplicate conflict, limit, pending receipt, expiry, wrong generation, out-of-order sequence and input retry category.

Verify (capture actual outputs): `cargo test --lib protocol`; `cargo fmt --check`. If unavailable record command-not-found as blocked, never PASS. Match idiomatic small Rust stdlib modules.

Reviewed clarifications: maximum payload 65,536 bytes; `status` returns distinct `OperationStatus::{NotAdmitted, InProgress, Completed}` and never the dispatch-authorizing `Admission::Execute`. Successful `admit` reserves before returning Execute. See P1B evidence for tests.

### P1C exact interface and owned files

Exclusive files: src/launch.rs, docs/progress/P1C.md. No edits to roadmap or roots. Public Environment = BTreeMap<OsString, OsString>. LaunchContext holds absolute cwd and client Environment; custom Debug must redact env values. EnvironmentOverride { name: OsString, value: Option<OsString> } custom Debug hides value. build_environment(account_defaults, context, layers: &[Vec<EnvironmentOverride>], owned) returns Result<Environment, LaunchError>. Precedence defaults, client, ordered override/unset layers, owned last. Reject NUL or '=' in env names and NUL values, empty names; reject any caller PANELINK_* (all layers except owned), allow account/client TERM but overwrite via owned, require owned TERM present/nonempty. No daemon env access or automatic persistence. Environment is explicitly volatile.

resolve_launch(executable: &OsStr, working_directory: &Path, context: &LaunchContext, environment: &Environment) -> Result<ResolvedLaunch, LaunchError>. Resolve relative cwd against context absolute cwd, canonicalize existing directory. Absolute executable canonicalize and check regular file plus executable mode; unqualified basename searches effective PATH in order. Reject slash-containing relative executable, missing PATH, empty/relative PATH components (validate entire PATH before searching); no shell expansion, aliases, Command::spawn, global env reads, or inferred credentials. Canonicalize selected executable; return absolute requested_path, executable_path and working_directory. Do not include environment in result. Keep error display from leaking values. Target Unix/macOS; portable resolution errors unsupported elsewhere if required. Tests use unique temp fixtures with deterministic cleanup, mode 0700 executable files; verify layer precedence/unsets, reserved values, invalid environment, PATH validation, explicit executable, cwd, no cross-client contamination. No provider integration or persisted secrets.

Verify (capture actual outputs): `cargo test --lib launch`; `cargo fmt --check`. If unavailable mark blocked. Match idiomatic stdlib Rust.

### Integrated Verify

`cargo fmt --check`; `cargo test`; `cargo clippy --all-targets -- -D warnings`; `cargo run -- --help`; `cargo run -- doctor` (expected exit 1); unknown option (expected exit 2). Review no external crates, no side-effectful launcher, no credential logging. No pass claim without outputs.

## Phase 2 — PTY and terminal engine feasibility (DONE 2026-10-22)

- [x] Record exact dependency review candidates/features and preliminary PTY API assessment: docs/progress/P2-DEPENDENCIES.md.
- [x] Specify ordered experiment, proposed checks and Apple Terminal manual checklist: docs/progress/P2-EXPERIMENT.md.
- [x] Restore source access, capture candidate resolution/checksums and review decisive source gates: docs/progress/P2-SOURCE-GATE.md. Candidate set fails unchanged; no dependency adoption.
- [x] Review/adopt scoped replacement core graph, notices/build/API evidence: docs/progress/P2-REPLACEMENT.md. Local VT100 corrections + Rustix/openpty; host-input library not adopted.
- [x] Implement/verify bounded core per docs/progress/P2-CORE.md; parent-reviewed lifecycle fixes and 42 passing integrated tests in docs/progress/P2-CORE-RESULT.md. Core ownership released.
- [x] Adopt parser-only bounded decoder and verify pure snapshot renderer: docs/progress/P2-INPUT-REVIEW.md; 49 application + 39 decoder tests passed.
- [x] Resolve/review the complete host-probe graph, native/build scripts and input/cleanup semantics; adopt exact host pins after review: P2-SIGNALS-REVIEW.md and P2-PROBE.md. Core/component graph review is complete; the full Crossterm IO framework remains unadopted. See docs/progress/P2-HOST-INPUT.md.
- [x] Implement one-shot host/CLI probe with fixed buffers, bounded IO, restore-first cached cleanup and explicit launch map; see P2-PROBE-RESULT.md and P2-CLI-RESULT.md.
- [x] Prove restore-before-child-wait with a ready HUP-ignoring child on a private PTY, for explicit cleanup and unwind Drop; see docs/progress/P2-RESTORE-ORDER.md.
- [x] Complete host acceptance after the incremental-rendering change: nine outer-PTY host tests ran and passed 9/9 in three serial runs (Haiku 5.5 worker, 2026-10-22) and 9/9 in an independent parent run; operator Apple Terminal short checklist previously confirmed. See P2-HOST-ACCEPTANCE.md.
- [x] Build one independent PTY with engine-owned screen state on Apple Terminal; deny programmatic clipboard requests. Operator OSC 52 write check in a live pane passed 2026-10-22 (clipboard unchanged); see P2-EXPERIMENT.md.
- [x] Validate shell/full-screen input, queries, resize and terminal cleanup: host tests cover typed keys/paste once, resize incl. undersized restore, HUP/SIGTERM/stall cleanup and termios restore; operator confirmed redraw/Unicode/vi/resize/paste/Ctrl-C and daily use.

Handoff: items Phase 2 closing did not prove (manual itemized Apple Terminal checklist, live Ctrl-X exit, redraw retest, other terminals) are listed in docs/progress/SESSION.md under "Phase 2 handoff".

Verify: use docs/progress/P2-EXPERIMENT.md and the explicit terminfo setup in README.md; `pty-probe` is implemented, while runtime acceptance remains pending. Source access is restored, but docs/progress/P2-SOURCE-GATE.md records failed allocation/launch/cleanup gates for the candidate set. Do not substitute a pipe, custom parser, or relaxed bound. Review viable replacements/corrections before P2B. Do not mutate parent environment or introduce an env-clearing shell wrapper. Full M1 also requires phases 3 and 4; phase 2 is not persistent-server or identity evidence.

Replacement checkpoint: docs/progress/P2-REPLACEMENT.md approves only the disposable core on Rust 1.99.1 / Apple Silicon macOS. VTE std must remain disabled; local engine source/patch/notices are preserved. Chosen history is 1,024 rows within the 2,000-row ceiling; capacity review includes resize/insert retention and remains toolchain-conditional. Parent independently reran nine engine and four PTY probe checks. The original complete-candidate adoption checkbox and Apple Terminal runtime checks remain unticked until host input/rendering review and actual integration. The core is now implemented and parent-verified; see P2-CORE-RESULT.md. The live Cargo registry recheck passed again with a writable CARGO_HOME. See P2-HOST-INPUT.md for the next source gate; no need to rerun rejected-candidate review. Luna saved partial follow-up fixes before a usage limit; parent finished/reviewed them. Parser-only crossterm-input is now adopted with reviewed fixed storage and no upstream IO framework; pure snapshot renderer is implemented. See P2-INPUT-REVIEW.md and P2-RENDER-RESULT.md. Prior component integration verified. Host and CLI owners have released their files. Parent source review and integrated checks passed: 66 application tests, nine explicitly ignored device-dependent tests; 39 separate decoder tests. See latest INTEGRATION.md. The harness establishes a controlling session, but independent /dev/tty open returns EPERM under the current sandbox. Do not replace the separate nonblocking open with duplicated stdio or count ignored cases as passes. Preserve the exact pins/notices; signal registrations intentionally live until process exit and must not be unregistered during bounded cleanup. Runtime output-stall/restore ordering and Apple Terminal checks remain gates before phase 3.

## Phase 3 — persistent server and reconnect (bounded runtime delivered; acceptance open)

The accepted bounded implementation now includes four-pane lifecycle,
explicit managed pane run, finite pane/workspace watches, reconciliation and
same-generation reconnect. The [runtime contract audit](docs/progress/P3-RUNTIME-CONTRACT-AUDIT.md)
and [independent scope review](docs/progress/P3-RUNTIME-SCOPE-REVIEW.md) reconcile
older one-child/proposal notes against current source. The expanded pathname
CLI fixture was explicitly run1/1 passing on2026-10-23; see
[the current result](docs/progress/P3-RUNTIME-CONTRACT-RESULT.md).
Full Phase3 remains open for independent acceptance. Historical milestone
wording below describes its original delivery scope, not a current absence
of later implemented functionality. Indefinite subscriptions, large Snapshot
queues and the unadopted standalone-launch proposal are separate future scope.
Inspect current state/ownership before assigning work; no worker must survive.

- [x] P3-A1: implement/verify bounded pure v1.0 wire envelope; opaque payloads only, no runtime; see docs/progress/P3-WIRE.md.
- [x] P3-B1: global ledger bounds, expiry reclamation and volatile lease issuance integrated under P3-LEDGER-TASK.md; see P3-LEDGER.md. No durable generation/socket/server capability implied.
- [x] R-15 design proposal reviewed: docs/AGENT-CONTROL.md covers full targets, shared CLI/socket commands, bounded reads/watches, input ownership, reusable caller contexts and access proposals. API implementation remains pending.
- [x] R-16 four-way split design proposal reviewed in docs/PANE-LAYOUT.md: all four pane-relative menu actions, shared pane.split, bounded tree geometry, stale revisions, dedup/launch/resize outcomes and proposed tests. Layout implementation/host validation remain pending.
- [x] P3-A2a/b: pure typed schema1 target/read/text/paste/submit/interrupt requests and Event kind7/requestID0 framing; see P3-CONTROL.md and P3-EVENT.md. Not a control CLI or runtime.
- [x] Pure Hello/Welcome codec and capability negotiation integrated; see P3-HANDSHAKE.md. No runtime feature/lease/auth claim.
- [x] Review and implement stable macOS peer credential wrapper over existing libc0.2.191; see P3-PEER-REVIEW.md. Three tests pass; pathname fixture EPERM remains pending.
- [x] Bounded response/session schemas, durable reusable caller contexts and input-owner CAS/claim/release/handoff; see P3-RESPONSE.md, P3-SESSION.md and P3-OWNERS-CONTEXT.md.
- [x] Implement private listener lifecycle, pre-handshake same-UID authentication, nonblocking deadlines and bounded workers; see P3-TRANSPORT.md and P3-IO.md. Existing endpoints are refused; automatic stale cleanup is unsupported.
- [x] Integrate StateOwner admission before effects and one-child startup intent journal with explicit storage initialization and per-domain durable generations; see P3-SERVER.md and P3-STORAGE.md. No cross-domain/backup-rollback uniqueness guarantee.
- [x] Prove real-PTY input nonreplay and same-child detach/resume over actual authenticated socketpairs; see P3-RECONNECT.md. Implement experimental server-probe/control frontend; see P3-CLI.md.
- [x] Verify experimental pathname listener/CLI scenario outside sandbox: operator confirmed corrected fixture passes; see P3-PATHNAME.md. No raw rerun counts/timings supplied.
- [x] N1/N2 notification foundation: fixed metadata payload and bounded generation-scoped journal; parent reviewed and wire integration passes. No runtime Event capability; see P3-NOTIFICATIONS-TASKS.md.
- [x] N3 explicit CLI receipt command and context-preservation checks; reviewed under P3-NOTIFICATIONS-TASKS.md, see P3-RECEIPT.md.
- [x] W1/W2/W3 bounded live-watch protocol, authoritative reconciliation and delivery; parent CLI/private-PTY integration passes. See P3-WATCH-INTEGRATION.md; expanded pathname CLI watch acceptance remains external.
- [x] Review managed launch/multi-session next-task plan under P3-MANAGED-LAUNCH-TASK.md. Plan is a proposal in P3-MANAGED-LAUNCH-PLAN.md, not adopted runtime/schema; ownership released.
- [x] Close descriptor/CLOEXEC/process-spawn source gate before enabling API launch (closed 2026-10-18 for macOS/aarch64 + Rust 1.99.1, foreign/native spawn out of scope; see P3-SPAWN-DESCRIPTOR-GATE.md 'Gate closure').
- [x] Review initial descriptor/spawn source inventory in P3-SPAWN-DESCRIPTOR-GATE.md: macOS PTY and standard-library socket/pipe flag windows remain; no concurrent-launch approval.
- [x] Review test-only atomic PTY opener spike and actual macOS evidence in P3-ATOMIC-PTY-SPIKE.md; this does not close socket/pipe/native-spawn gaps or adopt a production replacement.
- [x] Integrate/review production atomic PTY opener and cooperative shared spawn gate for current experimental macOS/aarch64; see P3-ATOMIC-PTY-INTEGRATION.md and P3-SPAWN-GATE-RESULT.md. No release minimum-OS or arbitrary native-spawn guarantee.
- [x] Implement full-descriptor child probe and deterministic real accepted-socket/managed-exec race fixture; run on a host permitting pathname bind/accept before enabling API launch (passed 6/6 on new Mac; see P3-ACCEPT-EXEC-RACE.md).
- [x] Four-pane binary split tree (MAX_PANES=4) and agent-facing `panelink pane|wait|events` CLI integrated and parent-verified; see P3-FOUR-PANES.md, P3-AGENT-CLI.md. Four-pane Apple Terminal acceptance pending. L16 four-pane relaunch, L17 private-PTY four-pane attach acceptance, L18 pane exit status (schema 10) delivered 2026-10-22.
- [x] Bounded workspace layout notifications: schema 11/capability 8, 64-record journal, atomic layout/cursor, shared finite-watch budget and agent CLI target pinning. See docs/progress/P3-LAYOUT-EVENTS-RESULT.md; broader Phase 3 and its acceptance remain open.
- [x] Bounded live normal-scrollback reads through existing control and agent CLI, with row/UTF-8 budgets, current-epoch truncation and separate screen/history sources; see P3-LIVE-SCROLLBACK-RESULT.md (2026-10-23).
- [x] Managed `pane run` with literal raw argv, explicit client cwd/env, exact target/revision pinning, bounded split-worker launch and structured uncertainty output; see docs/progress/P3-PANE-RUN-RESULT.md (2026-10-23).
- [x] Neutral generation-scoped owner discovery and automated post-ready attach SIGKILL/reattach acceptance, parent-reviewed and full-suite verified; see docs/progress/P3-OWNER-LIFETIME-RESULT.md (2026-10-23).
- [x] Bounded server SIGKILL acceptance: cooperative-shell exit, stale socket replacement, full generation/target fencing, old-context refusal/preservation and no completed-input replay; see docs/progress/P3-SERVER-DEATH-RESULT.md (2026-10-23). No arbitrary descendant cleanup guarantee.
- [x] Real input CLI SIGKILL after server Completed(Applied) but before caller acknowledgment; exact uncertain key/status/resync/sequence, same child and no token replay verified. See docs/progress/P3-INFLIGHT-CLIENT-RESULT.md (2026-10-23). Does not cover death before admission or owner-pending.
- [x] Reconcile and close the accepted bounded event/watch/launch/layout implementation scope: finite watches/reconciliation, workspace invalidations/gap recovery, four-pane lifecycle and managed pane run. Historical umbrella warning superseded; indefinite subscriptions/large Snapshot queues/direct-launch proposal are separate future scope. See docs/progress/P3-RUNTIME-CONTRACT-RESULT.md.
- [x] Review and verify the accumulated pre-delivery real-CLI and internal handler-boundary regressions with the earlier intermittent-acceptance fixes: complete serial Cargo1091/0/15 ignored and all seven supporting checks pass. Three Luna High workers found no additional required production task; see docs/progress/P3-ORCHESTRATION-RESULT.md (2026-10-24).
- [x] Repair user-reported agent context storage, wheel history/app scrolling and pane padding; preserve legacy saved geometry and immediate startup output. Parent final serial1111/0/15 ignored plus all seven supporting checks; see docs/progress/P3-USABILITY-FIXES-RESULT.md. Activation completed after explicit user approval; the fresh workspace uses the verified build.
- [ ] Complete independent Phase3 host/lifetime acceptance before marking Phase3 complete: human four-pane Apple Terminal evidence remains open; preserve explicit in-flight/client/server crash observation limits. This is the sole explicit remaining Phase3 gate after the 2026-10-24 audit. Do not infer provider/reboot acceptance or arbitrary descendant cleanup from automated prototype results.

## Phase 3A — right-click splits and new terminal sessions (DONE 2026-10-19)

User inserted this phase on2026-10-18 before the previously queued lifecycle/
stop and history work. Implement a usable two-pane terminal slice, reusing
the existing [four-way layout design](docs/PANE-LAYOUT.md) and current trusted
multi-session core. This priority change does not complete Phase3 or waive its
launch/runtime/host gates. Broader tabs/sidebar/workspace choices stay open.
Detailed staged work and exit evidence:
[P3A-RIGHT-CLICK-SPLITS.md](docs/progress/P3A-RIGHT-CLICK-SPLITS.md).

- [x] P3A-L1: bounded two-pane geometry, read-only split preview, guarded commit, validated resize and hit testing; 12 focused tests. This is pure groundwork only; see [P3A-LAYOUT.md](docs/progress/P3A-LAYOUT.md).
- [x] P3A-W1: pure split menu model, schema-7 split codec and offset-aware two-pane compositor; see P3A-MENU.md, P3A-SPLIT-CODEC.md, P3A-COMPOSE.md.
- [x] P3A-W2: trusted in-process split transaction with exact-payload dedup, layout-owned absolute resize and real two-PTY proof; see P3A-SPLIT-TRANSACTION.md. No runtime route yet.
- [x] Integrate exact clicked-target/layout-revision routing from the client menu through the runtime (`panelink serve`, schema 7/8) to the split transaction; see P3A-RUNTIME.md.
- [x] Add an in-terminal right-click overlay with keyboard fallback (Ctrl-B s), fit/capacity reasons and no menu-input leakage to the child; see P3A-CLIENT.md.
- [x] Wire each action to one fresh independent zsh session, preserving the original; explicit client context and global deduplication.
- [x] Integrate two live panes, focus/input routing, layout-owned PTY resize and incremental rendering (`panelink attach`).
- [x] Zero-configuration `panelink` (no args) opens/reattaches the workspace; `panelink stop`; last-shell exit closes it; see P3A-DEFAULT-OPEN.md.
- [x] Automate geometry/menu/failure/duplicate tests and real two-child input/resize/isolation/cleanup checks; outer-PTY fixture proves SGR right-click through the decoder. Apple Terminal delivery still needs the manual check below.
- [x] Parent review, integrated verification and actual usable split acceptance: user confirmed in Apple Terminal (2026-10-19) that `panelink`, right-click split, typing in both panes and Control-B NAV mode work.

The current two-session capacity remains authoritative: after one successful
split, additional splits report capacity until an explicit capacity review
adopts a larger bound. Launch/stop permission is not inferred from input or
focus. If a prerequisite is blocked, advance pure layout/menu/tests without
enabling an unsafe or nonfunctional split action.

Phase 3A was committed at `e1126c7`. The historical design/ownership notes
below this section describe the development sequence; the current pause
handoff above and docs/progress/SESSION.md supersede them on resume.

## Phase 3B — per-pane lifecycle/stop (scoped slice DONE 2026-10-20)

The user chose stop before history. Any authenticated same-UID client may
explicitly close a pane without acquiring input ownership or a separate stop
grant; this prototype policy does not grant launch authority. Right-click
Close pane and Ctrl-B then `x` require confirmation. `panelink panes` and
`panelink close N` operate on the default workspace; low-level `control panes`,
`control close TARGET REV`, and `control close-status SEQUENCE` use complete
targets and keyed status. A stopped child gets HUP, then KILL after a bounded
wait when needed. An exited pane collapses automatically when another pane
survives; that sibling fills the root, and a later split gets a new session ID.
The last child exiting still ends the workspace. See P3B-CLOSE-CORE.md,
P3B-CLOSE-CLIENT.md and P3B-CLOSE-RUNTIME.md for contracts and limitations.

Verification: `python3 scripts/verify_non_ui.py` 8/8; full Rust library
500 passed/4 ignored, binary 93 passed, 15 ignored in total; initial
close/runtime 2/2, subsequently expanded to 5/5 with saved-history coverage;
`p3a_default_open` 6 passed/1 ignored. Fixed a length-before-indexing bug
in the pane-status decoder and tested every truncated prefix. The
prototype still blocks its owner tick for up to approximately two seconds
during a close and does not durably restore live sessions after a crash.
This slice does not complete the wider Phase 3 gates.

History defaults selected by the user for the scoped P5 slice: store under the private per-user
Panelink application directory (`history/`); retain at most the engine's 1,024
normal scrollback rows and 8 MiB per session on disk, plus latest committed
normal and alternate screen checkpoints; target at most five seconds between
captures of changed screens and capture on detach/stop; retain 30 days after
session end, with per-session clear. Saved history is read-only, never replayed,
and distinct from provider conversations. These are prototype defaults;
the implemented subset and remaining recovery gates are in the saved-history
follow-up below.

The earlier live-checkpoint slice integrated visible-screen capture with the two-pane
server and checked together with pane-close lifecycle behavior. The store can
reopen committed checkpoints after server shutdown, and capture failures keep
shells running. Alternate screens that enter and leave between PTY read chunks
can be missed. Its original evidence predates saved scrollback and CLI
recovery; see [P5-LIVE-CAPTURE.md](docs/progress/P5-LIVE-CAPTURE.md) and the
subsequent saved-history follow-up below.

Saved-history follow-up: the owner now writes normalized normal scrollback in
bounded per-target segments, marks gaps and RIS epochs, records confirmed exit
times, and offers `panelink history list|show|search|clear` for read-only
recovery and explicit stopped-workspace deletion. Ended records are pruned
after 30 days on server startup. See
[P5-SAVED-HISTORY.md](docs/progress/P5-SAVED-HISTORY.md). This does not
restore workspace layouts, live processes, provider conversations or every
alternate-screen frame; V-HISTORY's complete reboot/layout acceptance remains
open.

## Phase 4 — early OpenCode identity gate (scoped managed integration DONE)

2026-10-24: two Luna High owners delivered the V2 observer and harness;
parent review corrected loading, warm-up and picker scoping. Three fresh pinned
2.0.32 private two-TUI runs pass A-B-A with the other client continuously on B,
zero rejected/missed/extra observations, verified empty user messages and full
cleanup. Python121/0/no skips. See [current result](docs/progress/P4-V2-TUI-RESULT.md).
This is a development gate, not a live Panelink adapter or recovery feature.

- [x] Pin installed OpenCode version; prove selected top-level identity and session changes with two agents in the same project. Scoped no-prompt private two-TUI proof3/3 on2.0.32; no model-workload or production integration claim. Historical1.19.0 missing-switch evidence remains in P4-OPENCODE-TUI-IDENTITY.md.
- [x] Scoped opposite-client B-A-B and keyboard new-conversation route outcome,3/3 fresh runs. Home has no selected ID; no new-ID guarantee. See P4-V2-EXTENDED-RESULT.md.
- [x] Scoped record-delivery pause, host-time observer expiry and explicit fresh reconciliation while both TUIs/server stay alive,3/3. Development monitor only; durability false.
- [x] Scoped linked-child selection as non-top-level, exclusion from root listing, keyboard parent return and original-client identity continuity,3/3 fresh runs. Empty-parent fork returns400; nonempty copied-history fork is untested.
- [x] Scoped separate-client directory/project identity isolation,3/3 fresh runs, exact new-directory selection and foreign-scope rejection. See P4-V2-ISOLATION-RESULT.md.
- [x] Scoped pinned-native fixture proves API-created/UI-selected new root, static nonempty transcript import plus actual full-history fork, and same-client Ctrl-O project round-trip; Luna run and independent parent run pass. Native forks are top-level with a separate `fork` origin. See P4-REMAINING-IDENTITY.md. Production adapter acceptance remains separate.
- [x] Actual managed adapter new/fork/Home/project transitions pass Luna and independent parent native runs. Foreign same-TUI observer selection, structured public manual refusal/no ticket/preserved A and same-instance return with exact newer runtime/disk revision pass. See P4-INTEGRATED-RESULT.md and durable sanitized reports. Scoped pinned2.0.32 contract only; no current-at-crash/automatic resume/replay.
- [x] Typed protocol-v1 admission wraps the binding core with source ordering, bounded caller lease, expiry, reconciliation and revision issuance.12 focused tests; no wire/source-auth/live integration claim.
- [x] Single-writer bounded binding store plus component storage/acknowledgment and actual process-kill/reopen boundaries.9 focused store and4 combined target tests; all restoration stale. See P4-BRIDGE-STORE-RESULT.md.
- [x] Bounded independent protocol-v1 Hello/Welcome/Observation codec plus same-user, one-use scoped-grant Unix endpoints.19 focused codec/channel tests and2 combined private IPC cases pass; no provider executable attestation, freshness barrier or live listener activation. See P4-BRIDGE-TRANSPORT-RESULT.md.
- [x] Opt-in pinned2.0.32 source/grant provisioning, fresh challenge snapshots, owner/worker dispatch and versioned500ms/300ms/3000ms heartbeat/IO/lease policy are integrated. Native bridge-failure survival passes Luna plus independent parent; no executable attestation or implicit reconnect. See P4-BRIDGE-RUNTIME.md and P4-NATIVE-BRIDGE-FAILURE.md.
- [x] Joint managed/provider record persistence, exact private completion acknowledgment, Prepared/Spent and process-kill/reopen gates pass; current-at-crash is explicitly rejected. Historical restoration stays stale. See P4-MANAGED-CRASH.md and P4-NATIVE-MANUAL-RECOVERY.md.
- [x] Exact-target historical inspection plus explicit SDK-validate/prepare/separate-resume/fresh-selected-save flow is integrated. Native missing/deleted/foreign/child, one-shot replay refusal and nonempty fork-root cases pass Luna plus independent parent. No latest-session scan, automatic resume or replay. See P4-MANUAL-RECOVERY.md and P4-NATIVE-MANUAL-RECOVERY.md.

## Phase 5 — workspaces, selection, history (in progress; scoped saved history DONE)

- [x] Scoped opt-in in-client workspace tabs, chooser and safe switching under Project -> workspace tabs -> split panes; parent full1344/0/15ignored and immediate private-PTY switching3/3. See P5-TAB-INTEGRATION-2026-10-25.md. Full host acceptance remains open.
- [x] Scoped visible-view keyboard copy mode, frozen caret/selection and local input isolation; parent full1356/0/15ignored and clipboard-free actual PTY2/0. See P5-KEYBOARD-COPY-RESULT-2026-10-25.md.
- [x] Scoped M2 pane-location swaps, interactive ancestor-divider preview/absolute commit and server-global zoom while processes continue; fullRust1399/0/15ignored, finalclient123/0, outerPTY3/0 and real4PTY1/0. See P5-PANE-OPERATIONS-INTEGRATION-2026-10-26.md. Broader host/workload acceptance stays open.
- [x] Scoped project/workspace model, strict metadata-only codec/store and opt-in isolated workspace CLI routing; actual two-server open/stop and legacy reachability acceptance. See P5-WORKSPACE-ROUTING-RESULT-2026-10-25.md.
- [x] Scoped frozen drag selection, separately negotiated wrap metadata, selected-cell compositor and attached-only async clipboard lifecycle; fake-sink and private-PTY checks pass. Human host/real clipboard remain open; bounded keyboard copy is now delivered separately. See P5-SELECTION-CLIENT-RESUMED-2026-10-25.md.
- [ ] Mouse routing, selection copying and context menus on Apple Terminal.
- [x] Scoped two-pane saved normal scrollback and visible-screen checkpoints, bounded read-only CLI show/search, 30-day ended-session pruning and stopped-workspace clear; see P5-SAVED-HISTORY.md. Does not imply workspace or provider recovery.
- [x] Deterministic software-injected pre/post-rename sync faults, retry acknowledgments and first-end-time retention, including real-PTY capture retries; see P5-RECOVERY-FAULTS.md.
- [x] Fresh-process read-only CLI acceptance for retained rows, latest normal/alternate checkpoints and unavailable overwritten content; see P5-RESTART-ACCEPTANCE.md. This does not prove a machine reboot or saved layout restoration.
- [x] Scoped real pathname server SIGKILL after independently decoded committed one-pane layout/history; fresh read-only inspection and explicit new-generation relaunch without input replay. Parent focused1/0, full verifier8/8 and final cleanup1/0; see [crash recovery evidence](docs/progress/P5-CRASH-RECOVERY-2026-10-26.md). Physical reboot remains open.
- [x] Bounded orphan-temporary discard, stopped-workspace `history cleanup` CLI and startup-before-capture helper; see P5-TEMP-CLEANUP.md. Full pathname startup acceptance remains blocked.
- [x] Single-machine synthetic release throughput baseline, two recorded runs, fixed 24-operation cases and quota validation; see P5-HISTORY-THROUGHPUT.md. This is not a ten-agent or UI latency acceptance pass.
- [x] Bounded history writer component and lock retention; see [P5-HISTORY-WRITER.md](docs/progress/P5-HISTORY-WRITER.md). Scoped live adoption is also delivered; see P5-LIVE-WRITER-RESULT.md.
- [x] Scoped bounded live history scheduling off the owner thread, retaining durable acknowledgment/gap/close contracts; blocked-writer input/control/split and quota/cap tests pass. See P5-LIVE-WRITER-RESULT.md.
- [x] Bounded actual-store live two-pane workload observations: two final release runs, raw timing/probe/history reports and validator; see P5-LIVE-HISTORY-BENCH.md. Burst failure flags and preliminary missing ends remain recorded, without a performance-budget pass.
- [x] User-selected ten-idle-OpenCode plus paced shell/real offline fixture-build profile measured twice; each60seconds/72acknowledgedprobes/55builds, no sticky capture failure, confirmed group cleanup. See [combined workload](docs/progress/P5-COMBINED-WORKLOAD-2026-10-26.md). RSS variance and numeric acceptance budgets remain open.
- [x] Fixed history failure/drain diagnostics and two schema2 observations; see P5-HISTORY-DIAGNOSTICS.md.
- [x] Preserve a newest bounded final suffix with all candidate screens under byte pressure; see P5-FINAL-TAIL.md. Prefix loss remains explicit; no quota/budget increase.
- [x] Scoped explicit-timeout finalization of accepted known-end metadata; canceled payloads/receipts remain canceled. See P5-DRAIN-METADATA.md. Unknown/unsubmitted end intent and payload loss remain separate.
- [x] Committed-record corruption/partial-tail refusal policy with byte-preserving store and fresh-process CLI acceptance; see P5-COMMITTED-CORRUPTION.md. No salvage, repair or arbitrary bit-rot detection is claimed.
- [x] Bounded metadata-only end reservations for unsubmitted end intent; explicit unknown end time; rows salvaged before screens under final byte pressure; see P5-HISTORY-END-INTENT.md. No durable unknown-ended label, complete-capture or performance-budget claim.
- [x] Recovery viewer + explicit relaunch mapping design; user decisions recorded 2026-10-21; see P5-RECOVERY-VIEWER.md.
- [x] Recovery stages 1-3: view model/scaled overview, read-only reader, offline TUI + `panelink recovery view [--previous]`; see P5-RECOVERY-STAGE12.md, P5-RECOVERY-STAGE3.md. Host /dev/tty acceptance not certified.
- [x] Recovery chooser at plain startup and in-workspace 'View saved layout' item; see P5-RECOVERY-CHOOSER.md, P5-RECOVERY-MENU.md.
- [x] Explicit relaunch (default shell, refuse while running) and 30-day old-to-new links; see P5-RECOVERY-RELAUNCH.md, P5-RECOVERY-LINKS.md.
- [x] Scoped saved-layout/restart engineering design and exact codec task, with independent review; see P5-SAVED-LAYOUT.md. No saved-layout implementation or new command is delivered by that design.
- [x] Historical-only saved-layout codec: current 280-byte records with supported legacy 160-byte migration; strict canonical validation and no live target restoration.
- [x] Bounded two-slot store/rotation, guarded worker, lifecycle observation and read-only inspection; see P5-LAYOUT-SLICE.md and P5-LAYOUT-COMPLETION.md. No reboot restoration claim.
- [ ] Full V-HISTORY reboot/layout recovery and representative workload/throughput acceptance; see P5-SAVED-HISTORY.md and docs/SPEC.md.

## Planned phase SA — Subagents visibility (not started)

Planned cross-cutting phase **SA — Subagents visibility** (requested2026-10-21):
automatically expose supported provider background/subagents in a dedicated
Subagents tab for optional observation. See
[SUBAGENTS-PLAN.md](docs/progress/SUBAGENTS-PLAN.md) for capability spike,
bounded registry/observer, tab UI and live acceptance slices. Provider proof
belongs with Phase4; tab presentation with Phase5. Scope is OpenCode and
compatible wrappers, with read-only activity viewing proposed first. This is planned,
not implemented, and does not waive the remaining Phase 3/4/5 gates.

S0 was attempted2026-10-21: CLI preflight passes on installed OpenCode2.0.32,
but server readiness is blocked by local socket permissions. No worker was
launched or observed. See [feasibility evidence](docs/progress/SUBAGENTS-FEASIBILITY.md).

## Phase 6 — alpha measurements and review evidence (DONE for personal alpha scope 2026-10-22)

- [x] Ten-agent workload and build/resource measurements on specified hardware: build/size (P6-MEASUREMENTS.md), synthetic ten-pane, and real idle OpenCode 10-session runs with Panelink overhead separated (P6-OPENCODE-WORKLOAD.md). Active-agent load not measured (user chose no model calls).
- [x] Dependency/advisory/license inventory (P6-DEPENDENCY-INVENTORY.md, cargo-audit 0 vulnerabilities), installation/upgrade/release evidence (docs/RELEASE.md, P6-RELEASE-EVIDENCE.md, release/MANIFEST.json) and security review pack (docs/SECURITY-REVIEW.md). Firm-specific criteria, approval and signing/notarization remain with the user's organization.

## Resume protocol

On an explicit resume request, read this file, AGENTS.md and the current
handoff in docs/progress/SESSION.md, then inspect git status and history.
Read the evidence for the chosen first incomplete gate. Probe toolchain/provider
availability without printing secrets; run permitted verification and record
blocked acceptance separately. Follow the current delegation instruction in
AGENTS.md. Never discard uncommitted changes. The older checkpoint paragraphs
below are historical evidence, not live task or delegation assignments.

Immediate resume handoff (2026-10-18): read P3A-RIGHT-CLICK-SPLITS.md,
P3A-LAYOUT-TASK.md, P3A-LAYOUT.md and P3A-MIMO-MENU-TASK.md. The Luna layout
owner has released its files; src/pane_layout.rs is exported and parent-reviewed.
Continue menu/input and transaction design, then implement bounded slices with
Luna xhigh and parent-owned integration. The external MiMo task has not run:
the installed OpenCode 2.0.32 CLI fails private ACP bootstrap because loopback
bind returns EPERM. If the user runs the saved controller from normal Terminal,
review its report before adopting any recommendations. Keep Phase3A incomplete
until menu-to-two-live-child acceptance and all required checks pass.
The user also authorized a checkpoint commit of the accumulated staged work,
but this environment rejected writes to `.git/index` with `Operation not
permitted`; the explicit commit request remains unfulfilled. Do not ask again
for authorization. Commit once Git metadata is writable, after checking both
staged and unstaged diffs. Update 2026-10-18 (new machine): Git is writable,
but the user said "no need to commit yet"; hold commits until asked again.

Latest operator-debug checkpoint: user ran nine ignored scenarios in Terminal (4 passed / 5 failed). Safe-Rustix experiments reproduce retained-slave ENOTTY and blocked exit when the probe owns the controlling session. The corrected harness keeps a supervisor alive across probe exit/assertions and drains the private PTY only during subsequent supervisor teardown. Two new lifecycle regressions pass; the direct application loop detects output stall in1,001ms with bounded cleanup. This supports the correction but does not certify the nine corrected operator scenarios; rerun serially outside the sandbox. See P2-OPERATOR-RUN.md, P2-HOST-DEBUG.md and P2-STALL-DEBUG.md. All ownership released; no production behavior/dependency changes, commits or pushes.

Operator follow-up: user confirmed "all corrected" after the requested serial host-suite rerun. Recorded as operator confirmation that the reported failures are resolved; no fresh raw output, exact counts or timings were supplied. The earlier failing log remains preserved as history. This does not establish manual Apple Terminal compatibility or the remaining restore-before-child-wait timing observation. See latest P2-OPERATOR-RUN.md.

Latest UX checkpoint: user reports most manual terminal behavior was fine, but Ctrl-Q stopped the probe before the checklist was complete. Clarified Ctrl-Q is explicit stop for this disposable probe, not detach or editor quit; child cleanup/restoration timing is deferred at the user's request. Full-screen reload on every change was a renderer defect: parent-supervised Luna xhigh implemented pure snapshot differences, parent integrated two preallocated display/target caches and commit-after-complete-write semantics. Startup/resize/discard fully repaint; ordinary changes update cells, wide topology changes update the affected row, identical frames emit no output. See P2-DELTA-RESULT.md and latest INTEGRATION.md. No dependency changes, commits or pushes; all ownership released. Remaining manual gates are not marked complete.

Latest automatic timing checkpoint: parent-supervised Luna xhigh added private-PTY observations for explicit cleanup and unwind Drop; parent independently observed all 31 restore bytes and saved termios after about 1 ms while the ready HUP-ignoring child remained alive. Child cleanup lasted about 1,013–1,015 ms in the parent focused run. Reversed-order mutation failed; restore-first production code is unchanged. Parent hardened the readiness parser to require its trailing marker before interpreting a PID, and required ESRCH rather than any error for the final unwind liveness probe. Final suite83 passed /9ignored, fmt and strict Clippy passed; see P2-RESTORE-ORDER.md. Special child timing no longer requires a manual fixture to establish this scoped source-level ordering; actual host rendering/cleanup acceptance remains pending. No dependency changes, commits or pushes; all ownership released.

Latest shortcut/next-phase checkpoint: operator reports all short retest items except Ctrl-Q worked; host presented a window-close confirmation. Added and documented Ctrl-X as local quit, retaining Ctrl-Q alias; exact host mapping remains unknown. Do not infer clipboard/full-checklist completion. Phase3 plan delivered; P3-A1 pure envelope implementation assigned with exact parent-approved byte layout/limits, no sockets/server/PTys/dependencies. Parent owns shared docs/exports and integration. Inspect P3-WIRE.md before resuming after disconnect; active agent ownership does not survive by assumption.

Latest combined integration: Ctrl-X is now the documented disposable-probe quit key; legacy Ctrl-Q remains an alias. Operator confirms the short redraw/Unicode/editor/resize/paste/Ctrl-C retest; no claim about the separate clipboard sentinel. Parent reviewed P3-A1 envelope: length validation before any body allocation, per-kind caps, borrowed input, exact version/flags/kinds, redacted payload diagnostics, writer errors propagated. Broader protocol plan remains proposed; no socket, typed operations or persistent runtime. Final94passed /9ignored, fmt/Clippy/diff check pass. All ownership released, no dependencies/commits/pushes. Next: parent specifies ledger/global bounds and typed handshake; Ctrl-X live exit/restoration remains operator-pending.

Agent-control design scope update: user explicitly requested an easy API/command system so agents can read/control other managed agents/processes/panes, using Herdr as a reference. Added confirmed requirementR-15 and discovery note; exact authorization/default scope/layout/provider contracts remain under design, no autonomous scheduling implied. A separate Luna xhigh research/design owner edits only docs/AGENT-CONTROL.md, using official current Herdr docs and existing invariants. Parent owns canonical integration; do not freeze typed operation schemas/server command routes until the plan is reviewed. Ledger task remains independent and unchanged.

Latest ledger checkpoint: parent reviewed source/counter tests, independently ran full103pass /9ignored suite and strengthened the full-global-operation boundary test to assert duplicate/conflict behavior before capacity rejection. Focused strengthened test, fmt/Clippy/diff checks pass. Explicit maintenance frees payload allocations/terminal metadata while pending identities can finish; issued IDs never reuse. Permanent4096lease-record exhaustion deliberately fails closed; future server maintenance/generation/runtime remain separate. No new dependencies or commits/pushes; ledger owner released. Agent-control design is concurrently under parent-requested refinement of target refs, input ownership and reusable CLI contexts; inspect its status before schema implementation.

Commit policy update (2026-10-17): user requested commits after every full phase completion. Parent owns selecting/staging the completed phase's code and affected docs/evidence after verification; no repeated confirmation is required. This supersedes historical blanket no-commit instructions for the parent, while worker no-commit rules remain. Current Phase2 acceptance and Phase3 runtime are incomplete, so no new completion commit is claimed. No push authorized.

Four-way split design update: user explicitly requests right-click Split Left/Right/Up/Down. Dedicated Luna xhigh owner edits only docs/PANE-LAYOUT.md; parent owns canonical requirements and phase integration. Plan tree/geometry/context targeting and shared R-15 pane.split API before layout schemas freeze; workspace/tab hierarchy remains unresolved.

Latest design integration: agent-control and four-way split owners delivered their documents, and parent reviewed/refined both. R-15 specifies full immutable target references, reusable caller/lease contexts, typed read/control/watch operations and explicit input-owner handoffs; R-16 adds four pane-relative directions with a shared API, layout-owned resizing and duplicate checking before stale-layout preconditions. Defaults/bounds/hierarchy remain proposals, not implemented capabilities. All ownership released. User phase-completion commit policy is saved in AGENTS.md; current unfinished phases do not trigger a completion commit. Graph refresh attempt after ledger review failed with Transport closed; prior owner index exists, use source/durable notes as current evidence. No push authorized.

Active P3-A2 tasks: typed_control_codec (gpt-6-luna xhigh) owns only src/control_protocol.rs and docs/progress/P3-CONTROL.md under P3-CONTROL-TASK.md; event_framing (same model/effort) owns only src/protocol_wire.rs and docs/progress/P3-EVENT.md under P3-EVENT-TASK.md. Parent owns exports/shared docs/integration/source review. Typed slice is pure five-operation request payloads with full target/owner revisions, not CLI/server effects. Event slice adds kind7/requestID0 and header-aware max-frame constants, not queues/subscriptions. Graph MCP remains unavailable (Transport closed), direct source fallback in use. Do not assume agents survive interruption; inspect owned files/evidence and rerun integration.

P3-A2 parent handoff: both owners released files; parent reviewed full source and added a request-codec/ledger integration regression proving changed target, owner revision or action conflicts under the same operation key. Final locked/offline suite122passed /9ignored, fmt/Clippy pass; no dependencies/manifest changes. Typed payloads validate/borrow bytes only, do not prove caller/target/owner validity or dispatch effects. Event kind7/requestID0 requires future capability negotiation. Initial server launch requests must fit the full65536byte request budget or reject; no staging adopted and probe1MiB cap remains separate. Future output accounting2162736bytes/writer includes maximum snapshot plus control reply, not implemented queues. Parent source review/compile-only check found selected peer APIs unavailable on stable macOS: see P3-SOCKET-GATE.md before transport implementation. Graph unavailable, no refreshed-index claim. All ownership released; Phase3 is not complete and no completion commit/push performed.


2026-10-18 checkpoint: user requested retry and continuation with another agent. Parent connector still returns Transport closed, but the Luna xhigh handshake owner successfully rebuilt the graph, then refreshed final source (832nodes/3759edges, fast/nonpersistent). Parent reviewed original stable macOS getpeereid wrapper, adopting a direct edge to existing libc0.2.191 without new package/features/build target; three focused checks pass. Pathname socket fixture bind returns EPERM here and is explicitly ignored, with an external rerun command in P3-SOCKET-GATE.md. Handshake owner delivered exact Hello/Welcome codec; parent added decoded semantic rejection and ledger TTL consistency tests and reviewed all source. Final139passed/10ignored and strict Clippy pass; initial suite hit existing child-reap timing assertion once, focused and final full reruns passed. No change to production PTY cleanup or weakened assertion. Phase2/3 remain incomplete, so no completion commit/push. All file ownership released.

Latest parallel integration checkpoint, 2026-10-18: supervised Luna xhigh
workers delivered transport/workers, responses/session codecs, durable contexts,
input ownership, StateOwner, PTY adapter, CLI and actual socketpair reconnect
proof under exclusive ownership. Parent integrated/reviewed them and added
explicit generation storage, startup Intent journal and owner driver. Final
standard suite248passed/13ignored and strict Clippy pass. Actual pathname CLI
attempt fails with listener_unavailable and independent bindEPERM; external
acceptance remains pending in P3-PATHNAME.md. Source-level same-child/lostACK
proof passes, with no automatic input replay. Missing metadata fails closed;
no backup rollback/provider identity/layout/events guarantees. See latest
INTEGRATION.md, P3-STORAGE.md and README quickstart. Phase2/3 remain incomplete;
no completion commit or push. Resume from current top checkpoint, not earlier
historical active-task paragraphs.
