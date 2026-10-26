# Phase 4 OpenCode TUI bridge plugin — 2026-10-24

## Current integration status

The plugin is now embedded and activated by opt-in `serve --opencode-bridge`.
Real pinned native processes reach durable initial identity; explicit recovery
acceptance also exercises SDK validation and public-router navigation. See
[P4-NATIVE-MANUAL-RECOVERY.md](P4-NATIVE-MANUAL-RECOVERY.md) and
[P4-MANAGED-BRIDGE.md](P4-MANAGED-BRIDGE.md) for current live gates. The source
now handles v2 kinds6–9 through `recovery.ts` in addition to snapshots. Socket
canonicalization resolves the parent directory and checks the final socket with
`lstat`; Bun cannot realpath a socket. The original component report below is
historical and does not describe the current activation state.


Status: the opt-in production source plugin and its local Unix-socket fixture
are implemented in `assets/opencode-bridge/tui.ts` and
`tests/test_opencode_bridge_plugin.py`. Live OpenCode/Panelink owner activation
and full Phase 4 remain open.

The plugin loads through OpenCode's native `Plugin.define` API from
`@opencode/plugin/tui`. `PANELINK_BRIDGE_SOCKET` is the sole required
activation value: Panelink supplies an absolute private Unix socket pathname.
`PANELINK_BRIDGE_HELLO` is optional; when present, its hex-encoded protocol-v1
Hello must byte-match server provisioning as an additional scope restriction.
The plugin removes both scoped keys from its own environment after reading
them. It validates the server-to-plugin V2 kind-5 Provision (challenge zero,
full v1 Hello payload), including exact provider version `2.0.32`, full target,
nonzero instance/capability, project bytes and private socket ownership/mode,
then sends the normal V2 kind-1 Hello. It validates V2 kind-2 Welcome and
handles only V2 kind-3 snapshot requests / kind-4 responses. The capability is
used only for scope equality and is cleared from the decoded buffer after
setup. No credential or diagnostic record is retained.

Each valid, increasing challenge is answered synchronously. Only after
accepting the request does the callback read `context.ui.router.current()` and
the selected session cache. It does not poll or infer state from lifecycle
events. A cached route with no parent is emitted as a top-level session; a
cached child is emitted as child activity without promoting the parent. Home
has no session ID. Unknown routes, missing cache entries, a changed or
unavailable canonical project path and lookup errors do not expose an ID under
the configured project identity. Frames carry no source timestamp. The
Panelink server owns receipt time, heartbeat and expiry.

Outer V2 packets use the 24-byte `PLBRSN02` header and at most 4,388 payload
bytes; nested v1 Hello, Welcome and Observation values use the existing bounded
`PLBRDG01` codec. The client caps buffered input to two maximum outer packets,
rejects malformed, out-of-order, unsolicited and replayed messages, limits
request handling to one response in flight, closes after an idle server gap
or a stalled write, and destroys the socket on protocol or IO failure. The
10-second idle close is only a dead-server bound; Panelink's 500 ms heartbeat
and 3,000 ms lease policy remain server responsibilities. Cleanup uses the
context cleanup hook when exposed and returns a disposer for the plugin host.

The source plugin checks that the socket path is absolute, resolves it with
`realpath`, and requires a socket owned by the current UID with no group/other
permission bits. It does not claim executable identity attestation or protect
against a hostile process running as the same user. Provision authenticity
and kernel peer-UID enforcement are server responsibilities. No user OpenCode
configuration, live provider, launcher, dependency, or committed binary was
changed by this slice. The plugin contract has not yet been exercised in a
live OpenCode process.

Focused automated evidence: `python3 -m unittest -v
tests.test_opencode_bridge_plugin` — 7 tests passed, 0 failed. The fixture
loads this exact TypeScript asset through Node's native TypeScript loader with
only the OpenCode plugin API mocked, and uses a real private Unix pathname
server. It verifies fragmented Provision/request reads, raw scope/identity and
sequence fields for A/B/Home/child/unknown routes, request-time route sampling,
socket-only activation without a Hello environment value, optional-Hello scope
mismatch, malformed Provision and pre-Welcome rejection, duplicate challenge
rejection, oversized-header shutdown, and absence of the Hello capability or
session IDs in plugin output.
The sandbox denied Unix pathname bind with `EPERM`; the exact focused command
was rerun under the approved local test execution and passed. This is a local
fixture result, not live provider or complete owner/server acceptance.
