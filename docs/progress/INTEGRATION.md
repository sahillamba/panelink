# Phase 1 integration evidence — 2026-10-17

Status: COMPLETE for the dependency-free foundation. This is not completion of the spec's M1 terminal/OpenCode gate.

## Model assignments and delivered code

Three gpt-6-luna agents at xhigh implemented disjoint tasks under the parent orchestrator's explicit phase specs: P1A CLI/package, P1B protocol ledger, P1C launch environment/resolution. Parent reviewed contracts, source and integration; no model cost/token claims. The generated Cargo.lock contains only Panelink. No third-party crates, mimalloc, copied application code or agent skills were added.

## Actual integrated verification

Host reports arm64; CLI reports macos/aarch64. rustc 1.99.1 (58cf8f9c3 2026-10-13); cargo 1.99.1 (5f94df478 2026-08-27). SDK discovery returned /Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk. Minimum deployment OS has not been selected or measured. rust-toolchain.toml selects the installed stable toolchain with rustfmt/Clippy; tested version is recorded above.

Parent independently ran:

| Command | Actual result |
| --- | --- |
| `cargo fmt --check` | Exit 0 |
| `cargo test --locked --offline` | Exit 0: 20 library tests and 1 binary test passed; 0 doctests |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0 |
| `cargo metadata --offline --no-deps --format-version 1` | Exit 0; dependencies array empty |
| `git diff --check` | Exit 0 |

P1A evidence additionally records actual CLI checks: --help and --version exit 0; doctor exit 1 with runtime-unimplemented status; unknown option exit 2. P1B and P1C evidence record targeted protocol (8) and launch (12) passing suites and module formatting checks. Early failed checks due to missing tools are historical and superseded by these results.

The first integrated run found a protocol test expecting high watermark 2 after admitting sequence 3; corrected expectation to 3. Parent source review also caught/corrected CLI exit type mismatch, relative executable normalization accepting trailing slash/dot, and observation/admission type conflation. A subsequent full test suite and lint check passed. Initial Xcode SDK discovery emitted sandbox cache/event-stream warnings; final tests and Clippy completed successfully. No representative build-time benchmark is claimed from these short checks.

## Current limits

Toolchain configuration selects installed `stable`, currently rustc 1.99.1. An attempted exact `1.99.1` channel pin made rustup try to download a separately named toolchain and write its restricted home temp directory; it was replaced with the already installed stable channel. Exact reproducible toolchain pinning remains a release-preparation task. After this correction, the full format/test/Clippy sequence passed again. No exact toolchain lock is claimed.

The executable is a diagnostic CLI. There is no TUI, PTY, daemon, socket, durable journal/history, OpenCode adapter, clipboard integration or host interactive compatibility proof. Ledger is volatile, requires synchronized mutable access and monotonic caller time, and takes caller-provided IDs. Total lease/retained-memory limits, expiry cleanup/tombstones, peer authority, actual process effects and persistent recovery must precede server use. Launch resolution checks filesystem paths and modes; it does not eliminate time-of-check/time-of-use changes or authorize credentials.

No provider identity, crash exactly-once, corporate approval, or security-certification claim. ms-rust remains absent. Git working files are saved, with no implementation commit/push attempted in this session.

## Next action and concrete blocker

Review exact terminal engine and host IO versions/features and specify phase 2's PTY experiment before implementation. `CARGO_NET_RETRY=0 CARGO_HTTP_TIMEOUT=10 cargo search alacritty_terminal --limit 1` exited 101: unable to resolve index.crates.io while downloading config.json. No registry source cache exists at ~/.cargo/registry/src. Obtain crate sources through an approved network/build environment; do not replace terminal emulation with a custom parser to evade this blocker. Later OpenCode identity checks require an installed/versioned OpenCode executable.

Resume with `ROADMAP.md`; no live subagent state is required.

## Phase 2 resume — 2026-10-17

Parent reran format, locked/offline tests and Clippy: exit 0, 21 tests passed. Rust/Cargo still report 1.99.1. Registry search again exited 101 because index.crates.io could not resolve; OpenCode was not found on the active PATH. Existing uncommitted work was preserved, with no commits/pushes.

Phase 2 preparation now has a [dependency review](P2-DEPENDENCIES.md) and [experiment plan](P2-EXPERIMENT.md). Parent reviewed versioned upstream documentation and launch API behavior; a gpt-6-luna xhigh agent drafted the bounded experiment/checklist under exclusive ownership. Exact review candidates are not manifest adoption, resolved transitive pins, audit results or compatibility evidence. The engine's convenience PTY launcher conflicts with explicit environment/cwd requirements, so a separate PTY owner needs review. No Rust source or dependency manifest was changed during this resume. Runtime implementation remains blocked on crate sources and completion of that review.

Parent reviewed the plan and requested corrections to query conformance/classification, tested TERM capabilities, parser/combining allocation bounds, bounded teardown and the prototype-only lifetime assumption; Luna applied them. Parent independently checked 22 local documentation links (0 unresolved) and `git diff --check` (exit 0); roadmap/progress whitespace/conflict-marker checks also found 0 errors. These checks verify documentation only. All agent file ownership is now released; no live agent is required to resume.

## Cargo-access and source-gate resume — 2026-10-17

Cargo registry access now works. A disposable manifest and writable temporary CARGO_HOME fetched 77 packages and produced locked/offline metadata and an Apple-target feature tree; all archive hashes matched its lockfile. The application manifest/lockfile and Rust source remain unchanged. See [source-gate evidence](P2-SOURCE-GATE.md) and [review artifacts](p2-review/INVENTORY.md).

Source inspection rejected the unchanged candidate set: uncapped engine combining/OSC storage prevents the experiment's finite memory bound; portable-pty adds cwd fallback, SHELL insertion and writer-drop input effects. An independent gpt-6-luna xhigh review confirmed the engine failures and lack of a public configuration fix. No candidate builds, PTY runtime checks or Apple Terminal compatibility claims. Full dependency/advisory/notice review remains incomplete. The experiment explicitly requires stopping before P2B when these gates fail.

Parent reran formatting, locked/offline tests (20 library + one binary) and Clippy with warnings denied; all exited 0. OpenCode remains absent from the active PATH. Resume by reviewing a viable engine/backend or scoped upstream correction, then complete dependency review before adoption. No commits or pushes; no active file ownership remains.

## Bounded replacement core integration — 2026-10-17

This checkpoint supersedes the earlier source-access blocker and unstarted-core status. Cargo registry search succeeded again using `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`; Rust/Cargo remain 1.99.1 on macOS 26.5.2 arm64. The [replacement review](P2-REPLACEMENT.md) records adopted exact core dependencies, local engine corrections, full graph/build/notices/advisory evidence and toolchain-conditional allocation model.

The bounded same-process library core is implemented and parent-reviewed. Parent verified the final tree with locked/offline tests: **30 library + 1 binary + 11 real-PTY tests = 42 passed**, zero failed, zero doctests. Formatting and Clippy with warnings denied exited 0. CLI help/version exited 0, doctor 1, unknown option 2, as documented. Actual summaries and review fixes are in [P2-CORE-RESULT.md](P2-CORE-RESULT.md). The HUP test now waits for trap readiness, proves SIGKILL and the one-second grace interval; paste assertions use child-observed exact bytes. Explicit cleanup closes the controller, rejects further IO and caches cleanup; natural child exit still drains final output. A zero-byte nonempty write discards/disables pending input.

The Luna core follow-up and host-source reviewers hit a usage limit. Parent inspected the saved partial lifecycle changes, completed regression tests and this evidence. No agent file ownership remains. No commits/pushes. The CLI has no host probe; this is not Phase 2 or M1 completion. OpenCode is still absent on active PATH. Next action is the [bounded host-input gate](P2-HOST-INPUT.md), followed by snapshot renderer/restoration and actual Apple Terminal checks. Crossterm is not adopted: incomplete sequence/paste allocation, conversion expansion, filtered queues, EOF/error return and size fallback require a bounded reviewed path.

## Parser-only decoder and snapshot renderer integration — 2026-10-17

The parent adopted the bounded parser/types-only `crossterm-input` local variant after inspecting normalized source deltas, adapter guards and provenance, independently rerunning all 39 tests, and validating all 13 fingerprints plus the preserved registry originals/license. Source is now durable under `vendor/crossterm-input/`; temporary candidate paths are not needed to resume. [P2-INPUT-REVIEW.md](P2-INPUT-REVIEW.md) records exact selection, complete component graph, licenses, source integrity, build inputs, dated maintenance and baseline OSV results. All-platform lockfile: 15 third-party components; Apple-filtered graph: 12. The filtered metadata deliberately omits other-platform packages. Only the previously reviewed libc/Rustix custom-build targets remain. Direct unicode-width reuses the existing version/features.

The original Panelink snapshot renderer is implemented, streamed and reviewed. Seven tests cover text/control canaries, conservative width fallback, styles/cursor, bounds and write failure; its 80×240 styled/combining counting case emitted 1,506,040 bytes. Parent retained the semantic <=4 MiB test bound and removed the brittle exact-byte-count assertion; the measurement remains an observation, not a required encoding length. No frame is retained by this function.

The initial resume recheck exposed a test-fixture race: final-output drain also sent a query without reading the answer, so a legitimate hard write failure could start required cleanup. Parent removed that query from the normal-drain fixture; query-response fixtures consume their replies, and hard writes have independent injected checks. The corrected drain fixture passed targeted verification and the final full suite. No claim is made that output draining survives a hard write failure.

