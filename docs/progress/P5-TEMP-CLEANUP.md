# P5 interrupted temporary-file cleanup — 2026-10-21

`HistoryStore::cleanup_temporary_files` removes orphan writer temporary files
after the caller has excluded active writers. The same stopped-writer
precondition applies to `clear`; the store cannot determine whether another
process is actively writing. The parent owns the explicit CLI and startup
lock wiring.

Cleanup accepts only exact names emitted by the history writer:
`.{canonical-target-final-name}-{pid}-{counter}.tmp`. The five target fields
must each be nonzero, 16-character lowercase hexadecimal values. PID and
counter must be nonzero canonical decimal `u32` and `u64` values. The final
name is reconstructed through `history_name` or `name` before it is accepted.
Unknown names, malformed lookalikes and non-UTF8 names are left untouched.

The scan is bounded to 16,384 total directory entries and 4,096 recognized
candidates. Before deleting anything, cleanup opens every candidate relative
to the held directory descriptor with `RDONLY|NOFOLLOW|CLOEXEC|NONBLOCK` and
checks for a current-user, regular, mode-0600, single-link file within the
rows or screen size bound. It stores only candidate names and inode metadata,
then revalidates the named directory and each candidate before each relative
unlink. Temporary bodies are never read, promoted or replayed. Committed rows
and screens are never validated, repaired, truncated or removed by cleanup.

Cleanup syncs the directory before returning success, including when it finds
no candidates. A sync or unlink error can follow visible deletions; cleanup
does not claim transactional rollback. A retry still syncs the directory and
returns an error until that sync succeeds. Identity revalidation narrows path
replacement races; the caller's single-writer coordination is the concurrency
contract and this is not isolation from a malicious process with the same UID.

The integration fixture uses private, mode-0700 directories below
`/private/tmp` and mode-0600 files. It checks byte-identical committed rows
and normal/alternate screens after cleanup and reopen; partial and complete
orphan temps without promotion; exact filename grammar; symlink, hard-link,
FIFO, directory, wrong-mode and oversize refusal before any safe-temp deletion;
directory replacement denial; and both scan bounds. The filesystem returned
`EILSEQ` when asked to create an invalid-UTF8 name, so the integration test
could not create that entry on this APFS volume. The implementation skips
non-UTF8 names without mutation, but that branch lacks a filesystem fixture
here.

An initial library run exited 101 because the first parser version reversed
the `rsplit_once` tuple; this was corrected. The first integration run also
exited 101 on that parser defect and took 102 seconds because fixture files
were individually synced. The fixture no longer syncs synthetic entries, and
the final focused run below passes.

Final focused verification on this tree:

| Command | Result |
| --- | --- |
| `cargo test --locked --offline --lib history_store` | exit 0 — 4 passed, 0 failed, 0 ignored |
| `cargo test --locked --offline --test p5_history_cleanup` | exit 0 — 6 passed, 0 failed, 0 ignored |
| `cargo test --locked --offline --test p5_history_store` | exit 0 — 10 passed, 0 failed, 0 ignored |
| `cargo fmt --check` | exit 0 |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | exit 101 — blocked by `clippy::type_complexity` at `examples/history_store_bench.rs:190`; this file is outside the cleanup owner's scope |

The targeted cleanup tests do not verify the parent-owned CLI denial or
startup-before-capture integration; those remain subject to the parent
integrated review and verification.

Parent integration follow-up: three real CLI tests and one startup-helper test
pass. They verify stopped-workspace lock exclusion, missing metadata refusal,
read-only queries leaving temporaries alone, exact cleanup/zero-count retry,
unsafe-candidate rejection and preservation of committed bytes. Startup calls
cleanup while the driver holds the storage lock, before `enable_history` can
install any capture writer. An unsafe temporary makes the setup helper fail;
the runtime reports history capture unavailable and continues its shell path.
Full pathname runtime acceptance remains blocked by this sandbox's listener
permission, rather than being inferred from the helper test.

The parent independently reran the six cleanup cases, four store unit cases,
three CLI cases, ten existing store cases, one fresh CLI recovery case and
three checkpoint cases; all passed. Together with the startup-helper case,
that is 28 focused checks. The initial parent CLI run had one pass/two failures
on the reversed parser tuple; the corrected run passed 3/3. Final integrated
strict Clippy passed after the benchmark's type alias resolved the earlier
owner-blocking finding. Current integrated limits are in
[INTEGRATION.md](INTEGRATION.md).

## Actual pathname startup follow-up — 2026-10-25

The historical listener-permission blocker above is superseded for this
fixture. Parent ran the production `serve` path in private directories:
**2 passed, 0 failed**, including orphan deletion before post-readiness capture
and unsafe-temp refusal with the shell server still running. See
[P5-HISTORY-STARTUP-ACCEPTANCE-2026-10-25.md](P5-HISTORY-STARTUP-ACCEPTANCE-2026-10-25.md)
for the initial test compile correction, final log, source ordering and limits.
This proves startup integration in the current environment; physical reboot
and power-loss durability remain separate gates.
