# Bounded pane operations integration — 2026-10-26

Status: bounded pane operations pass automated integration and supporting
checks. Phase 3 and Phase 5 remain open for separate product acceptance gates.

The existing four-pane layout now supports exact-target location swaps,
absolute ancestor-divider adjustment, and server-authoritative zoom/unzoom.
Targets, processes and input ownership remain stable. The additive negotiated
schema13 view retains every underlying pane and explicit confirmed PTY sizes;
the attached client projects only the visible zoom target. Runtime ratios and
zoom do not establish reboot restoration. Existing saved observation codecs
and legacy wire records remain unchanged.

Prototype client controls are Ctrl-B m/z/e. Divider arrows preview locally and
Enter sends one absolute commit; Escape cancels. Pending operations retain
their captured targets, revision, ID and payload. Uncertain results never
trigger a replacement-key replay. Split and close require unzooming first.

## Parent verification

All Cargo commands use the existing isolated home and target under
`/private/tmp`, locked/offline, with serial test threads. The installed runtime
has not been activated or replaced.

| Scope | Actual result | Log under /private/tmp |
| --- | --- | --- |
| Full library before final client integration | 871 passed, 0 failed, 4 ignored | panelink-p5-pane-lib-r1.log |
| Complete owner runtime helpers | 12 passed, 0 failed | panelink-p5-pane-owner-all-r3.log |
| Complete client including exit-refresh socket regression | 123 passed, 0 failed | panelink-p5-pane-client-refresh-r2.log |
| Actual four-PTY protocol continuity | 1 passed, 0 failed | panelink-p5-pane-pty-r7.log |
| Outer terminal tabs, copy and pane controls | 3 passed, 0 failed | panelink-p5-pane-ui-pty-r2.log |
| Python suite | 232 run, 231 passed, 1 opt-in skip | panelink-p5-pane-python-final.log |
| Component source fingerprints | match | panelink-p5-pane-fingerprints-final.log |
| Root formatting | passed | panelink-p5-pane-fmt-final.log |

Full serial Rust verification passes **1399/0/15ignored across75 groups**:
`panelink-p5-pane-full-r1.log`. Group totals take the last summary from each
Cargo Running/Doc-tests group, avoiding nested probe double-counting.

The first strict Clippy run found question-mark/identical-branch lints,
copy-clones, a redundant numeric conversion and large worker result variants.
Parent simplified those expressions and boxed poll/operation result payloads;
the bounded queue byte reservations remain in force. Strict all-target Clippy
passes at `panelink-p5-pane-clippy-r2.log`; initial findings remain at r1.
After this cleanup, final client123/0 and outer-PTY3/0 reruns pass at
`panelink-p5-pane-client-final.log` and `panelink-p5-pane-ui-pty-final.log`.
No additional full-suite count is inferred from the focused reruns.

All seven supporting checks pass: decoder39/0, component fingerprints,
rootfmt, strict Clippy, isolated build, Python232run/1skip and final docs.
Logs `panelink-p5-pane-{decoder,fingerprints,fmt,build,python,docs}-final.log`
plus `panelink-p5-pane-clippy-r2.log`. The active binary is unchanged.
No phase completion commit or push was made.

## Review and corrections

Independent owner/runtime review found no material source issue. Independent
client review found that requiring the querying occupant in a workspace view
could permanently stall refresh after that pane exited. Validation now checks
generation/workspace scope. A parent socket-pair test exercises the complete
refresh path through the exited connection, removes it, then refreshes through
the survivor. Cross-scope views remain rejected.

Parent corrected compiler integration, last-pane zoom exit cleanup, actual
versus effective pending resize tracking, and wrong-sized snapshot rendering
during divider preview or pending zoom. New deterministic regressions cover
these boundaries. The earlier helper failures remain in owner r1/r2 logs.

The actual4PTY fixture required corrections for marker wrapping, bounded key
batches and resuming idle connections on their original leases. The outer UI
fixture required pane-local parsed-cell markers, asynchronous focus readiness,
padded content dimensions and axis-correct divider keys. Earlier failed runs
remain in their respective evidence notes; none were counted as passes.

## Evidence limits

The four-PTY fixture proves original shell PIDs/state, actual stty dimensions,
hidden output during zoom, root resize and unzoom, duplicate/conflict behavior,
and unnegotiated denial through authenticated runtime transport. The outer PTY
fixture proves the actual attached keyboard controls, rendered geometry and
terminal restoration. Neither establishes human Apple Terminal behavior,
real clipboard acceptance, physical reboot/power-loss durability, or
representative workload budgets. Those gates remain separate and open.

No dependencies, installed runtime activation, real clipboard operation,
physical reboot, commit or push occurred in this slice.
