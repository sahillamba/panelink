# Phase 3 resume audit — 2026-10-25

Read-only audit of the Phase 3 gate at `054ac3baeec0dbddd5e8d17ae0d3f71544751f74` (`codex/p3-scrollback-integration`). No tests, builds, activation, Terminal interaction, or live-workspace operations were performed. This note does not close Phase 3.

## Finding

The sole explicit remaining Phase 3 acceptance gate is a **human four-pane acceptance in Apple Terminal**. The roadmap, current session handoff, runtime-scope review, and closure audit agree that bounded four-pane implementation and private-PTY acceptance are delivered, but do not count those as human Apple Terminal evidence. The gate cannot be satisfied by automated PTY tests alone. A test that launches or drives Terminal.app would still not substitute for the explicitly required human observation and is outside this audit's authorization.

No separate required production-code defect was identified in the reviewed current records. The 2026-10-23 runtime scope review found no missing behavior in the accepted bounded event/watch/launch/layout scope. Earlier concrete four-pane defects (nested-divider validation, recovery-view offset, invalid test CLI forms and a geometrically impossible fixture) were fixed and covered by later parent verification. The existing human-host gate is an evidence gap, not a newly discovered code defect.

## Existing automated evidence and safe fixture

`tests/p3_attach_four.rs` is the strongest safe repeatable proxy: it starts the real `panelink serve` and `panelink attach` binaries on a private 100x24 outer PTY, uses private contexts and target-scoped control, and checks four-pane split/input isolation/rendering/dividers, fifth-pane refusal, close/reflow, detach restoration and child survival. `P3-L17-ATTACH4-RESULT.md` records three consecutive passes on macOS arm64. The fixture uses a fresh `/private/tmp/pl-p3-attach-four-*` directory and does not need to touch a user's Terminal window or workspace.

If a fresh automated proxy rerun is useful, the bounded command is:

```sh
cargo test --locked --offline --test p3_attach_four -- --test-threads=1
```

Expected scope is one passing private-PTY integration test. Treat startup `SKIPPED`/`listener_unavailable`, a non-macOS/non-arm64 zero-test run, or a sandbox denial as blocked evidence, never a pass. `tests/p3_four_panes.rs` separately covers real-PTY split-to-four, input isolation, close/collapse and survivor use; `P3-FOUR-PANES.md` records three passing runs. Neither test uses Apple Terminal as the host UI.

## Missing evidence for the gate

A human must run a fresh, disposable four-pane session in Apple Terminal on the intended Apple Silicon host and record the tested macOS and Terminal versions and exact build/commit. Observe and record:

1. Split to four panes in the intended arrangement; confirm each pane accepts input independently and displays its own unique marker.
2. Confirm rendered pane bounds/dividers and resize the Terminal window; confirm the panes remain usable and correctly laid out.
3. Attempt a fifth pane and confirm refusal without changing the layout.
4. Close an interior pane and confirm the remaining panes reflow and still accept isolated input.
5. Detach with the documented key; confirm the host terminal is restored and the managed children remain alive. Stop only the disposable fixture afterward and record cleanup.

The private fixture already supplies automated assertions for many of these behaviors, but the missing evidence is observation through Terminal.app itself. The environment inspection found macOS 26.5.2 on arm64, `/dev/tty` present, `TERM=dumb`, no `CI` or `SSH_TTY` value, and Terminal.app present/running at `/System/Applications/Utilities/Terminal.app`. Those facts establish host availability only; no Terminal UI or active workspace was touched. The PATH launcher still points to `/Users/sahillamba/panelink/bin/panelink`, and the recorded active workspace uses older server code, so it must not be mistaken for this checkout's build.

## Lifetime and historical wording

Current automated lifecycle evidence includes orderly detach/client-process survival and reattach, post-ready killed-attach-client reconnect, server SIGKILL replacement with generation/context fencing, and killed control-client recovery after a matching `Completed(Applied)` response was withheld. The records explicitly leave death before admission or while owner-pending, arbitrary descendant cleanup after SIGKILL, provider identity and reboot recovery unproved. These are documented coverage/product limits; the current roadmap retains them as limits and names human Apple Terminal four-pane evidence as the remaining explicit Phase 3 gate. Do not expand the gate to arbitrary descendant cleanup or unrelated Phase 4/5 proof.

Some older prose is historical, not current status: `P3-FOUR-PANES.md` first says the automated test was pending, then its parent-integration section records the test passing three times; `P3-L17-ATTACH4-RESULT.md` records three later passes. The current roadmap and scope review supersede the early pending wording. Older one-child Phase 3 proposal text likewise predates the delivered four-pane, watches, managed pane run and lifetime slices.

## Checkout and audit limits

### Independent parent rerun

After the audit, the parent ran the command above with
`CARGO_HOME=/private/tmp/panelink-p4-cargo-home` and
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.
The current target contains **two** tests (the earlier one-test expectation
above is historical): killed-attach-client reattachment and real four-pane
routing/render/close/detach. Both passed: **2 passed, 0 failed, 0 ignored**,
11.39 seconds, exit 0. No `SKIPPED` or listener-unavailable path occurred.
This refreshes automated proxy evidence and leaves the human-host gate open.

The requested checkout was `054ac3b` and initially reported clean. During the audit, a later status check found modifications to `ROADMAP.md` and `docs/progress/SESSION.md` (2/2 and 17/0 lines respectively), timestamped 15:37. Their diff records the parallel 2026-10-25 Phase 3/Phase 5 resume and audit-task handoff. They were preserved untouched. This audit adds only this file; no commit or push was made.
