# P5 committed-record corruption and truncation tests — 2026-10-21

## Scope and policy exercised

`tests/p5_history_corruption.rs` adds six bounded integration regressions using
private temporary directories and the public `HistoryStore` API. They exercise
the conservative committed-file policy: a row segment or screen checkpoint
that fails structural validation is rejected as a whole. Readers never expose
a valid prefix; writes, clear, and retention do not repair or replace a corrupt
committed source. This slice changes no production code, storage format,
manifest, dependency, or cleanup behavior.

The compact three-row segment is reopened and checked at every incomplete byte
prefix, including a prefix ending at a complete row boundary. Restoring the
complete original bytes makes all three rows readable. A bounded malformed
record set covers a bad segment header, inconsistent count, invalid flags,
nonzero reserved byte, row target/epoch/ordinal mismatch, unsafe cell content,
and trailing bytes. Each source remains byte-for-byte unchanged after a failed
read, and a separate healthy target remains readable.

For corrupt rows, `begin_history`, `append_rows`, `mark_history`, `mark_ended`,
`clear`, and `prune_expired` all refuse the operation while preserving the
committed bytes. The test keeps the other target live; it makes no promise of
directory-wide rollback if pruning removed an earlier eligible target before
finding corruption. Both normal and alternate checkpoints for the corrupt-row
target remain readable through their independent APIs, as do both buffers and
rows for the healthy target.

Checkpoint tests reject representative incomplete prefixes at empty, header,
header/body boundary, middle, and final-byte offsets. They retain the exact
corrupt source, refuse a valid replacement and clear, and leave the healthy
other buffer and another target readable. The minimum supported screen size is
5 by 20, so checkpoint truncation is sampled rather than iterating thousands
of offsets.

There is no checksum or authenticity tag. A valid-shaped cell text change can
remain structurally valid and is demonstrated as readable. These tests do not
claim detection of every changed byte, physical power-loss behavior, or full
machine-reboot recovery. The separate fresh-process CLI acceptance and orphan
temporary cleanup evidence remain in their own progress notes.

## Verification

Final commands were run from the repository root after the last test edit:

```text
$ rustfmt --edition 2021 tests/p5_history_corruption.rs
exit code: 0
no output

$ cargo test --locked --offline --test p5_history_corruption -- --test-threads=1
   Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.30s
     Running tests/p5_history_corruption.rs (target/debug/deps/p5_history_corruption-afc4348a0a304148)

running 6 tests
test corrupt_checkpoint_refuses_replacement_and_clear_but_keeps_other_files_readable ... ok
test corrupt_rows_refuse_lifecycle_mutations_and_leave_both_screen_buffers_readable ... ok
test every_incomplete_committed_row_segment_prefix_is_rejected_and_preserved ... ok
test incomplete_checkpoint_prefixes_are_rejected_without_using_a_prefix ... ok
test malformed_committed_rows_are_rejected_without_replacement_or_implicit_repair ... ok
test valid_shaped_cell_changes_are_not_detected_without_a_checksum ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
exit code: 0
```

During fixture development, the first compile caught a shadowed helper name;
the next run caught that a 1 by 1 terminal size is outside the public snapshot
range. The helper now uses the accepted 5 by 20 minimum. A public decoder
validates the fixture-only alternate-buffer tag before storage. The final
focused run above passed. Parent integrated checks and the separate fresh
process CLI acceptance remain outside this slice; this note does not claim
Phase 5 completion.
