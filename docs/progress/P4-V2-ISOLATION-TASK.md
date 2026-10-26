# Phase 4 fork/child and project isolation slices — 2026-10-24

Status: both scoped fixture slices pass 3/3 fresh runs; see
[P4-V2-ISOLATION-RESULT.md](P4-V2-ISOLATION-RESULT.md). Parent orchestrates two GPT-6 Luna High owners with
exclusive files; shared harness integration, review and live verification are
parent-owned. No commits or pushes and no production bridge activation.

## Scope

1. Identify supported no-prompt fork/child creation and selection on the pinned
   OpenCode 2.0.32 owned server. A child must never become its parent's selected
   top-level identity. Record unsupported paths as unproven.
2. Attach a fresh private project identity and prove foreign-project records
   cannot establish identity in the original project. Keep original observers
   fresh and unchanged. Same-client project switching is a separate gate if no
   supported keyboard route is demonstrated.

The exact pin, loopback-only sandbox, private configuration, ownership-before-
authentication, bounded observations and explicit cleanup from
[P4-V2-EXTENDED-RESULT.md](P4-V2-EXTENDED-RESULT.md) remain required.
No prompts, user accounts, installed configuration or live Panelink panes are
used. Reports contain only allowlisted metadata. New fixture sessions require
empty user-message reads. Missing capability is not a successful acceptance.

## Ownership and verification

- Luna child owner: `probe_opencode_v2_child_routes.py`, its focused tests and
  child notes only.
- Luna project owner: `probe_opencode_v2_project_isolation.py`, its focused
  tests and project notes only.
- Parent: scenario dispatch, shared driver, integration tests and durable docs.

Parent reviews source, runs focused and full Python discovery, compilation,
documentation and whitespace checks, then bounded fresh live scenarios through
normal approval review. No Rust/build inputs are planned. Provider recovery,
durable acknowledgment and current-at-crash remain disabled and open.
