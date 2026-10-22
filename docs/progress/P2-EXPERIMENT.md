# Phase 2 — one-PTY terminal-engine feasibility experiment

Status: core implementation parent-verified; one-shot host probe implemented under integration review, with host runtime/manual checks pending. This plan defines the bounded local experiment; completed evidence is recorded separately and is not terminal compatibility.

Source-gate update: [downloaded-source review](P2-SOURCE-GATE.md) found uncapped engine combining/OSC storage and backend launch/cleanup conflicts. The candidate set fails unchanged; P2B–P2E remain unstarted. Cargo access is restored. Preserve the stop conditions below when reviewing replacements.

Replacement update supersedes that unstarted checkpoint: [P2-REPLACEMENT.md](P2-REPLACEMENT.md) adopts the locally corrected VT100 engine and Rustix/openpty for the disposable core, with 1,024 normal history rows inside the 2,000-row ceiling. [P2-CORE.md](P2-CORE.md) specifies the implemented core; [P2-CORE-RESULT.md](P2-CORE-RESULT.md) records actual verification (42 integrated tests). [P2-HOST-INPUT.md](P2-HOST-INPUT.md) records the next source gate. The [parser-only decoder and pure renderer](P2-INPUT-REVIEW.md) are now adopted/implemented (49 application + 39 separate decoder tests); host IO/restoration and probe integration are now implemented under P2-PROBE.md; see P2-PROBE-RESULT.md for the tested subset and access restrictions. The original candidate list below is historical, not the current manifest.

## Goal and boundary

Run one child process in one engine-owned PTY from a single Panelink process on Apple Silicon macOS. Panelink parses child output into terminal state and renders snapshots in Apple Terminal. It must accept ordinary and full-screen input, resize the PTY, answer only an explicit set of safe terminal queries, deny programmatic clipboard requests, and restore the host terminal on exit.

The process owns both the PTY and terminal engine for this experiment. There is no daemon, socket, reconnect, durable history, layout manager, or OpenCode integration. A successful result proves only that this same-process terminal path is feasible. Phase 3's independent server lifetime and reconnect, and phase 4's OpenCode identity gate, remain required for M1; passing phase 2 does not satisfy M1.

Review candidates only: `alacritty_terminal = "=0.26.0"` with default features disabled; `crossterm = "=0.29.0"` with default features disabled and `events`/`bracketed-paste`; and `portable-pty = "=0.9.0"` with default features disabled. They are not adopted dependencies. The parent is reviewing the dependency tree and source. Do not edit Cargo manifests or the lockfile for this experiment until the parent approves exact versions, features, sources, licenses, build inputs, and the PTY backend. See the [phase 2 dependency review](P2-DEPENDENCIES.md).

## Blocking PTY API gate

The parent’s source review of `alacritty_terminal` 0.26.0 reports that its Unix PTY setup does not meet Panelink’s launch and cleanup contract as-is: `Options.env` adds variables to an inherited environment, the `from_fd` path builds a command with ambient environment, it reads parent `USER`/`HOME`/`SHELL`, ignores a `chdir` result, and PTY drop sends HUP then performs a blocking wait. This is a review finding supplied for this plan, not a test run in this task; details are recorded in the [phase 2 dependency review](P2-DEPENDENCIES.md).

Before PTY implementation (P2B), the parent must approve a PTY backend or narrowly scoped adapter that can:

- Spawn the resolved executable with the complete `Environment` produced from the explicit client launch context and Panelink-owned values, with no ambient-variable inheritance.
- Apply the resolved working directory and return failure if it cannot be entered.
- Resize the PTY and report errors without desynchronizing the engine’s accepted dimensions.
- Reap the child with bounded shutdown behavior. Dropping an object must not wait forever for a child that ignores HUP.

`portable-pty` 0.9.0 is the next PTY candidate. Its reviewed `CommandBuilder` API exposes `env_clear`, `OsStr` arguments/environment, and cwd, but its Unix spawn behavior still needs source review for cwd errors, environment injection, fd ownership, process groups, and bounded teardown. A reviewed adapter is another option. If no approved API satisfies these requirements, stop at P2A and record the blocker. Do not mutate process-global environment, use a shell wrapper to clear or reconstruct it, or weaken the launch contract to make a candidate fit.

## Ordered work and proposed file ownership

The parent owns dependency selection and integration. Once the P2A gate passes, one assigned implementation owner should own the listed files for P2B–P2E; do not split ownership of PTY, engine, and renderer state across agents.

