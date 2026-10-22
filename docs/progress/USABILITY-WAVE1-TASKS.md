# Usability wave 1 assignments — 2026-10-22

User requested 2–3 GPT-6-Luna agents at high/medium on the usability work.
This first implementation wave follows the reviewed recommendations. It does
not adopt the pending workspace hierarchy or claim provider observation works.
Three exclusive workers; parent owns integration, docs, final checks and commits.

## U1: typed keyboard support — Luna high

Own only `src/terminal/mod.rs`, `src/terminal/unit_tests.rs`,
`src/ui_protocol.rs`, `src/ui_client/keys.rs`,
`docs/terminfo/panelink-probe.terminfo`, and `docs/progress/U1-INPUT-RESULT.md`.

Extend typed keys, never arbitrary host escape streams. Keep current wire
codes1–12 byte-identical. Add code13 Control(u8) with scalar1–26, code14
AltChar(char) with non-control Unicode scalar, code15 Function(u8)1–12;
codes16–20 Delete/Insert/PageUp/PageDown/BackTab with scalar0. Preserve reserved
bytes, validation-before-effect, input classification, payload redaction and
all current length limits. Unknown codes/invalid scalar fail closed; old
servers reject new codes before effects, so no mixed-version support claim.

Encode controls to byte1–26, AltChar as ESC plus UTF8, F1–4 as ESCOP/OQ/OR/OS,
F5–12 as CSI15/17/18/19/20/21/23/24~, Delete CSI3~, Insert CSI2~,
PageUp CSI5~, PageDown CSI6~, BackTab CSIZ. Match advertised terminfo key entries.
Client keeps CtrlB NAV and CtrlX detach; CtrlC stays Interrupt. Map remaining
unmodified control letters, Alt printable chars (optional Shift), plain F1–12,
Delete/Insert/Page keys and BackTab. Ignore release/unsupported combinations.
No mouse/modifier-generalization/vendor/dependency change. Disposable probe
mapping stays scoped to its existing profile; parent documents the difference.

Tests: pure core byte encoding/invalid values/redacted debug, existing golden
wire stability plus strict new-key roundtrips/malformed entry rejection, real
decoder-to-map cases and reserved/release tests. Commands:
`cargo test --locked --offline --lib ui_protocol -- --test-threads=1`,
`cargo test --locked --offline --lib terminal::unit_tests -- --test-threads=1`,
`cargo test --locked --offline --bin panelink ui_client::keys -- --test-threads=1`.

## U2: discoverable help — Luna medium

Own only `src/ui_client/state.rs`, `src/ui_client/state_tests.rs`, and
`docs/progress/U2-HELP-RESULT.md`. No key-map/module/protocol changes.

Add CtrlB then ? help using the existing safe compositor overlay. Bounded
static text, fits/clips safely to viewport, no full-screen clear. Include current
NAV/split/focus/menu/close/detach/reattach actions; no unimplemented features.
Keep help open until Esc/? dismissal; CtrlX detaches. Consume all key/paste/
mouse events while help is open (except host resize observation), no child
effect. Flush pre-help pending input once to its already pinned target before
opening; no focus change, close, split or hidden input. Help opening consumes
the NAV question-mark; help must not expire with NAV timeout. Explicit help
state, higher presentation priority than transient notices, no menu/prompt
overlap. Add ? help hint to NAV banner. Existing semantics otherwise preserved.

Test using recording fake: open/dismiss/isolate keys/paste/mouse, preserve focus,
CtrlX detach, no expiry, resize/tiny viewport, incremental render and unchanged
frames. Command:
`cargo test --locked --offline --bin panelink ui_client::state -- --test-threads=1`.

## U3: bounded cleanup drain — Luna high

Own only NEW `src/terminal/cleanup_discard.rs` and
`docs/progress/U3-CLEANUP-RESULT.md`. U1 owns terminal/mod.rs; do not edit it.
Parent wires the helper after U1 releases its files.

Implement standalone std-only internal helper
`pub(super) fn discard_available(reader: &mut impl std::io::Read) -> bool`.
One stack4096-byte buffer, at most16 successful read attempts per invocation,
EOF returns true, WouldBlock/Interrupted/other errors return false, exhausted
budget returns false. Do not allocate, loop on Interrupted, close/signals or
change caller EOF on budget exhaustion. This returns control to wait_bounded's
existing child/deadline checks while preserving macOS queued-output draining.
Deterministic tests: perpetually readable reader stops at16 calls/65536 bytes,
finite output reaches EOF, each error kind ends promptly, fragmented reads.
Compile/run standalone with
`rustc --edition=2021 --test src/terminal/cleanup_discard.rs -o /private/tmp/panelink-cleanup-discard-tests`
then `/private/tmp/panelink-cleanup-discard-tests`.

Also produce a bounded design for the remaining synchronous launch/close and
client IO work identified by the architecture review; no claim this helper
fixes those larger responsiveness findings.

## Shared rules and parent integration

FIRST read ROADMAP.md, SESSION.md, AGENTS.md and application start documents.
Use codebase-memory first for code discovery. Read every file before editing.
ms-rust remains uninstalled; no guideline stamp. Preserve existing dirty docs.
Match current style; no source from Herdr, dependencies, vendor changes, commits,
or pushes. Append-only roadmap convention normally applies; for this wave the
parent exclusively updates roadmap/state, so workers make no roadmap edits.
Record actual commands/results, deviations and gotchas in owned result note.
No blocked/ignored host test is a pass. Cross-worker compilation may wait for
the other owner; do not edit their files to fix a transient build.

Parent adds cleanup module declaration and replaces discard_queued_output's
unbounded loop after release. Parent reviews full diffs, adds independent
cross-boundary acceptance where needed, verifies all-target fmt/Clippy/build,
library/binary tests and appropriate real-PTY checks, docs and source fingerprints.
Keep socket/host/provider blocked gates explicit. No broad phase completion.
This wave temporarily takes priority over the queued saved-layout codec; resume
that task afterward unless the user steers the next usability wave.
