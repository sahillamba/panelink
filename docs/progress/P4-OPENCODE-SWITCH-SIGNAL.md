# P4 OpenCode 1.19.0 switch-signal probe

Historical1.19.0 evidence. The installed CLI is now2.0.32; see the
[current V2 preflight](P4-V2-IDENTITY-RESULT.md). This result is not V2
compatibility or identity proof.

Status: **selected-session switch not proven**. Read-only inspection confirms
keyboard session-list selection navigates directly and does not dispatch
`tui.session.select`. That event is a separate push signal. The TUI plugin host
has no route subscription, so changed-route polling is required for keyboard
selection. The current bounded diagnostic reached readiness and created two
sessions, but the TUI plugin did not load. No controlled transition is claimed.

## Isolation and launcher

The resolved launcher was `/Users/sahillamba/.opencode/bin/opencode`, using the
home-directory fallback because `opencode` was not found on PATH. Its isolated
version command returned `1.19.0`. The live probe creates a fresh mode-0700
temporary HOME, XDG roots, and project per repetition, binds the local OpenCode
server only to `127.0.0.1` on an ephemeral port, uses a private 24x100 PTY, and
creates only two empty local sessions. No provider or model request and no
prompt submission are made. IDs are hashed in records; raw server logs, PTY
output, IDs, response bodies, credentials, user config, and provider accounts
are not included in output or evidence. Process groups are killed during
cleanup. Before this follow-up run, no probe-owned process group or stale
private probe directory remained; no unrelated or ambiguous process was touched.

The macOS `sandbox-exec` loopback wrapper is now optional and **off by default**;
it can be requested with `--sandboxed`. This run used the default unsandboxed
path. The probe uses a loopback-only HTTP client with inherited proxies disabled,
but OS-level network enforcement was not applied in this run.

## Exact 1.19.0 bundle evidence

Read-only inspection of `/Users/sahillamba/.opencode/bin/opencode` found a
144,372,192-byte native executable. These exact snippets are from the installed
bundle at decimal byte offsets:

- **Offset 76,645,485 — keyboard session-list selection:**

  ```text
  onSelect:(H)=>{n.navigate({type:"session",sessionID:H.value}),U.clear()}
  ```

  The picker calls `navigate` directly and clears the dialog. The selection
  handler does not publish a `tui.session.select` event.
- **Offset 76,666,276 — separate application event listener:**

  ```text
  D.on("tui.session.select",(k,{workspace:G})=>{
    if(G!==V.workspace.current())return;
    g.navigate({type:"session",sessionID:k.properties.sessionID})
  })
  ```

  This handler is separate from the keyboard picker's `onSelect`.
- **Offset 65,670,271 — explicit TUI API select-session dispatch:**

  ```text
  return yield*nm(m.get(z.payload.sessionID)),
    yield*i.publish(In.SessionSelect,z.payload),!0
  ```

  The distinct select-session HTTP API publishes the push event; this is not
  the keyboard picker path.
- **Offset 78,781,640 — TUI plugin API wrapper:**

  ```text
  H={register(C){...},navigate(C,j){J.route.navigate(C,j)},
    get current(){return J.route.current}},
  ...K={on(C,j){return X.track(J.event.on(C,j))}}
  ```

  The plugin wrapper exposes `route.current`, `route.navigate`, and a general
  event listener. No route subscription or route-change callback appears in
  this wrapper. The adjacent plugin declaration bundle is version 1.18.32,
  not an exact CLI 1.19.0-matched contract.
- **Offset 63,392,764** embeds `<leader>n` (new session) and `<leader>l`
  (session list); the embedded leader declaration is `ctrl+x`.

Conclusion: `tui.session.select` is a usable push signal when explicitly
published, but the inspected 1.19.0 keyboard-selection path does not publish
it. For keyboard selection, a plugin without a route subscription must poll
`route.current` for changes.

## Readiness regression diagnosis and driver/fixture

