# Phase 4 managed foreign refusal — 2026-10-25

Status: **the exact foreign route and public manual-refusal gate now pass;
return-to-A remains blocked by the strict picker/observer gate**. This is not
Phase 4 completion.

## Bounded probe change

The foreign title remains in the report as a screen diagnostic. Acceptance no
longer depends on that title after the independent observer proves the exact
foreign project/session on the same TUI instance at a newer sequence. The
settled screen still must be known with the picker closed, client input owner,
and the exact pane live/applied.

While that foreign route remains selected, the probe calls the existing public
`provider-binding prepare` command with the explicit foreign fixture session
ID and the original project and full managed target. Only the fixed error,
state, return code, revision, and booleans are emitted; the helper's ticket and
provider session ID are reduced to `ticket_present` and discarded. Acceptance
requires the exact `manual_selection_required` refusal with
`provider_project_mismatch` or `provider_recovery_unavailable`, no ticket or
selected confirmation, a newer same-instance foreign observer record after
the refusal, and the same durable A revision/hash before and after the CLI
call. The refusal command does not navigate.

The guarded UI return then requires a same-instance A observer sequence newer
than the post-refusal foreign sequence, a fresh owner `last_confirmed` result,
and a disk revision exactly equal to that fresh owner revision and newer than
the stale A revision. Picker ambiguity continues to withhold Enter.

## Focused verification

- `python3 -m unittest -v tests.test_panelink_provider_bridge_probe` — 30
  tests run, 29 passed, 1 opt-in native test skipped, 0 failures.
- `python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.
- Scoped `git diff --check` for the probe and test files — passed.
- No build was run; native evidence used the existing Panelink binary.

The acceptance regression confirms that a missing rendered title does not
override a valid exact observer/refusal result. Missing or wrong observer
identity, an A binding mutation, a successful prepare, or a ticket blocks
acceptance.

## Native managed probe

Fixed report: `/private/tmp/panelink-p4-20261025/foreign-refusal-r8.json`.
Launcher: `/Users/sahillamba/.opencode/bin/opencode`, OpenCode 2.0.32; the
existing Panelink binary matched its pinned SHA-256. The run enforced the
loopback-only sandbox, used empty fixtures, submitted zero prompts, and
completed cleanup.

The refusal gate passed. The independent observer acknowledged the exact
foreign project/session on the same TUI at sequence 115, then again at
sequence 117 after the CLI refusal. The public prepare returned
`manual_selection_required` / `provider_project_mismatch`, return code 1,
with no ticket and no selected confirmation. The existing owner was stale or
unavailable at last-confirmed revision 7; A's disk revision and provider hash
were unchanged before and after prepare. The screen title remained invisible,
while the picker was closed and the pane was live/applied with `client` input
ownership. Thus `managed_foreign_project_refused=true` despite
`foreign_title_visible=false`.

The return gate did not pass: `managed_foreign_project_returned_to_a=false`,
reason `foreign_project_return_to_a_not_proven`. In the final strict attempt,
the full A title appeared twice in the fresh modal projection (query and
result), so the row guard withheld Enter and no exact A observer route was
accepted. One prior bounded attempt used the shorter `Identity A` query; it
sent Enter but still did not produce an exact same-instance A route. Neither
result justifies relaxing the modal or observer guards. Final revisions were
A=7 and B=1; cleanup completed and prompt submissions remained zero.

The independent r9 modal-row report remains
`/private/tmp/panelink-p4-20261025/modal-rows-r9.json`; this report addresses
the subsequent refusal/return gate only. No Rust, assets, helper, canonical
source, build, commit, push, activation, or user configuration was changed.
Parent review and an independent native rerun remain required.
