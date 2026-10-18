# Panelink implementation roadmap

```text
current-phase: 3 bounded live-watch/reconciliation integrated; managed-launch plan reviewed; Phase 2 host acceptance pending
last-updated: 2026-10-18
last-verified: 2026-10-18 — 303passed/13ignored; strict all-target Clippy and cargo fmt --check pass; atomic PTY opener spike passes on current macOS host; expanded CLI pathname fixture remains external; see INTEGRATION.md
next-action: specify production macOS atomic PTY opener integration and owner-path spawn coordination, including remaining socket/pipe windows and supported native-spawn scope; concurrent API launch remains disabled; clipboard/zsh and pathname watch retests pending
blocked-on: clipboard sentinel failed from empty-reply input injection, fixed with retest pending; full checklist unspecified; short redraw/Unicode/vi/resize/paste/Ctrl-C and Ctrl-X return-to-shell operator-confirmed; OpenCode absent at last check
phase-1: DONE — 2026-10-17 — see docs/progress/INTEGRATION.md
```

## Control commands

- `continue Panelink roadmap`: read this file, inspect working tree and evidence, execute next action.
- `roadmap status`: report saved state without building.
- `pause`: complete current edits, checkpoint agent reports and next action, stop new work.
- `verify`: run available verification commands for completed phases; record failures honestly.

## Model routing

| Work | Model | Effort |
| --- | --- | --- |
| Precise bounded implementation and targeted tests | gpt-6-luna | xhigh |
| Architecture, ambiguity, security review, integration | Parent orchestrator | inherited |
| Escalation after two failed attempts at same defect | Parent orchestrator | inherited |

User authorized implementation and cheaper-model delegation on 2026-10-17. No token-cost telemetry is available; report model assignments, not invented savings.

## Architecture and constraints

Read AGENTS.md, docs/SPEC.md and docs/PREBUILD.md. Apple Terminal on Apple Silicon is the first host. Use original Rust application logic. Start with the standard allocator. Phase 1 has zero third-party crates; this does not commit later PTY/emulator work to hand-written terminal parsing or handwritten OS bindings. Later dependencies require exact versions/features and justification. Preserve existing uncommitted planning changes. Parent must commit each fully completed phase after its required checks pass, per the user's2026-10-17 authorization. Workers do not commit. No pushes without a separate request; do not mix incomplete phase work into completion commits.

Phase 1 is a library/CLI foundation, not yet a multiplexer. No raw terminal input/output relay may be presented as terminal emulation. No provider auto-resume or conversation identity claim without M1 evidence. Secrets must stay out of debug output and persisted metadata. Future leases are server-issued, generations unique per server start; foundation takes explicit caller-supplied IDs and a monotonic logical time, without claiming runtime identity issuance.

Prototype assumptions, not confirmed product choices: local only, persistent server eventually, explicit stop, launch/monitor/resume without autonomous scheduling. Workspace hierarchy remains unresolved and is outside phase 1.

Gotchas: Rust was initially absent, then became available during implementation; Rust/Cargo 1.99.1 now tested and recorded. Cargo registry access now works; restricted home-cache writes require a writable CARGO_HOME (review used /private/tmp/panelink-p2-cargo-home). Sources in /private/tmp are disposable; durable candidate lock/tree/inventory are in docs/progress/p2-review/. Alacritty/vte's selected std path has uncapped combining and OSC storage; small feed chunks do not bound retained state. See docs/progress/P2-SOURCE-GATE.md. Host input is a separate allocation boundary: incomplete paste buffering, UTF-8 conversion, filtered queues and bounded EOF/error return must all be reviewed before enabling a UI. Explicit core shutdown closes the PTY and forbids further IO; natural child exit still drains final output. HUP tests must handshake after trap installation before claiming KILL escalation. The requested ms-rust skill is absent; no guideline compliance stamp. Graph index project: Users-sahillamba-panelink; refresh after source changes. Status observations never authorize dispatch. Path::components normalizes trailing separators; reject slash bytes before basename resolution. Global lease/operation/payload limits and expiry reclamation are now implemented; a real server must schedule maintenance and preserve non-reusable tombstones.

Graph freshness gotcha: application sources are currently untracked relative to
the initial documentation commit. Git-diff-based detect_changes may report only
tracked documents and miss application edits. Use a full index refresh plus
direct source/tests; do not treat an empty source diff as a current graph proof.

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

## Phase 2 — PTY and terminal engine feasibility (preparation underway)

- [x] Record exact dependency review candidates/features and preliminary PTY API assessment: docs/progress/P2-DEPENDENCIES.md.
- [x] Specify ordered experiment, proposed checks and Apple Terminal manual checklist: docs/progress/P2-EXPERIMENT.md.
- [x] Restore source access, capture candidate resolution/checksums and review decisive source gates: docs/progress/P2-SOURCE-GATE.md. Candidate set fails unchanged; no dependency adoption.
- [x] Review/adopt scoped replacement core graph, notices/build/API evidence: docs/progress/P2-REPLACEMENT.md. Local VT100 corrections + Rustix/openpty; host-input library not adopted.
- [x] Implement/verify bounded core per docs/progress/P2-CORE.md; parent-reviewed lifecycle fixes and 42 passing integrated tests in docs/progress/P2-CORE-RESULT.md. Core ownership released.
- [x] Adopt parser-only bounded decoder and verify pure snapshot renderer: docs/progress/P2-INPUT-REVIEW.md; 49 application + 39 decoder tests passed.
- [x] Resolve/review the complete host-probe graph, native/build scripts and input/cleanup semantics; adopt exact host pins after review: P2-SIGNALS-REVIEW.md and P2-PROBE.md. Core/component graph review is complete; the full Crossterm IO framework remains unadopted. See docs/progress/P2-HOST-INPUT.md.
- [x] Implement one-shot host/CLI probe with fixed buffers, bounded IO, restore-first cached cleanup and explicit launch map; see P2-PROBE-RESULT.md and P2-CLI-RESULT.md.
- [x] Prove restore-before-child-wait with a ready HUP-ignoring child on a private PTY, for explicit cleanup and unwind Drop; see docs/progress/P2-RESTORE-ORDER.md.
- [ ] Complete host acceptance after the incremental-rendering change: nine outer-PTY cases remain ignored locally; operator previously confirmed corrections without a fresh raw log; Apple Terminal retest/manual completion pending.
- [ ] Build one independent PTY with engine-owned screen state on Apple Terminal; deny programmatic clipboard requests.
- [ ] Validate shell/full-screen input, queries, resize and terminal cleanup.

