# Phase 4 next two slices — 2026-10-24

Execution: both scoped cases pass3/3; see [results](P4-V2-EXTENDED-RESULT.md).
Home was observed without a selected ID; no allocation-timing claim is made.


The user requested two further slices using Luna. Baseline selected-route and
two-client A-B-A evidence remains in [P4-V2-TUI-RESULT.md](P4-V2-TUI-RESULT.md).
Preserve the activated workspace and all dirty work. No Rust/product bridge,
durability, recovery, model prompts, user accounts, commits or pushes are in scope.

## Exclusive ownership

- Luna High identity owner: only existing V2 TUI harness and its Python test.
  Generalize active/passive roles for opposite-client B-A-B, then observe the
  keyboard new-conversation outcome without sending a prompt. A home route
  with deferred session creation is an explicit limitation, not selected-ID proof.
- Luna High health owner: only a new V2 bridge-health helper/test plus the
  fixture plugin and its Node-based behavioral test. Add optional private
  pause control that stops observation writes while both TUIs/server stay alive.
- Parent: bootstrap scenario dispatch, owned read-only new-session validation,
  review, live verification, roadmap/spec/handoff and evidence. Workers do not
  edit planning files or commit. Files have one editor at a time.

## Shared interface and claims

Scenarios are `baseline`, `reverse-new`, and `bridge-health`. Keep the existing
default path unchanged. The bootstrap supplies the same exact2.0.32/hash gate,
fresh private roots, loopback-only sandbox, fixture credentials and owned
server PID/port. TUI config remains plural `plugins` pointing to a directory
with `tui.ts`; do not repeat the previous file/legacy-schema failures.

After the baseline route gate and before cleanup, an optional `after_routes`
hook receives live TUIs, record paths, client nonces, private control paths,
project/session hashes and a bounded pump that drains both clients. The health
scenario must expire only the first observer while the second stays fresh.
Every callback must preserve cleanup on all exits. All reports remain allowlisted.

The diagnostic health policy uses host receipt time, a 1s expiry, and only new
contiguous observations to refresh. Duplicate reads cannot renew freshness.
After expiry, retain only a clearly labeled last observation; it is not durably
saved and cannot authorize recovery. Resumed transport alone cannot restore
eligibility: explicit reconciliation needs a further fresh selected-route row.
Wrong project/epoch/nonce/sequence and unknown identity fail closed. This is
an experiment policy, not the production heartbeat contract.

New-conversation controls require the documented Ctrl-X then N shortcut and
fresh route observations. A new ID must be matched by exact hash through the
owned server and validated for the same canonical project/top-level status;
never infer it from the directory's newest entry. No ID appears in reports.
If the provider waits for a first prompt to create the conversation, record
that behavior and leave the stronger identity case open. Fork/child/project
switch acceptance remains a later gate unless separately exercised.

## Verification

Workers run focused behavioral tests and compilation, then release ownership.
Parent independently reviews, runs one frozen live diagnostic per scenario,
and repeats only passing gates with fresh fixtures. A blocked stage requires
a concrete correction before retrying. Verify empty user messages and full
cleanup. Then run all Python tests with permitted loopback fixtures, docs,
tracked/untracked whitespace, compilation and a nonpersistent graph refresh.
Preserve failed observations, exact counts and receipt paths in the result.