1. **P2A — dependency and API gate.** Review the engine, host I/O, and PTY APIs against the constraints above. Record exact approved versions/features, complete dependency and build review, PTY spawn/resize/cleanup semantics, and the rejected alternatives. No application source changes. The parent owns `Cargo.toml`, `Cargo.lock`, and `docs/progress/P2-DEPENDENCIES.md`.
2. **P2B — one child and one engine.** Add a `TerminalSession` that receives an already resolved executable, argument array, cwd, and explicit environment; creates exactly one PTY and one engine; feeds PTY output into the engine; and holds no more than the stated input and history bounds. Proposed owned files: `src/terminal/mod.rs`, `src/terminal/session.rs`, `src/terminal/pty.rs`, and `src/terminal/limits.rs`.
3. **P2C — input, queries, and resize.** Route host key/paste events only to the active PTY, process engine replies only back to that PTY, and apply validated absolute resizes. No raw child output or host input escape stream may be forwarded between terminals. Proposed owned file: `src/terminal/io.rs`.
4. **P2D — snapshot-only rendering.** Produce immutable screen snapshots from engine state and render their cells through a host renderer. Treat text/control scalars safely and ignore unsupported engine events. Proposed owned files: `src/terminal/snapshot.rs` and `src/terminal/render.rs`.
5. **P2E — local probe and evidence.** Add an explicitly experimental CLI entry point and focused PTY fixture tests, then run the automated checks and Apple Terminal checklist below. For this disposable probe only, normal probe exit stops its child; this is cleanup behavior under test, not a product lifetime decision. Proposed owned files: `src/p2_probe.rs`, the smallest necessary `src/main.rs` dispatch change, `tests/p2_pty.rs`, and `tests/fixtures/p2_child.sh`. The parent owns integration review and the actual phase status update.

The probe must use the existing `src/launch.rs` resolution and environment builder. A test launch supplies a synthetic client environment with a known `PATH`, a harmless marker variable, and Panelink-owned `TERM`/session values. Choose and record a conservative `TERM` profile only after its terminfo entry, engine behavior, query replies, and renderer behavior agree; tests assert the exact value. Never copy the outer host's `TERM` or claim features that were not tested. The PTY spawn API must receive that entire map and an absolute cwd. Tests must also set a unique parent-only sentinel that is absent from the explicit map and prove the child cannot see it. Never print or persist the full environment, paste data, or credentials.

## Engine, rendering, and side-effect rules

The data path is `PTY bytes → terminal engine → bounded engine state → snapshot → Panelink renderer → host terminal`. Child bytes are never written directly to Panelink stdout/stderr or to Apple Terminal. The renderer may emit only its own host-terminal cursor/style controls and sanitized cell scalars. A screen snapshot is a view of engine state, not a raw output log and not input to another terminal parser.

Before emitting a cell scalar, replace C0/C1 controls, DEL, ESC, bidi controls, and Unicode line/paragraph separators with a fixed visible replacement. Preserve ordinary Unicode and combining sequences using the engine’s cell-width data. Do not stringify child titles, OSC payloads, or raw control strings into the renderer. Tests must include an escape/control canary and show that no child-provided control sequence reaches the host-output capture.

Only these child terminal queries may receive a response, and each response is written to the child PTY, never to the host terminal. These are proposed responses for a conservative terminal profile, not claims about the candidate engine's behavior. P2A must inspect the engine source and P2C must prove conformance to the exact response bytes. If the engine emits another response, record and review it before changing this table or selecting the `TERM` profile; do not approve arbitrary replies to make the test pass.

| Request | Proposed response |
| --- | --- |
| Primary device attributes (`CSI c`) | `CSI ? 1 c`; advertise only the baseline VT profile. |
| Device status (`CSI 5 n`) | `CSI 0 n`. |
| Cursor position (`CSI 6 n`) | `CSI row;column R`, using the current engine position in one-based form. |
| Text-area size (`CSI 18 t`) | `CSI 8;rows;columns t`, using the current accepted PTY dimensions. |

The adapter must allow only this list and tests must assert the exact response bytes, including the engine-generated output path. All other queries are ignored unless a later reviewed change extends this list. Do not proxy a query to the host terminal or derive a response from host capabilities that Panelink has not established.

Alacritty may surface generated terminal responses as `PtyWrite` events. Such an event is not blanket-approved for forwarding. Source review must establish how each allowlisted query produces its response and whether the adapter can classify the actual bytes before writing them. P2C may send only exact responses proven to correspond to the allowlist. If the engine writes replies directly or exposes an unclassifiable `PtyWrite`, it does not meet this boundary without a reviewed adapter; stop and report that result.