The first follow-up five-run probe reported `local_server_readiness_timeout`
5/5. That was a probe regression, not an OpenCode limitation. The earlier probe
had passed readiness with `127.0.0.1`. The changed probe had used `localhost`
while the server bound IPv4 loopback and had installed the TUI plugin config
before `opencode serve`. Bounded diagnostics that printed only HTTP status and
timing isolated the reproducible config regression: with the plugin config
present when the server starts, `/session` did not respond during the
diagnostic bound; without that config, `/session` returned HTTP 200 in roughly
0.5–0.6 seconds. The fixed probe uses `127.0.0.1` and starts the server before
creating the project-local TUI fixture. A standalone check returned HTTP 200
after 5 attempts in 538.1 ms. The fixed probe's bounded single repetition
reported HTTP readiness in 9 attempts / 552.939 ms and created two sessions.
No response bodies or raw logs were printed.

The PTY driver uses screen-state waits rather than inter-key sleeps. It waits
until the rendered `Sessions` dialog and both expected rows are visible before
typing; after title search it waits for the target row and absence of the other
row before Enter. It records the latest plugin observation before selection,
waits for the requested session hash, then waits for the selected session title
before opening the next picker. The fixture registers `tui.session.select` and
records a hash-only push event if emitted. In parallel, a 40 ms changed-route
poll records transition changes only. Unit tests cover parsing, transition
ordering/classification, launcher precedence, and picker/filter-state
predicates.

## Previous five-repetition result (superseded)

Command executed: `python3 scripts/probe_opencode_tui_identity.py`.

Actual output after the readiness fix but before plugin startup behavior was
restored:

```json
{"blocked_reason": "one_or_more_repetitions_blocked", "classification": "blocked", "current_at_crash": "not_tested", "defaults": {"leader": "ctrl+x", "session_list": "l", "session_new": "n"}, "durable_acknowledgment": "not_tested", "launcher": "opencode", "network_scope": "loopback_only_local_session_creation", "plugin_api_package_version": "1.18.32", "plugin_api_version_matches_cli": false, "prompt_submissions": 0, "repetitions": [{"auth_prompt_seen": false, "elapsed_ms": 12684.674, "extra_transition_count": 0, "initial_route_captured": false, "latency_bounds_passed": false, "local_plugin_path_seen": false, "missed_transition_count": 0, "new_session": "not_attempted", "plugin_error_log_seen": false, "plugin_loaded": false, "plugin_module_name_seen": false, "pty_output_bytes": 1972, "readiness_attempt_count": 8, "readiness_elapsed_ms": 518.485, "readiness_last_http_status": null, "reason": "tui_plugin_not_loaded", "record_rejected_count": 0, "repetition": 1, "route_change_callback_api": "listener_unavailable_route_poll_fallback", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "blocked", "switch_A_B_A_observed": false, "transitions": [], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 12672.611, "extra_transition_count": 0, "initial_route_captured": false, "latency_bounds_passed": false, "local_plugin_path_seen": false, "missed_transition_count": 0, "new_session": "not_attempted", "plugin_error_log_seen": false, "plugin_loaded": false, "plugin_module_name_seen": false, "pty_output_bytes": 1972, "readiness_attempt_count": 9, "readiness_elapsed_ms": 522.336, "readiness_last_http_status": null, "reason": "tui_plugin_not_loaded", "record_rejected_count": 0, "repetition": 2, "route_change_callback_api": "listener_unavailable_route_poll_fallback", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "blocked", "switch_A_B_A_observed": false, "transitions": [], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 12659.558, "extra_transition_count": 0, "initial_route_captured": false, "latency_bounds_passed": false, "local_plugin_path_seen": false, "missed_transition_count": 0, "new_session": "not_attempted", "plugin_error_log_seen": false, "plugin_loaded": false, "plugin_module_name_seen": false, "pty_output_bytes": 1972, "readiness_attempt_count": 9, "readiness_elapsed_ms": 509.08, "readiness_last_http_status": null, "reason": "tui_plugin_not_loaded", "record_rejected_count": 0, "repetition": 3, "route_change_callback_api": "listener_unavailable_route_poll_fallback", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "blocked", "switch_A_B_A_observed": false, "transitions": [], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 12672.475, "extra_transition_count": 0, "initial_route_captured": false, "latency_bounds_passed": false, "local_plugin_path_seen": false, "missed_transition_count": 0, "new_session": "not_attempted", "plugin_error_log_seen": false, "plugin_loaded": false, "plugin_module_name_seen": false, "pty_output_bytes": 1972, "readiness_attempt_count": 9, "readiness_elapsed_ms": 508.766, "readiness_last_http_status": null, "reason": "tui_plugin_not_loaded", "record_rejected_count": 0, "repetition": 4, "route_change_callback_api": "listener_unavailable_route_poll_fallback", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "blocked", "switch_A_B_A_observed": false, "transitions": [], "tui_process_alive_after_plugin_wait": true}, {"auth_prompt_seen": false, "elapsed_ms": 12816.382, "extra_transition_count": 0, "initial_route_captured": false, "latency_bounds_passed": false, "local_plugin_path_seen": false, "missed_transition_count": 0, "new_session": "not_attempted", "plugin_error_log_seen": false, "plugin_loaded": false, "plugin_module_name_seen": false, "pty_output_bytes": 1972, "readiness_attempt_count": 12, "readiness_elapsed_ms": 662.696, "readiness_last_http_status": null, "reason": "tui_plugin_not_loaded", "record_rejected_count": 0, "repetition": 5, "route_change_callback_api": "listener_unavailable_route_poll_fallback", "route_change_callback_observed_count": 0, "sessions_created_count": 2, "status": "blocked", "switch_A_B_A_observed": false, "transitions": [], "tui_process_alive_after_plugin_wait": true}], "route_change_callback_api": "SDK tui.session.select push event; keyboard route changes use changed-route polling fallback", "version": "1.19.0", "version_status": "proven"}
```

