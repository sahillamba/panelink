# P4 OpenCode 1.19.0 private-PTY identity experiment

Status: **not proven** — the local TUI plugin loaded and captured the initial
session route in all three repetitions, but one of the two controlled idle
session switches was not observed in each repetition. No selected-session
switch guarantee is established. Do not advertise verified current identity
or automatic provider-session recovery; use last-confirmed/manual recovery.

## Scope and isolation

The experiment was run with the installed native CLI, resolved by the required
launcher order as `opencode` at `/Users/sahillamba/.opencode/bin/opencode`.
`--version` reported `1.19.0`. Each repetition used a fresh mode-0700 temporary
HOME and XDG config/data/state/cache roots, one fresh project directory, a
minimal environment, update/model-fetch disabling, and a private PTY sized
24x100. The server bound only `127.0.0.1` on an ephemeral port. Only two empty
local sessions were created; no provider or model request and no prompt
submission occurred. TUI output, session identifiers and response bodies were
not printed or saved as evidence. The fixture records SHA-256 session hashes,
monotonic observation time, route name and sequence in private JSONL. Child
process groups are stopped in `finally` before temporary roots are removed.
Each repetition completed below the 60-second experiment deadline.

The executable wrapper path is supported as `tescode workflow custom ARGS`;
launcher resolution is exercised in unit tests. This run used native `opencode`.

## Installed API and keybinding evidence

Read-only inspection of the installed CLI bundle and local package declarations
found:

- `/Users/sahillamba/.opencode/node_modules/@opencode-ai/plugin/dist/tui.d.ts`
  lines 13–24 define TUI route `session` with `params.sessionID`; lines 459–476
  expose `TuiPluginApi.route.current` and `route.navigate`; lines 407–410 expose
  the typed TUI event bus; lines 505–510 define the TUI plugin entry.
- The package metadata at
  `/Users/sahillamba/.opencode/node_modules/@opencode-ai/plugin/package.json`
  lines 1–5 identifies version `1.18.32`, older than the observed CLI `1.19.0`.
  It is useful surface evidence, not an exact 1.19.0-matched type bundle.
- `/Users/sahillamba/.opencode/node_modules/@opencode-ai/sdk/dist/v2/gen/types.gen.d.ts`
  lines 2170–2178 define the `tui.session.select` event with `sessionID`; the
  inspected TUI plugin API has no route-change-specific callback in its typed
  contract. The fixture attempts an event listener and records whether it was
  accepted; it was registered, but no callback records were observed during
  this run.
- `/Users/sahillamba/.opencode/node_modules/@opencode-ai/sdk/dist/v2/gen/types.gen.d.ts`
  lines 9703–9712 show v2 session create has no title field. The fixture uses
  the existing local `/session` create then PATCH `/session/{id}` title path;
  these names are local UI labels and are not persisted by the probe.
- The installed CLI bundle embeds default bindings at the `session_new` and
  `session_list` declarations: `<leader>n` (“Create a new session”),
  `<leader>l` (“List all sessions”), and leader `ctrl+x` (“Leader key for
  keybind combinations”). The bundle also lists `session.list` and
  `session.new` commands. The experiment opens the switcher with Ctrl-X then L
  and uses title search plus Enter to select without submitting prompt text.
  Ctrl-X then N was reserved for the optional empty-new-session attempt, which
  was skipped because switching was not proven.
- The CLI config loader's inspected bundled `TuiConfig.loadState` implementation
  looks for global `tui.json`, optional `OPENCODE_TUI_CONFIG`, project
  `tui.json`, and `.opencode/tui.json`; it also scans `.opencode` plugin
  directories. The successful fixture is project-local under
  `.opencode/tui.json` plus `.opencode/plugins/panelink-tui-probe.js`.
  A preliminary wrong config location and wrong project argument form failed;
  those attempts are superseded by the final corrected run below.

## Final actual command and output

The exact requested script command was also executed. Its separate final run
was blocked before session creation: the local server did not become ready in
three attempts (each reported `local_server_readiness_timeout`). The following
JSON is the completed 3-repetition experiment from the same implementation,
invoked with the equivalent `run()` entry point; it is the conclusive result
because the script command's attempt was environment-flaky before the TUI gate.

