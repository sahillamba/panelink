# Workspace event protocol and journal component result — 2026-10-23

## Scope

Worker A implemented only `src/workspace_watch_protocol.rs` and
`src/workspace_event_journal.rs`, as assigned by
`docs/progress/P3-LAYOUT-EVENTS-TASK.md`. No shared module exports, owner or
runtime integration, CLI code, spec, roadmap, or task contract were changed.

The codec implements schema 11 request/reconciliation/watch payloads, reuses
the existing `LayoutReply` codec for strict inner layout validation, checks
generation/workspace scope, all reserved bytes, lengths, status/count rules,
event flags and sequence continuity, and redacts cursor/revision identities in
Debug and error text. The journal owns an independent generation/workspace
cursor and a 64-record/1,536-byte bounded queue with checked sequence advance,
scope/future/expiry checks, and the specified last-evicted cursor boundary.
Focused unit tests cover golden shape/round trips, corruption, scope, bounds,
ordering, expiry, rollover, overflow and redaction.

## Code discovery

The codebase-memory project `Users-sahillamba-.codex-worktrees-6730-panelink`
was ready at 11,105 nodes / 54,805 edges. Graph queries located
`ui_protocol::{encode_layout_reply,decode_layout_reply}` and the existing
`EventJournal` cursor/expiry/overflow contract. Source inspection confirmed
the concrete APIs before implementation. The graph index was used only for
discovery; source remains authoritative.

Actual discovery calls: codebase-memory `index_status` for that project;
`search_graph(query="LayoutReply validation encode decode layout codec")`;
`search_graph(name_pattern=".*(LayoutReply|layout_reply|WatchStatus|CodecError).*")`;
and `search_graph(query="event journal cursor retained rollover expiry sequence exhaustion")`.
Shell inspection used `rg -n` and `sed -n` against `src/ui_protocol.rs`,
`src/watch_protocol.rs`, `src/scoped_watch_protocol.rs`, `src/event_journal.rs`,
and `src/lib.rs`.

## Verification

- `rustfmt --edition 2021 src/workspace_watch_protocol.rs src/workspace_event_journal.rs` — completed with exit 0 and no output.
- `rustfmt --edition 2021 --check src/workspace_watch_protocol.rs src/workspace_event_journal.rs` — exit 0 and no output.
- `cargo test --locked --offline --lib workspace_ -- --test-threads=1` — final run passed: 13 passed, 0 failed, 0 ignored, 677 filtered out. Earlier attempts recorded the in-progress parent integration errors (missing owner APIs, then a `PaneRect` type typo); both were resolved before this passing run.
- `cargo fmt --check` — initial run returned exit 1 on formatting diffs in concurrent shared edits. Final run returned exit 0 after those shared edits were formatted. The worker-owned files were formatted and checked with `rustfmt --edition 2021 --check` (exit 0).
- `git diff --check -- src/workspace_watch_protocol.rs src/workspace_event_journal.rs docs/progress/P3-LAYOUT-EVENTS-COMPONENT-RESULT.md` — exit 0, no output.

This is a component result only. Owner permission enforcement, capability
negotiation, event emission, atomic snapshot/cursor integration, socket watch
delivery and CLI behavior remain parent/other-worker scope and are not claimed
as implemented or verified here.
