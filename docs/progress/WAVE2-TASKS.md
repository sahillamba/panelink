# Wave 2 ownership and review gate — 2026-10-22

User approved the proposed next wave: client responsiveness/Luna high, server
close progression/Luna high, pure saved-layout codec/Luna medium. This model
selection supersedes the older xhigh note for the codec's current assignment.
Parent uses roadmap-orchestrator, reviews concurrency before source adoption,
and integrates/verifies. Existing dirty Wave1 and planning work is preserved.

## Assignment ownership (all released to parent)

| Owner | Stage and exclusive files | Gate |
| --- | --- | --- |
| w2_layout_codec, medium | `src/saved_layout.rs`, `docs/progress/P5-SAVED-LAYOUT-CODEC.md` only | Exact existing codec task; parent exports through lib.rs, independent checks. No persistence/runtime/CLI. |
| w2_client, high | Design first: `docs/progress/W2-CLIENT-DESIGN.md` only. After parent approval: ui_client.rs and ui_client/{host,net,conn,state,state_tests}.rs, bounded new worker/test modules in that directory, W2-CLIENT-RESULT.md | Parent checks ordered bounded ownership, durable reserve before input send, no replay, asynchronous split/close result identity/focus, responsive local UI and bounded worker shutdown before go. No keys.rs/protocol/server edits. |
| w2_server, high | Design first: `docs/progress/W2-SERVER-DESIGN.md` only. After approval: terminal/{mod,unit_tests}.rs, server.rs, server/multi_session.rs, server/multi_session/close_tests.rs, server_pty.rs, multi_runtime.rs, new tests/w2_server_close.rs, W2-SERVER-RESULT.md | Tick-driven close must preserve dedup/result/status/history/ownership, continue sibling work, and avoid hidden blocking during retirement. No wire changes without specific parent review; async launches are separate scope. |

Parent owns lib.rs, canonical docs, roadmap/session, independent acceptance and
integration. Workers never edit shared roadmap (explicit exception to normal
append-only convention), commit, push, change dependencies/vendor or overwrite
other owners. Graph-first discovery and read-before-edit apply; ms-rust absent.
No host/socket/provider blocked case becomes a pass. No manual test handoff.

## Verification and completion evidence

Parent reviewed both concurrency designs and authorized Stage 2 on 2026-10-22.
Client implementation must reserve bounded result capacity at admission,
preserve retained-key ordering before later paste/mutations, gate split focus
by the host focus generation, and restore before bounded worker shutdown.
One worker serializes pane IO; this slice does not promise independent sibling
network progress. Server implementation must expose pending close internally
without a false terminal wire reply, keep active transactions out of ordinary
auto-collapse, retain unknown child handles with input disabled, and avoid
wait loops/destructors on the live tick. No wire change is approved.

The codec worker initially froze its source after seven component tests passed.
Parent review found that snapshot conversion accepted canonical split shapes
with undersized leaves, while decode correctly rejected their encoding. The
worker corrected that invariant with side-by-side/stacked regression tests;
parent independently reran all eight codec tests successfully. Independent parent
wire-fixture and exhaustive single-byte canonicality tests passed (two tests);
these do not substitute for the snapshot-conversion regression.

Implementation review also required these corrections before adoption:
the default phased stop hook must never call synchronous legacy cleanup;
pending-close errors must produce a finite reply instead of silently retaining
a waiter; queue saturation must never prevent CtrlX from restoring/detaching;
and split focus must await validated target membership while respecting later
user navigation. Workers added focused regressions and released their files.
The code graph
transport became unavailable during review; discovery had succeeded earlier,
then direct source inspection was used as fallback. No final refreshed-index
claim is made. Parent integration and final evidence are in WAVE2-RESULT.md.

Codec follows every API/format/invariant/test in
[P5-SAVED-LAYOUT-CODEC-TASK.md](P5-SAVED-LAYOUT-CODEC-TASK.md). Pure tests do not
prove layout persistence, reboot recovery or historical-target dispatch.

Client tests must delay socket reads and durable context writes independently
while local navigation/help/render/detach remain usable. Bound command/results
queues by counts and bytes; preserve exact target and input order; never send
before durable reserve or replay uncertain input. Explicitly test split/close
completion after focus/layout changes and overload/disconnect/shutdown.

Server tests must use fake time for HUP/KILL deadlines and real two-PTY/socketpair
fixtures showing sibling output/input/queries during a HUP-ignoring close.
Check duplicate/in-progress/status/terminal result, capacity before effects,
history final capture, survivor resize and no blocking retire/Drop surprises.

Both concurrency owners specify exact API, ownership, cancellation, bounded
shutdown, failure/unknown handling and commands in their design. Parent sends
explicit implementation go only after reviewing those designs. Once frozen,
parent runs focused checks, strict all-target Clippy/fmt/build, appropriate
integration/component checks and the full offline verifier with the populated
registry workaround recorded in Wave1. Record actual failures and ignored gates.