This five-run result is superseded. Comparing with `HEAD` identified a fixture
startup regression: the committed fixture wrote `plugin_loaded` immediately
and used a 100 ms polling interval. The edited fixture changed that poll path
and the TUI remained on its loading screen. The fixture now restores the
committed load record and 100 ms poll interval, retaining the separate event
listener, and suppresses unchanged session route samples. A bounded diagnostic
outside the experiment recorded 41 fixture records and observed the TUI leave
the loading screen. The new required run follows.

Durable acknowledgment ordering, current-at-crash identity, provider resume,
and crash recovery are **not tested**. Keep last-confirmed/manual recovery; do
not claim verified selected-session identity from this run.

## Verification

`python3 -m unittest tests.test_opencode_tui_identity_probe -v`:

```text
Ran 8 tests in 0.001s
OK
```

`python3 scripts/check_docs.py`:

```text
Checked 198 Markdown file(s); 0 finding(s).
```

## Parent wrap-up — 2026-10-21 (supersedes any later-run claims above)

Worker: GPT-6-Luna (xhigh), resumed three times (a machine sleep + LLM auth
broker timeout interrupted the first run). Parent stopped it after ~75
minutes past the original timebox and killed its isolated `opencode serve`
and TUI processes.

Durable findings:
- Bundle evidence above stands: in 1.19.0 keyboard session selection calls
  `navigate` directly and does NOT publish `tui.session.select`; the plugin
  API has no route subscription, so `route.current` polling is the only
  keyboard-path signal. A push-based selected-session guarantee is therefore
  not available from this OpenCode version.
- Two probe regressions introduced in this session were diagnosed: writing
  the TUI plugin config before `opencode serve` started hung server
  readiness (parent independently confirmed an isolated `serve` answers
  `GET /session` 200 in ~1 s), and a 40 ms fixture poll interval hung TUI
  startup (100 ms, as in the committed fixture, loads). Their fixes are in the
  working tree but the five-repetition rerun did NOT complete.
- Classification remains **not proven**; the earlier P4-OPENCODE-TUI-IDENTITY
  result (initial route 3/3, one switch missed per run) is the latest
  completed live experiment. Keep last-confirmed/manual recovery.

Unit tests: `python3 -m unittest tests.test_opencode_tui_identity_probe` OK.
Next step if resumed: one diagnostic repetition with the current driver,
then the 5-repetition gate; consider requesting an upstream route-change hook.
