# P5 saved-layout implementation slice — result — 2026-10-21

Post-integration completion and shutdown hardening (2026-10-22): see
[P5-LAYOUT-COMPLETION.md](P5-LAYOUT-COMPLETION.md). The counts below describe
the original implementation run, not the current combined tree.

Scope: implement the accepted [ADR](P5-SAVED-LAYOUT.md) stages 1-5 for the
current two-pane prototype. Parent orchestration with Kimi K2.6 workers
(user-selected model for this run; two parallel at a time) and parent review,
integration and verification. Parent phases 3/5/6 remain OPEN.

## Delivered

| Stage | Files | Owner | Evidence |
| --- | --- | --- | --- |
| 1 codec | src/saved_layout.rs | Kimi A + parent fixes | [P5-SAVED-LAYOUT-CODEC.md](P5-SAVED-LAYOUT-CODEC.md) |
| 2 store | src/layout_store.rs, tests/p5_layout_store.rs | Kimi B (resumed) + parent fix | [P5-LAYOUT-STORE.md](P5-LAYOUT-STORE.md) |
| 3 writer | src/layout_writer.rs, tests/p5_layout_writer.rs; ServerStorage::claim_layout_writer | Kimi C + parent (claim, fixes) | [P5-LAYOUT-WRITER.md](P5-LAYOUT-WRITER.md) |
| 4 driver | multi_runtime LayoutObserver/enable_layout/Drop, multi_session layout_terminal_empty, p3_cli serve wiring | Parent | [P5-LAYOUT-LIFECYCLE.md](P5-LAYOUT-LIFECYCLE.md) (Kimi E acceptance) |
| 5 CLI | main.rs dispatch/help, default_open layout_show, tests/p5_layout_cli.rs | Kimi D + parent fix | [P5-LAYOUT-CLI.md](P5-LAYOUT-CLI.md) |
| review | read-only | Kimi F | [P5-LAYOUT-REVIEW.md](P5-LAYOUT-REVIEW.md) |

Task contracts: P5-SAVED-LAYOUT-CODEC-TASK.md, P5-LAYOUT-STORE-TASK.md,
P5-LAYOUT-WRITER-TASK.md, P5-LAYOUT-CLI-TASK.md, P5-LAYOUT-LIFECYCLE-TASK.md.

## Parent review corrections (all with regressions)

- Codec: one-pane decode lacked header-scope check; two-pane snapshot lacked
  root bounds.
- Store: FIFO at a slot name could block a read (now O_NONBLOCK).
- Writer: older deferred candidate could follow a newer one (store refusal
  -> terminal unavailable); detached worker could start queued saves after
  timeout/Drop (now canceled).
- CLI: unsafe layout directory reported as absent (now invalid).
- Independent review: no blocker; major observer-test gap closed by the
  10-scenario lifecycle suite; minors documented/fixed (sequence exhaustion
  comment, canonical-equality definition). Remaining minor suggestions
  (defensive scope recheck at begin_shutdown, a main.rs parser unit test)
  are not adopted; the binary-level usage-error test covers parsing.

## Driver semantics implemented

Observation runs at the end of every owner tick on the owner thread and
submits nonblocking. A candidate is emitted only when the canonical
LayoutSnapshot value changes, or once on terminal-empty (every layout target
positively Exited). Sequence is checked per generation; missing clock or
format-incompatible snapshot disables saving only. Drop observes once, sets
the freeze marker before prepare_driver_shutdown, closes layout admission
(one final nonblocking try-submit) and then shares ONE 250 ms deadline with
history. Serve startup prepares server/layout, cleans recognized layout temps
under the held singleton and enables the writer; failure prints a fixed
warning and keeps shells running.

## Actual verification (parent, this tree)

```text
python3 scripts/verify_non_ui.py            -> 8/8 checks passed
cargo test --locked --offline --lib         -> 626 passed; 0 failed; 4 ignored
cargo test --locked --offline --bin panelink -> 95 passed; 0 failed
all 35 integration targets (serial each)   -> 162 passed; 0 failed; 11 ignored
  p5_layout_store 32, p5_layout_writer 5, p5_layout_cli 11,
  p5_layout_lifecycle 10 (3 consecutive runs, all pass)
strict all-target Clippy, fmt, build        -> pass
decoder 39, Python unittest, component fingerprints -> pass
docs check                                  -> 184 files, 0 findings
```

This environment permitted pathname socket binding, so previously
environment-blocked socket/startup cases ran and passed here; that is new
evidence for this environment only and does not certify Apple Terminal host
or provider gates. The first verifier run in this slice failed docs_check on
28 worker trailing-whitespace lines; corrected before the passing run. Ignored
tests (9 host, 1 pathname, 1 default-open, 4 lib) are not passes.

## Limits (not claimed)

No layout restoration into live control, no relaunch/provider resume, no
machine reboot acceptance, no checksum/authenticity, no history/layout
multi-file atomicity, no durability at Drop return, no performance budget.
Coalesced intermediate topologies may be lost by design. Workspace
hierarchy, provider identity and representative workload remain open.
