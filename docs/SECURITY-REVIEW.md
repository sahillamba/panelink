# Panelink security review pack — evidence for review

Date: 2026-10-22. Scope: repository evidence mapped to the proposed SEC-01 through
SEC-08 criteria in [SPEC.md](SPEC.md#proposed-security-review-requirements).
This is an implementation evidence pack for the current prototype. It is not a
security audit, approval, certification, or statement of compliance. Product
requirements and prototype boundaries remain as stated in the canonical spec.

## Executive summary

The prototype runs locally on macOS/aarch64. Its runtime control endpoint is a
pathname Unix-domain socket in a per-user protected state tree; it checks the
kernel-reported peer UID before protocol input. Metadata and history stores
validate private directory/file ownership, modes and file identity. Wire and
terminal inputs have explicit bounds, clipboard requests from child output are
dropped, and stored terminal content is presented as sanitized text/checkpoints
rather than replayed effects.

Important limitations for reviewers:

- Same-user processes share the user's authority. The socket/UID checks do not
  isolate another process or agent running as that user, or constrain an agent's
  network access (SEC-04).
- Current-session OpenCode identity and in-TUI switching were not proven. There
  is no live Panelink provider adapter or automatic provider-conversation resume;
  recovery uses a fresh shell and manual provider selection (SEC-04/06).
- Saved history can contain secrets rendered by a child. It is protected local
  storage, not a secret-redaction system. User-facing capture disable and
  copy-on-selection settings are not implemented (SEC-05).
- Dependency inventory/advisory evidence is partial; release traceability and
  firm-specific assessment/update process remain review gates (SEC-01/02/08).

## Data flow and persisted data

```text
local terminal UI/client
   ⇄ authenticated framed messages over Unix socket
per-user Panelink server / state owner
   ⇄ server-owned PTYs (one per managed child)
shells, agents, and other explicitly launched child processes
```

The client displays and forwards local host input. The server validates bounded
requests, owns PTY descriptors and child lifetimes, parses PTY output with the
selected terminal engine, and returns bounded screen/metadata responses. A PTY
child's stdout/stderr are terminal bytes consumed by the server-side emulator;
they are not written raw to the server's diagnostics log. The default detached
server log receives server stdout/stderr.

The usual state root is `$PANELINK_HOME` when that value is absolute, otherwise
`$HOME/Library/Application Support/Panelink` ([`resolve_root`](../src/default_open.rs#L110)).
Its runtime directories include `server/`, `terminfo/` and `contexts/`; the server
endpoint is `server/control.sock` ([`StateRoot`](../src/default_open.rs#L85),
[`prepare_root`](../src/default_open.rs#L195)). The root and these directories
are created with mode `0700` and validated as current-user-owned with no
group/other permission bits; existing ancestors must be root- or
current-user-owned and not writable by other users, with the documented macOS
root symlink/sticky-directory cases
([`ensure_private_dir`](../src/default_open.rs#L136),
[`ancestors_ok`](../src/default_open.rs#L160)).

| Location / record | Persisted content | Enforced mode / bound |
| --- | --- | --- |
| `server/control.sock` | Local framed control connection; no file payload at rest. | Server directory `0700`; socket `0600`; pathname is fixed `control.sock`, existing endpoints are refused at bind ([`transport.rs`](../src/transport.rs#L31), [`bind`](../src/transport.rs#L321)). |
| `server/server.lock`, `generation`, `launch.journal` | Lock identity, generation counter, and fixed lifecycle facts: operation key, managed-session ID and state. Journal records are 56 bytes and capped at 1 MiB/4,096 keys. | Server directory `0700`; files current-user-owned, mode `0600`, single link ([`server_storage.rs`](../src/server_storage.rs#L17), [`validate_file`](../src/server_storage.rs#L36), [`P3-STORAGE.md`](progress/P3-STORAGE.md#startup-and-identity)). |
| `contexts/*` | Client generation/lease, sequence and uncertain-operation reconciliation metadata; not request bodies. | Parent directory `0700`; file `0600`; no-follow/exclusive create; existing file never reset ([`client_context.rs`](../src/client_context.rs#L22), [`create`](../src/client_context.rs#L192)). |
| `server/history/*` | Sanitized normal-scrollback rows plus separate latest normal/alternate screen checkpoints, keyed by target. Capture is enabled by the runtime setup path, not by an end-user off setting. | History directory `0700`; committed files `0600`, single link; 1,024 rows and 8 MiB per history segment; 30-day retention after end ([`history_store.rs`](../src/history_store.rs#L17), [`validate_file`](../src/history_store.rs#L69), [`enable_history`](../src/multi_runtime.rs#L526), [`P5-SAVED-HISTORY.md`](progress/P5-SAVED-HISTORY.md#p5-bounded-saved-history-2026-10-20)). |
| `server/layout/current.layout`, `previous.layout` | Fixed 160-byte historical layout snapshots: dimensions, orientation, target IDs and snapshot metadata. No launch specification, cwd, environment, title, PID or provider identity ([`SPEC.md`](SPEC.md#scoped-saved-layoutrestart-design-implemented-as-bounded-snapshot--read-only-view)). | Layout directory `0700`; records `0600`, single link; two slots ([`layout_store.rs`](../src/layout_store.rs#L21), [`open`](../src/layout_store.rs#L151), [`P5-LAYOUT-SLICE.md`](progress/P5-LAYOUT-SLICE.md)). |
| `server/links/links.bin` | Bounded old-to-new target IDs, operation outcome and timestamp for explicit relaunch links; no cwd/environment. | Link directory `0700`; file `0600`, single link; max 64 links, 30-day retention ([`recovery_links.rs`](../src/recovery_links.rs#L15), [`validate_file`](../src/recovery_links.rs#L137), [`P5-RECOVERY-RELAUNCH.md`](progress/P5-RECOVERY-RELAUNCH.md#behavior-implemented)). |
| `server.log` and `server.log.1` | Server stdout/stderr diagnostics; no PTY output relay. | `0600` on creation; rotates after 1 MiB ([`open_log`](../src/default_open.rs#L407), [`spawn_server`](../src/default_open.rs#L434)). |
| Compiled terminfo and fingerprint under `terminfo/` | Panelink's compiled `panelink-probe` entry and source fingerprint. | Containing directories are private; source/fingerprint writes use `0600` ([`ensure_terminfo`](../src/default_open.rs#L229), [`write_private`](../src/default_open.rs#L286)). |

Persisted history is display data, not guaranteed safe or secret-free: a child
may render a credential, prompt, or other sensitive value into visible cells or
scrollback. The store keeps normalized/sanitized cells rather than raw PTY byte
streams or executable escape sequences. It does not persist keystrokes, paste
contents, clipboard requests/replies, inherited environment values, launch
arguments, or provider conversation contents. The launch lifecycle journal is
metadata-only; launch arguments and environment values are volatile and dropped
after preparation ([`P3-STORAGE.md`](progress/P3-STORAGE.md#admission-and-launch-effects),
[`multi_startup.rs`](../src/multi_startup.rs#L123)). Provider-owned conversation
data remains outside Panelink's storage.

## Assets and trust boundaries

| Asset / boundary | Prototype treatment | Residual trust assumption |
| --- | --- | --- |
| Local control authority | Unix socket under private state; peer effective UID is kernel-authenticated before handshake bytes are read. Input additionally requires a session-specific owner lease/revision. | Same-user local processes can act with the same OS identity; UID is not a workspace permission grant. |
| Child process, provider credentials and network | Explicit executable/arguments/cwd/environment are constructed per launch; server startup environment is not silently used as child baseline; child owns its normal provider credentials and network behavior. | Panelink does not sandbox or network-isolate an agent, does not own provider accounts, and does not enforce organization egress policy. |
| Saved rows/screens/layout | Private per-user files, bounded codecs, fixed diagnostics and exact target binding; reads do not launch children or replay saved terminal effects. | Same user can access the files; terminal text may itself contain secrets; checksum/authentication against same-user edits is absent in these formats. |
| Input and clipboard | Bounded typed input; keyed commands deduplicate; keystrokes/paste/signal effects are not resent after uncertain disconnect. Programmatic child clipboard operations are dropped. | Explicit local user paste/copy controls are distinct; the user controls terminal and provider software. No organization policy UI exists yet. |
| Build and release inputs | Locked dependency evidence, notices, selected target/toolchain observations and measurements are linked below. | Reproducibility, complete firm review, approved distribution, signing/notarization and SBOM format are not established by the repository evidence cited here. |

## SEC-01 — Complete dependency graph and native/build components

**Requirement.** Review the complete dependency graph, including build-time
components and native code.

**What Panelink does / evidence.** The dependency inventory records the current
locked graph, direct/transitive roles, target/build metadata, local vendored
components, licenses and review flags: [P6 dependency and license
inventory](progress/P6-DEPENDENCY-INVENTORY.md). It reports 17 third-party
rows (7 direct, 10 transitive); its target-filtered Apple Silicon graph
contains 14 non-root packages. `cargo-audit` 0.22.2 was run against the
18-entry lockfile on 2026-10-21 with RustSec database commit
`4b587f00996ebe5c05a73eaef105ef241e876414`: `vulnerabilities: found=false,
count=0`; warnings for unmaintained/unsound/notice advisories: none. The database
contained 1,348 advisories. This is a dated known-advisory result, not a safety
finding or approval. The inventory explicitly records the failed offline
all-target metadata resolution and UNKNOWN license metadata for several
platform-only crates; `cargo-deny` was not run. Vendored `vt100` and
`crossterm-input` changes require separate source/notice review
([`P2-REPLACEMENT.md`](progress/P2-REPLACEMENT.md#whole-graph-notices-build-and-advisories),
[`P2-INPUT-REVIEW.md`](progress/P2-INPUT-REVIEW.md#complete-component-graph-licensing-and-build-inputs)).

**Status.** Partial (prototype).

**Gaps.** Complete organization review of every target/build/native path,
resolution of UNKNOWN license rows, disposition of inventory review flags,
ongoing audit/notice cadence, approved dependency manifest, and machine-readable
SBOM (for example CycloneDX) remain open. The advisory scan does not audit
vendored patches, prove reachability/safety, or replace firm criteria.

## SEC-02 — Source-to-release traceability

**Requirement.** Make releases traceable to reviewed source and build inputs.

**What Panelink does / evidence.** Phase 6 measurements record the observed
Apple Silicon host/OS, Rust 1.99.1, clean/incremental release builds and binary
sizes ([`P6-MEASUREMENTS.md`](progress/P6-MEASUREMENTS.md#buildresource-completed)).
The local-alpha release/install procedure and limits are documented at
[`docs/RELEASE.md`](RELEASE.md). The concurrent
[`P6-RELEASE-EVIDENCE.md`](progress/P6-RELEASE-EVIDENCE.md) exists, but its
command/output and manifest-value sections are blank placeholders; it is a
planned artifact, not completed build/install evidence. This pack points to
both without editing their contents.

**Status.** Partial (prototype).

**Gaps.** Firm-approved source revision/build inputs and distribution process,
reproducibility evidence, artifact hash traceability, signing/notarization where
required, SBOM attachment and minimum supported macOS version need explicit
review. A measured local build is not a controlled release.

## SEC-03 — Local, user-level core

**Requirement.** Document Unix socket endpoints, file paths/permissions,
executable launches and environment inheritance; propose no core internet
access, TCP listener, telemetry, self-update downloads or privileged helper.

**What Panelink does / evidence.** The current runtime endpoint is
`server/control.sock`, a Unix-domain stream socket with a `0700` parent and
`0600` socket mode. The state root defaults to the current user's Application
Support directory and rejects non-absolute `PANELINK_HOME` values
([`default_open.rs`](../src/default_open.rs#L85),
[`transport.rs`](../src/transport.rs#L31)). New children are launched by explicit
executable path, args, cwd and constructed environment; the PTY path calls
`env_clear()` and applies only that environment ([`terminal/mod.rs`](../src/terminal/mod.rs#L834),
[`launch.rs`](../src/launch.rs#L182),
[`managed_launch_prepare.rs`](../src/managed_launch_prepare.rs#L106)). The
server itself is spawned locally as `panelink serve`; terminfo generation uses
the local `/usr/bin/tic` utility ([`default_open.rs`](../src/default_open.rs#L434),
[`default_open.rs`](../src/default_open.rs#L246)).

Repository scans over `src Cargo.toml` for `TcpListener`, `TcpStream`, `reqwest`
or HTTP(S) URL literals returned no matches (ripgrep exit 1); scans for
telemetry, self/automatic update, update-download and privileged-helper markers
also returned no matches (exit 1). These are absence observations over the
searched runtime source/manifest, not a statement about user-launched children
or all dependency behavior. `scripts/probe_opencode_identity.py` separately uses
loopback HTTP only for its isolated local OpenCode feasibility probe, as
documented in its evidence; that probe is not part of the Panelink runtime core.
Exact commands/results are in
[P6 security-review evidence](progress/P6-SECURITY-REVIEW-EVIDENCE.md).

**Status.** Met for the documented prototype boundary.

**Gaps.** This does not constrain child software networking, establish a
firm-managed policy, or prove future release builds retain this boundary.

## SEC-04 — Agents and providers are separately governed

**Requirement.** Record executable/version/activation, provider/auth ownership,
child networking and organization approval; do not claim same-user process
isolation or agent network enforcement.

**What Panelink does / evidence.** Local kernel peer credentials identify the
effective UID; launch context and child environment are explicit and
provider-owned credentials remain provider-owned. The provider identity state
machine requires a correlated, acknowledged observation before calling a
binding verified ([`provider_binding.rs`](../src/provider_binding.rs#L355),
[`SPEC.md`](SPEC.md#durable-provider-session-identity)). The isolated OpenCode
1.19.0 probe demonstrated session list/export and SSE availability, but did not
prove selected TUI identity or an idle session switch. It ran with fresh
private HOME/XDG directories and no credentials/model calls
([`P4-OPENCODE-IDENTITY-PROBE.md`](progress/P4-OPENCODE-IDENTITY-PROBE.md#boundaries-and-observations)).
Explicit recovery relaunch starts fresh shell processes and reports provider
conversations are not resumed; selection is manual
([`P5-RECOVERY-RELAUNCH.md`](progress/P5-RECOVERY-RELAUNCH.md#behavior-implemented),
[`P5-RESTART-ACCEPTANCE.md`](progress/P5-RESTART-ACCEPTANCE.md#remaining-acceptance-gaps)).
Peer evidence includes `socket_pair_reports_kernel_credentials_without_consuming_data`
and `uid_mismatch_fails_closed_even_when_gid_matches`; the pathname integration
test `pathname_connection_reports_same_user_in_a_private_fixture` is ignored in
source because this sandbox returns EPERM on pathname bind.
The same-UID limitation is explicit in [`peer_identity.rs`](../src/peer_identity.rs#L1)
and [`SPEC.md`](SPEC.md#current-implementation-checkpoint--2026-10-18).

**Status.** Partial (prototype).

**Gaps.** A supported live provider adapter/version and activation contract,
correlated selected-session identity/switch proof, provider/auth ownership
review, child-network behavior and firm approval are not established. Panelink
does not isolate other same-user processes or enforce their network policy.

## SEC-05 — Saved history and clipboard behavior

**Requirement.** Make capture enable/disable, retention/clear, permissions,
copy-on-selection and diagnostic-content policy explicit; do not promise
secret-free storage or complete heuristic redaction.

**What Panelink does / evidence.** The store caps each ended target's normal
history at 1,024 rows/8 MiB, retains ended data for 30 days, stores separate
normal/alternate checkpoints and exposes target-scoped clear. Clear refuses a
live server and preserves provider conversation data
([`history_store.rs`](../src/history_store.rs#L17),
[`history_clear`](../src/default_open.rs#L971),
[`P5-SAVED-HISTORY.md`](progress/P5-SAVED-HISTORY.md#p5-bounded-saved-history-2026-10-20)).
Storage requires private `0700` directories and `0600` single-link files.
Diagnostics use fixed labels and redacted Debug implementations; launch
environment override values, prepared launch requests and saved history Debug
are covered by named tests (`debug_output_redacts_environment_values_and_override_values`,
`fixed_errors_and_debug_redact_request_data`, `validates_full_identity_order_unique_buffers_and_redacted_debug`).

**Status.** Partial (prototype).

**Gaps.** No user-facing history capture-off setting, copy-on-selection toggle,
or organization diagnostic-content policy is implemented. Persisted visible
terminal text can contain secrets; mode `0600` protects against other OS users,
not the current user or another same-user process. The UI clipboard selection
feature is not implemented, although programmatic child clipboard access is
denied (SEC-07).

## SEC-06 — Agent integration and local command authority

**Requirement.** Require opt-in bridge activation, correlated identity, bounded
messages, peer checks/socket access, no unsolicited global config edits and no
treating terminal output as commands.

**What Panelink does / evidence.** The current wire format has kind-specific
payload caps (65,536-byte control/event; 2,097,152-byte screen snapshot),
validates declared length before allocating a frame body, and poisons failed
connections ([`protocol_wire.rs`](../src/protocol_wire.rs#L11),
[`transport.rs`](../src/transport.rs#L226)). Kernel peer UID is checked before
any frame bytes are accepted ([`peer_identity.rs`](../src/peer_identity.rs#L36)).
The explicit command protocol uses typed operations and bounded payloads;
terminal output is screen/history data, never a dispatched command
([`control_protocol.rs`](../src/control_protocol.rs#L62),
[`P3-HANDSHAKE.md`](progress/P3-HANDSHAKE.md#delivered)).
The local host decoder caps ordinary escape sequences at 16,384 bytes and
bracketed-paste frames at 65,548 bytes before buffer admission, with paste
payload capped at 65,536 bytes ([`decoder.rs`](../vendor/crossterm-input/src/decoder.rs#L6),
[`parse.rs`](../vendor/crossterm-input/src/parse.rs#L5)).

**Status.** Partial (prototype).

**Gaps.** The live provider bridge/opt-in activation is not implemented, so
there is no proven selected-session correlation or provider plugin setup
contract. The user-facing agent read/control API remains a subset, and same-UID
peer checks do not grant isolation from other local processes. Firm policy on
global configuration and agent command scope remains unknown.

## SEC-07 — Terminal side-effect boundaries

**Requirement.** Deny programmatic clipboard reads/writes, including detached;
keep user copy/paste separate; verify terminal query handling, safe history
rendering, bounded control sequences and stale-client behavior.

**What Panelink does / evidence.** The selected VT100 callback surface has
OSC 52 copy/paste callbacks ([`callbacks.rs`](../vendor/vt100/src/callbacks.rs#L24)).
Panelink's terminal adapter implements clipboard-read denial without a reply;
the default no-op copy callback does not write to a host clipboard. The
control-sequence parser stores OSC in a fixed buffer and limits combining
scalars. Query replies are restricted to fixed bounded CSI responses, and
normal history stores normalized cells. Tests `clipboard_reads_writes_and_clears_never_queue_child_input`,
`clipboard_denial_does_not_inject_reply_bytes_before_the_next_shell_input`,
and `fixed_osc_storage_survives_chunked_oversize_and_combining_is_capped`
exercise these contracts ([`terminal/mod.rs`](../src/terminal/mod.rs#L698),
[`unit_tests.rs`](../src/terminal/unit_tests.rs#L74),
[`p2_pty.rs`](../tests/p2_pty.rs#L274)).

For uncertain client input, the client reconciles operation status rather than
resending input; the server test
`logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay` checks a
lost input acknowledgement against the same PTY/process ([`p3_detach.rs`](../tests/p3_detach.rs#L156),
[`conn.rs`](../src/ui_client/conn.rs#L270)).

**Status.** Partial (prototype).

**Gaps.** The separate user-authorized system clipboard Copy/Paste feature and
copy-on-selection policy are absent. Existing synthetic tests do not prove
every real host, detached/reconnect timing, every OSC dialect, or all stale
client races. These controls do not imply a general terminal compatibility
guarantee.

## SEC-08 — Vulnerability and update process

**Requirement.** Name owner/contact, supported versions, assessment/remediation
process, approved update distribution and organization-specific exceptions.

**What Panelink does / evidence.** The Phase 6 dependency inventory preserves a
dated RustSec scan and points out its scope/limitations
([`P6-DEPENDENCY-INVENTORY.md`](progress/P6-DEPENDENCY-INVENTORY.md#advisory-scan)).
[`docs/RELEASE.md`](RELEASE.md#support-limits-and-vulnerabilityupdate-process-sec-08-stub)
records a local unsigned release path, latest-tag support target, repository
owner/reporting contact, and a proposed pre-tag assessment/audit step. Known
advisory results are distinguished from dependency trust or reachability; no
automatic updater is present in searched application source/manifest.

**Status.** Partial (prototype).

**Gaps.** Supported versions are limited to the latest repository tag only;
there is no older-version support promise, remediation SLA, signed/authenticated
update distribution, or firm-specific exception process. The release document's
owner/process is a local-alpha stub that needs reviewer confirmation. A clean
scan is only a dated known-advisory observation.

## Release and dependency traceability

- Dependency graph, license/source flags and audit result:
  [`P6-DEPENDENCY-INVENTORY.md`](progress/P6-DEPENDENCY-INVENTORY.md).
- Build/release traceability:
  [`docs/RELEASE.md`](RELEASE.md) and
  [`P6-RELEASE-EVIDENCE.md`](progress/P6-RELEASE-EVIDENCE.md), whose execution
  fields remain blank placeholders in this review pass.
- Product requirements and remaining release/security review gates:
  [`SPEC.md`](SPEC.md#proposed-security-review-requirements),
  [`ROADMAP.md`](../ROADMAP.md).

These are pointers to evidence, not an assertion that an organization has
accepted the evidence or approved a release.

## Open items for the reviewing organization

1. Provide firm-specific security/dependency criteria and an approved
   exception/remediation path; they are not present in this repository.
2. Decide release signing/notarization and distribution requirements; no
   signing/notarization evidence is established here.
3. Select and require an SBOM format (for example CycloneDX); no such SBOM is
   currently generated in this pack.
4. Review/accept the dated cargo-audit scope and resolve dependency inventory
   flags, including UNKNOWN platform-only license metadata and local vendored
   patches.
5. Review the following source unsafe-block inventory. It is a scoped source
   count, not a safety audit; each listed block has a nearby `SAFETY:` or
   `Safety:` rationale in source. Vendor source contains no `unsafe { ... }`
   blocks according to the same scan.
6. Decide whether same-user trust and unrestricted child-process networking
   meet the intended workplace deployment boundary.
7. Define capture-off, clipboard UI/policy, provider integration and
   signed/approved update expectations before treating the corresponding
   partial/not-met items as release controls.

### Unsafe blocks counted in `src/` (19)

Count definition: 19 `unsafe { ... }` expressions matching the repository scan
in application `src/`; vendored source is not included in this count.

| File:line | Nearby safety comment (abridged source text) |
| --- | --- |
| `src/default_open.rs:192` | `getuid` has no preconditions and cannot fail. |
| `src/default_open.rs:355` | `fd` is a valid open descriptor owned by `file`; successful probe lock is released. |
| `src/default_open.rs:358` | `as above` (the same live owned descriptor). |
| `src/default_open.rs:389` | `fd` is a valid descriptor owned by `file`. |
| `src/default_open.rs:461` | `setsid` is async-signal-safe and detaches the child from this terminal's session. |
| `src/default_open.rs:484` | `proc_bsdinfo` is plain data; zeroed is a valid initial value. |
| `src/default_open.rs:487` | Buffer pointer/size describe `info` exactly. |
| `src/default_open.rs:822` | `kill` uses a verified PID (same owner and kernel start time). |
| `src/p3_cli.rs:1595` | `TIOCGWINSZ` writes one winsize into the valid pointer supplied. |
| `src/peer_identity.rs:56` | Borrowed fd remains live; output pointers are distinct initialized ABI `uid_t`/`gid_t` values. |
| `src/server_io.rs:888` | Descriptor is a live Unix stream and size/pointers are checked for the socket call. |
| `src/terminal/mod.rs:881` | Child hook calls only reviewed session/controlling-terminal wrappers; no allocation, locking, logging or environment access. |
| `src/terminal/mod.rs:1513` | libc queries inspect this process and fd 0; pointers refer to initialized local values. |
| `src/terminal/pty.rs:22` | `posix_openpt` receives integer flags; fresh descriptor ownership is transferred once. |
| `src/terminal/pty.rs:29` | `posix_openpt` returned a fresh valid descriptor not wrapped/closed elsewhere. |
| `src/terminal/pty.rs:51` | `controller` is the live PTY master owned by `open_pty`; `grantpt` does not transfer ownership. |
| `src/terminal/pty.rs:60` | `controller` is the live PTY master owned by `open_pty`; `unlockpt` does not transfer ownership. |
| `src/terminal/pty.rs:70` | Private `TIOCPTYGNAME` ioctl writes within its documented 128-byte buffer. |
| `src/transport.rs:1544` | Descriptor is a live Unix stream; value and length refer to the valid local peer-credential structure. |

The exact source scan command/output, including the count and all 19 source
locations/comments, is preserved in
[P6-SECURITY-REVIEW-EVIDENCE.md](progress/P6-SECURITY-REVIEW-EVIDENCE.md).