Actual final parent checks with `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

| Command | Result |
| --- | --- |
| `cargo test --locked --offline` | Exit 0: 37 library + 1 binary + 11 real-PTY = 49 passed; 0 doctests. |
| Candidate `cargo test --locked --offline --manifest-path /private/tmp/panelink-host-decoder/Cargo.toml` | Exit 0: 39 passed (30 upstream parser + 9 adapter). Copied vendor fingerprints match. |
| `cargo fmt --check` | Exit 0, no output. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0; selected local dependency also compiled/checked. |
| CLI help/version/doctor/unknown through `cargo run --locked --offline -- ...` | Expected exits 0/0/1/2. |

The separate decoder suite can now be resumed with `cargo test --locked --offline --manifest-path vendor/crossterm-input/Cargo.toml` using the same writable Cargo home. It is a dependency suite, so root Cargo tests do not run those 39 tests automatically. Parent has refreshed the code graph. All agent file ownership is released; no commits/pushes. The CLI remains diagnostic only. Next is a precisely specified bounded host IO/terminal-restoration guard and `pty-probe`, including outer-PTY failures/panic/EOF/non-TTY verification, before actual Apple Terminal checks. Phase 2 and M1 are incomplete; server/reconnect and OpenCode identity remain later gates.

## One-shot host probe integration — 2026-10-17

Cargo access was rechecked successfully using the writable `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`: `CARGO_NET_RETRY=0 CARGO_HTTP_TIMEOUT=10 cargo search signal-hook --limit 1` exited 0 and returned 0.4.5. The exact host dependency is reviewed in [P2-SIGNALS-REVIEW.md](P2-SIGNALS-REVIEW.md): default features disabled, two added registry packages, 17 third-party lockfile components / 14 Apple-target components. Signal-hook's selected build script is empty; no cc/C source, channel, iterator, extra thread or custom signal FFI. Notices, archive hashes, filtered graph and dated baseline advisory observations are preserved in `p2-core-review/`. Handler registrations live until this one-shot process exits; unregister is excluded because its source has an unbounded barrier and does not restore dispositions.

Luna xhigh owners implemented the host adapter and CLI under exclusive file lists; a separate Luna reviewer assessed signal APIs. Parent specified the loop/lifecycle bounds and reviewed integration. The executable now accepts `pty-probe --terminfo <absolute-directory> -- <executable> [arguments...]` on macOS/aarch64. It captures a bounded explicit client environment, owns a separately opened NONBLOCK/CLOEXEC `/dev/tty`, decodes input through the fixed parser, renders snapshots into one fixed 3 MiB frame and restores host state before bounded child shutdown. Runtime diagnostics also use a nonblocking 100 ms write budget, with no blocking stderr fallback. This is a disposable child lifetime, not server/reconnect behavior. The source accounting shares the existing 20 MiB snapshot allowance across snapshot/frame/decoder/launch copies; see [P2-PROBE.md](P2-PROBE.md). It is not an RSS or hard realtime guarantee.

Parent review corrected same-chunk pending-sequence timestamps, local quit during child-exit draining, initial executable-length admission, nonprintable key forwarding, one-byte incomplete UTF-8 timeout, queued input flushing and post-cleanup diagnostics. Final harness review moved original termios/flags capture before subprocess spawn (including the no-drain test, which had saved raw state), removed a rapid-exit raw-observation race, and capped each capture drain at eight 8 KiB reads so continuous output cannot hide a caller's deadline. Ignored fixtures compile but have not been validated by execution.

Actual final parent checks after those corrections:

| Command | Result |
| --- | --- |
| `cargo test --locked --offline` | Exit 0: 37 library + 16 binary + 2 host fixture/preflight + 11 core real-PTY = **66 passed**; **9 host tests ignored**; 0 doctests. |
| `cargo test --locked --offline --manifest-path vendor/crossterm-input/Cargo.toml` (earlier this resume, component unchanged) | Exit 0: **39 passed**. |
| `cargo fmt --check` | Exit 0, no output. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0. |
| Binary help/version/doctor/unknown/relative-terminfo syntax | Expected exits 0/0/1/2/2; fixed diagnostics. |
| Local component fingerprints | All 16 VT100 + 13 decoder records match. |
| `git diff --check` | Exit 0. |

The harness's `setsid`/`TIOCSCTTY`/`tcgetsid == getsid` check succeeds. The actual probe's independent `/dev/tty` open returns EPERM in this filesystem-restricted command environment, before child/raw-mode effects. Nine outer-PTY runtime scenarios are explicitly ignored with that reason: normal rendering/restoration, input/paste, local quit, SIGTERM, exit draining, output stall, resize, overflow/suffix flushing and hangup. Run `cargo test --locked --offline --test p2_host -- --ignored` where the device open is permitted, then address failures. Do not count ignored cases as passes. The panic unit fixture proves eventual saved-termios restoration after unwind; it does not observe restoration during a ready child's HUP grace window. That ordering test remains pending. See [host result](P2-PROBE-RESULT.md) and [CLI result](P2-CLI-RESULT.md).

No actual Apple Terminal shell/editor/Unicode/paste/clipboard checklist was run. README gives the explicit `tic` and probe commands; terminfo existence is not conformance. Phase 2 and M1 remain incomplete. OpenCode is still absent from the active PATH. All agent file ownership is released; root graph refreshed; no commits/pushes. Resume the external-device/runtime gate in ROADMAP.md before phase 3.

## Operator host-test debugging — 2026-10-17

The user ran the nine ignored cases outside the sandbox: four passed, five failed. Four failures were retained-slave ENOTTY assertions after probe exit; the fifth was the five-second process-output-stall deadline. Exact case names and evidence provenance are in [P2-OPERATOR-RUN.md](P2-OPERATOR-RUN.md). This supersedes the previous all-unrun operator status, but no corrected operator suite has passed yet.

Parent supervised Luna xhigh diagnosis and disjoint corrections. The host harness now keeps a separate controlling-session supervisor alive until post-probe assertions finish, atomically publishes bounded PID/status files and preserves terminal stdin for the asynchronous probe. Cleanup signals only the still-owned supervisor group, releases/reaps with deadlines, and drains private output only after the no-drain process-exit assertions. Two runnable regressions cover retained terminal queryability and suppressed output followed by clean supervisor teardown. The overflow fixture sends the overflow byte/tail in one short write and produces no bytes afterward. The external-signal fixture waits for the child's trap-readiness marker. See [harness result](P2-HOST-DEBUG.md).

A safe-Rustix disposable experiment reproduced both consequences of the old fixture: retained-slave tcgetattr fails after a controlling session leader exits, and a leader with 256 queued output bytes stays pending until the master is drained. An otherwise equivalent nonleader exits before draining. This reproduces a session-exit delay with and without a read-only /dev/tty open, supporting the supervisor change. It does not identify the exact stack of the operator's former timeout. The added application-loop unit regression returns OutputStalled in1,001ms, confirms NONBLOCK across raw entry and verifies exact termios restoration plus child status after bounded cleanup. No production logic/deadline, dependency, parser or manifest was changed. See [stall diagnosis](P2-STALL-DEBUG.md).

Final parent verification after all corrections, using the writable Cargo home:

| Command | Actual result |
| --- | --- |
| `cargo test --locked --offline -q` | Exit0: 37 library + 17 binary + 4 host fixtures/preflight + 11 core PTY = **69 passed**, **9 host cases ignored**, 0 doctests. |
| `cargo test --locked --offline --test p2_host` | Exit0: 4 passed / 9 ignored. |
| Focused unread-PTY `run_loop` test with `--nocapture` | Exit0: 1 passed, stall observed after1,001ms; total2.74s including cleanup. |
| `cargo fmt --check` | Exit0, no output. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit0. |
| `git diff --check` | Exit0. |

The 39-test decoder component and vendored sources are unchanged from the preceding verified checkpoint; that separate suite was not rerun for these test-only corrections. Parent refreshed the root code graph. All agent ownership is released, no commits/pushes. The nine corrected process/device scenarios still require Terminal: `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`. Run them serially to isolate any remaining failure before the manual Apple Terminal checklist. Phase2/M1 remain incomplete.

Latest operator confirmation: user replied "all corrected" after the requested host-suite rerun. Recorded as confirmation of resolution of the reported failures, with no new raw log/counts/timings. See P2-OPERATOR-RUN.md; prior failure history is retained. Next gate is manual Apple Terminal validation and the outstanding restoration-before-child-wait observation. Phase2/M1 remain incomplete. No source changes, tests, commits or pushes in this confirmation turn.

## Incremental display integration — 2026-10-17

The user reported full-screen reloads on every change and mostly-successful manual terminal behavior, without per-item results. Ctrl-Q stopped the disposable probe before completion; remaining child-cleanup/restoration-timing observation is deferred at their request. The checklist now distinguishes stopping the probe from quitting its editor. No manual gate is marked complete from that report.

A supervised Luna xhigh owner implemented original pure `render_diff` under exclusive renderer/evidence ownership. Parent reviewed it, integrated preallocated displayed/target snapshot caches, preserved commit-after-complete-write semantics across partial output, and invalidated the baseline on resize/discard. Stable views update changed cells/cursor; wide-cell topology changes repaint their row; startup/invalidated state repaint fully. Identical frames write nothing. Typed styles and scalar sanitization remain in the output path; no raw child controls or new dependencies. Memory accounting remains within the existing shared source-derived allowance. See [P2-DELTA-RESULT.md](P2-DELTA-RESULT.md) for detailed evidence, including the corrected immediate-exit fixture race discovered during verification.

Final parent commands using the writable Cargo home: `cargo test --locked --offline -q` exited 0 (**81 passed / 9 ignored**: 48 library, 18 binary, 4 host fixture/preflight, 11 core PTY); `cargo fmt --check`, `cargo clippy --locked --offline --all-targets -- -D warnings` and `git diff --check` exited 0. No dependency/vendored component changes; separate unchanged decoder tests were not rerun. Root graph refreshed, ownership released, no commits/pushes. Visual retest and the deferred timing/manual gates remain pending; phase 3 has not started.

## Automated restoration ordering — 2026-10-17

A supervised Luna xhigh task added two cfg(test) cases using a private host PTY and a real child that handshakes after ignoring HUP. During production explicit cleanup and unwind Drop, the observer verifies all saved termios fields plus the exact 31-byte visual reset while the child is alive and cleanup is still waiting. The child remains until the one-second HUP grace expires; cleanup completes within a bounded receive deadline. A reversed-order negative mutation failed and was restored. Parent reviewed the fixture, required complete PID markers before signaling and exact ESRCH for the final unwind liveness check. No production behavior/dependency changes. See [P2-RESTORE-ORDER.md](P2-RESTORE-ORDER.md).

Parent focused observation: restore at1ms while cleanup took1,015ms explicit /1,013ms unwind. Final full suite exit0: **83 passed /9ignored**, consisting of48library +20binary +4host fixture/preflight +11core PTY; fmt/strict Clippy/diff check exit0. No unchanged decoder rerun. The private-PTY ordering observation is complete, so the operator need not construct a special child fixture for that scoped check. Apple Terminal visual redraw/cleanup and remaining manual acceptance are still pending. Graph refreshed, ownership released, no commits/pushes.

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

## Stable macOS credentials and handshake — 2026-10-18

Parent reviewed the installed SDK declaration/man page and exact existing
libc0.2.191 binding, features, graph, preserved notices and build script before
adopting a direct macOS/aarch64 dependency edge (defaults disabled). No package,
version, feature, license file or build target was added. The original wrapper
borrows a UnixStream, confines one unsafe getpeereid call to typed live stack
outputs and returns OS failures immediately. Same-user verification compares
kernel effective UID with safe Rustix geteuid; no PID/client assertion is used.
Three focused tests pass for kernel pair credentials/data preservation, real
ENOTSOCK with file ownership retained, and numeric UID-mismatch denial. No real
second-user fixture was available. See [review](P3-PEER-REVIEW.md).

The private pathname fixture creates an exclusive0700 directory, but its bind
actually failed with OS1/PermissionDenied/EPERM here. It is explicitly ignored,
not silently skipped; [P3-SOCKET-GATE.md](P3-SOCKET-GATE.md) records the external
command and remaining production filesystem/listener/deadline gates.

A separate Luna xhigh owner delivered pure28byte Hello/40byte Welcome schema1,
nonce echo, generation/lease/TTL validation and required/optional capability
negotiation. Parent read full source, integrated its export, and added decoded
semantic-rejection and shared ledger TTL invariant tests. Capabilities remain
vocabulary, not implemented server effects/grants/issuance. See
[handshake evidence](P3-HANDSHAKE.md) and [task](P3-HANDSHAKE-TASK.md).

Actual parent checks using `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

