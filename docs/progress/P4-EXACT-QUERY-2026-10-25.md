# Phase 4 exact foreign query — 2026-10-25

Status: **the full-title query reached the global picker, but the managed
foreign route remains blocked before Enter**. The remaining guard correctly
rejects selectable rows that are not recognized as fixture sessions. No guard
was weakened and no second native attempt was made.

## Bounded change

The foreign-project selection now filters with the complete fixture title,
`Panelink Foreign Root`, in both the fresh managed-child read closure and
`select_session`. The caller default already used the target label for the
fresh foreign return-to-A read, so that route was left unchanged. Full target
row identity, stable fresh reads, exact query/result layout, Projects-category
rejection, unknown-selectable-row rejection, and the nonce/sequence/same-TUI
observer checks remain enforced.

Focused regression coverage confirms that the full-title filtered query can
pass the exact target guard while Projects and unknown selectable rows still
fail closed.

## Verification

- `python3 -m unittest -v tests.test_panelink_provider_bridge_probe` — 27 run,
  26 passed, 1 opt-in native test skipped, 0 failures.
- `python3 -m py_compile scripts/probe_panelink_provider_bridge.py tests/test_panelink_provider_bridge_probe.py` — passed.
- Scoped `git diff --check` for the probe and test files — passed.
- No build was run. The existing Panelink binary SHA-256 was
  `1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`.

## Single fixed-report native attempt

- Launcher: `/Users/sahillamba/.opencode/bin/opencode`; `opencode` was not on
  `PATH`, and `tescode` was not used.
- OpenCode: pinned `2.0.32`, SHA-256
  `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
- Report: `/private/tmp/panelink-p4-20261025/exact-query-r6.json`; command exit
  2, `classification=blocked`,
  `reason=foreign_project_scope_refusal_not_proven`.
- Preconditions and prior routes: pinned binary matched; loopback-only sandbox
  enforced; two managed serve children started; initial A binding, passive B,
  A→B→A, new-root, imported-fork selection and return, and fresh Home return
  all passed. The Home observer route was valid, same-instance, and advanced
  its sequence.
- Foreign picker: Ctrl-O and the full title query were written. Fresh
  pre-action and modal reads were valid; the filtered screen had one exact
  target row plus the query (two query/target occurrences), no Projects
  category, and no known competitor fixture. The guard nevertheless reported
  `unknown_selectable_row_count=6` and `filtered_child_read_valid=false`.
  `enter_sent` is absent/false: Enter was withheld. The trace records seven
  fixture titles, target index 6, heading row 7, Sessions row 11, and owner
  revision 3. No foreign observer route acknowledgement followed; the session
  picker was still open. Thus managed foreign selection, refusal, and
  return-to-A are all false/unproven; return was not attempted.
- Safety: `prompt_submissions=0`, before/after user-message-absence checks and
  aggregate absence verification passed, `cleanup_completed=true`, and
  `network_scope=loopback_only_sandbox`.

The unknown-row count needs a narrowly scoped diagnosis using the fixed
coordinates and the pinned picker semantics before another native attempt.
Do not bypass or weaken this guard. Phase 4 foreign-project UI selection,
refusal, and return remain open; this report is not acceptance evidence for
those transitions.