```text
{"blocked_reason": "one_or_more_repetitions_not_proven", "classification": "not_proven", "current_at_crash": "not_tested", "defaults": {"leader": "ctrl+x", "session_list": "l", "session_new": "n"}, "durable_acknowledgment": "not_tested", "launcher": "opencode", "network_scope": "loopback_only_local_session_creation", "plugin_api_package_version": "1.18.32", "plugin_api_version_matches_cli": false, "prompt_submissions": 0, "repetitions": [{"auth_prompt_seen": false, "elapsed_ms": 15381.359, "extra_transition_count": 0, "initial_route_captured": true, "latency_bounds_passed": false, "local_plugin_path_seen": true, "missed_transition_count": 1, "new_session": "skipped_switch_not_proven", "no_extra_transitions": true, "plugin_error_log_seen": false, "plugin_loaded": true, "plugin_module_name_seen": true, "pty_output_bytes": 9125, "reason": "controlled_session_selection_not_observed", "record_rejected_count": 0, "repetition": 1, "route_change_callback_api": "tui.session.select_listener_registered", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "not_proven", "switch_A_B_A_observed": false, "transition_2d000ccd_ecd9d22d": "not_observed", "transition_ecd9d22d_2d000ccd": "observed", "transitions": [{"extra_transition_count": 0, "latency_ms": 306.936, "observed": true, "within_latency_bound": true}, {"extra_transition_count": 0, "latency_ms": null, "observed": false, "within_latency_bound": false}], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 14846.701, "extra_transition_count": 0, "initial_route_captured": true, "latency_bounds_passed": false, "local_plugin_path_seen": true, "missed_transition_count": 1, "new_session": "skipped_switch_not_proven", "no_extra_transitions": true, "plugin_error_log_seen": false, "plugin_loaded": true, "plugin_module_name_seen": true, "pty_output_bytes": 9125, "reason": "controlled_session_selection_not_observed", "record_rejected_count": 0, "repetition": 2, "route_change_callback_api": "tui.session.select_listener_registered", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "not_proven", "switch_A_B_A_observed": false, "transition_7360a4c8_77e333f4": "not_observed", "transition_77e333f4_7360a4c8": "observed", "transitions": [{"extra_transition_count": 0, "latency_ms": 301.833, "observed": true, "within_latency_bound": true}, {"extra_transition_count": 0, "latency_ms": null, "observed": false, "within_latency_bound": false}], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 14173.651, "extra_transition_count": 0, "initial_route_captured": true, "latency_bounds_passed": false, "local_plugin_path_seen": true, "missed_transition_count": 1, "new_session": "skipped_switch_not_proven", "no_extra_transitions": true, "plugin_error_log_seen": false, "plugin_loaded": true, "plugin_module_name_seen": true, "pty_output_bytes": 9125, "reason": "controlled_session_selection_not_observed", "record_rejected_count": 0, "repetition": 3, "route_change_callback_api": "tui.session.select_listener_registered", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "not_proven", "switch_A_B_A_observed": false, "transition_cca97f66_f7b740c7": "not_observed", "transition_f7b740c7_cca97f66": "observed", "transitions": [{"extra_transition_count": 0, "latency_ms": 282.402, "observed": true, "within_latency_bound": true}, {"extra_transition_count": 0, "latency_ms": null, "observed": false, "within_latency_bound": false}], "tui_process_alive_after_plugin_wait": true}], "route_change_callback_api": "SDK event type tui.session.select; listener observation depends on loaded TUI plugin", "version": "1.19.0", "version_status": "proven"}
In repetitions 1–3, respectively, A→B, A→B, and B→A were observed within the
5,000 ms bound (306.936 ms, 301.833 ms, 282.402 ms). The other transition was
not observed in each repetition. Thus
switch A→B→A did not meet the ordered-correlation gate. The run observed no
extra transitions. The callback was registered according to the plugin's
typed event API, but it yielded 0 callback records; this is not callback-based
switch proof.

## Classification and next step

- **Proven:** installed native CLI version 1.19.0; project-local fixture loads;
  initial selected route and session hash are readable through `route.current`;
  switcher appears and one controlled transition per repetition is correlated
  within the 5 s bound.
- **Not proven:** repeatable ordered A→B→A session selection. Every repetition
  misses one transition, and the candidate `tui.session.select` callback yields
  zero records. Optional new-session navigation was skipped because the switch
  gate did not pass.
- **Blocked:** no authentication prompt occurred and no provider/model traffic
  was made. The installed plugin type package is version 1.18.32 rather than
  a verified CLI 1.19.0-matched package, so API compatibility beyond the
  observed load/route behavior is not established.
- **Separate gates, not tested:** durable acknowledgment ordering, crash
  boundary/current-at-crash identity, provider resume, and recovery behavior.
  No such property follows from route polling or this experiment.

Precise next step: inspect the exact OpenCode 1.19.0 embedded TUI plugin host
and menu selection flow to determine whether its candidate `tui.session.select`
event is emitted on keyboard selection, then rerun the automated private-PTY
correlation only after that trigger/API is established. Keep last-confirmed /
manual recovery until ordered switches are captured on every required run.

## Verification

The pure tests cover allowlisted JSONL parsing, session-hash validation,
transition order/miss/extra/latency classification and launcher precedence.
Actual results:

```text
$ python3 -m unittest tests.test_opencode_tui_identity_probe -v
Ran 6 tests in 0.001s
OK
$ python3 scripts/check_docs.py
Checked 192 Markdown file(s); 0 finding(s).
```

The separate requested script invocation returned exit 2 after three
`local_server_readiness_timeout` results; it is a failed live attempt, not a
pass. The completed `run()` invocation above reached the plugin/switch gate and
returned `not_proven`. No process remained running and each temporary
experiment tree is removed by `TemporaryDirectory` cleanup.