Programmatic OSC 52 and equivalent clipboard reads, writes, and clears are denied in every state. Never call `pbcopy`, `pbpaste`, or a host clipboard API in response to child output; never forward a clipboard request to Apple Terminal; never queue one for later attachment. Ignore OSC 52 read queries without replying; an empty OSC response can become unintended shell input. Drop writes/clears and malformed or unsupported clipboard requests. Explicit user paste remains a single user-authorized input operation, separate from child requests.

Ignore unsupported title, hyperlink, shell-integration, and host-window events for this probe. Bound control-string parsing before allocation. A parser event overflow or unsupported security-sensitive engine event must fail visibly or be dropped according to the fixed deny rules; it must not fall back to forwarding raw bytes.

## Experiment bounds and failure behavior

These are temporary probe limits, not product budgets or confirmed user requirements:

- One child PTY; one emulator; one attached local view.
- PTY dimensions between 5×20 and 80×240 cells. Reject invalid or out-of-range dimensions. Coalesce rapid absolute resizes to the newest pending size; only commit engine dimensions after PTY resize succeeds.
- At most 64 KiB of pending input bytes. Accept key input once. Accept a paste atomically only if the complete encoded payload fits; reject an oversized paste without sending a prefix. Do not log input or paste contents.
- On partial PTY writes, remove only the bytes the OS accepted. Keep the remainder only in this process’s bounded queue. On hard write failure, report partial/unknown delivery and discard the remainder; never retry it after child exit or a later process/session start. `EAGAIN` may retain the unsent suffix only while this same PTY remains live.
- Feed PTY output to the parser in chunks no larger than 16 KiB and do not build an unbounded intermediate output queue. Source/API review must prove a finite control-string limit of at most 16 KiB and bounded parser/event storage before oversized input is passed to the engine. If the engine does not enforce these limits, stop; do not write a second terminal parser.
- Cap engine normal-buffer scrollback at 2,000 rows and active grid size at the dimension limit above. Source/API review must prove that the engine caps combining storage at no more than 8 extra scalars per cell and account for cell and combining allocations at worst-case dimensions and history. If combining storage is unbounded or its allocation limit cannot be established, stop rather than claiming bounded memory or adding a custom parser.
- Bound terminal-engine retained state to a proposed 64 MiB maximum, including active and alternate grids, scrollback cells and combining data, parser/control-string storage, event storage, and the one visible snapshot. Derive a conservative upper bound from the reviewed source/API and stated caps; observed test RSS alone is not proof. If the bound cannot be established, reduce the experiment caps or stop.
- Process engine callbacks synchronously where possible. If an event queue is required, cap it at 64 events per PTY and 16 KiB per event; reject/drop over-limit payloads before copying them. Limit terminal query responses to 64 per second and 256 bytes per response. Clipboard payloads are discarded without copying into a queue.
- Keep at most one pending render snapshot; newer snapshots may replace stale pending frames, but must not discard terminal state or query replies.

Failure cases and required outcomes:

- Invalid executable, cwd, or environment: fail before PTY creation; name the error without exposing environment values.
- PTY open/spawn/resize failure: show a local error, keep the host terminal recoverable, and do not claim the child started or that the engine accepted an uncommitted size.
- PTY read EOF: drain bytes already read, mark the child exited, and render the final snapshot. Read error: mark the pane failed and begin cleanup.
- Input queue full or oversized paste: reject before adding bytes. Partial write: report accepted-byte count only if it cannot reveal input content; retain no unbounded retry buffer.
- For this disposable probe only, normal probe exit stops its child; this is a cleanup assumption for phase 2, not a product lifetime decision. Send HUP and wait up to 1 second. If the child remains, send KILL and poll/reap for up to 1 more second. Report a cleanup timeout if it is still unreaped; never wait indefinitely in `Drop` or a terminal guard. Close the PTY and restore host terminal mode on the same path. A renderer/host I/O error follows the same cleanup path.
- Panic and ordinary termination: use a scoped terminal guard plus explicit cleanup. The guard must not block indefinitely; hard kill cannot run cleanup and remains a limitation. Phase 3 tests client loss while a separate server owns the child.

## Automated core checks and proposed host checks

The core checks have passed as recorded in P2-CORE-RESULT.md. The one-shot `pty-probe` command is now implemented; host checks require an environment that allows independently opening `/dev/tty`. P2-PROBE-RESULT.md records actual checks and any blocked cases. `--offline` is intentional once the approved source set is locally available; absent sources are a blocker, not a test failure or reason to substitute a pipe.