- `cargo test --locked --offline -q`: final exit0, **139passed/10ignored**
  (103library,21binary,4host fixture/preflight,11corePTY; one library pathname
  fixture plus nine host cases ignored; zero doctests).
- `cargo clippy --locked --offline --all-targets -- -D warnings`: exit0.
- `cargo fmt --check` and `git diff --check`: exit0; document check covered
  11documents/89local links with zero missing links/whitespace/conflict issues.
- Post-adoption `cargo tree --locked --offline -e features -i libc`: same exact
  version/default/std features, one new direct Panelink edge.

Initial full-suite run before handshake integration failed the existing
`unread_outer_pty_returns_output_stalled_before_bounded_cleanup` assertion that
bounded cleanup had collected the child status. It detected output stall at
1000ms as expected. Focused rerun passed at1001ms, and the final full suite
passed unchanged. This is an intermittent cleanup/timing observation, not a
resolved defect or guaranteed scheduling bound; preserve it for follow-up if it
recurs. No PTY cleanup code or assertion was changed to make verification pass.

Parent graph connector still returned Transport closed, but the worker's
connector successfully indexed and refreshed final source:832nodes/3759edges,
fast/nonpersistent. All ownership released. Phase2 manual acceptance and Phase3
runtime remain incomplete; no phase-completion commit or push was performed.

## Parallel single-child server integration — 2026-10-18

The user explicitly requested parallel GPT-6-Luna at xhigh effort for the
five-item transport/authentication/context/admission/reconnect list. Three
workers used exclusive file ownership for bounded implementation slices; the
parent specified interfaces, reviewed source and integrated shared modules,
startup storage, runtime and CLI routing. Follow-up worker reviews and real
PTY/socketpair scenarios ran independently of the still-open Phase2 host gate.

Delivered experimental scope:

- A protected local listener and nonblocking connect/frame IO with absolute
  deadlines. Kernel same-UID authentication precedes all handshake reads.
  Existing endpoints are refused, never automatically unlinked as stale.
- Typed responses and session/resume operations; durable, locked caller
  contexts with uncertainty reconciliation; input-owner claim/release/handoff
  using explicit target and owner revision. Input is never automatically resent.
- One state owner behind at most eight connection workers, with bounded
  queues and duplicate admission before new live-state preconditions/effects.
  No event capability or subscription/snapshot queue implementation is claimed.
- Explicit initialization of private generation/lock/journal storage and
  durable Intent before the startup spawn. Result uncertainty retains the owned
  child as Unknown; incomplete journal facts never execute recovery actions.
- One private server-owned PTY with sanitized bounded reads, typed input and
  SIGINT, exposed through experimental `server-probe`/`control` commands.
- An actual authenticated socketpair integration scenario: lose a Submit ACK,
  resume the same generation/lease, reconcile status without retransmission,
  observe the same child PID, explicitly detach and reconnect again. An ordered
  application marker proves the deliberate duplicate regression probe did not
  repeat the original input effect.

Parent review corrected response key/target consistency, context-slot
identity/rollback transitions, normal-startup metadata recreation/reset risks,
counter opening before the singleton lock, unsafe directory preparation order,
oversized frontend read acknowledgment and PTY poll-error request starvation.
Added targeted regressions and rechecked dependency graph/features. A
startup test-directory collision was corrected with an atomic suffix; no
production assertion was weakened. The existing intermittent Phase2 child-reap
observation remains historical; this combined run passed it unchanged.

Current parent verification before the external CLI fixture was added:

- `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline -q`:
  exit0, **248passed/12ignored** (201 library,28 binary,4 host preflight,
  11 core PTY,3 logical-PTY and1 socketpair reconnect).
- The12 ignores comprise nine host scenarios, two pathname socket scenarios
  and one internal subprocess context-lock helper. The helper is deliberately
  invoked by its passing parent regression; it is not an external failed gate.
- `cargo clippy --locked --offline --all-targets -- -D warnings` with that
  CARGO_HOME: exit0. No new package was added. Existing Rustix1.1.5 gains only
  the reviewed macOS net feature edge; its resolved feature tree was checked.

The foreground server is one explicit child, not a finished multiplexer.
Generations are monotonic within established storage; random initial seeds
are not cross-domain uniqueness or hostile backup-rollback protection. A nominal
100ms input drain can additionally enter the core's two-second cleanup path on
error; no hard whole-call100ms guarantee. Query replies behind a discarded
input suffix are also discarded. Applied means accepted PTY bytes or signal,
not child application success. Live scrollback, general API launch/restart,
events/watch, layouts, multi-session discovery and provider recovery remain
unimplemented. Same-UID trust does not isolate local agents from each other.

Parent code graph connector remains unavailable with Transport closed; workers
used their functioning connector before source inspection. Source and test
outputs remain authoritative. The actual pathname CLI acceptance fixture and
remaining Phase2 Ctrl-X/clipboard/host checks stay explicit acceptance work.
See [P3-STORAGE.md](P3-STORAGE.md), [P3-CLI.md](P3-CLI.md),
[P3-RECONNECT.md](P3-RECONNECT.md) and [P3-SOCKET-GATE.md](P3-SOCKET-GATE.md).
Phase2 and Phase3 remain incomplete, so no phase-completion commit or push was
made. The user's parent phase-completion commit authorization remains active.

