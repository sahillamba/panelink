# Execution reference and exclusive ownership

## Starting procedure

The parent first completes ticket 00. Every worker then reads the repository's applicable AGENTS instructions, README, SPEC, DISCOVERY, PREBUILD, ROADMAP and current SESSION, plus the current Panelink usage skill when its scope touches user/agent behavior. Read the relevant Rust skill if available and record its pinned revision; do not claim a missing skill is installed.

Use codebase-memory first for code discovery and refresh/reindex the selected checkout before trusting source ranges. Read current source before edits. The pointers below were verified at the recorded HEAD and are navigation aids, not permanent coordinates. Newer origin changes were reviewed with `git show` without changing the checkout.

## Ownership leases

Only the parent edits shared planning/evidence documents and registers sibling modules unless it explicitly grants that file to one worker. A worker needing an unleased file reports the dependency; the parent either expands the lease with no overlapping writer or makes the small wiring edit. A file lease lasts until the parent reviews and integrates that ticket.

| Ticket | Exclusive existing files | Proposed new files / reference anchors |
| --- | --- | --- |
| 00 | Parent only; no source edits | Own baseline/evidence record outside live target |
| 01 | `src/ui_client/host.rs` | Private host dispatch helper if needed; keep its declaration local |
| 02 | `src/ui_client/state.rs`, `src/ui_client/state_tests.rs` | Private child `src/ui_client/state/viewport.rs`; HEAD ScreenSlot:374, apply_scroll_update:2328, scroll/flush:2683/2766; newer origin adds FIFO logic |
| 03 | Same UI-state lease, transferred after 02 | Private child workspace presentation module; HEAD TabDialog:97, metadata state:199, tab/picker event handlers:4616 onward |
| 04 | Same UI-state lease, transferred after 03 | Private interaction/selection module as justified; HEAD handle_event:3331, copy:3126 onward, pane interaction and pointer capture later in file; existing `selection.rs` only on explicit lease |
| 05 | Same UI-state lease plus `src/terminal/compose.rs` | Private render child; optional `src/terminal/compose/chrome.rs`; HEAD render:6524, compose chrome geometry:172 onward. Preserve current compositor exports via reexports if useful |
| 06 | `tests/p3_attach_four.rs` | HEAD wait_split_settled:626; avoid editing shared outer-PTY support without a separate lease |
| 07 | `src/server/multi_session.rs` | Child test modules under `src/server/multi_session/`; main test blocks begin HEAD:3752, 6495, 8261. Extend to `multi_runtime` only in a separately leased follow-up |
| 08 | Parent owns docs/evidence, affected skill and integration wiring | Update stale current-status entries while preserving dated evidence |

Private child modules are suggestions, not mandatory filenames. Keep the public interface and binary/library placement stable. A set of child `impl App` files which all mutate the same root fields may be a mechanical preparation, but is not sufficient completion for tickets 02–04. Those tickets move ownership and transitions together.

## Focused verification map

All build/test commands use a unique `CARGO_TARGET_DIR` outside the repository, distinct per concurrent worker. Keep the installed launcher and existing live process unchanged. Set `RUST_TEST_THREADS=1`; Cargo invocations use `--locked --offline`. Run focused checks after substantive changes, then one canonical combined gate at integration; preserve failures before diagnosing.

- UI behavior: `cargo test --locked --offline --bin panelink ui_client::state::tests`.
- Host dispatch: `cargo test --locked --offline --bin panelink ui_client::host` and private-PTY targets `p3a_attach_host`, `p3a_default_open`, `p5_workspace_tabs`.
- Viewport/history: UI behavior suite, `p3_scroll`, and relevant existing wheel worker/connection/server tests discovered at the selected baseline. Keep tests for fresh FIFO order/expiry, old/new capability negotiation, retained claims, failed/unknown delivery and stale epochs.
- Workspace presentation: UI behavior suite and `p5_workspace_tabs`, `p5_workspace_lifecycle`, `p5_recovery_chooser`.
- Interaction/render: UI behavior suite, `cargo test --locked --offline --lib terminal::compose`, and the affected private-PTY scenario from `p3a_attach_host`, `p3_attach_four` or `p3a_default_open`.
- Fixture barrier: `cargo test --locked --offline --test p3_attach_four`; demonstrate the new predicate rejects the formerly premature screen state. Repetition is justified only to investigate an observed timing concern, not to erase a failure.
- Server test layout: compare `cargo test --locked --offline --lib -- --list` before/after; allow only expected module-prefix changes. Run the relocated scenarios with `cargo test --locked --offline --lib server::multi_session`. Keep all ignored tests and their reasons.
- Static checks: canonical `cargo fmt --check` (not `cargo fmt --all`, which also reformats upstream vendor sources), strict all-target Clippy, component fingerprint and doc checks.
- Combined gate: `RUST_TEST_THREADS=1 CARGO_TARGET_DIR=<isolated-directory> python3 scripts/verify_non_ui.py`. Refer to `docs/CI.md` for environment/toolchain and check scope. A new baseline needs a new result; the older green log is not transferable evidence.

Do not run opt-in native/provider/performance tests as part of structural acceptance. If a real failure is found, reproduce a tight failing check, then either make a separately reviewed fix or leave the ticket blocked with evidence. Do not weaken assertions, increase sleeps/timeouts to obtain green, or label unrun checks as passes.

## Agent dispatch prompt

Read this bundle's README, SPEC, this reference, and ticket NN. Confirm current checkout identity and assigned file lease. Implement only that ticket against the parent's selected integration base. Preserve current behavior and the safety contracts in SPEC. Use existing test surfaces; no live pane operation, install, dependency change, worker commit or push. Report a concrete design obstacle rather than broadening scope. Completion requires the acceptance checklist and the handback below. No nested delegation unless the task explicitly assigns it.

Use a separate managed worktree for concurrent agents; let the delegation tool create it. Same-provider, in-turn native tasks may use native agents; user-requested cheaper model/provider settings or a separate checkout use T3 delegation. Honor the current orchestrator instructions and returned `next`; do not poll running delegates. Suitable mechanical test relocation may use GPT-6-Luna with parent review; choose experienced implementation/review agents for viewport/input ownership work. Do not substitute unavailable providers or models silently.

## Handback format

1. Base SHA, resulting checkout/branch, changed files and ownership respected.
2. Module shape: state owned, callers simplified, interface ordering/error rules.
3. Acceptance checklist with evidence: exact commands, results, failures and unrun checks.
4. Effect/target/epoch/metadata/render invariants retained and evidence covering them.
5. Remaining risks and any follow-up outside scope. Parent reviews the diff and runs combined verification. Source control and any PR publication follow current user authorization; this draft grants neither a push nor an installation.
