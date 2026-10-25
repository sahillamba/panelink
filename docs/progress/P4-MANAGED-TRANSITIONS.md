# Phase 4 managed transition fixtures

This slice supplies disposable session choices for the production managed
picker acceptance probe. `scripts/p4_managed_transition_fixtures.py` creates
three distinct choices through the pinned OpenCode loopback fixture API:

- a newly API-created root in the granted project;
- a top-level fork created from an imported, nonempty static synthetic-history
  transcript, with verified import round trip, parent/boundary link, remapped
  child message IDs, and matching history hash;
- a root in a separate private project, for visible refusal/manual fallback
  testing against the managed session's fixed project scope.

The helper delegates the schema-sensitive import/fork construction to
`probe_panelink_manual_recovery._forked_root_choice` and verifies root
project/top-level identity using the pinned identity probe's lookup helpers.
It requires the primary project and the newly created foreign project to stay
under the disposable fixture root, checking canonical parents before any
directory creation or session API writes.
It does not open a menu or issue navigation, so the owned picker probe must
still perform guarded exact-title selection and establish the provider source
revision, atomic managed binding revision, selected-session hash, and project
hash together. Fixture IDs and canonical paths stay in memory; the only
intended report surface is `safe_evidence`, which emits fixed booleans and
SHA-256 values. The fixture roots must have no user messages, and no prompt or
model request is made.

Run the focused checks with:

```sh
python3 -m unittest tests.test_p4_managed_transition_fixtures -v
python3 -m py_compile scripts/p4_managed_transition_fixtures.py
```

## Evidence status

The helper and its unit tests establish fixture-construction contracts only.
No managed UI transition has been run or accepted by this slice. Standalone
OpenCode identity fixture evidence does not substitute for the production
managed bridge gate, and this note does not change Phase 4 completion status.
