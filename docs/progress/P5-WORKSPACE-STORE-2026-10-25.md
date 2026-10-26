# P5 workspace-index registry store — 2026-10-25

Status: implementation reviewed; temporary-path substitution check corrected.
Focused Cargo validation is pending parent execution. Runtime routing,
default-open discovery, UI, server integration, and reboot recovery remain out
of scope.

## Contract

`workspace_index::store::WorkspaceIndexStore` accepts only a caller-supplied,
existing private application root. It does not inspect environment variables
or discover a root. The registry is `workspace-index.v1`, with its lifetime
lock in `workspace-index.lock`; the local codec byte cap is 23,352 bytes so
the pure codec surface remains unchanged.

Open validates every root path component, requires the final directory to be
owned by the effective user with mode `0700`, opens it without following
symlinks, and holds a nonblocking exclusive `flock` on a regular `0600`,
single-link lock file. Root, held lock, and the named lock are revalidated by
inode before access. The registry is read without following symlinks and must
be a regular, user-owned `0600`, single-link file within the size cap. Decode
errors, unsafe objects, and I/O failures are returned as fixed, redacted error
categories; the store's `Debug` output contains no root path or names.

`load()` returns `None` only for an absent registry. It preserves corrupt
bytes and never repairs or replaces them. `initialize_legacy()` is the only
initializer: it writes Project 1 `Default`, tab 1 `Main`, revision 1,
`next_id` 2, and touches no existing server/context/history/socket/process
state. `save(expected_revision, record)` requires the next revision, a
nondecreasing allocator watermark, non-reused new IDs, and immutable project
ownership for an existing tab. An exact retry is `AlreadyDurable` only when
the expected prior revision matches and the committed bytes are identical.

Publication creates one fixed-name temporary with `create_new`/no-follow,
writes and syncs it, revalidates root/lock/record identity and content, renames
within the held directory, and syncs the directory. It removes a temporary
only when its inode still matches the exact temporary created by that call.
An ambiguous post-rename directory-sync failure disables further mutations
until reopen; `load()` remains available to report the record currently on
disk. Existing or unrelated temporary files are not scanned or removed.

Review correction: immediately before rename, the store now reopens the named
temporary without following symlinks and checks its device/inode, owner, mode,
link count, size, and bytes against the file created by the current save. The
prior code trusted the temporary pathname after syncing its open descriptor;
a substituted symlink or hard link could otherwise have been renamed over the
registry. Unit fault cases now exercise both substitutions and require the old
registry bytes to remain loadable. A same-user race between this final check
and `renameat` remains outside what pathname-based POSIX rename can make
conditional; the private `0700` root and exclusive lock are still required.

## Verification

Added `tests/p5_workspace_store.rs` coverage for additive legacy migration,
legacy subtree preservation, permission bits, same-process and child-process
lock denial, exact lost-ack retry, stale revision rejection, corrupt-byte
preservation, symlink/hardlink/root replacement refusal, allocator watermark
and tab-project ownership checks. A private module unit test injects faults
before and after rename to check old-record preservation, temporary cleanup,
post-rename visibility, and the reopen latch. The new private unit test also
injects temporary symlink and hardlink substitution before publication.

The previous store owner attempted this command, but compilation stopped in
the then-incomplete dirty compositor before store tests ran:

```sh
CARGO_HOME=/private/tmp/panelink-p4-cargo-home \
CARGO_TARGET_DIR=/private/tmp/panelink-p4-bridge-store-target \
cargo test --locked --offline --test p5_workspace_store -- --test-threads=1
```

That earlier compile failure reported unresolved `validate_selection` at line
292 and `paint_selection` at line 347 in `src/terminal/compose.rs`; it is
historical and not a result for the current tree. The current task explicitly
prohibited this implementation sub-agent from running Cargo, builds, or tests;
the parent owns serial validation in its existing target directory. This
sub-agent ran `rustfmt --edition 2021 src/workspace_index/store.rs` (exit 0)
and `git diff --check` (exit 0). The updated store unit and integration tests
have not been run in this task, so no pass count is claimed.

The accepted hierarchy remains Project → workspace tabs → split panes, as
recorded in `P5-WORKSPACE-TABS-DESIGN-2026-10-25.md`. The codec contract and
23,352-byte limit are described in `P5-WORKSPACE-CODEC-2026-10-25.md`. This
store does not imply runtime activation, tab UI, namespace creation, or
process migration.