Final fixture integration: the parent independently compiled and attempted the
actual binary pathname scenario. `cargo test --locked --offline --test
p3_pathname -- --ignored --nocapture` exited101, **0passed/1failed**: actual
server stdout was `listener_unavailable`, and the fixture's independent bind
reported EPERM. This is recorded as a blocked acceptance attempt, not a passing
or silently skipped test. The fixture bounds subprocess waits and captured
streams, compiles the checked-in terminfo privately, and is directly runnable
outside the sandbox:

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --test p3_pathname -- --ignored --nocapture
```

With this explicitly ignored scenario included, the final standard suite
exited0: **248passed/13ignored**. Strict all-target Clippy also exited0. The
additional ignore is the pending actual CLI/pathname scenario; the internal
context-lock helper distinction above still applies. See
[P3-PATHNAME.md](P3-PATHNAME.md) for the fixture scope and external handoff.

Final formatting and diff checks exited0. The parent checked39 current
documents and122 local Markdown links: no missing destinations, trailing
whitespace or conflict markers. Worker file ownership is released; the parent
took back the three canonical planning documents for final link corrections.
No final code-index refresh is relied upon as verification evidence.

Operator follow-up, 2026-10-18: the pathname run reached successful startup and
the init response, then failed while the test parsed `init generation`. Parent
reproduced a fixture bug: some JSON readers received the labeled subprocess
capture rather than its JSON object. Central object extraction fixes those
readers; the new regression fails before and passes after the change. Focused
pathname target now reports1passed/1ignored, and focused strict Clippy, fmt and
diff checks pass. The prior whole-suite248passed/13ignored result remains the
last full-suite run; no new full-suite count is inferred. External acceptance
still needs the corrected ignored test rerun.

User also requested a single-word launch command. Added executable development
launcher `bin/panelink`: no arguments opens the disposable shell prototype with
private temporary terminfo and cleanup; explicit arguments forward to the built
binary. `/bin/sh -n` and `bin/panelink --version` pass. README records the one-time
symlink into the already configured `~/.local/bin`. The restricted session cannot
create that home-directory symlink; user runs that final command. No phase
completion or commit is claimed from these fixes.

Corrected external pathname rerun, 2026-10-18: the operator replied "passed"
after the supplied rerun command. P3-PATHNAME.md records the fixture as
operator-confirmed passing, without inventing raw output/counts/timing. The
pathname scenario is no longer an outstanding operator gate; it stays ignored
by default in the restricted local test environment. Remaining immediate host
checks are Ctrl-X/outer-shell restoration and the separate clipboard sentinel.
Phase2/3 remain incomplete; the phase-completion commit policy remains active.

## Ctrl-X and single-word launcher confirmation — 2026-10-18

After being asked to run `panelink`, press Ctrl-X and confirm return to the
normal outer shell without a close dialog or broken typing, the operator
replied "works". Record that scoped launcher/local-quit/restoration check as
operator-confirmed passing. No process timing, raw output or new host-version
record was supplied. The separate clipboard sentinel remains unconfirmed; this
response does not establish every item in the original full checklist.

## Clipboard reply injection corrected; zsh launcher — 2026-10-18

The operator's manual OSC52 read produced `52;c;` at the shell prompt, then
`52` and `c` command-not-found errors. Source confirmed that the clipboard
callback queued an empty OSC52 response into child input. Although it contains
no clipboard data, a shell with no clipboard-response consumer can interpret
its bytes as input. This is a failed manual acceptance observation, not proof
that clipboard contents were exposed or changed.

The callback now discards clipboard reads without responding, matching the
write/clear denial. The canonical spec, core contract and experiment were
updated; four supported CSI queries retain their existing behavior. The former
empty-response contract and source-only tests missed shell behavior. Unit and
real-PTY regressions both failed before the fix; the PTY observed16injected
bytes for BEL/ST reads. After the fix both pass with zero reply bytes, and an
exact hex comparison confirms only the explicitly submitted next line reaches
the shell. No clipboard API, host control forwarding or payload storage added.

Full locked/offline suite:249passed/13ignored; strict all-target Clippy, build,
format checks pass. The executable used by the launcher was rebuilt. Actual
clipboard sentinel/manual behavior still requires an operator retest. Earlier
source-only empty-response evidence is historical and does not certify the new
policy or the failed operator scenario. Phase completion remains pending.

At the user's request, the single-word development launcher now starts
`/bin/zsh` rather than `/bin/sh` (macOS Bash3.2). Explicit CLI arguments are
unchanged. The interpreter for the launcher itself remains portable `/bin/sh`;
the visible child shell is zsh. No home shell configuration or login-shell
default was changed. Shell syntax and argument forwarding were checked; live
zsh host behavior awaits the next operator launch.

## Parallel notification foundation and CLI receipts — 2026-10-18

User requested next-phase work with parallel subagents. Parent applied the
roadmap-orchestrator workflow and assigned three GPT-6-Luna xhigh workers with
disjoint files and exact contracts in P3-NOTIFICATIONS-TASKS.md. All owners
released their files after checks; parent reviewed source, exported modules,
added cross-component verification and updated shared documentation. No new
dependency, capability advertisement or complete-phase claim was introduced.

Delivered:

- `event_protocol.rs`: exact72byte schema3 metadata payload, immutable full
  target and nonzero cursor; ScreenChanged, InputOwnerChanged and ChildExited.
  Fixed errors, identifier-redacted diagnostics and strict decode invariants.
  No terminal output, input, environment or provider state is retained.
- `event_journal.rs`: volatile generation-scoped monotonic cursors, newest64
  events,6144logical encoded bytes including headers. Pop-before-push prevents
  a65th retained event. Wrong-generation/future/expired cursors fail explicitly;
  bounded pages never claim to be a full current snapshot. Invalid appends and
  cursor overflow leave existing state unchanged. This is not an RSS bound.
- `control ... receipt THROUGH_SEQUENCE`: explicit retirement of handled server
  results through the existing typed route. Locked local context rejects any
  uncertainty and out-of-range sequence before connecting; successful result
  must match the exact key and Completed(Applied). No sequence reservation,
  local metadata write, acknowledgment clearing or automatic retry occurs.
  Existing server receipt idempotence and result-retirement tests remain in
  place. New socketpair tests verify malformed/wrong-key/wrong-body/server-error
  handling and unchanged context metadata.

Parent's `tests/p3_notifications.rs` independently bridges the real journal,
payload codec and wire envelope. It forces retention loss, checks explicit
cursor expiry, reads every remaining event in ordered pages and round-trips
actual96byte Event frames with requestID0. The64frames total exactly6144bytes.
This tests components; it is not a live notification subscription.

Actual parent checks using CARGO_HOME=/private/tmp/panelink-p2-cargo-home:

- `cargo test --locked --offline -q`: exit0, **268passed/13ignored**:
  215library,32binary,4host preflight,11core PTY,3logical PTY,
  1notification integration,1pathname-parser regression and1socketpair reconnect.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: exit0.
- `cargo build --locked --offline`: exit0; launcher binary rebuilt.
- `cargo fmt --check`, `git diff --check`: exit0.
- `bin/panelink control --help`: exit0 and shows explicit receipt syntax.
- Current44document link/whitespace/conflict check: no missing local links or
  findings. Workers separately recorded6codec and8journal focused passes plus
  11focused CLI passes; those are subsets, not additional aggregate tests.

Initial worker corrections: the codec golden fixture was70bytes instead of72;
the temporary journal harness initially omitted protocol_wire; one receipt test
initially had an unused variable. All were corrected before final verification.
Parent source review found no further concrete defect in this bounded scope.
Parent graph connector still returns Transport closed; all three workers used
their functioning graph connector before source exploration. No final refreshed
index is relied on as evidence; source and command outputs are authoritative.

The runtime still does not negotiate Events or expose watches, subscriptions,
snapshot/cursor reconciliation, general managed launch, layouts or provider
recovery. P3-NOTIFICATION-RUNTIME.md records the next integration constraints.
Clipboard denial was fixed in the previous checkpoint, but the actual sentinel
and zsh launcher host retest remain pending. Phase2/3 stay incomplete; no
phase-completion commit or push was performed. All worker ownership released.

## Bounded live watches and atomic reconciliation — 2026-10-18

User requested parallel agents on live event/watch delivery and snapshot
reconciliation. Three GPT-6-Luna xhigh owners implemented exact contracts in
P3-LIVE-WATCH-TASKS.md with exclusive codec, owner and delivery/driver/transport
files. Parent reviewed the released source, added CLI/PTY/export changes and
expanded the actual authenticated socketpair/private-PTY test. Full evidence,
timing/bounds and remaining scope are in P3-WATCH-INTEGRATION.md.

The real-PTY test observes an actually pending watch before another connection
may submit TOKEN, then verifies ordered metadata events, terminal WatchEnd,
screen/owner/cursor consistency with the same child PID, ordered CHECK count1,
and collected ChildExited plus final reconciliation. The previous lost-input-ACK
and detach/resume scenario still passes. CLI tests validate complete batches
before JSON output and preserve the stored context's sequence and uncertainty.
An independent read-only Luna CLI review found no concrete protocol/security
bug; normal timeout exit1 is intentional and documented, not task failure.

Parent full locked/offline suite exited0: **302passed/13ignored**;
244library,36binary,4host preflight,11core PTY,3logical PTY,
1notification integration,1pathname-parser regression and2socketpair/private-PTY
tests. Strict all-target Clippy, build, formatting and diff checks passed.
The launcher binary was rebuilt; control help exposes reconcile/watch syntax.
Parent focused CLI15/15 and realPTY pending-watch1/1 are subsets of this total.

Worker corrections before release included a duplicate derive/missing delivery
helper during shared compilation, an overly generic Debug test canary, and
enforcement of each reconcile request's line limit in addition to global codec
limits. Parent requested metadata sync after authority checks in read_events,
sync before/after reconcile without hidden polling, deterministic pending-watch
observation, and nondecreasing ResyncRequired cursor hints. Final tests passed
after those corrections. No dependency was added or assertion weakened.

After the user's additional next-work request, a separate GPT-6-Luna xhigh
worker extended the existing actual CLI pathname fixture with reconcile/watch,
unchanged context bytes and an explicit zero-wait timeout check. Parent reviewed
the extension and independently ran its normal target:1passed/1ignored. The
worker compiled it and ran focused strict Clippy/fmt; its one explicit ignored
attempt exited101 with actual listener_unavailable and independent bindEPERM.
The earlier operator-confirmed pathname scenario predates these new commands;
no live-watch pathname pass is inferred. See P3-WATCH-PATHNAME.md for the
external rerun and scope. Default ignore count is unchanged.

All W1/W2/W3 and pathname-fixture ownership is released. At this checkpoint a
GPT-6-Luna xhigh next-work owner is preparing only
docs/progress/P3-MANAGED-LAUNCH-PLAN.md under P3-MANAGED-LAUNCH-TASK.md; inspect
its release/status before resuming. Parent remains responsible for review,
canonical docs and roadmap. No future managed-launch schema or multi-session
runtime is adopted merely by that plan. Clipboard/zsh operator acceptance and
broader Phase2/3 requirements remain open; no completion commit or push.

Next-work plan handoff: the GPT-6-Luna xhigh owner released
P3-MANAGED-LAUNCH-PLAN.md after a full source graph refresh,13resolving local
links and clean whitespace/conflict/diff checks. Parent reviewed and refined
the proposal: one global ledger/grant authority, original-wire duplicate checks
before re-resolution, separately bounded resolved paths, explicit stop deadline
and escalation rather than implicit KILL authority, and normal generation
advance rather than reinitialization after a prototype cap. The proposed
session-bound cursor/schema5 migration is not adopted by this plan. The next
bounded task is descriptor/CLOEXEC and all-spawn-path source review before API
launch. No managed-launch application tests or implementation were claimed.
All agent ownership is now released; resume from ROADMAP's current checkpoint.

Next assignment, 2026-10-18: user requested GPT-6-Luna on next work. Agent
spawn_descriptor_gate (xhigh) owns only P3-SPAWN-DESCRIPTOR-GATE.md for the
descriptor/CLOEXEC and all-process-spawn source gate. No application changes,
dependency additions or commits are assigned. Parent owns roadmap/integration
and reviews findings before managed-launch implementation. This records an
assignment, not a completed audit; inspect agent status and report on resume.

Descriptor source-gate review: spawn_descriptor_gate released its report with
24 resolving Markdown links and clean whitespace/conflict/diff checks. Parent
independently inspected TerminalSession::spawn, the pinned rustix-openpty
implementation and startup ordering: the macOS openpty-to-fcntl window exists;
the current bootstrap spawns before connection workers. The private mutex does
not establish protection against every process-spawn path. Concurrent API
launch remains disabled. No application behavior changed or tests were rerun
for this source-review checkpoint.

Next bounded owner: GPT-6-Luna xhigh atomic_pty_spike may edit only
tests/p3_atomic_pty.rs and P3-ATOMIC-PTY-SPIKE.md. It investigates an atomic
macOS PTY-opening path and private-PTY/exec inheritance evidence in test-only
code. Parent reviews before production adoption; socket/pipe/native-spawn
gaps and minimum-OS decisions remain separate. No dependency or API change
is authorized by the spike alone. Inspect ownership/status before resuming.

Atomic PTY spike handoff: the Luna xhigh owner released tests/p3_atomic_pty.rs
and P3-ATOMIC-PTY-SPIKE.md. Parent reviewed the full test and requested an
explicit child success code (so zero matched tests cannot pass), known-inherited
descriptor control, immediate control-descriptor cleanup, and child cleanup on
polling errors. Final prototype passes on this macOS/aarch64 host: master and
slave flags, recorded descriptor identities after exec, two-way bounded private
PTY IO and window sizing. The test checks recorded slots, not the complete
child descriptor table; source evidence separately supports creation-time
CLOEXEC. Published Apple source is not release-matched; minimum OS remains open.

Parent independently ran the final focused test (1passed/0ignored), the full
locked/offline suite (303passed/13ignored), strict all-target Clippy, normal
cargo fmt --check and git diff --check: all passed. Initial targeted Clippy
found two test style issues, corrected before final checks. The worker's
broader cargo fmt --all -- --check also examines vendored vt100 and reports
existing formatting differences; no vendor edits were made, and this is
distinct from the passing project cargo fmt --check command.

No production opener, managed launch API, dependency or cursor schema changed.
All ownership is released. Next: specify production opener integration and
owner-path spawn coordination, including socket/pipe windows and the supported
scope for independent native spawn, before enabling concurrent API launch.
Phase2/3 remain incomplete; no phase-completion commit or push was made.

Quota-bounded assignment: user requested productive GPT-6-Luna medium work.
Agent operator_checklist owns only OPERATOR-NEXT-CHECKS.md to consolidate the
remaining zsh/clipboard/pathname-watch checks with copy-paste commands and a
PASS/FAIL template. No source changes or acceptance claims are assigned;
parent owns review and roadmap. Main runtime next-action remains unchanged.

Operator checklist handoff: GPT-6-Luna medium completed
OPERATOR-NEXT-CHECKS.md. Parent reviewed the commands and clarified the
disposable clipboard prerequisite and three separate PASS/FAIL entries.
Four local links resolve and whitespace/conflict/final-newline checks pass.
No clipboard commands, host acceptance or Rust tests were run for this
documentation-only task. Ownership released; runtime next-action unchanged.

Quota-bounded tooling assignment: user requested GPT-6-Luna medium work with
reported5% remaining. Agent docs_checker owns only scripts/check_docs.py and
DOC-CHECKER.md: a read-only stdlib Python checker for local Markdown links,
conflict markers, whitespace and final newlines, including untracked docs.
Parent owns review/shared state. No Rust/application/dependency changes or
unchanged application-suite reruns are assigned. Runtime next-action unchanged.

Documentation checker handoff: GPT-6-Luna medium released scripts/check_docs.py
and DOC-CHECKER.md. Parent reviewed source and requested fixes for indented
fence closing, invalid-input exit2 and malformed-link handling. Final parent
fixtures independently passed for those cases, spaced paths, finding exit1
and invocation from /private/tmp. Default scan checked91Markdown files with
zero findings. The checker is read-only, uses no third-party dependencies and
reports hygiene inside code fences while ignoring their example links.
No Rust/application changes or full-suite reruns; runtime next-action remains
unchanged. All ownership released; no phase-completion commit or push.

Explicit checkpoint commit request: user requested "commit now". Parent
inspected Git status/history, source/notices inventory and filename/secret
patterns, excluded generated Python bytecode via .gitignore, and reran the
documentation checker (91files/0findings) and diff whitespace check. The
attempt to stage the checkpoint failed before any staging or commit:
Git could not create .git/index.lock (Operation not permitted). This session
has a read-only Git metadata boundary and cannot elevate. Commit remains
pending for an unrestricted Terminal; no push attempted. Intended message:
"Checkpoint Panelink terminal runtime and agent control foundation".
This is an explicit work-in-progress checkpoint, not Phase2/3 completion.

Continuation after checkpoint: Git history now shows b6ee66b (Initial Commit)
and the working tree was clean on entry; this supersedes the earlier pending
commit note. User reaffirmed ongoing Luna delegation with medium/xhigh effort
chosen by task. Two xhigh owners are active: production_atomic_pty owns only
src/terminal/mod.rs, src/terminal/pty.rs and P3-ATOMIC-PTY-INTEGRATION.md;
spawn_coordination_contract owns only P3-SPAWN-COORDINATION.md. Parent owns
canonical/shared docs and final verification. Production adoption is scoped
to current experimental macOS/aarch64 with fail-closed syscall errors; no
minimum-OS release claim or concurrent-launch authorization follows. Inspect
ownership status before resuming after interruption.

Parallel UI research assignment: user requested one additional Luna agent to
study Herdr UI before supplying desired behavior. Luna medium herdr_ui_study
owns only docs/HERDR-UI-STUDY.md, using official current docs/screenshots and
labeling documented vs visually observed behavior. No Herdr code/assets are
copied or installed. This is reference research, not approved Panelink UI
requirements or new phase priorities; await user preferences before spec and
phase-list changes. The two runtime/coordination tasks continue unchanged.

Herdr UI study handoff: Luna medium released docs/HERDR-UI-STUDY.md with
official documentation references and explicit documented/inference/unknown
labels. Browser attempts failed (iab unavailable; Chrome visibility capability
unavailable), so no screenshot-based layout claim is made. Parent reviewed
the study and linked it from research. Await the user's desired UI before
updating the specification or phase list; no application tests apply.

Coordination contract review: parent adopted the cooperative gate scopes in
P3-SPAWN-COORDINATION.md for current experimental binary paths. The same Luna
xhigh owner now owns src/spawn_gate.rs, src/transport.rs and
P3-SPAWN-GATE-RESULT.md; parent added only the private macOS/aarch64 lib.rs
module declaration. Terminal gate adoption follows release by the atomic PTY
owner. No launch API/cursor/capability is enabled by this implementation wave.

## Production atomic PTY and cooperative spawn gate integrated — 2026-10-18

Both xhigh owners released their sources and evidence. Parent reviewed the
full helper/spawn/transport diffs and replaced macOS/aarch64's private
SPAWN_LOCK with the shared gate, dropping it immediately after command.spawn
returns. Other Unix targets retain their prior opener and lock. The atomic
helper uses existing libc/rustix APIs, fixed128byte slave-name retrieval,
OwnedFd cleanup and no non-atomic fallback. The gate serializes cooperative
bind/socket/accept creation and child spawning; waits, authentication, normal
IO and cleanup remain outside it. No Cargo or dependency change was made,
no API launch enabled, no release OS floor chosen.

Production-path tests cover CLOEXEC flags, initial dimensions and canonical/
echo defaults, stdio mapping, child session/foreground-group ownership of
the controlling PTY, and two concurrent children's independent identity,
input/output and cleanup. Parent caught a shell-test command-substitution FD
mistake and the sandbox's /dev/tty restriction; the final proof uses inherited
descriptors and a self-exec helper. Readiness requires complete numeric marker
terminators. Gate tests cover exclusion, deadline/no-effect and isolated
poison; transport tests cover callback lock ownership/error release and
synthetic accept/auth paths. These are not a complete child descriptor scan
or an actual pathname accept/exec race proof.

Final parent locked/offline suite:315passed/13ignored (256library,36binary,
4host preflight,11PTY,1atomic spike,3logical detach,1notification,
1pathname regression,2socketpair/private-PTY). Strict all-target Clippy,
cargo fmt --check, build, documentation checker (95Markdown/0findings) and
git diff --check all passed. The launcher binary was rebuilt. The13ignored
cases remain9host,3external pathname and1internal context-lock helper;
no new host acceptance or pathname pass is inferred.

Next: implement the full-descriptor child probe and real accepted-socket/
managed-exec race fixture, then run where pathname bind is permitted. API
launch remains gated; managed-launch schemas and the proposed session cursor
are unadopted. Herdr UI reference is ready; await user preferences before UI
requirements/phase-list changes. All ownership is released. Phase2/3 remain
incomplete, so no completion commit or push.

## Continuous five-milestone execution — 2026-10-18

User requested continued Luna work through all five non-UI milestones with
automated tests and no manual verification handoffs. NON-UI-EXECUTION.md
records the queue. Initial xhigh owners delivered a reusable actual Darwin
descriptor inventory and a real pathname accept/managed-exec race fixture.
Parent reviewed both. Probe tests pass4/4: clean child contains only its
three shared PTY stdio handles; positive control finds an inheritable handle
at fd64 or above after the child alone lowers its soft limit to16; negative
control rejects the known leak. The bounded proc_pidinfo inventory avoids
assuming RLIMIT_NOFILE covers already-open higher descriptor numbers.

The real accept fixture compiled; its single actual attempt failed at bind
with EPERM before threads or child exec started. It remains an ignored,
runnable automated check, not a pass or a manual-verification request. This
adds one ignored pathname test. The fixture uses the real gate helper and
actual listener accept syscall; public PrivateListener::accept has no barrier
seam and is covered separately. The shared helper's markers now start on a
fresh line to avoid libtest-prefix wrapping in the narrower race PTY.

Parent's first broader run passed257library/4ignored, then stopped on the
existing stalled-output child-reaping assertion in the binary tests. A focused
rerun passed, and a subsequent complete binary target passed36/36. No root
cause is established; no cleanup guarantee/assertion was weakened. Parent
added the cached child-cleanup result to failure diagnostics for any recurrence.
Strict Clippy, formatting, docs and diff checks passed after probe corrections;
the integrated full-suite checkpoint will follow the next codec integration.

The owners were reassigned: one xhigh Luna implements only the pure schema6
launch/status/result codec under P3-MANAGED-LAUNCH-CODEC-TASK.md; parent added
its library export. The other xhigh Luna researches pinned OpenCode TUI
selection evidence and creates a bounded read-only CLI preflight with tests.
OpenCode is absent on PATH. Parent's bounded artifact acquisition attempt
failed with curl exit6 (github.com DNS resolution); nothing was downloaded or
executed. Runtime launch and provider verification remain unclaimed while
independent work continues. No manual handoff, dependency, commit or push.

## Five-milestone component checkpoint — 2026-10-18

Parent reviewed the released Luna slices and integrated schema6 launch
validation, a fixed two-identity registry, a trusted launch transaction core,
schema5 session-scoped watch validation, provider-binding state/record codecs,
and a bounded read-only screen checkpoint codec. Runtime/CLI launch and
multi-session dispatch remain disabled. Parent added journal preflight for all
168 bytes of Intent/Spawned/Applied, non-reused per-generation journal session
IDs, and an allocation-free ledger capacity preflight that never authorizes
execution. Existing one-child startup now checks full journal headroom before
admission/Intent/spawn. No dependency or manifest changed.

Parent review corrected schema6 Accepted/Unknown target presence, tightened
bridge revision continuity and provider-version tokens, constrained registry
mutable access to EffectSink, and added the launch coordinator's storage-health
latch across borrowed journal reopens. Real-child tests require complete PID
sentinels and signal0/ESRCH cleanup observations. Both Spawned and Applied journal
faults retain a child that accepts a fresh post-fault token through the same PTY
and acknowledges with the same PID; no duplicate launch callbacks execute.

The parent full locked/offline suite passed **400 tests, 14 ignored**:
332 library,36 binary,4 host preflight,11 PTY,1 atomic spike,3 logical detach,
4 complete-FD probe,2 managed launch,1 notifications,1 pathname regression,
2 socketpair/private-PTY,2 screen checkpoint and1 ten-child workload. The ignored
set is four library helpers/fixtures (including actual accept/exec), nine host
cases and one external pathname case. Ignored results are not passes. The prior
stalled-output cleanup assertion did not recur in this full run; its root cause
remains unestablished and the stronger diagnostics/assertion remain intact.

Parent independently reran the provider preflight tests (9/9), provider record
codec (7/7), binding core (9/9), scoped watch (9/9), registry (12/12), journal
(11/11), startup (4/4), and the synthetic ten-child fixture. Its measured sample
was44ms,16,840PTYbytes,5rounds/50pumpcalls, test-process peak RSS2,244,608 to
7,159,808bytes. This is one synthetic shell sample, not ten OpenCode agents,
aggregate child RSS, a performance threshold or a ten-session server capability.
Checkpoint/private-PTY tests preserve styled Unicode normal/alternate cells
and deny raw OSC/query/input replay; no persistence/recovery behavior is inferred.

The checkpoint owner hit the Luna usage limit after implementation/checks and
while applying final review. Parent verified the interrupted files, retained
atomic0700 fixture creation and cell-text control checks, and added a regression
allowing numeric metadata/color byte27 without treating it as terminal ESC.
Final formatting/lint/build and documentation checkpoint follows. A medium Luna
verification-runner task was assigned; inspect status before trusting delivery.
The next global-owner refactor plan owner failed at the same model limit before
delivery. No commit/push, manual test handoff or five-milestone completion claim.

Continuation after the user's resume: a fresh Luna medium worker successfully
completed the verification runner despite the earlier xhigh-owner limit.
Parent reviewed bounded cleanup, interruption propagation and CLI handling,
added a relative-CARGO_HOME regression (10 focused tooling tests now pass),
and reassigned that medium owner to the exact private TargetState data split
in P3-OWNER-STATE-SPLIT-TASK.md. It owns only src/server.rs and its result note.
This groups per-target state while keeping one global ledger/grants table and
all existing behavior/signatures; multi-session routing remains future work.
Checkpoint final review reran8codec/2private-PTY tests and strict Clippy
successfully after numeric-byte27 and fixture control-stream corrections.

The parent reviewed the complete TargetState diff: all effect/admission/ownership
branches and public signatures are preserved; changes are private field grouping
and existing fixture paths. The integrated verifier then passed all six checks:
full Rust suite400passed/14ignored, package fmt, strict all-target Clippy, build,
Python19passed and115Markdown/0findings. Build refreshed the launcher binary.
The fixed verifier JSON marks component verification only and host/provider
certification unavailable; it does not convert ignored cases to acceptance.
Its full output is transient at /private/tmp/panelink-non-ui-verification.log;
this durable note preserves the actual result. Next: review the separately
assigned multi-session borrowed-owner contract before source adoption.

## Borrowed authority, pinned leases and launch preparation — 2026-10-18

The private borrowed handler now shares one GlobalAuthority with the unchanged
single-target facade. Parent review compared the moved branches and confirmed
global duplicate lookup still precedes target and ownership preconditions.
Reusable grants pin the full target at issuance, cannot broaden on resume,
and cannot hand off input to a grant on another target. Parent corrected scope
guards to propagate non-authorization errors rather than silently continuing,
and extended denied-Submit checks with valid ownership but mismatched scope;
all32owner tests passed after these corrections.

The explicit launch adapter captures raw client argv/environment bytes, uses
the existing resolver and owned terminal values, and consumes its preparation
once to spawn. Parent reviewed its source and added zero-field target and
terminfo-boundary assertions. Its8unit tests and2real managed-launch tests pass.
A non-UTF-8 filesystem fixture creation was blocked by EPERM; passing raw-byte
argument/environment and missing-path tests do not certify that filesystem case.

The completed integrated verifier log independently aggregates to **411 Rust
passed,14ignored**, Python19, package formatting, strict all-target Clippy,
build and121Markdown/0findings. That run preceded the parent's final scope
guard correction; the32owner rerun covers that correction and another integrated
run follows the global-preflight slice. An earlier run during launch-adapter
editing caught formatting and unused-import/helper-argument lint failures;
the owner corrected them before the passing run. No ignored result is acceptance.

Next assignments use Luna medium: immutable global request preflight and reply
encoding before target selection (server.rs only), plus a separate exact
trusted multi-owner assembly proposal (one new task note). Existing endpoint,
provider and history gates remain unresolved. No phase completion or commit.

The global-before-routing preflight slice is now delivered and parent-reviewed.
It performs only live-context/capability/key checks and the existing global
exact-byte lookup. Continue does not reserve a key or authorize dispatch; target,
scope, owner and actual admission checks remain in the borrowed handler. Pure
reply encoding no longer needs a selected target. The parent independently
reran all33owner tests successfully. No runtime/schema/authority was expanded.

The verifier now runs the parser-only vendored decoder's separate locked/offline
suite automatically after root Cargo tests. Parent reviewed its exact command
list and injected-runner failure tests. All seven checks then passed: root
Rust412passed/14ignored, separate decoder39passed, Python20passed, fmt, strict
all-target Clippy, build and124Markdown/0findings. The combined executed Rust
count is451; it includes the separate decoder suite. No vendor source/lock or
dependency changed. A direct root `cargo test -p vt100 --lib` attempt was
rejected because the non-workspace package requires dev-dependencies; it is
not included as a passing standalone vendor suite.

The next Luna medium assignment is the amended trusted multi-owner assembly
contract. Parent adopted a public experimental library core to avoid unused
private production code/dead-code suppressions, with private target/ledger
fields and consumed crate-private borrow seams. Its startup ledger remains
caller-provided; launch authorization/storage-lock responsibility stays with
the trusted caller. Preparation failure does not spend a session identity;
Intent/spawn failure after reservation does. No runtime/CLI/socket route,
generic session mutation routing, watch or stop policy is included.

## Trusted multi-session core integration — 2026-10-18

Parent reviewed the new two-session library owner, pre-spawn target seed,
exact concrete borrow seams, and one-pass global expiry/per-target polling.
The existing constructor/order remain intact; the old one-target runtime still
uses its original capability set. The new core withholds Events while scoped
read methods are deferred. Parent requested additional router tests proving
cached results/conflicts precede lookup with an empty fake registry and fresh
reads/polling remain available after both post-spawn journal failure points;
all7new owner tests pass.

The separate Luna-owned default real-PTY assembly test passed. It uses actual
launch preparation/spawn and encoded Read calls, keeps the storage lock alive,
checks distinct READY/PID-END sentinels and fresh post-readiness isolation,
denies cross-target reads without admission, rejects the third target with
exact Capacity classification, and checks both PIDs reaped after owner drop.
This is in-process core evidence, not pathname/runtime adoption.

Parent then ran all seven verifier checks successfully: **root Rust420passed,
14ignored; separate decoder39passed; Python20passed; fmt, strict all-target
Clippy, build and128Markdown/0findings**. Combined executed Rust count459.
The next bounded Luna medium task routes the seven existing session requests,
including public Claim before Input on both targets. No schema, endpoint,
dependency or stop authority is introduced. Scoped-read contract review follows.

The seven-variant session router is released and reviewed. Parent independently
passed12new-owner unit tests and both real-PTY tests; a final positive same-target
Handoff regression brings the owner's focused count to13 (worker executed).
It proves the recipient can submit, the former owner is denied without admission
using a current owner revision, and the sibling session remains unaffected.
The real-child case uses public Describe/Claim/Submit, exact same-key retries,
and subsequent count2/PID ACK barriers to detect replay. Both children are
reaped after owner drop while storage remains locked. Scoped read implementation
and its real-PTY assertions are now separately assigned; no runtime endpoint.

The scoped-read contract uses the actual128-byte schema-5 prefix, which embeds
the session ID in schema-4 reserved bytes. An initial parent premise that it
added eight bytes was corrected by source inspection before implementation;
no136-byte header or reduced payload budget is adopted. A separate Luna medium
tooling task automates the existing two component fingerprint manifests without
changing them or treating source equality as a security audit.

## Scoped multi-session core and final verifier — 2026-10-18

The scoped read owner released17focused unit tests and the parent reviewed
the complete target/capability/cursor order, shared preflighted-body extraction,
unchanged128-byte prefix and final schema-5 encoding bound. Old StateOwner
wrappers preserve their authorization/error order. New leases may negotiate
Events with healthy metadata now that both scoped read methods exist; old
grants cannot widen on resume. Real-PTY assertions exercise both snapshots and
event pages, reject crossed session cursors and cross-target reconciliation,
and preserve both read-only and public input/no-replay scenarios.

Parent review of the new source verifier caught blocking FIFO opens before
fstat and a metadata-only aggregate byte cap vulnerable to file growth. The
Luna owner corrected both and added bounded FIFO subprocess regressions plus
growth-after-preflight accounting. Parent independently passed22focused Python
tests and both live fingerprint sets. This verifies equality with checked-in
records; it is not upstream authenticity or a security audit. No vendor source,
manifest, dependency, socket runtime or CLI behavior changed.

Final parent `python3 scripts/verify_non_ui.py` returned0 with all eight checks
passed: **root Rust431passed/14ignored, separate decoder39passed, Python31passed,
both component fingerprint sets match, package fmt, strict all-target Clippy,
build and133Markdown/0findings**. Combined executed Rust count470. Separate
warnings-denied rustdoc passed. All worker files are released and the launcher
binary is rebuilt. No phase is marked complete and no commit/push was attempted.

The five-milestone objective is still incomplete. Pathname accept/exec proof
remains bind-EPERM blocked; multi-session runtime migration, lifecycle authority
and explicit stop, live OpenCode identity/switch proof, bounded durable history
and binding stores/recovery, and representative ten-provider workload remain.
The next independent slices are lifecycle/stop contract review and adopting
the reviewed history-extraction hooks with complete memory/provenance updates.
No manual verification handoff, ignored-result acceptance or permission bypass.

## 2026-10-18 layout and commit handoff

Luna xhigh delivered and released the pure fixed-two-pane layout module in
`src/pane_layout.rs`; parent exported and reviewed it. Its 12 focused tests,
strict Clippy, formatting, rustdoc and the full eight-check non-UI verifier
passed. The verifier reported 443 root Rust tests passed/14 ignored, 39
separate decoder tests, 31 Python tests and 137 Markdown checks with no
findings. See `P3A-LAYOUT.md` for scope and exact test coverage.

The full context-sensitive menu, event routing, transaction, live second PTY,
incremental pane rendering and host mouse acceptance remain unfinished. The
phase checklist marks only the pure geometry groundwork complete; Phase3A is
still open. Continue from the immediate resume handoff in `ROADMAP.md` and the
bounded tasks in `P3A-RIGHT-CLICK-SPLITS.md` and `P3A-LAYOUT-TASK.md`.

The user explicitly requested a checkpoint commit of the accumulated staged
work. This records a checkpoint only; it does not mark Phase3, Phase3A, or any
other unfinished phase complete. OpenCode 2.0.32 is installed, but its private
ACP server cannot start under this execution environment: loopback bind returns
EPERM. No MiMo prompt was sent. A requested checkpoint commit was attempted,
but Git could not write `.git/index` (`Operation not permitted`); the request
remains authorized and pending, with some final handoff-note edits unstaged.
The next model should inspect both staged and unstaged changes, verify them,
then commit when Git metadata is writable without asking the user again. The
exact read-only menu-analysis task and
normal-Terminal controller command are in `P3A-MIMO-MENU-TASK.md`; inspect its
report before incorporating any external findings.

## Priority insertion: right-click splits — 2026-10-18

User requested a new phase before the next planned work: right-click split
options that open a fresh terminal session on the other side. Phase3A now
precedes the lifecycle/stop and history queue. It reuses the reviewed four-way
PANE-LAYOUT design and current two-session core; it does not start a competing
layout model or settle tabs/sidebar/workspace choices. Each action preserves
the original process and starts exactly one new independent terminal on the
chosen side. Tests must cover the complete menu-to-child interaction, keyboard
fallback, target identity, deduplication, resize, isolation and cleanup.

This turn changes planning/requirements only; no menu, mouse handling, pane
renderer or runtime launch is claimed implemented. Current capacity and
pathname/host gates remain authoritative. A Luna medium owner drafts only
P3A-RIGHT-CLICK-SPLITS.md; parent owns canonical priority integration and review.

The owner released the plan and parent reviewed it against PANE-LAYOUT,
clarifying exact-byte deduplication, volatile-ledger limits, and Partial/Unknown
outcomes after layout commit. The phase scope/acceptance plan is adopted;
individual source tasks, wire/transaction interfaces and presentation defaults
still need scoped adoption. Full Markdown check:134files,0findings; whitespace
check passed. No Rust/application behavior changed or phase marked complete.

User authorized starting Luna implementation. A fresh gpt-6-luna xhigh owner
`split_layout` owns only src/pane_layout.rs and P3A-LAYOUT.md under the exact
P3A-LAYOUT-TASK.md contract. Parent owns module export, API review and integrated
checks. Scope is pure fixed-two-pane geometry, nonmutating preview/guarded
commit, atomic validated resize, hit testing and boundary tests; no menu/runtime
or PTY launch yet. Inspect live ownership/results before assuming delivery.

## New-machine setup and host-gate reruns — 2026-10-18

Fresh Apple Silicon Mac (macOS 26.6.1, build 25G76, arm64) at HEAD
`e93929b`. Rust was absent; installed through the official rustup installer
(`--profile minimal`, stable, rustfmt and clippy): rustc/cargo 1.99.1, Clippy
0.1.99, the same version as the earlier verified machine. Default
`CARGO_HOME=~/.cargo`; no temporary cache path was needed. Cargo registry and
loopback/pathname socket access both work in this environment.

`cargo build --locked` passed. `python3 scripts/verify_non_ui.py` returned 0
with all eight checks passed. `cargo test --locked --offline`: 443 passed,
14 ignored (373 lib/4 ignored, 36 binary, p2_host 4/9 ignored, p3_pathname
1/1 ignored, remaining integration targets all passing).

Previously environment-blocked ignored checks were run here by the parent:

- `cargo test --locked --offline --test p2_host -- --ignored --test-threads=1`:
  9 passed, 0 failed (5.41 s). This is the first agent-captured raw pass of the
  corrected outer-PTY suite; the earlier "all corrected" was operator-only.
- `cargo test --locked --offline --test p3_pathname -- --ignored`:
  `pathname_server_cli_context_reuse` passed (0.68 s), confirming the earlier
  operator result with captured output.

These are automated fixture results. They do not establish manual Apple
Terminal behavior: the clipboard sentinel retest and the remaining manual
checklist stay pending.

Launcher defect found and fixed: `bin/panelink` derived the checkout from
`dirname "$0"`, so the README's `~/.local/bin/panelink` symlink resolved to
`~/.local` and reported "Build Panelink first". It now follows symlinks
before locating the checkout. Checked through the symlink from `/tmp`:
`--version` 0, `doctor` 1, unknown option 2; direct `./bin/panelink` unchanged.
The verifier passed again afterwards. Not committed (user deferred commits).

OpenCode on this machine reports version 1.19.0 (not the 2.0.32 recorded
previously). Loopback bind succeeds here, so the earlier ACP EPERM blocker may
not apply; the MiMo task has still not been run and no identity claim is made.

## P3A waves 1–2: menu, split codec, composition, split transaction — 2026-10-18

Parent specified wave 1 in `P3A-WAVE1-TASKS.md` and dispatched three parallel
exclusive-file tasks plus one wave-2 task through the harness's implementation
subagents. Model selection is not exposed by this harness, so no gpt-6-luna
assignment is claimed for this work. Parent reviewed the diffs, owns the
`src/lib.rs` exports (`split_menu`, `split_protocol`) and `terminal::compose`.

- Wave 1A `src/split_menu.rs` (`P3A-MENU.md`): pure four-item menu with
  captured target/revision, `can_split`-derived enablement, consumed-while-open
  input, clamped normal/compact/too-small overlay; 21 tests.
- Wave 1B `src/split_protocol.rs` (`P3A-SPLIT-CODEC.md`): schema-7 request
  (96-byte prefix + exact embedded schema-6 launch, same key), status query and
  128-byte result; 14 tests. Parent accepts `Partial` as a valid split result
  outcome (layout committed, resize pending), unlike schema 6.
- Wave 1C `src/terminal/compose.rs` (`P3A-COMPOSE.md`): offset-aware two-pane
  compositor with divider, overlay clipping, commit-after-write; private
  `render.rs` internals take a placement, public output unchanged; 15 tests.
- Wave 2 (`P3A-SPLIT-TRANSACTION.md`): `MultiSessionOwner::execute_split`,
  `resize_layout`, `layout_snapshot`; coordinator `execute_admitted` so the
  split key is admitted once with the full split bytes; `PtyEffects::resize`.
  Real-PTY `tests/p3_split.rs` proves two distinct children, per-target ACKs,
  `stty size` after split and root resize, lost-ack replay without a second
  child, and teardown reaping.

Parent-accepted deviations: (1) layout commits after the child is acquired,
not with a `starting` leaf before spawn — the pure layout has no starting-leaf
state and a pre-spawn commit would spend one of two session slots on an empty
pane. Known spawn failure is therefore `Failed` with no layout change;
`Partial` means resize pending; post-spawn journal/attach uncertainty is
`Unknown` with the child retained and layout committed. (2) A duplicate of a
previously failed split returns the same Failed result with reason
`PreviouslyFailed`. Layout, ledger and split result remain volatile.

Verification (parent): `python3 scripts/verify_non_ui.py` 8/8 passed;
rustdoc with `-D warnings` passed; `tests/p3_split.rs` passed 3 parent reruns;
full `cargo test --locked --offline` reaches 508 passed/14 ignored on clean
runs. Two pre-existing intermittent failures were observed under repeated
full-suite load: `server_startup::tests::explicit_startup_commits_metadata_and_keeps_singleton_while_child_lives`
(src/server_startup.rs:305) and `p3_detach::interrupt_delivers_sigint_after_a_ready_marker`
(tests/p3_detach.rs:125). Both also failed on an untouched HEAD worktree
(2 failures in 10 full runs), so they predate P3A; not fixed or weakened here.

Not done: no runtime route (ServerDriver still owns a single StateOwner), no
interactive client, no host mouse enablement, no Apple Terminal acceptance.
Phase 3A remains open.

Independent reviewer pass on waves 1–2 (read-only) found no double-spawn or
abandoned-child path. It raised three findings, all fixed and re-verified by
the parent:
- High: same-size `resize_layout` skipped pending PTY resizes. Per-pane
  pending/confirmed size is now tracked, same-size calls retry only pending
  panes without a revision change, and `pending_resizes()` exposes the state.
- Medium: the menu allowed activation in a 5x14–19 viewport. Interaction is
  now gated on the 5x20 terminal minimum; the menu shows TooSmall below that.
- Low: the real-PTY proof used `/bin/sh`. `tests/p3_split.rs` now runs the
  scenario with `/bin/zsh -f` (explicit empty HOME/ZDOTDIR) plus an sh variant.

Parent re-verification: verifier 8/8, rustdoc clean, `p3_split` 2/2 across
3 reruns, full suite 513 passed / 0 failed / 14 ignored.

## Descriptor/spawn gate closed — 2026-10-18

On the user's instruction ("close it"), the parent reran the outstanding gate
evidence on this host. The accepted-socket/managed-exec race fixture passed
6/6, the other pathname fixtures passed, `p3_fd_inheritance` 4/4 passed, and
the atomic PTY and concurrent-spawn tests passed. The gate is closed for
macOS/aarch64 with Rust 1.99.1. Foreign/native concurrent spawn is explicitly
out of scope, and the minimum macOS version is still unset. See the
"Gate closure" section of P3-SPAWN-DESCRIPTOR-GATE.md. This allows API launch
routing (P3A wave 3) through the single owner path. It does not complete
Phase 3.

## P3A wave 3: multi-session server and interactive client — 2026-10-19

- W3A (`P3A-RUNTIME.md`): new `panelink serve` runtime (`multi_runtime.rs`,
  `multi_startup.rs`) over `MultiSessionOwner`. It sits alongside the unchanged
  one-child `server-probe`. Routes: schemas 1/2/4/5, 6 (status only), 7 (split)
  and a new schema-8 UI protocol (`ui_protocol.rs`: layout, styled screen
  snapshot, key input, root resize). The server uses one connection per pane,
  attached through a Hello AttachTarget trailer. Split, keys and resize need
  `--allow-input`; keys also need input ownership. The first child launches
  through `execute_launch` with schema 6.
  - Changes outside the agent's file list: opt-in snapshot frames in
    `transport.rs` (approved by the parent).
  - Review fixes: inherited `PANELINK_*` filtered before preflight; schema-4
    watch/reconcile mapped to the pinned target; exact two-pane partition
    validation; pathname test fails by default, skipping only with an explicit
    env var on EPERM/EACCES.
- W3B (`P3A-CLIENT.md`): `panelink attach` interactive client (`ui_client/`).
  - It renders through the compositor, routes keys to the focused pane and
    opens the split menu on right-click or Ctrl-B `s`. Menu input never
    reaches a child. Mouse events are not forwarded to panes.
  - A split uses the client's own env/cwd (minus `PANELINK_*`) and `/bin/zsh`.
    The client then focuses the new pane. Ctrl-X detaches; children survive.
  - `src/terminal/checkpoint_load.rs` loads styled snapshots.
  - Review fixes:
    - Proactive lease renewal.
    - Restore-first exit with bounded network work.
    - Nonblocking `/dev/tty` diagnostics.
    - Validated receipts.
    - Bootstrap reuses an existing context, so 70 attach cycles do not leak
      leases.
- Parent also fixed `scripts/verify_non_ui.py`. It now prefers `~/.cargo` when
  that directory has a registry; previously a recreated, empty
  `/private/tmp/panelink-p2-cargo-home` broke offline checks.

Parent verification: verifier 8/8; rustdoc -D warnings clean; p3a_attach_host
5/5, p3a_runtime, p3a_serve_pathname and p3_split pass in repeated runs;
full suite 562 passed / 0 failed / 14 ignored.

Automated host coverage uses an outer-PTY fixture, not Apple Terminal. These
are not verified:
- real Apple Terminal right-click delivery;
- the manual feel of the client;
- live 1,500-second lease renewal.

Phase 3A exit requires that manual check; the phase is not marked complete.
Known limits: no function/Alt keys over the wire; full-screen snapshot per
change (~0.9 MB max); application-cursor mode not carried in snapshots.

## Zero-configuration `panelink` — 2026-10-19

The user asked that typing `panelink` should just work. Two delegations made
no progress (an empty result, then an auth-broker timeout), so the parent
implemented `src/default_open.rs` directly:
- `panelink` opens the workspace, or reattaches if it is already running.
- `panelink stop` ends the workspace.
- `panelink probe` runs the old disposable probe.
- When the last shell exits, the server stops and the client shows a notice.

The reviewer raised four High findings (pid race and identity; lock-probe
TOCTOU; context-create retry; root and ancestor validation) and one Medium
(client server identity). All were fixed with a lifecycle lock, pid plus
start-time identity, an EEXIST-only retry, ancestor and 0700 checks, and a
socket-inode check. Verification: verifier 8/8, rustdoc clean, full suite 581
passed / 0 failed / 15 ignored, and `p3a_default_open` 5/5 across 5 runs.
The real-environment smoke test passes. See `P3A-DEFAULT-OPEN.md`. Phase 3A
now awaits only the user's Apple Terminal check.

## Phase 3A complete — 2026-10-19

The user confirmed in Apple Terminal that plain `panelink` works:
right-click split, typing in both panes, Control-B navigation mode, and
Ctrl-X detach and reattach. The earlier "Ctrl-B doesn't work" report came from
pressing Command-B, which Apple Terminal does not pass to programs.

Final parent verification:
- `verify_non_ui.py` 8/8 checks pass; rustdoc with `-D warnings` is clean.
- Full suite: 588 passed / 0 failed / 15 ignored.
- `p3a_default_open` passes 6/6 across repeated runs.

Phase 3A is marked done and committed under the user's phase-completion
authorization. `.codegraph/` is now in `.gitignore`.

Still open and not part of 3A:
- the Phase 2 clipboard sentinel and full manual checklist;
- the remaining Phase 3 runtime items (event/lifecycle/stop contracts, more
  than two panes);
- Phase 4 OpenCode identity.
