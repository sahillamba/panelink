# Fixed two-session registry — 2026-10-18

Implemented `src/session_registry.rs` as a prerequisite for a future multi-
session owner. The registry allocates exactly two fixed slots at construction
and spends session IDs 1 and 2 in one nonzero generation. Its prototype-only
target mapping is workspace 1, pane equal to session ID, session ID equal to
the allocation sequence, and occupant revision 1. It records every reserved
identity through Failed or Exited; it has no removal, eviction, reuse, or reset
operation.

The registry compares all five fields of a supplied `TargetRef` before state,
record, or effects access. It exposes immutable record summaries through a
two-element iterator. Callers can borrow the retained handle immutably; mutable
access is limited to the existing `EffectSink` trait object so the concrete
owned handle cannot be replaced through the registry API. This remains an
ownership invariant, not a security boundary: the trusted `EffectSink`
implementation can shut down or internally mutate its own resources.

`next_target()` is a pure capacity/checked-ID preflight. Repeated peeks do not
reserve an identity, and the next successful `reserve()` returns that same full
target. `reserve()` uses this preflight before changing a slot or counter.

The lifecycle and handle rules are:

| Current state | Allowed next state | Handle rule |
| --- | --- | --- |
| Reserved | Intent, Failed | No handle; reserve and Intent perform no process effects. |
| Intent | Failed, Unknown | Direct transition has no handle. If a child was acquired, attach it first and transition from Spawned. |
| Intent | Spawned | `attach_spawned` stores the owned handle and changes state atomically. |
| Spawned | Applied, Unknown, Exited | The same owned handle remains retained. |
| Applied | Unknown, Exited | The same owned handle remains retained. |
| Unknown | Exited | Exited requires a retained handle. |
| Failed, Exited | Same state only | Exact duplicates are idempotent; no later state or handle change is allowed. |

Failed means a known pre-spawn failure. Unknown represents uncertainty. If a
child was spawned and its result later becomes uncertain, callers first attach
the acquired handle and then transition the record to Unknown. Exited retains
that same handle as a tombstone; another owner remains responsible for reaping
and cleanup.

`reserve` checks capacity and checked-ID advancement before mutation. Rejected
handle attachment returns both a fixed error and the exact supplied `E` value,
including for a stale target, wrong source state, or duplicate attachment.
Registry and record debug output redacts generation/targets, and attachment
errors never format `E`.

This module performs no spawn, cleanup, durable write, permission check,
authentication, grant management, or command admission. The surrounding owner
must retain one generation-wide operation ledger and grant table, write and
flush Intent before spawning, and preserve the handle after post-spawn
uncertainty. The prototype mapping does not select Panelink's workspace/tab/
pane hierarchy or enable an API launch path.

## Automated evidence

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline session_registry`
passed **12 registry tests, 0 failed**. Coverage includes repeated pure peeks
and peek/reserve identity equality; the full transition matrix and idempotence;
capacity after failed/exited records; checked-ID/capacity preflight; all five
target fields; handle-returning rejection; retained handles through Unknown and
Exited; no handle on reserve/Intent; and redacted debug output. The test handle
intentionally has no `Debug` implementation.

`CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings`
passed with exit 0. `cargo fmt --package panelink -- --check` passed with exit
0. `python3 scripts/check_docs.py docs/progress/P3-SESSION-REGISTRY.md` checked
one Markdown file with zero findings. `git diff --check` exited 0.

The first focused compile exposed an unnecessary test-only `Debug` bound from
`unwrap_err()` on `Result<&E, _>`; the assertion now inspects `.err()` and the
generic handle remains without `Debug`. That compile also encountered an
invalid format string in a concurrent parent-owned `provider_binding.rs`
change; the parent corrected it before the final test run. The first format
check after adding the overflow assertion reported a rustfmt-only line wrap;
the owned source was formatted and the final package check passed.

## Limits and handoff

The tests establish in-memory registry behavior only. They do not test durable
Intent storage, process spawn, child cleanup/reaping, global deduplication or
grants, socket routing, crash recovery, or a user-visible layout. The parent
owns source review, `src/lib.rs` export, integrated verification and any runtime
integration. This assignment releases ownership of `src/session_registry.rs`
and this evidence note after the recorded checks complete.