```sh
cargo test --locked --offline --test p2_pty
cargo test --locked --offline --lib terminal
cargo fmt --check
cargo clippy --locked --offline --all-targets -- -D warnings
cargo run --locked --offline -- pty-probe --terminfo "$probe_terminfo_dir" -- /bin/sh
```

Compile the terminfo entry first using the README commands, which set `probe_terminfo_dir`. The remaining acceptance suite must assert:

- The probe creates one PTY and one engine for one child; PTY output changes engine snapshots and exit status is collected.
- The child receives the explicit resolved cwd and environment, including the harmless client marker, and does not receive the parent-only sentinel. Invalid PATH/cwd and spawn errors fail before launch.
- A full-screen test child receives key input and reports the current PTY size. Resize success updates both the child and snapshot; failure leaves the previous accepted dimensions intact.
- Normal-buffer and alternate-buffer content stay separate, scrollback never exceeds the configured cap, and output floods do not grow intermediate queues without bound.
- Query tests cover every allowlisted request, exact response bytes observed from the selected engine, unsupported queries, oversized/malformed controls, and the rule that replies go only to the child PTY. No blanket `PtyWrite` forwarding is permitted.
- OSC 52 read/write/clear tests prove no clipboard API or host-control output is invoked and no write changes a sentinel clipboard. Simulate renderer suppression followed by snapshot rendering again while the same process and PTY remain alive; prove no clipboard action is queued or replayed. This is not detach/reconnect, which belongs to phase 3. Clipboard read queries receive no response bytes, including with BEL or ST termination.
- Renderer tests include C0/C1, ESC, DEL, bidi controls, line separators, combining characters, and wide cells. Child control canaries do not appear in host output.
- A 64 KiB input queue boundary, oversized atomic paste rejection, partial write, `EAGAIN`, hard write error, and child-exit-with-pending-input each produce the specified one-time/no-replay behavior.
- Combining-scalar and control-string cap tests prove the source/API reviewed limits, and repeated output stays within the recorded conservative memory bound.
- Rapid resize coalescing, invalid dimensions, child exit, spawn failure, PTY read/write failure, renderer failure, ordinary quit, panic cleanup, and a child that ignores HUP all restore the host and reap it within the 1-second HUP plus 1-second KILL/reap bounds or report timeout without blocking.

Capture actual stdout/stderr, exit codes, and test counts in the phase result record. Use P2-CORE-RESULT.md for the subset actually implemented and run. Use P2-PROBE-RESULT.md for host-specific evidence; the full list is not completed by a passing core or component suite.

## Apple Terminal manual checklist — partial operator report

Operator update (2026-10-17): the user reports most manual terminal checks were fine, but did not identify each completed item. Ctrl-Q exited the probe before they finished; the child-cleanup/restoration-timing check is deferred at their request. Full-screen reload on every change was reported and is corrected in source by the incremental renderer; live retest is pending. This is partial qualitative evidence, not a completed checklist.

Use Ctrl-X (Control plus X) to stop the whole disposable probe and its child; use it last. Ctrl-Q remains a compatibility alias, but the operator reports it triggers a host window-close dialog in their setup. To quit `vi` while remaining in the probe, press Esc, type `:q!`, then Enter. Restart the probe command for another test after quitting. The child timing question is whether host input/display are restored before waiting for a slow child to stop. This ordering now has [automated private-PTY evidence](P2-RESTORE-ORDER.md) for normal cleanup and unwind Drop; a special manual child fixture is no longer needed for that scoped observation. Actual Apple Terminal cleanup and visual behavior remain manual acceptance items.

Record the macOS version, Apple Terminal version, machine architecture, exact approved dependency versions/features, and experiment revision for every run. Do not claim compatibility beyond the recorded host. Run these checks from an interactive Apple Terminal outside the agent filesystem sandbox after compiling the terminfo entry as described in README.md. A compiled entry alone does not demonstrate conformance.

