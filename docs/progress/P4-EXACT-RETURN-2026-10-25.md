# Phase 4 exact managed return to fixture A — 2026-10-25

Status: **the managed foreign refusal and exact return-to-A gates pass in one
native run**. This note records evidence for the bounded probe; parent
independent acceptance and final review remain outstanding.

## Exact picker query

The pinned OpenCode 2.0.32 global picker source accepts a full session ID
matching `/^ses_[0-9A-Za-z]{26}$/` and resolves that session through the public
session API before merging its exact session options. The source reviewed for
this query mode is [`dialog-open.tsx`](https://raw.githubusercontent.com/anomalyco/opencode/v2.0.32/packages/tui/src/component/dialog-open.tsx),
lines 136–145 and 147–220.

The probe now has a fixture-only `fixture_session_id` query mode. It accepts
only the exact disposable A ID passed by the harness and matching the pinned
format. In the live managed child screen it requires a single query coordinate
between the `Open` title and `Sessions`, a single A title coordinate after
`Sessions`, exactly one bounded session result row, no other known or unknown
session rows, no Projects category, verified modal bounds, client input
ownership, and two stable reads across the native debounce before sending
Enter. IDs and input text remain in process memory; the report contains only
the safe mode name, fixed booleans, hashes, and sequence evidence. Existing
title-query occurrence checks remain unchanged.

The independent observer, not the picker, remains the identity acceptance
gate. Return requires the same TUI instance, the exact original project and A
session, a sequence newer than the post-refusal foreign route, a fresh
`last_confirmed` provider snapshot newer than A revision 7, and a disk binding
revision equal to that runtime revision.

## Focused checks

- `python3 -m unittest -v tests.test_panelink_provider_bridge_probe` — 31
  tests run, 30 passed, 1 opt-in native test skipped, 0 failures.
- `python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.
- Regression coverage rejects malformed or wrong fixture IDs, the wrong query
  mode, duplicated query or A coordinates, other fixture results, Projects,
  unstable reads, and a mismatched title without sending Enter. The accepted
  exact-ID case sends the native picker query and Enter.
- No build was run. The native probe used the existing binary.

## Native managed run

Fixed report: `/private/tmp/panelink-p4-20261025/exact-return-r9.json`.
Launcher: `/Users/sahillamba/.opencode/bin/opencode`, version 2.0.32, SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`. Existing
arm64 Panelink binary SHA-256:
`1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`. The run
used the enforced loopback-only sandbox, submitted zero prompts, verified no
user messages, and completed cleanup. One native attempt was made.

The public refusal remained exact: `manual_selection_required` /
`provider_project_mismatch`, return code 1, with no ticket or selected
confirmation. The observer confirmed the foreign route on the original TUI at
sequence 115 and again after refusal at sequence 118. Fixture A then returned
through the global native picker using the exact session-ID query; the trace
records `filter_query_mode=fixture_session_id`, `enter_sent=true`, and fresh
managed-child row authorization.

The return observer confirmed the original project and A session on the same
TUI at sequence 132. Runtime and disk binding both advanced from the
pre-refusal A revision 7 to revision 8, with the original A provider-session
hash. The report sets `managed_foreign_project_refused=true` and
`managed_foreign_project_returned_to_a=true`. Ordinary A→B→A, new-root,
imported-fork, fork return, Home stale/return, passive B, historical lookup,
and cleanup gates also pass. Final revisions are A=8 and B=1.

The fixed JSON report contains no raw session IDs or picker input. It includes
only fixed identity hashes and route sequence diagnostics. No Rust, assets,
helpers, canonical source, build, commit, push, activation, or user
configuration was changed.
