# P4 managed plugin composition — 2026-10-25

Status: **harness correction implemented and native launch verified; UI
acceptance remains blocked at the ordinary A-to-B picker guard**. The native
run confirmed both managed servers, initial bindings, production bridge
behavior, historical lookup, message absence, and cleanup. It did not prove
the guarded session change or reach the foreign-project route/refusal/return.

## Cause and composition

The startup follow-up had moved the nonce/sequence observer outside the empty
server storage directory, which allowed managed `serve` to start. The next
probe still reached `initial_managed_snapshot_not_confirmed`. The harness
constructed a private CLI config with the pinned keymap and then assigned
`plugins = [observer_dir]`; this discarded the production registration supplied
by `src/provider_bridge_launch.rs` (`identity-plugin/tui.ts` and
`identity-plugin/recovery.ts`).

Pinned OpenCode 2.0.32's bundled loader code at byte offset 113,457,251 was
inspected: its `Oe` merge keeps object merge behavior only for non-array
objects (`ke`); otherwise it assigns the supplied value. Therefore inline
`OPENCODE_CLI_CONFIG_CONTENT` replaces the plugin array. The fixed harness now
provides both entries: `../identity-plugin`, resolved relative to the private
`opencode-config/cli.json`, and the absolute path to the disposable observer
folder outside server storage. This preserves the exact Panelink-generated
production plugin and adds the independent route observer without changing
production files or launch behavior. The observer remains read-only and its
existing same-instance nonce/sequence guards remain unchanged.

The path contract follows the pinned config loader's source-relative local
plugin resolution. The post-fix native run reached two `last_confirmed`
snapshots at revision 1 and verified the exact initial A binding, which
confirms that the Panelink-generated production plugin loaded through the
composed CLI config. The route observer was included alongside it, but this
run stopped before the guarded switch could establish a same-instance route.
A regression test constructs the launch directory layout, resolves
`../identity-plugin` from `opencode-config`, checks it points at the production
plugin directory, and checks that the observer is also registered.

## Post-fix native report

- Launcher: `opencode` absent from PATH; used
  `/Users/sahillamba/.opencode/bin/opencode`; no `tescode` wrapper.
- Pinned OpenCode 2.0.32 SHA-256:
  `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
- Existing Panelink binary SHA-256:
  `1723ccece8d9c39662b8f0833455747f8ac7951de4d9e8d22a73cac4cf92f357`.
- Fixed-schema native report:
  `/private/tmp/panelink-p4-20261025/plugin-merge-native-report.json`.
- Result: `classification=blocked`,
  `reason=managed_session_switch_not_proven`. Startup passed for two managed
  children; both initial snapshots reached `last_confirmed`; exact initial A
  binding and passive B state passed; cleanup, post-stop historical lookup,
  and no-user-message checks passed. No prompts were submitted.
- The native picker reached the filtered row but the exact guard refused
  Enter: `filtered_child_row_layout_safe=false`, target row count 2 and
  competitor row count 1. This is the next blocker; the report does not prove
  a selected B route, any foreign route, refusal, or return-to-A.
- The report records `cleanup_completed=true`, loopback-only sandbox
  enforced, and `prompt_submissions=0`. The existing app binary was reused;
  no Rust build was needed.

## Verification

Focused Python unit suite: 22 passed, 1 opt-in native test skipped, 0 failures.
`py_compile` and scoped `git diff --check` passed. The one fixed-schema native
managed report is recorded above. The acceptance guard remains unchanged and
failed closed. No Rust or asset changes are part of this slice. No commit or
push is authorized for this subtask.
