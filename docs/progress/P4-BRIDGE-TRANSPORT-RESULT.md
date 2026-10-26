# Phase 4 bridge wire and scoped Unix channel — 2026-10-24

Status: **both bounded component slices implemented and parent-verified**.
Two GPT-6 Luna High owners implemented exclusive modules/notes; the parent
reviewed, integrated, strengthened coverage and ran full checks. No provider
plugin, global listener, live workspace, launcher or recovery feature is enabled.
Full Phase4 remains open.

## Slice 1: bounded independent wire protocol

`provider_bridge_protocol` adds the distinct `PLBRDG01` version1 envelope with
Hello, Welcome and Observation messages. Header32 bytes; maximum complete
message4388 bytes. Kind-specific length caps are validated before body
allocation. Exact field lengths, reserved bytes, tags, target/instance/sequence,
version and identity alphabets are validated before a typed observation is
returned. Encoded bytes contain identity metadata and the handshake capability;
Debug/errors redact them, and callers must not log or persist wire bytes.

The codec uses read-only Frame accessors added by the parent. It introduces no
new dependency or change to the existing control protocol. Tests include every
truncated prefix for all three message kinds, trailing bytes, wrong/control
magic, invalid lengths/tags/reserved fields, maximum fields and redaction.
See [format and evidence](P4-BRIDGE-WIRE.md) and the exact
[task contract](P4-BRIDGE-TRANSPORT-TASK.md).

## Slice 2: same-user grant-scoped Unix channel

`provider_bridge_transport` accepts caller-supplied connected streams. Both
endpoints check kernel same-user credentials before protocol bytes. The reader
consumes a non-Clone grant bound to full target, project, exact2.0.32 version,
source instance and32 opaque capability bytes. `Grant::issue` uses the OS RNG;
`Grant::hello` provisions the matching typed Hello. The sender rejects an
unsupported version before sending the capability and requires matching
Welcome. Observation operations enforce that complete scope on both sides.

The stream remains private, nonblocking, close-on-exec and NOSIGPIPE. Each
message uses one absolute deadline shared across header/body IO; handshake
reads/writes share the supplied deadline. EOF, timeout, malformed input, wrong
role or scope permanently poison and shut down the channel. Neither endpoint
exposes control/input/process authority or a raw-stream escape hatch.
See [channel contract](P4-BRIDGE-CHANNEL.md).

This verifies kernel user identity and possession of an issuer-created grant.
It does not attest a provider executable, isolate hostile same-UID processes,
establish a global replay/revocation registry, or prove when a route was
observed. Buffered metadata is not a selection-ordering or fresh-reconciliation
barrier. Trusted issuer integration, fresh instance assignment, secret delivery,
bounded queues and fresh snapshot semantics remain owner/worker work.

## Combined parent acceptance

`tests/p4_bridge_transport.rs` runs two actual local IPC scenarios:

- A private0700 fixture with a0600 pathname socket performs Hello/Welcome,
  transports selected A then B, admits and saves each through BridgeReceiver
  and ProviderBindingStore, then transports Home. Home revokes current
  eligibility while retaining B. Reopening the saved record starts stale.
- A raw fixture deliberately fragments a valid handshake and observation A,
  then sends only an oversized fixed header. Rejection occurs before the
  declared body is read. The channel remains poisoned; explicit owner-side
  failure handling marks the receiver unavailable and retains saved revision1.

Both targets pass in the full run. Source/target rejection, wrong capabilities,
unsupported versions, partial EOF, stalled and fragmented IO, output backpressure
and redaction also have actual socketpair unit coverage. The new channel does
not have a different-UID process acceptance test; it calls the existing kernel
peer-credential boundary, whose unit mismatch gate remains separate evidence.
No provider account, prompt, user config or real Panelink pane is involved.

## Verification

All Cargo commands used locked/offline resolution and the existing isolated
cache/build directories:
`CARGO_HOME=/private/tmp/panelink-p4-cargo-home`,
`CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target`.

| Check | Actual result |
| --- | --- |
| Full serial Rust suite | **1157 passed,0 failed,15 ignored**,188.292s; library762, main binary148, integration247 |
| Focused bridge library | **31 passed**,0 failed/ignored: receiver12, codec7, channel12 |
| New combined integration target | **2 passed**,0 failed/ignored, including private pathname socket |
| Strict all-target Clippy | Pass |
| Formatting and isolated build | Pass |
| Separate input decoder | **39 passed**,0 failed/ignored |
| Component source fingerprints | Both match |
| Python discovery | **160 passed**,no failures/skips,8.182s |
| Documentation and whitespace | 299 Markdown files checked; zero findings |

The full serial command ran separately with a360s deadline; seven supporting
checks ran separately. No unmodified180s combined-verifier8/8 claim is made.
Ignored tests and the separate human Phase3 host gate remain open.
Receipts/logs are `/private/tmp/panelink-p4-20261024/bridge-transport-*`, including
`bridge-transport-cargo-summary.json`, `bridge-transport-cargo-serial.log` and
`bridge-transport-checks.json`.

Initial development failures are retained in the worker notes: a declared
sibling module had not yet landed, a transient duplicate helper blocked compile,
a test incorrectly expected Hello to reach the Observation maximum, and strict
Clippy found needless borrows. Parent review added the missing issued-grant
provisioning path, unsupported sender-version refusal before IO, expanded
failure coverage and all-kind truncation checks. Final formatting fixed import
ordering. All final code was present before the passing full serial run.

No dependency or lockfile change. The activated `target/debug/panelink` SHA-256
is unchanged:
`3b67218515bc482d2a1d016e7e344291fb97cee6b3dcdc39cc11baa2642a669e`.
Prior live provider and process-kill/store evidence remains separate and valid
within its recorded scope; it is not expanded by these transport fixtures.

## Next slices

Implement opt-in owner/worker wiring: trusted grant/source provisioning and
revocation, explicit fresh-snapshot reconciliation, bounded IO queues, ordered
storage completions and visible expiry/failure states. Keep slow IO outside the
state owner. A production heartbeat policy and joint managed-session binding
transaction remain open, as do cross-generation/manual recovery and the
new-conversation/nonempty-fork/same-client-project-switch identity gates.
No automatic resume or current-at-crash claim is enabled. No commit or push.
