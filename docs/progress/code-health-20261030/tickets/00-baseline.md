# 00: Refresh the integration baseline

**Status:** complete; parent-reviewed baseline and fixture repair

**Lane:** Parent

**Blocked by:** None

**What to build:** Identify the real repository, active launcher/build target and dirty work. Preserve all active work; do not reset or stash it implicitly.

Read the [specification](../SPEC.md) and [execution reference](../EXECUTION.md), then use the source ownership row for this ticket. Treat its file lease as exclusive.

## Acceptance criteria

- [x] Identify the real repository, active launcher/build target and dirty work. Preserve all active work; do not reset or stash it implicitly.
- [x] Select a current integration base containing the scoped-wheel and compact-modal changes. The recorded origin SHA is a reference, not a forced checkout. Read current handoff/spec/skill and inspect changes since this bundle.
- [x] Measure large files, distinguishing production from complete test-only modules; record a current behavior/test map. Confirm whether newer source already resolves a proposed extraction.
- [x] Run the canonical non-UI verifier in an isolated target and retain its log. A baseline failure is recorded and diagnosed before structural work starts.
- [x] Publish exclusive file leases and integration order to the chosen agents. Finish when the base, baseline result, refreshed source pointers and every lease are explicit.

## Completion

Return the execution reference handback. Stop when these criteria are met; leave additional refactors for a new ticket.
