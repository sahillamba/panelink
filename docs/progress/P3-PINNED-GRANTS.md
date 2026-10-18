# Prototype target-pinned grants

## Scope

The private `LeaseGrant` now stores the complete immutable `TargetRef` selected
when a lease is issued. Resuming a lease against any different target identity
returns the existing fixed `UnknownContext` error before lease renewal, and
renewal retains the original target. A fresh explicit lease is needed to
represent a different target.

Control and owner mutation paths preserve the existing order: live connection,
key and capability checks, global exact-key admission lookup, full target
staleness check, target-scope check, then validation/admission/effect. Exact
cached results and changed-byte conflicts therefore keep their global
precedence. Denied target scope returns `Unauthorized` without admitting the
new key. Describe checks the selected session scope before creating its info
reply; reconcile and event reads check it before metadata synchronization or
sink access. Handoff verifies that the recipient's active input grant has the
same full target identity.

## Evidence

- `cargo test --locked --offline server::tests::`: 32 passed, 0 failed.
- `cargo test --locked --offline --test p3_detach`: 3 passed, 0 failed.
- `cargo test --locked --offline --test p3_managed_launch`: 2 passed, 0 failed.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --offline --no-deps`: passed.
- `python3 scripts/check_docs.py`: 121 Markdown files checked, 0 findings.
- Added private tests exercise resume and Describe/watch scope for all five
  `TargetRef` fields, control/owner denial for the four fields representable
  with the operation generation, no admission/effect or grant renewal on
  denial, global exact-result and conflict precedence, and cross-target
  handoff denial.
- The operation protocol binds target generation to its operation key
  generation. Consequently a target with a different generation is rejected
  by the existing stale-target/protocol identity rules before control/owner
  scope can be reached; its grant mismatch is still checked on resume and on
  Describe/watch.
- These focused checks do not replace the parent's integrated verification
  run; the component remains a prerequisite, not a milestone completion.

## Limits

This is a conservative private-library prerequisite for a future multi-session
owner. It does not define permanent cross-pane permissions, add a multi-session
public type or endpoint, alter launch/stop authority, or certify any of the
five non-UI milestones complete. The runtime still owns one target, and the
parent must complete integration verification and review before accepting
this step.
