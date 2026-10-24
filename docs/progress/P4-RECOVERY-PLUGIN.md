# Phase 4 OpenCode recovery companion

Status: the TUI bridge imports a companion that implements exact-ID validation
and one-shot route-action handlers for the pinned OpenCode API. The companion
does not own socket bytes or report selected-session proof. Runtime atomic
persistence, explicit UI/CLI integration, post-navigation observation, and
Phase 4 acceptance remain open.

`createRecoveryController(context, project, providerVersion)` exposes
`handle(kind, payload)` for kinds 6 and 8 plus `dispose()`. Validation parses
the bounded ASCII ID and calls the public exact lookup
`context.client.session.get({ sessionID })`. It checks returned `id`, canonical
`location.directory`, `parentID === null`, the active `context.location`, the
granted project bytes, and `context.app.version === "2.0.32"`. It does not use
the synchronous session cache. A top-level fork with `parentID === null` is
accepted regardless of its separate fork-origin metadata; a non-null parent
is a child result. Missing 404 responses map to Missing; project mismatch,
child-only, unsupported version, and uncertain lookup failures map to their
fixed wire statuses.

Validation has a 250 ms total async deadline, leaving time inside the host's
absolute 300 ms framed-I/O deadline, and performs no navigation. A
timed-out SDK promise is not awaited for later state changes; its late result
cannot establish a validated ID. Disposal cancels the pending race and clears
that ID. Navigation requires a successful validation of the same exact ID,
valid nonzero revision and ticket, and no earlier navigation attempt on this
controller. It spends its in-memory one-shot before a fresh exact SDK lookup,
then invokes `context.ui.router.navigate({ type: "session", sessionID })` at
most once. `ActionIssued` means only that the native route command returned;
the host must still obtain a later fresh selected-session snapshot. Timeout,
deletion, scope/version mismatch, disposal, or other failure returns
`NotIssued`; the host must not retry the spent ticket. The UI adapter owns the
durable spend boundary and must check its shared cancellation flag before
calling navigation.

The controller performs no model call, prompt submission, input injection,
session creation, or project switching. It emits no logs and does not access
environment variables or global provider configuration. It handles only the
kind and payload; outer challenges are checked by the main bridge asset.

`BridgeLaunch` embeds both TypeScript assets and writes them with `create_new`
and mode `0600` inside the launch-private mode `0700` plugin directory. The
main asset accepts control frames only after Welcome and serializes snapshot,
validation, and navigation requests under one increasing challenge. It holds
the one-request-at-a-time guard through the async SDK call and response write;
only snapshot requests advance selected-frame sequence numbers. Socket stop
disposes the controller, and late SDK results cannot produce responses.

## Verification

Run the isolated companion fixture with:

```sh
python3 -m unittest tests.test_opencode_bridge_recovery -v
```

It imports the TypeScript directly with Node's type stripping and uses mocked
SDK/router objects in temporary local project directories. It covers exact
root and fork-root acceptance, missing/foreign/child/unsupported/uncertain
results, exact identity mismatch, malformed kinds and payloads, one-shot
navigation, fresh revalidation, timeouts, late results, disposal, and zero
router actions on failures. The integrated fixture covers shared challenge
sequencing, validation without route sampling, later selected snapshots, and
pre-Welcome and malformed control rejection over a local Unix socket. Neither
fixture exercises a live provider API; that remains a separate acceptance gate.

Focused verification on 2026-10-24 passed: 14 main-asset socket tests plus 1
companion test, and 3 `provider_bridge_launch` Rust tests. Both socket-based
test groups required reviewed execution outside the sandbox after its local
AF_UNIX bind returned `EPERM`. Python bytecode compilation and
`rustfmt --check src/provider_bridge_launch.rs` also passed. The integrated
SDK timeout fixture returns Uncertain within the host's 300 ms deadline and
confirms that timed-out validation cannot enable navigation.
