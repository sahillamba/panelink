# P4 native manual-recovery acceptance

Status: an acceptance probe is prepared for the public explicit recovery CLI
against the pinned OpenCode 2.0.32 client. It reuses the API bootstrap and
managed `serve` observer, with a fresh temporary HOME and loopback-only
sandbox. The probe starts no provider model turn, creates only empty fixture
sessions, injects no terminal input, and does not attach a Panelink UI.

The primary managed target begins on an API-created root session A. The probe
tries a verified-missing ID, an API-created then deleted ID, a top-level
session in another project, and a linked child session. Each must return its
fixed manual-selection error while the source's last-confirmed record remains
at revision 1. It then prepares exact root B and checks that prepare publishes
a `Prepared` ticket without changing the source mapping. The separate resume
command is the only action that may navigate; success requires the CLI's
`selected_and_saved` receipt and a later fresh selected snapshot at the
prepared revision. Replaying the same ticket must be rejected without
advancing the binding revision. After shutdown, historical inspection must
show the exact B hash, the supported version, and `managed_launch_linked`.

A second managed target starts on a separate API-created root and prepares and
resumes a top-level fork with a nonempty imported history. The fixture creates
one inert `Session.Message.Synthetic` record using the pinned export schema,
exports the session, deletes it, imports the exact transcript, and forks it.
It checks import roundtrip, the distinct child ID, fork parent and boundary,
top-level project listing, and the fork-history digest before passing the fork
root through managed validation, prepare, explicit resume, and selected
binding save. This proves the native SDK accepts a fork-origin root while the
linked child is still refused.

The emitted report is allowlisted to fixed statuses, booleans, revision
numbers, and SHA-256 hashes. Raw session IDs, operation tickets, API bodies,
paths, and subprocess diagnostics remain in memory. User-message absence is
verified through read-only API queries for every surviving fixture session;
the deleted fixture is checked before deletion. Cleanup terminates and reaps
both managed children and the bootstrap server.

## Verification

Run the focused helper tests with:

```sh
python3 -m unittest tests.test_panelink_manual_recovery_probe -v
```

Run the live acceptance with the isolated rebuilt binary using:

```sh
python3 scripts/probe_panelink_manual_recovery.py \
  --panelink-binary /private/tmp/panelink-p4-bridge-store-target/debug/panelink
```

The live result is recorded only after execution. A missing, foreign-project,
child, unsupported-version, or uncertain lookup is a manual-required outcome;
uncertain results never count as missing-session passes. Passing this fixture
does not close Phase 4 on its own.

### Current native result (2026-10-24)

The focused companion and native-probe helper tests passed **4/4**. The native
fixture ran under the enforced loopback-only macOS sandbox with pinned OpenCode
**2.0.32** and SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`. The
embedded Panelink binary used for this run had SHA-256
`1497ac732020573d633df85d38fdd594a92b31d7f1b8ee29d06e0087e52ec9ee`.

The complete native fixture classified an absent ID and a deleted ID as
`provider_session_not_found`, refused the foreign-project session and linked
child with their fixed outcomes, and preserved the last-confirmed source across
invalid choices and preparation. The separate explicit resume returned
`selected_and_saved`; a later fresh observer snapshot confirmed that revision;
replaying the ticket was rejected without advancing it. Historical inspection
after shutdown confirmed the exact selected hash, provider version, and
managed-launch link. On the second managed target, the imported nonempty
fork-origin root passed SDK validation and the same prepare/resume/save flow.
The fixture verified its transcript digest, parent link, fork boundary, and
top-level classification before using it. No user messages, model prompts, or
provider model calls were made, and all private fixture processes were reaped.

This is native manual-recovery evidence for the pinned client, not Phase 4
closure. The evidence does not establish behavior for other OpenCode versions,
other clients, or cross-project recovery; a project mismatch remains an
explicit manual-required result.
