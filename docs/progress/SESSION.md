# Implementation session checkpoint — 2026-10-17

## Current pause handoff — 2026-10-21 (read this first)

**Paused at local `main` commit `f82d968`** after the user requested a stop.
At the start of this handoff, the working tree was clean; `main` was three
commits ahead of `origin/main` (`e1126c7`, `c3e207e`, `f82d968`). These commits
were authorized locally; no push was requested. Check status again before
editing or any source-control action. This handoff/documentation refresh is
post-`f82d968` work and should not be mistaken for another source phase.
The user subsequently requested a push on 2026-10-21; inspect remote
tracking state after publication rather than treating the earlier no-push
observation as the current instruction.

**Done within their explicitly bounded scope:** Phase 3A two-pane splits
(`e1126c7`); Phase 3B individual pane stop/close, auto-collapse, re-split and
CLI/menu actions (`c3e207e`); P5 sanitized visible screen checkpoints, bounded
normal scrollback and read-only `panelink history list|show|search`, plus
stopped-workspace `clear` (`c3e207e`, `f82d968`). The later user-requested
independent reviewer found close-output, row-retry, clear-lock, end-time and
cursor issues. The parent addressed them before `f82d968`; see
[P5-SAVED-HISTORY.md](P5-SAVED-HISTORY.md). The final eight-check verifier
passed 8/8 (root library 500 passed/4 ignored, binary 93 passed, 15 Rust
ignored overall; cursor 6/6, history store 10/10, screen-checkpoint 3/3,
close/history runtime 5/5, decoder 39, Python 31). Strict Clippy, fmt,
rustdoc and component fingerprints passed. Earlier test failures and their
reruns remain recorded in the evidence; ignored tests are not acceptance.

**Open gates, not phase-completion claims:** parent Phase 3 still needs wider
event/watch/launch/layout contracts and host acceptance; parent Phase 5 still
needs full V-HISTORY reboot/layout and partial-tail acceptance. A deterministic
post-rename fsync-fault test, expiry after actual end-time sync failure,
interrupted temporary-file repair and representative disk-throughput review
remain unproved. Phase 2 Apple Terminal/clipboard sentinel acceptance and
Phase 4 live OpenCode identity/session-switch proof are also open. Normal
scrollback/checkpoints are distinct from provider conversations; no stored
terminal effects are replayed. The firm review, workspace hierarchy and
broader release options remain unresolved.

**On explicit resume:** read [ROADMAP.md](../../ROADMAP.md), AGENTS.md,
[SETUP.md](../SETUP.md), [P3B-CLOSE-RUNTIME.md](P3B-CLOSE-RUNTIME.md),
[P5-LIVE-CAPTURE.md](P5-LIVE-CAPTURE.md) and
[P5-SAVED-HISTORY.md](P5-SAVED-HISTORY.md). Inspect the current Git tree and
run targeted tests for the chosen gap. Use `python3 scripts/verify_non_ui.py`
for the integrated suite, counting ignored/blocked cases separately. Follow
the later user instruction to implement directly unless they specifically
request an agent. Await that instruction before beginning another phase.

## Historical checkpoints (superseded where inconsistent with the above)

Authority: user requested implementation, parent orchestration, gpt-6-luna xhigh where possible, and durable status for interruptions.

P1A, P1B and P1C source tasks are finished. Parent reviewed integration and independently verified 21 passing tests, formatting and Clippy with warnings denied. See INTEGRATION.md for actual commands, limits, fixes and next step. All task ownership is released after reports; no active agent is required for resumption.

Initial Rust absence was resolved during the session; Rust/Cargo 1.99.1 now tested and recorded. Cargo generated a dependency-free lockfile. ms-rust still absent. Cargo registry DNS remains unavailable, preventing fetching the next phase's terminal-engine candidates. No implementation commits or pushes attempted; preserve existing planning edits and current working tree.

ROADMAP.md is authoritative. Resume follow-up: preliminary exact dependency candidates and the phase 2 experiment are now recorded in P2-DEPENDENCIES.md and P2-EXPERIMENT.md. Parent reran the 21 foundation tests, formatting and Clippy successfully. Cargo registry DNS is still unavailable and OpenCode is still absent from active PATH. Next: obtain sources, resolve/review the full dependency graph and portable-pty spawn/cleanup behavior, adopt pins, then implement P2B. Alacritty's convenience PTY launcher cannot meet the explicit environment/cwd contract unchanged. No source/manifest changes, commits or pushes in this follow-up. The working binary currently only supplies help/version/doctor, not a multiplexer. No corporate approval or OpenCode recovery claim.

