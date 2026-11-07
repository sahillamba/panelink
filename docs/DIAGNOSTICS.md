# Local diagnostics and property tests

Tracing is off by default. On macOS Apple Silicon, set `PANELINK_TRACE_DIR` to
an absolute private directory to enable structured local diagnostics:

```sh
mkdir -m 700 /private/tmp/panelink-trace
PANELINK_TRACE_DIR=/private/tmp/panelink-trace panelink
```

Use the selected executable's `build-info --json` and `server-info --json` to
identify client and server separately. A newly launched client does not enable
tracing in an already running server. Enable the variable when starting a new
server too. Do not stop a workspace just to enable tracing without authority
to end its processes. Unset the variable for subsequent launches to disable;
already running processes keep their startup configuration.

The directory must be owned by the effective user with mode `0700`; its
canonical ancestors must be root/current-user owned and not writable by others
(root-owned sticky temporary directories are allowed). Current log files must
be regular, single-link, user-owned `0600` files. Symlinked/unsafe paths and
initialization failures silently disable diagnostics without failing the command.
On macOS, mode bits are not the only access control: private directories and
files reject non-owner ACL allow entries, including inherited entries, and
ancestors reject non-owner ACL grants that permit mutation. ACL inspection
errors also disable diagnostics. If initialization silently disables tracing,
inspect the selected path and ACLs read-only (for example,
`ls -lde /path/to/trace`); do not automatically strip or rewrite ACLs. Choose a suitable
private directory only when its ownership and ACL policy are understood.
One writer per role per directory holds a nonblocking exclusive `client.lock`
or `server.lock`. Other simultaneous clients/servers using that same role and
directory do not trace. Use separate directories for separate traced sessions.

Files are `client.log` or `server.log`, with two retained generations (`.1`,
`.2`). Each is capped at 4 MiB during runtime; records are capped at 4 KiB and
the lossy queue holds at most 128 records. Oversized existing files disable
startup. Overflow drops records and counts losses; shutdown summaries report
dropped/truncated records and sink errors. Abrupt process death may lose queued
records and the summary. Diagnostics never write to the terminal or CLI JSON.
Shutdown waits at most 150 ms per call, after terminal restoration; a blocked
writer may be abandoned at process exit. Disk failures affect diagnostics only.
These files are separate from the existing launcher `server.log`.

Recorded data is restricted to fixed operation/stage/outcome labels, numeric
identifiers, lengths/counts and monotonic durations. It excludes input/paste and
screen contents, arguments, paths, environment values, credentials and provider
conversation IDs. Do not add raw Debug/error formatting or export traces
automatically. Same-user processes retain authority over these local files.

The pilot covers attach negotiation, client queue/service/result application,
wire round trips, server queue/dispatch and PTY acceptance. Wire events join on
generation, lease and connection-local request ID; do not join on request ID
alone. Nested span context carries causal relationships within a process.
Owner input-stage records without a wire ID report zero, never an operation
sequence disguised as a wire request ID. Host-to-wire attribution and deferred
launch/watch/close completion tracing remain limited; timings are stage durations,
not proof of child/provider acceptance or visible display latency. Routine
screen/layout queries are omitted. Native host/provider acceptance and measured
performance tolerances remain separate gates.

## Property tests

`proptest` is a development dependency. Root tests exercise pure codecs,
layout operations and the existing input decoder without changing vendored
sources. Run the bounded pilot:

```sh
cargo test --locked --offline --test property_protocol --test property_layout --test property_input -- --test-threads=1
PROPTEST_CASES=4096 cargo test --locked --offline --test property_layout -- --test-threads=1
```

Default: 256 cases/property, 4,096 shrink iterations. Process timeouts are
provided by the normal verifier, not Proptest's disabled fork/timeout features.
Failures persist seeds under `proptest-regressions/property_*.txt`; retain
reviewed synthetic seeds in Git and rerun the same target to replay. Important
minimized failures also deserve named regression tests because changing a
strategy may change seed interpretation. Seeds never come from live pane data.
Property tests supplement deterministic, race, PTY and native acceptance tests.

See the [adoption plan](design/TRACING-PROPTEST-PLAN.md),
[dependency review](progress/TRACING-PROPTEST-DEPENDENCIES.md) and
[implementation evidence](progress/TRACING-PROPTEST-RESULT.md).