1. Launch a shell: `cargo run --locked --offline -- pty-probe --terminfo "$probe_terminfo_dir" -- /bin/sh`. Confirm one child starts in the requested directory with the probe’s explicit environment. Exit normally and confirm the Apple Terminal prompt, cursor, colors, and input mode return.
2. In the child, run `printf 'P2-UNICODE: café 東京 👩‍💻\n'`. Confirm readable glyphs and cell alignment; record any width/rendering issue without treating a single test as broad Unicode support.
3. Run `/usr/bin/vi` in the child. Confirm entry/exit, arrow keys, insert/delete, Ctrl-C, and redraw work. Resize the Apple Terminal window several times; confirm the app and `stty size` reflect the accepted PTY size. The child’s full-screen controls must remain within Panelink’s snapshot renderer.
4. Paste a harmless unique marker once using Apple Terminal’s normal paste action. Confirm the child receives one copy, with bracketed-paste framing only when the engine mode requires it. Oversized paste must be rejected atomically and visibly.
5. In the outer Apple Terminal shell, before launching the probe, set a harmless sentinel with `printf 'P2-KEEP' | /usr/bin/pbcopy`. In the inner child shell managed by Panelink, emit the proposed test sequences `printf '\033]52;c;UDItWA==\007'` (write `P2-X`), `printf '\033]52;c;\007'` (clear), and `printf '\033]52;c;?\007'` (read). Confirm the outer host clipboard remains `P2-KEEP`, no prompt appears, and no OSC 52 is emitted to the host. Exit the probe to the outer shell and verify with `/usr/bin/pbpaste`. The `pbcopy`/`pbpaste` calls are manual operator actions in the outer shell; Panelink must not run them in response to child output.
6. Exercise the four allowlisted query classes through the automated fixture and `/usr/bin/vi`; confirm there is no visible query text or host-terminal escape leakage. Unsupported queries must not be forwarded.
7. Launch a long-running harmless child, then exit Panelink through Ctrl-X. For this disposable probe only, confirm the child is stopped/reaped within the two 1-second cleanup windows and Apple Terminal is restored. Repeat with a child that ignores HUP; cleanup must still finish or report a timeout within the declared bound without hanging. This behavior is not a product decision about server/client lifetime. Record terminal restoration failures rather than rerunning until one passes.
8. Flood the pane with synthetic output, then exit and restart the probe. Confirm the in-process scrollback cap is respected and no history, checkpoint, or child input survived the process. This is not a persistence/reconnect test.

## Evidence and non-claims

This experiment does not test multiple PTYs, daemon/server lifetime, client disconnect and reattach, IPC deduplication, durable storage, saved scrollback, OpenCode session identity, provider status, or reboot recovery. Those gates remain required for M1 as tracked in the [roadmap](../../ROADMAP.md). Do not describe a pipe, a raw PTY byte relay, or a passing smoke test as a terminal engine implementation.

The initial plan was written before application-core tests or dependency adoption. Subsequent source review, dependency checks and core verification are now recorded in [integration evidence](INTEGRATION.md) and [P2-CORE-RESULT.md](P2-CORE-RESULT.md). This plan alone is not completion evidence; the manual checklist remains partially reported, with the deferred Apple Terminal cleanup check and incremental-rendering retest pending. Automated restore-before-child-wait evidence is recorded separately in P2-RESTORE-ORDER.md.

Host cleanup restores the saved termios exactly and leaves standard-descriptor flags unchanged. ANSI private visual modes reset conservative defaults; their previous values are not queried. Output/restoration/reporting have nonblocking deadlines; failed sinks can prevent visual restoration or a visible error. Already-queued host input is flushed before returning to the shell, but bytes a producer sends after the final flush/exit cannot be suppressed. Child memory and inherited runtime state are outside the source-derived Panelink allocation estimate. No RSS or hard realtime guarantee is claimed.

## Latest short retest feedback

The user reports everything else in the short retest works, except Ctrl-Q triggers a host keyboard/window-close confirmation. This confirms the short checklist's redraw, Unicode display, vi/resize, paste and Ctrl-C behavior by operator report; no fresh version/log/timing information was supplied. It does not imply the separate clipboard sentinel or every original full-checklist case ran. Source contains no host-window-close request in the local quit path; the exact host key mapping is unverified. Ctrl-X is now the documented quit key, with Ctrl-Q kept as an alias. A decoder-through-dispatch regression verifies Ctrl-X quits locally without queuing child input, while plain x still queues normally. The new shortcut and actual return-to-shell cleanup await a final operator check.

## Operator clipboard acceptance — 2026-10-22

The user ran the OSC 52 clipboard-write check inside a live `panelink` pane in
their own Apple Terminal (`printf '\e]52;c;%s\a' "$(printf HACKED | base64)";
pbpaste`) and reported the result as good: the host clipboard was not
replaced by child output. Operator confirmation without a raw log; the read
and clear variants and the sentinel-across-renderer-suppression case were
covered by automated tests, not repeated manually. This closes the Phase 2
clipboard-denial host gate for the user's Apple Terminal.