Latest resume supersedes the access blocker above: Cargo registry access restored; use a writable CARGO_HOME under current restrictions. Downloaded/resolved candidate sources reveal failed engine allocation and backend launch/cleanup gates. See P2-SOURCE-GATE.md and p2-review/ for exact evidence and durable resolution artifacts. Independent Luna xhigh review confirmed the engine gate failure; no agent owns files now. Foundation tests (21), fmt and Clippy pass again. P2B remains unstarted, with application sources/manifest/lockfile unchanged. Next action is viable replacement/upstream-correction review, followed by full dependency review before adoption. No commits/pushes or host/PTY/provider compatibility evidence.

Active replacement resume: P2-REPLACEMENT.md now approves/adopts the scoped core graph and local VT100 correction, with notices/provenance preserved. Independent disposable probes passed nine engine and four PTY checks, rerun by the parent. Core implementation is assigned to one Luna xhigh owner (src/terminal/**, tests/p2_pty.rs, P2-CORE-RESULT.md) under P2-CORE.md; parent owns shared files. Host input/rendering/manual Apple Terminal checks remain pending. Root lib export now references the in-progress terminal module: if interrupted before the owner finishes, inspect that module and rerun integration before assuming the working tree builds. No commits/pushes.

Latest completed checkpoint: the bounded replacement library core is implemented and parent-verified; see P2-CORE-RESULT.md. All retained lifecycle allocations now precede spawn; shutdown closes the controller and rejects later IO; dead input/replies are discarded and WriteZero is a hard failure. Exact paste bytes, fixed OSC/combining bounds, final output draining and handshaked HUP→KILL cleanup have regression coverage. Final locked/offline suite: 30 library + 1 binary + 11 PTY = 42 passed; fmt/Clippy exit 0; diagnostic CLI exits checked. Cargo registry search works with the writable temporary CARGO_HOME. OpenCode remains absent. No commits/pushes.

The follow-up agents hit a usage limit after partial core edits/source findings; parent finished/reviewed those edits and recorded results. No live agent ownership remains and no agent must survive to resume. Follow ROADMAP.md next-action: specify/review the bounded host input correction and its complete graph before adoption; P2-HOST-INPUT.md records source findings and acceptance criteria. Then implement snapshot rendering/restoration and the host probe. Apple Terminal compatibility, persistent server/reconnect and OpenCode identity remain unproved. The executable still has help/version/doctor only. Preserve the working tree; do not infer Phase 2 completion from the passing core library suite.

Latest host-component checkpoint: crossterm-input0.29.0-panelink.1 parser/types-only extraction is adopted under vendor/crossterm-input, with originals/patch/provenance/fingerprints/MIT notice. No upstream host framework is included. Parent independently reran39 decoder tests and verified13 fingerprints; root final suite49 passed (37lib+1binary+11PTY), fmt/Clippy/expected CLI exits passed. Pure streamed renderer has7 focused tests; maximum styled grid measured1,506,040 bytes. See P2-INPUT-REVIEW.md and latest INTEGRATION.md. A query/exit race in a drain test fixture was corrected; the test now measures normal EOF drain only. All agent ownership released; no temporary candidate source is required to resume, no commits/pushes.

Resume ROADMAP.md next-action: specify bounded owned-TTY host IO and restoration guard, then CLI pty-probe with outer-PTY EOF/error/panic/non-TTY tests before Apple Terminal validation. No host IO/raw-mode/restoration/interactive probe exists yet. The diagnostic binary still cannot run a managed terminal session. Phase2/M1 remain incomplete. Do not use unbounded Crossterm global readers or repeat the rejected candidate review; preserved parser-only component and snapshot renderer are the accepted building blocks.

Latest host probe checkpoint supersedes the unimplemented-host notes above: binary-private `src/p2_probe.rs` and the strict `pty-probe --terminfo <absolute-directory> -- <executable> [arguments...]` CLI are implemented and source-reviewed. Parent independently verified 66 application tests passing, nine device-dependent outer-PTY scenarios explicitly ignored, unchanged component's39 decoder tests passing earlier this resume, fmt/Clippy/CLI exits/fingerprints. See latest INTEGRATION.md and P2-PROBE-RESULT.md. Signal-hook0.4.5 defaults=false adds process-lifetime flag handlers only; no unregister in bounded cleanup. Exact graph/notices are preserved.

All ownership is released and no live agent is needed to resume. Cargo registry access was rechecked successfully again. The harness establishes a controlling session, but the probe's separate `/dev/tty` open returns EPERM in this sandbox. Do not duplicate stdio or weaken nonblocking ownership to bypass it. Run the nine ignored host scenarios in an environment with device access, add the ready-child restore-before-wait observation, then complete the Apple Terminal checklist in P2-EXPERIMENT.md. The existing unwind test only establishes eventual restoration. Phase2/M1 remain incomplete; OpenCode absent; no source-control publication occurred. Preserve all working files.

Latest operator-debug checkpoint: user ran the nine ignored host cases in Terminal, with4passed/5failed (four post-exit slave ENOTTY assertions, one no-drain process timeout). See P2-OPERATOR-RUN.md. The harness now keeps a controlling-session supervisor alive across probe exit/assertions, then drains/reaps only during teardown; preserves bounded metadata/process cleanup; finite overflow/tail producer; HUP-readiness marker. Two new host fixture regressions and an application-loop output-stall regression pass. Safe-Rustix experiments reproduce session-leader exit waiting for unread output, and retained-slave ioctl revocation; nonleader exits promptly. This supports the correction but does not establish that the complete corrected operator suite passes.

Parent final locked/offline application suite69 passed /9ignored, fmt/strict Clippy pass; graph refreshed. See latest INTEGRATION.md, P2-HOST-DEBUG.md and P2-STALL-DEBUG.md. No production semantics/dependency changes, commits/pushes; all agent ownership released. Resume with serial external command `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`, then remaining timing/manual Apple Terminal gates. Local sandbox still denies the full probe's independent /dev/tty open.

Latest operator confirmation: user replied "all corrected" after the requested host-suite rerun. Recorded as confirmation of resolution of the reported failures, with no new raw log/counts/timings. See P2-OPERATOR-RUN.md; prior failure history is retained. Next gate is manual Apple Terminal validation and the outstanding restoration-before-child-wait observation. Phase2/M1 remain incomplete. No source changes, tests, commits or pushes in this confirmation turn.

Latest UX resume checkpoint (2026-10-17): user reports most manual checks were fine, without identifying each completed item. Ctrl-Q intentionally exited the entire disposable probe, interrupting the checklist; remaining child-cleanup/restoration-timing observation is deferred at their request. Full-screen redraw on each change was traced to unconditional erase/full paint and corrected: pure render_diff plus two maximum-capacity, pre-spawn snapshot caches; baseline advances only after the full pending target is accepted by host writes. Resize/discard invalidates it. See P2-DELTA-RESULT.md and latest INTEGRATION.md for actual 81-pass/9-ignored verification. User should retest the changed renderer when ready; remaining manual gates are incomplete. No active agent ownership, dependency changes, commits or pushes. Resume ROADMAP.md; do not discard the working tree or infer persistent server readiness.

Latest restore-order checkpoint (2026-10-17): explicit cleanup and unwind Drop now have runnable private-PTY tests observing exact visual reset and saved termios while a ready HUP-ignoring child is still in its shutdown grace. The negative reversed-order mutation failed as intended; production order remains restore-first. Parent review hardened complete-marker PID parsing and exact ESRCH checking. Final83passed /9ignored, fmt/strict Clippy pass; see P2-RESTORE-ORDER.md and INTEGRATION.md. This scoped timing observation no longer depends on a manual child fixture. Remaining Apple Terminal redraw retest and interrupted manual acceptance remain pending; no phase3 runtime, dependency changes, commits/pushes or active ownership. OpenCode still absent from active PATH at this resume.

## Ctrl-X and P3-A1 checkpoint — 2026-10-17

Operator reports the short redraw/Unicode/vi/resize/paste/Ctrl-C checklist works; Ctrl-Q produces a host window-close confirmation. Source has no host-window-close action; exact host mapping unknown. Added/documented Ctrl-X as quit with Ctrl-Q alias; actual decoder/dispatch test confirms local quit queues no input while plain x still reaches the queue. Live Ctrl-X/outer-shell restoration remains pending, and no separate clipboard/full-checklist completion is inferred.

Parent approved the bounded pure wire-envelope task and supervised Luna xhigh implementation. Original std-only codec enforces the exact24byte v1.0 header, six kinds, kind-specific payload caps, strict complete-frame lengths, borrowed payloads, redacted diagnostics and validated prefix-preserving writes. Parent reviewed full source and independently verified full suite **94passed /9ignored**, fmt, strict Clippy and diff checks. Broader plan/typed messages/socket authentication/ledger bounds/journal/server remain pending; no persistent runtime claim. All ownership released, no dependency/lockfile changes, commits or pushes. Resume ROADMAP.md and P3-WIRE-TASK.md/P3-WIRE.md; next independent task is parent specification of ledger/global limits and typed handshake, while operator tests Ctrl-X.

## Global ledger and agent-control planning — 2026-10-17

Parent-specified and supervised Luna xhigh P3-B1 delivered global admission caps, explicit expiry payload/terminal-metadata reclamation preserving pending completion, and monotonic volatile lease issuance. Parent reviewed full source/accounting tests and independently verified103passed /9ignored, fmt/strict Clippy/diff checks. Full-global-count duplicate/conflict assertions were strengthened and their focused test passed. Ledger ownership released; no dependency/manifest/lock changes, commits or pushes. See P3-LEDGER.md and P3-LEDGER-TASK.md.

During this task the user explicitly requested a Herdr-like easy API/command system for agents to discover/read/control other managed panes/processes. Recorded requirementR-15; a separate Luna xhigh owner researches official Herdr behavior and edits only docs/AGENT-CONTROL.md. Parent review requested stable target references, non-interleaving input ownership, reusable caller/lease context for short-lived CLI commands, and scoped collaboration access without per-key prompts. This design must inform typed schemas/server routes before implementation; it does not imply autonomous scheduling or a working control API. Inspect the plan/ROADMAP.md for latest owner/status; Phase2 Ctrl-X/clipboard evidence remains pending.

Policy/design steering: user requested four-direction pane splits in right-click menus; a dedicated design owner edits docs/PANE-LAYOUT.md, with parent review/canonical integration. Agent-control plan has completed its targeting/ownership/context refinements and awaits final parent integration. User also authorized parent commits after each fully completed/verified phase, including affected code/docs/evidence. No push authorized and workers remain no-commit. Pending phases are not marked complete merely to create a commit; do not stage mixed incomplete work. See AGENTS.md and ROADMAP.md.

## Agent-control/four-way split design handoff — 2026-10-17

Both design owners released files. Parent reviewed docs/AGENT-CONTROL.md (official Herdr behavioral references, independently written R-15 proposal) and docs/PANE-LAYOUT.md (R-16 four-direction menus/shared API). Refinements include exact target refs before input, reusable caller lease/sequence context, explicit input-owner CAS/handoff, layout-owned resize, dedup before stale-layout preconditions, tentative split IDs before committed watermarks, viewport-bounded menus and truthful resize-pending geometry. These are design contracts/proposed defaults, not implemented API/UI or completed host tests. Canonical spec/discovery and roadmap link both plans.

Document validation checked6documents and38local links with no missing links, trailing whitespace or conflict markers; git diff --check passed. Earlier code verification remains103passed /9ignored; no new production code was changed during final design review. Index refresh failed with Transport closed after the earlier owner refresh, so no final refreshed-graph claim is made. All ownership released. Next independent work is typed schemas/bound choices/socket API source review; Phase2 Ctrl-X/clipboard evidence remains pending.

User authorized parent commits after every fully completed and verified phase, including its affected code/docs/evidence. AGENTS.md records this override of older no-commit guidance; workers still do not commit, no push authorized. Current phases remain unfinished and no completion commit was made merely to checkpoint partial work.

## Typed control and event framing — 2026-10-17

Parent specified two disjoint Luna xhigh tasks: control_protocol.rs/P3-CONTROL.md and protocol_wire.rs/P3-EVENT.md. They delivered schema1 borrowed Request payloads for bounded Read/Text/Paste/Submit/Interrupt, full numeric target tuples and expected owner revisions; Event kind7 uses requestID0 with explicit payload/frame caps. Parent reviewed source, added a cross-component ledger-identity test, and independently verified **122 passed / 9 ignored** (86 library, 21 binary, 4 host fixture/preflight, 11 core PTY), fmt/strict Clippy/diff checks. No dependencies or manifest/lock changes. Transient worker compile/format issues were corrected and recorded before passing verification. All ownership released.

Pure codecs validate bytes and preserve nonreplayable input classification; they do not authenticate or resolve live targets, claim input ownership, run commands, send events, or provide the agent-control CLI. Initial server API launch requests will fit the complete 65,536-byte encoded budget or reject before effects; probe 1 MiB capture remains separate, staging deferred. Future queue accounting now includes 24-byte headers and control-reply reserve; no queue implementation exists.

Parent checked installed Rust 1.99.1 and Rustix 1.1.5 peer-credential entry points. std peer_cred compile check fails E0658 (unstable), Rustix socket_peercred is Linux-only. P3-SOCKET-GATE.md records source evidence and required stable safe macOS wrapper review before operational sockets; no manifest adoption or weaker identity fallback. Code-index tools remain unavailable with Transport closed, so source fallback and durable evidence are authoritative for this update. Next is wrapper/API review and typed handshake/owner/context/response contracts. Phase2 Ctrl-X/clipboard acceptance and Phase3 runtime remain incomplete; saved completion-commit policy remains, no new completion commit/push.
