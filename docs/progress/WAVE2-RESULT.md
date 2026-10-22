# Wave 2 integration — 2026-10-22

Status: scoped source reviewed, integrated and verified under the limits below.
Broad phase, host and provider gates remain open. All three Luna owners have
released their files; no live agent is required to resume.

## Delivered behavior

- Active attach socket/context work runs on one bounded IO worker. The host
  owns UI state, rendering and local controls. Thirty-two outstanding commands/
  results share a4MiB payload/reservation budget including in-flight work;
  transport/decode buffers and screen caches are additional bounded memory.
  Unadmitted keys stay pinned under overload, while CtrlX always detaches and
  reports any discarded unsent batch after restoration. Split focus waits for
  validated layout membership and respects later user navigation.
- Live pane close is a server-owned transaction advanced across ticks through
  HUP, exit polling and KILL escalation. Sibling PTY work continues; loss of
  the reply worker does not cancel close. Input/resize stop immediately for
  the closing target, Unknown retains ownership, and only terminal results use
  schema9. Driver teardown signals children before shared grace waits.
- The pure saved-layout model/codec validates exact160-byte historical records.
  It has no filesystem, recovery, live dispatch or runtime integration.

No dependencies, vendor sources, manifests or lockfiles changed. Existing
Wave1/planning changes were preserved. No commit/push or broad phase completion.

## Parent review corrections

Parent found and corrected undersized split leaves accepted by snapshot
conversion; a default phased-stop hook calling synchronous cleanup; a pending
waiter error without finite disposal; synchronous error cleanup during phased
stop; direct resize bypassing the closing guard; legacy/phased close overlap;
queue saturation preventing CtrlX; and split focus preceding target membership.
Focused regressions accompany these fixes. Parent also strengthened shutdown
ordering checks for either child iteration order and exact ESRCH absence.

Strict Clippy initially found a public unit error, an eighth dispatch argument
and a Copy-type clone. Parent used the existing sanitized stop outcome, grouped
pending-capacity facts and removed the clone; no lint suppression was added.
The initial failed verifier log is retained alongside later evidence.

## Verification

Parent independently passed all50 client tests and eight codec component tests.
The selected integration command passed26 tests after the last input guard:

```text
cargo test --locked --offline --test saved_layout_contract --test w2_server_close --test w2_server_shutdown --test usability_input_pty --test p2_pty --test p3a_runtime --test p3b_close --test p3_split --test p3_multi_owner --test p5_live_history_driver -- --test-threads=1
```

This includes real sibling input/output during logical HUP close, shared
two-child shutdown, legacy close/split, both saved-history driver regressions,
typed wire-to-PTY input, exact wire fixture/canonicality and core PTY tests.
Library coverage includes the real socketpair dropped-waiter close regression
and phased-stop PTY-error regression. The strengthened shutdown test passed in
1.09s independently and1.08s in the selected final run; these are scoped
observations, not an adopted performance budget.

The final combined verifier after the last input guard was7/8: library569passed/
4ignored; binary109passed/1failed at the known pathname bind EPERM; decoder39
and Python45passed; strict all-target Clippy, project formatting, build,
component fingerprints and docs191/0 passed. Three focused PTY adapter tests
also passed after the last guard. The root Cargo test stops at that binary
failure, so it does not run all external integration targets; the26 selected
tests above were run independently and passed. The4 ignored library cases are
not passes.

Logs are under `wave2-evidence/`. The verifier uses the already documented
writable temporary CARGO_HOME with a symlink to the populated read-only
registry; no credentials/config were copied.
The final combined log is `verifier-final-after-input-guard.log`; selected
integration evidence is `integration-final.log`. The initial `verifier.log`
retains the corrected Clippy failures rather than replacing failed evidence.

## Remaining boundaries and next work

Initial connect still runs before raw mode. One client worker serializes both
panes, so sibling IO can wait behind a slow exchange. Filesystem sync has no
cancellation deadline; late worker ownership persists until it exits. Tests
prove worker isolation, socketpair stall handling and restore-before-detach
ordering; no end-to-end delayed-fsync injection or full host-terminal PTY
acceptance is claimed. PaneConn's durable-before-send/nonreplay ordering is
unchanged and was source-reviewed.

Launch preparation/journal sync ran synchronously on the owner at this
checkpoint; the live split path subsequently adopted the serialized worker
in [ASYNC-LAUNCH-RESULT.md](ASYNC-LAUNCH-RESULT.md). Legacy
synchronous cleanup outside phased close remains separate. The saved-layout
store/rotation, guarded writer, lifecycle observation and read-only inspection
were pending at this checkpoint; the incoming implementation subsequently
delivered them (see [P5-LAYOUT-SLICE.md](P5-LAYOUT-SLICE.md)). No tabs/sidebar choice, Subagents
observer, provider identity/recovery or machine-reboot acceptance was added.
Pathname/host/provider restrictions remain gates, not passes or manual tasks
assigned to the user.
