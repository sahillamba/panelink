# P5 pane operations PTY continuity acceptance

Date: 2026-10-25
Status: acceptance test authored; no Cargo/build/test was run by this owner.

## Parent source review corrections — 2026-10-26

Before execution, the test sub-agent corrected fixture assumptions in the
two exclusively owned files. Readiness now waits for a complete
`P5PO-READY-<name>-PID-<positive numeric pid>-END` line within the bounded
poll, and the PID parser accepts only that complete line. Ordinary shell
barriers still require exact lines. The fixture attaches a client pinned to
each new pane before querying that pane's screen or using it as the split
target. Each pane's input claim is refreshed with the same connection lease
before input, so later checks do not depend on completing within the
30-second claim TTL.

Zoom comparisons now check the complete target set, underlying rectangles,
root dimensions, and dividers while ignoring `pending_confirmed` resize
metadata, which is expected to differ while the zoom PTY has full-root size.
The hidden-output producer starts only after the authoritative zoom view has
arrived, and the hidden pane's own pinned client observes its final marker.
The duplicate swap request bytes are retained for both replay and conflict
checks.

The 50-second scenario and screen-poll deadlines remain bounded by
four-second socket reads/writes. A driver deadline failure can therefore
spend up to one additional socket timeout while the scoped client worker
unwinds and joins; a screen poll can likewise pass its deadline by at most
one bounded screen query. Private fixture directories and server-owned PTYs
remain cleaned through normal fixture teardown. No Cargo, build, dependency,
or test command was run for these corrections; this is not a test result.

## Scope and source grounding

Added `tests/p5_pane_operations_pty.rs` as an Apple Silicon macOS integration
scenario over the existing `StartedMultiServer` and `MultiServerDriver`
socketpair fixture pattern from `tests/p3a_runtime.rs`. It starts a real
`/bin/zsh` process, creates three more shells through the existing split wire
path, and sends schema-13 requests through authenticated production transport.
The code graph connector supplied the initial fixture and protocol symbol
discovery. A later follow-up query returned `Transport closed`; I then used the
already identified local source and the read-only `tests/p3a_runtime.rs`
fixture as a direct fallback for the concrete signatures.
Each private PTY gets a unique ready marker, a PID captured from its actual
screen, and a shell variable whose value includes that PID. The test does not
inject fake resize effects, write to a user clipboard, or change existing
fixtures or runtime code.

The test relies on the exported `pane_operations_protocol` API: `Request`,
`encode_request`, `decode_request`, and `decode_view`, including `ViewQuery`,
`Swap(PaneSwap)`, `DividerSet(PaneDividerSet)`, and
`ZoomSet(PaneZoomSet)`. Mutations are checked through the existing response
status codec. The fixture checks the full unprojected `LayoutReply`, all four
complete `TargetRef`s, `zoomed_target`, and explicit `confirmed_sizes`.

## Intended observable proof

The scenario captures the four real shell PIDs and target references, checks
each shell's independent variable, and asks each shell to print its actual
`stty size`. It then checks source/destination rectangles after a swap,
absolute movement of the selected root divider, exact duplicate replay after
the original layout revision has gone stale, and same-key/different-payload
rejection without a second revision.

For zoom it checks that the complete underlying tree remains published, the
zoomed shell reports full-root content dimensions from `stty size`, and a
hidden shell continues producing a later marker observable through its own
screen query. Actual `stty size` is checked after swap and divider adjustment,
then through zoom, root resize during zoom, and unzoom against each target's
current content size. The shell variables and PIDs are checked after the
operations to detect a respawn or target/process reassignment.

An additional connection omits capability 64. It can still query the legacy
layout, while schema-13 query and mutation requests must receive
`capability_denied`; a view captured before those attempts is compared with the
view afterward.

## Integration gate and limitations

The test asserts that the negotiated client actually receives capability
64 from the production offer and intentionally has no per-test offer bypass.
The parent must verify the current integrated offer/owner/runtime path before
execution. This test does not establish dispatch availability, PTY continuity,
or any pass result yet.

The required parent verification remains serial and outside this owner:

```text
cargo test --locked --offline --test p5_pane_operations_pty -- --test-threads=1
```

The scenario does not inject partial-resize failures; fake resize-effect
coverage belongs to the owner tests. It does not claim physical Terminal UI
acceptance, clipboard behavior, saved-layout/reboot recovery, or phase
completion. Cleanup is private to the fixture: clients disconnect, the driver
is dropped through normal owner teardown, and captured child PIDs are checked
for exit.

## Parent live integration — 2026-10-26

Parent enabled capability64 in both the multi-session owner mask and the
shared grant builder, after library871/0 and independent clean source review.
The single-session offer remains unchanged. The first fixture run stopped at
negotiation because the grant-builder mask was missing the new bit; corrected.
The next live run and a same-binary backtrace recheck timed out at the
post-divider state marker. The backtrace locates assert_state_marker after
swap/divider; its expected output was wider than the narrowed pane and wrapped
across physical rows. Parent shortened the state and phase marker to fit the
minimum content width while retaining PID and shell-state validation.

Logs `/private/tmp/panelink-p5-pane-pty-r{2,3}.log` and
`panelink-p5-pane-pty-backtrace-r3.log` retain those failures. The corrected
fixture is rebuilt separately in `panelink-p5-pane-pty-r4-build.log` and is
running at `panelink-p5-pane-pty-r4.log`; no final runtime pass claimed yet.
Client UI files are still in flight, so that build is not a verified client
checkpoint. No clipboard operation, activation, commit or push occurred.

The r4 run progressed past swap/divider/state checks and failed while encoding
the longer hidden-output producer: InvalidKeyCount. Parent changed the fixture
input helper to send bounded MAX_KEYS chunks with distinct keys, checking each
applied status before sending the next; Enter remains only at the end. No
input is replayed. This corrected helper still awaits execution after the
client UI owner releases source. The r4 failed run is retained and is not a
runtime pass.

## Final focused real-runtime result — 2026-10-26

The r5 run reached post-unzoom checks then lost an idle socket. Backtrace
located the final per-pane stty query; production connection workers use a
one-second read deadline, while the fixture concentrates on other panes.
Parent explicitly resumes the same lease before those checks, preserving its
next operation sequence and replaying no input or mutation. A first helper
compile caught a moved mutable reference (r6), corrected by returning it.

Final locked/offline serial actual4PTY target **1 passed, 0 failed, 0 ignored**
in2.84s: `/private/tmp/panelink-p5-pane-pty-r7.log`. This executes stable PIDs,
independent state, exact target identities, actual stty sizes after swap,
divider, zoom, root resize and unzoom, hidden output produced while zoomed,
exact duplicate/no-extra-revision, key-conflict and unnegotiated denial.
It does not prove outer attach-client keyboard behavior or native Apple
Terminal acceptance. Those remain separate gates. Earlier failed runs are
retained; no phase-completion claim.