Verify: use docs/progress/P2-EXPERIMENT.md and the explicit terminfo setup in README.md; `pty-probe` is implemented, while runtime acceptance remains pending. Source access is restored, but docs/progress/P2-SOURCE-GATE.md records failed allocation/launch/cleanup gates for the candidate set. Do not substitute a pipe, custom parser, or relaxed bound. Review viable replacements/corrections before P2B. Do not mutate parent environment or introduce an env-clearing shell wrapper. Full M1 also requires phases 3 and 4; phase 2 is not persistent-server or identity evidence.

Replacement checkpoint: docs/progress/P2-REPLACEMENT.md approves only the disposable core on Rust 1.99.1 / Apple Silicon macOS. VTE std must remain disabled; local engine source/patch/notices are preserved. Chosen history is 1,024 rows within the 2,000-row ceiling; capacity review includes resize/insert retention and remains toolchain-conditional. Parent independently reran nine engine and four PTY probe checks. The original complete-candidate adoption checkbox and Apple Terminal runtime checks remain unticked until host input/rendering review and actual integration. The core is now implemented and parent-verified; see P2-CORE-RESULT.md. The live Cargo registry recheck passed again with a writable CARGO_HOME. See P2-HOST-INPUT.md for the next source gate; no need to rerun rejected-candidate review. Luna saved partial follow-up fixes before a usage limit; parent finished/reviewed them. Parser-only crossterm-input is now adopted with reviewed fixed storage and no upstream IO framework; pure snapshot renderer is implemented. See P2-INPUT-REVIEW.md and P2-RENDER-RESULT.md. Prior component integration verified. Host and CLI owners have released their files. Parent source review and integrated checks passed: 66 application tests, nine explicitly ignored device-dependent tests; 39 separate decoder tests. See latest INTEGRATION.md. The harness establishes a controlling session, but independent /dev/tty open returns EPERM under the current sandbox. Do not replace the separate nonblocking open with duplicated stdio or count ignored cases as passes. Preserve the exact pins/notices; signal registrations intentionally live until process exit and must not be unregistered during bounded cleanup. Runtime output-stall/restore ordering and Apple Terminal checks remain gates before phase 3.

## Phase 3 — persistent server and reconnect (single-child experimental runtime integrated)

User explicitly authorized parallel GPT-6-Luna xhigh implementation of the transport, authentication, response/context/ownership, admission and reconnect list while Phase2 manual checks remain open. The parent reviewed and integrated the workers' bounded slices and a one-child private-PTY experiment. Actual authenticated socketpair reconnect is verified; pathname CLI acceptance remains separate. This is not completed host acceptance, a multi-pane product or provider recovery. If interrupted, inspect the task/evidence files and ownership before assigning work; do not assume an agent survives.

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
- [ ] Close descriptor/CLOEXEC/process-spawn source gate before enabling API launch; then specify exact launch/cursor/capability contracts and bounded registry implementation.
- [x] Review initial descriptor/spawn source inventory in P3-SPAWN-DESCRIPTOR-GATE.md: macOS PTY and standard-library socket/pipe flag windows remain; no concurrent-launch approval.
- [x] Review test-only atomic PTY opener spike and actual macOS evidence in P3-ATOMIC-PTY-SPIKE.md; this does not close socket/pipe/native-spawn gaps or adopt a production replacement.
- [ ] Specify/review production opener integration, minimum-OS evidence and owner-path spawn coordination before changing launch effects.
- [ ] Complete event/watch/launch/layout contracts and implementation; event framing alone does not enable subscriptions, full snapshot queues or multi-session lifecycle.
- [ ] Complete independent host lifetime/acceptance and remaining bounded runtime capabilities before marking Phase3 complete.

## Phase 4 — early OpenCode identity gate (not started)

- [ ] Pin installed OpenCode version; prove selected top-level identity and session changes with two agents in same project.
- [ ] Persist and acknowledge binding revisions; demonstrate or explicitly reject current-at-crash claims.
- [ ] Implement last-confirmed/manual fallback without latest-session heuristics.

## Phase 5 — workspaces, selection, history (not started)

- [ ] Resolve hierarchy; implement keyboard layout operations.
- [ ] Mouse routing, selection copying and context menus on Apple Terminal.
- [ ] Bounded normal history and screen checkpoints, safe search/recovery, retention/deletion.

## Phase 6 — alpha measurements and review evidence (not started)

- [ ] Ten-agent workload and build/resource measurements on specified hardware.
- [ ] Dependency/advisory/license inventory, installation and release evidence; firm review constraints before corporate use.

## Resume protocol

Read this file, AGENTS.md, docs/progress/*.md, then git status. Probe rustc/cargo/provider availability without printing secrets. Check source files against phase ownership and claims. Run current phase verification if tools exist. Continue first incomplete task and update STATE, evidence and last-verified before stopping. Do not assume an active subagent survives a session. Never discard uncommitted changes. Source files plus written evidence are the handoff; chat is optional context.

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
