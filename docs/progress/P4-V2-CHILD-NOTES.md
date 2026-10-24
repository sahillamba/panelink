# Phase 4 OpenCode v2 child-route notes — 2026-10-24

Status: **parent live verification passes 3/3 fresh runs**; see
[P4-V2-ISOLATION-RESULT.md](P4-V2-ISOLATION-RESULT.md). The helper creates one fixture-only
linked child without a prompt, observes it selected in a third private TUI,
checks that the child stays outside the top-level session listing, and returns
that TUI to its original top-level parent. It also checks that both original
observers continue to report their hook-entry identities and that parent and
child user-message reads are empty. No provider prompt is sent.

## Pinned provider behavior

The helper is tied to the native OpenCode **2.0.32** binary with SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`.
Read-only inspection of that installed bundle showed the native
`POST /api/session/:sessionID/fork` operation. Its embedded description says it
copies projected history before a message and copies the full history when the
`before` value is omitted. The handler maps `Session.ForkEmptyError` to the
`empty_session` API error. Since the owned fixture starts with empty sessions,
the probe makes one bounded fork request and records only its HTTP status; it
does not claim to have observed the error kind from the response.

The same bundle documents `POST /api/session` with `parentID` as creating a
linked child at its parent's location. For the empty fixture, the helper uses
that no-prompt route after a rejected fork request. Its report calls this a
`parent_id` child; it does not call it a copied-history fork. It verifies the
child's exact `parentID` and `location.directory`, then reads a bounded
top-level listing and requires exactly the two original fixture sessions.

## Boundaries and remaining evidence

The callback requires the pinned version and binary hash, exact owned listener,
private project and fixture roots, two known starting identities, and the
existing two-client route gate. It uses bounded loopback requests, a 10-second
case deadline, PTY output caps, hash-only route records, allowlisted result
fields, and unconditional cleanup of the additional TUI. Every authenticated
request, including the final empty-message reads, rechecks that the exact server
process still owns the listener.

The focused tests exercise strict project/parent parsing, top-level child
exclusion, foreign and unknown selected routes, Home during a child return,
listener-loss refusal, late empty-read and cleanup downgrades, fail-closed hook
scope, and a fully mocked child creation/selection/return lifecycle. They do not prove live
OpenCode TUI behavior. The parent separately verified live HTTP status, selected child route, parent
return, passive observations, empty-message reads and cleanup in the linked result. The result says
`not_proven` unless all checks pass.

Focused verification: `python3 -m unittest discover -s tests -p
'test_opencode_v2_child_routes.py'` — **8 tests passed, 0 failed** after parent integration.
