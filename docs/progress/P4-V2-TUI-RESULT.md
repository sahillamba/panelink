# Phase 4 V2 selected-route gate — 2026-10-24

Later [opposite-client and observer-health slices](P4-V2-EXTENDED-RESULT.md)
also pass3/3 each. This document retains the original baseline evidence.


Status: **scoped selected-route/two-client gate passes, 3/3 fresh runs.**
Two concurrent private OpenCode TUIs in the same project independently expose
their selected top-level identity. Keyboard selection switches the first A-B-A
while the second keeps fresh B observations. This is development-fixture
evidence, not a live Panelink bridge, durable binding or recovery feature.
Full Phase 4 remains open.

## Pin, scope and isolation

Native launcher: `/Users/sahillamba/.opencode/bin/opencode`, resolved on PATH.
Version: **2.0.32**. SHA-256:
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
The version/help probe independently confirmed `--server` and `--session`.
The old 1.19.0 guard remains unchanged.

Each run uses a fresh mode 0700 HOME/XDG/project root, two empty API-created
top-level sessions, one owned loopback server and two private 24x100 PTYs.
The existing authenticated bootstrap checks exact listener PID/version before
mutation. Fixture-only random credentials stay out of reports. Sandbox-exec
denies networking except loopback; no user service, provider credentials,
Panelink workspace, installed settings or live panes are changed.

The original fixture plugin polls the selected route at 100ms, hashes the
canonical location directory and selected session ID, checks the selected
cached row's top-level status, and records a per-client nonce and per-plugin
instance. Missing cached data during startup remains unknown. Child sessions
are never converted to their parent identity. Node-based behavioral tests
cover unrelated child activity, selected child routes, missing cache data,
caps and disposal; no live child/fork acceptance is inferred from those mocks.

The driver enforces bounded records/output/time, contiguous sequence numbers,
finite ordered timestamps, correct client/project, no unexpected post-initial
routes, and a maximum 1s observation gap after identity becomes ready. Warm-up
records may have unknown project/cache data but cannot satisfy identity proof.
Both PTYs drain throughout waits. Passive B must advance after each observed
active transition. Missing controls, observations, cleanup receipts or passive
freshness prevent a passing result.

Keyboard controls are Ctrl-X then `l`, a fixture-title filter, and Enter only
after the picker guard accepts the target. Route observations, not title text
or launch arguments, establish the resulting session identity. After the TUIs
close, read-only user-message queries independently verify both sessions remain
empty. Reports contain only fixed labels, booleans, counts and timing values;
raw terminal output, IDs, project paths, message bodies and secrets are omitted.

## Final frozen repetitions

Receipt: `/private/tmp/panelink-p4-20261024/v2-tui-final-repetitions.json`.

| Run | Wall time ms | A-B-A latency upper bounds ms | Passive B fresh | User messages absent | Both cleanup receipts |
| --- | ---: | --- | --- | --- | --- |
| 1 | 8176.526 | 101,101 | pass | pass | pass |
| 2 | 7656.868 | 102,100 | pass | pass | pass |
| 3 | 7481.101 | 101,101 | pass | pass | pass |

Every run exits0 with `selected_tui_identity_observed`, both initial routes,
two issued selections, zero misses/extras/rejected records, and HTTP200 for
both empty-user-message reads. The passive client remains B while the active
client returns to A. These are two TUI clients with empty conversations, not
two agents performing model work or a representative performance workload.

Latency uses the most recent preselection timestamp from the plugin's own
clock. It is a conservative polling upper bound, not the exact keystroke time.
Polling provides **no ordering barrier with durable storage** and no guarantee
of current-at-crash identity. No such guarantee is enabled.

## Failed diagnostics and parent corrections

Keep these receipts as failed observations; they are not acceptance passes.
All executed attempts completed fixture cleanup.

| Receipt suffix under `/private/tmp/panelink-p4-20261024/` | Observation |
| --- | --- |
| `v2-tui-diagnostic.json` | 19,021.553ms; no plugin records, no controls |
| `v2-tui-startup-markers.json` | 19,007.057ms; same failure with fixed boolean diagnostics |
| `v2-tui-cli-config-fixed.json` | 18,866.190ms; incorrect singular legacy config hypothesis; no records/controls |
| `v2-tui-package-entry.json` | 18,909.259ms; both plugins loaded and produced 133 rows each, but warm-up rejection prevented initial-route acceptance |
| `v2-tui-warmup-fixed.json` | 9680.343ms; both initial identities verified, picker filter guard withheld Enter |
| `v2-tui-filter-diagnostic.json` | 9984.790ms; both labels visible to the global screen guard; no partial-display/line-erase explanation and no selections issued |
| `v2-tui-picker-scope.json` | First scoped pass, 8131.301ms; A-B-A 113/101ms and passive B; superseded by final runs with empty-message verification |

The first worker audit mistakenly treated a direct file as a supported plugin
target. A later audit confused the legacy singular `plugin` migration schema
with the current CLI schema. Parent tracing of the exact bundle established:

- Current `H0e` schema contains plural `plugins` around byte 113412619; current
  CLI config spreads those fields around 113451567. Singular `plugin` belongs
  to old `tui.json` migration, not current `cli.json`.
- CLI reconciliation explicitly skips direct file targets around 116607000-
  116609000. Its host resolver at113680570 looks for a `tui` entry in a directory.
- The final harness writes private `identity-plugin/tui.ts` and points
  `cli.json`'s `plugins` at that directory. No package installation is needed.
- The parser initially rejected an unknown-project startup row, then treated
  every later sequence as a gap. The fix preserves valid warm-up ordering
  while withholding identity proof until the exact project/top-level row exists.
- The global filter guard counted the underlying current-session header as a
  competing picker row. The V2 guard now scopes text from the modal's Sessions
  title onward; competing rows in that region still prevent selection.
- The top-level switch flag now reflects the nested observed result. Server
  or TUI cleanup failure downgrades the result rather than leaving a pass label.

One planned warm-up rerun was not executed because automatic approval review
temporarily hit a usage limit. After its stated retry time, normal approval
review permitted execution. That review failure is not host/provider evidence.

## Verification and remaining work

- Parent final Python discovery: **121 passed**, no failures/skips, in 6.962s.
  Focused bootstrap/TUI/plugin checks: 39 passed. New mocks cover the full
  two-client driver, passive drift, failed cleanup, schema/nonce/project/gap
  rejection, warm-up and picker-background cases, and empty-message reads.
- A restricted-sandbox full run earlier had118 passes/1 failure because the
  old probe's loopback-listener fixture could not bind. The permitted final
  run above passes; the restricted failure is not counted as a pass.
- Final Python compilation and tracked whitespace checks pass; eight new/
  affected files have zero whitespace findings. Documentation: 285 Markdown
  files/0 findings. The in-memory code graph was refreshed without an artifact. No Rust/build inputs
  changed; the separate activated Phase 3 baseline remains 1111/0/15 ignored.

Next: extend this pinned fixture to new/forked conversations, child-session
isolation and bridge loss/staleness, including opposite-client switching.
Then implement the versioned live bridge and durable binding store/acknowledgment
against the existing pure core/codec. Preserve explicit last-confirmed/manual
fallback and test crashes before/after acknowledgment. The current two-client
gate does not prove those cases or complete Phase 4. No commit or push.
