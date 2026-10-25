# Phase 4 managed Panelink/OpenCode bridge probe

Status: **scoped managed native acceptance passes — 2026-10-25**, in a Luna
run and an independent parent run. See [integrated completion](P4-INTEGRATED-RESULT.md).

The probe in `scripts/probe_panelink_provider_bridge.py` runs pinned native
OpenCode2.0.32 in a fresh private HOME/XDG fixture and enforced loopback-only
macOS sandbox. It starts two real managed Panelink servers and an attach PTY.
No prompt or model request is submitted, and every fixture is checked for
absence of user messages. User configuration and the active binary are untouched.

## Accepted managed behavior

Both runs establish exact initial A/B bindings, native A-to-B-to-A with
advancing durable revisions, passive B remaining at revision1, an API-created
new root, an imported nonempty-history fork, return from the fork, Home
revocation with retained A and fresh return, actual same-client foreign-project
selection, visible manual fallback with A preserved, and exact UI return to A.
The final A revision8 equals the fresh runtime acknowledgment; passive B is1.
Exact-target historical inspection after graceful shutdown reports
`current_at_crash=false` and `automatic_resume=false`.

The independent read-only observer checks its client nonce, global sequence,
project allowlist and single TUI instance before accepting route records.
Exact initial A/passive B are required before input. Foreign selection must
have a later same-instance exact foreign project/session observation. The
public `provider-binding prepare` command then returns
`manual_selection_required` / `provider_project_mismatch`, without a ticket or
navigation; a newer foreign observation and unchanged A disk binding prove
that refusal preserves state. Return requires another newer same-instance
original-project/A route, plus a fresh owner acknowledgment and matching
newer disk revision. Displayed titles are diagnostics, not identity evidence.

The parent run records observer sequences **135 → 138 → 152** (foreign,
post-refusal foreign, returned A). Luna records **115 → 118 → 132**.
Durable sanitized reports: [parent](p4-evidence/managed-native-parent-2026-10-25.json)
and [Luna](p4-evidence/managed-native-luna-2026-10-25.json). Both classify proven,
verify message absence, submit zero prompts and complete cleanup.

## Input and fixture boundaries

The private keymap uses Ctrl-P for the local session list and Ctrl-O for global
project navigation, avoiding Panelink's Ctrl-X detach and Ctrl-B navigation.
This is test apparatus; it changes no user-owned keymap or shipped shortcut.
The native default Ctrl-X provider leader conflict remains a compatibility
boundary, not a claim closed by this fixture.

Before typing, a fresh control-read screen must establish the exact modal and
client ownership. Before Enter, two identical fresh reads at least200ms apart
must prove the filtered result, with another stable check immediately before
activation. All seven known fixture titles are covered, including imported
parent/seed roots. The global picker requires source-derived centered modal
bounds, contiguous one-row session results ending at a blank modal row, no
Projects category, no unknown/competing result, and no overflow/clipping.
The exact return uses the pinned public session-ID picker lookup: one fixture
ID query above Sessions and one full A title result beneath it. IDs and input
stay in memory; fixed reports contain only modes, booleans, coordinates,
revisions and identity hashes.

The attach has a continuous10ms PTY reader with a512KiB rolling transcript.
Fresh child reads authorize input; transcript reconstruction is diagnostic.
Observer files live beside the initially empty server directories. Inline CLI
configuration explicitly composes the production `../identity-plugin` with
the absolute observer path because OpenCode replaces plugin arrays.

## Verification and retained failures

Parent full serial Rust **1231pass/0fail/15ignored**; unchanged Rust/shipped
plugin tree. Seven supporting checks pass: decoder39, component fingerprints,
formatting, strict all-target Clippy, isolated build, Python **232run/231pass/
1 opt-in native skip**, and Markdown **332/0**. The explicit native runs are
separate from that skip. Final Luna review found no material issues.

Earlier failures remain in the dated [startup](P4-STARTUP-2026-10-25.md),
[plugin composition](P4-PLUGIN-MERGE-2026-10-25.md),
[picker semantics](P4-PICKER-2026-10-25.md),
[observer](P4-OBSERVER-2026-10-25.md),
[exact query](P4-EXACT-QUERY-2026-10-25.md),
[modal bounds](P4-MODAL-ROWS-2026-10-25.md),
[foreign refusal](P4-FOREIGN-REFUSAL-2026-10-25.md) and
[exact return](P4-EXACT-RETURN-2026-10-25.md) notes. They are not relabeled as
passes. The API-mutation diagnostic's background-worker cleanup accounting was
also corrected; it now reports incomplete cleanup while fixture work is live.

OpenCode SHA-256:
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
Accepted isolated Panelink SHA-256:
`1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`.
Launcher used `~/.opencode/bin/opencode` after PATH lookup. No wrapper, build
input/dependency change, user configuration change, activation or push is
implied. Separate Phase3 human host and Phase5 reboot/workload gates remain open.
