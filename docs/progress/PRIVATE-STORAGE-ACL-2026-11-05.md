# macOS private-storage ACL validation — 2026-11-05

## Scope and policy

This change closes a gap in mode-only private-storage validation: macOS
extended ACL entries can grant access beyond directory `0700` and file `0600`
mode bits. The shared validator in `src/private_acl.rs` reads ACLs from the
already-open descriptor and compares entry qualifiers with the object's owner
UUID.

Private files and directories reject every non-owner ALLOW entry, including
inherit-only entries. Empty ACLs and deny-only ACLs are accepted. Existing
ancestor directories may grant non-owner read, search and metadata-read access,
but non-owner grants for mutation rights are rejected, including when the
ancestor has a sticky bit. ACL retrieval, validation and owner UUID lookup
errors fail closed. An absent descriptor ACL (`ENOENT` or `ENOATTR`) is
accepted as no extended ACL; other retrieval errors are refused. The validator does not remove or rewrite ACLs and does not
silently retry using another state root.

The shared checks are wired through socket/state directories, server metadata,
saved history/layout/recovery links, private record stores, workspace registry,
caller contexts, diagnostics and default state-root handling. Diagnostics
silently disable when their private path has an unsafe or uninspectable ACL.
Fixed existing error categories remain in use; storage and wire formats and
dependencies are unchanged. Existing trust between processes with the same UID
is unchanged.

## Reproduction and regression coverage

The macOS/aarch64 integration fixture in `tests/private_storage_acl.rs` creates
its own temporary directory under `/private/tmp`, applies test-only ACL entries
with `/bin/chmod +a`, and asserts behavior at the application storage APIs.
Coverage includes:

- A mode-`0700` private directory with an `everyone` read/list/search ALLOW
  grant is refused before storage payloads are created.
- Inherited read/search ACL grants are rejected on a newly created private
  directory before writes.
- An existing mode-`0600` saved record with an `everyone` read grant is
  rejected without changing its bytes.
- A grant added after store initialization prevents new history writes.
- A writable ancestor ACL is rejected even when the final directory is
  private; each supported ancestor mutation permission is covered.
- Deny-only ACLs, owner-only grants and harmless ancestor read/search/metadata
  grants remain usable.
- Later ACL changes are checked through the retained descriptor, and caller
  context files reject late grants without replacing their contents.

Fixture cleanup clears ACLs only within that fixture so the test can remove its
temporary tree. This is test cleanup, not product behavior. Do not apply
recursive ACL-removal commands to a user's existing state directory.

Read-only inspection of an affected path and its ancestors can use macOS
`ls -lde PATH`. Review ACL entries and ownership before any manual change.

## Native API review

The implementation uses the installed macOS SDK `sys/acl.h` and
`membership.h` contracts without adding a crate. Owned ACL and qualifier
allocations are freed through `acl_free`; caller file descriptors remain
borrowed. ACL traversal is bounded by the SDK's 128-entry limit. Darwin
`acl_get_entry` returns zero for an entry and `-1/EINVAL` at the end, rather
than Linux's return convention. Unknown tags or permission bits are refused.
Owner UUID resolution is lazy and owner-only grants are checked explicitly.

Parent native probing observed that ACL-free descriptors for `/`, `/private`,
`/private/tmp` and a fresh private directory return null with `ENOENT`. This
corrected the initial overly strict implementation before acceptance; the
positive no-ACL/deny-only/owner-only cases pass. A bounded independent source
review reported no material findings in the native API code and inspected
call sites; this is source review, not a separate native-user acceptance gate.

## Verification status

Baseline: clean `main` at `2958393c4c043e6cba008966d960ae7f76b1fa1b`.
The initial integration run reproduced all five security-negative failures.
The deny-only positive exercised the intended behavior but its fixture
cleanup initially hit EACCES; test cleanup was corrected without weakening
any product assertion.

Focused verification completed:

- `cargo test --locked --offline --test private_storage_acl -- --test-threads=1`:
  **11 passed**, zero failed/ignored.
- `cargo test --locked --offline --lib -- --test-threads=1 client_context server_storage history_store layout_store recovery_links workspace_index provider_binding_store managed_provider_store diagnostics`:
  **105 passed**, zero failed, one ignored helper. This preceded the additional
  diagnostic ACL test, which was then run separately.
- Binary `granting_acls` test: **1 passed**; library `granting_acls` test:
  **1 passed**. These prove refused ACLs do not truncate/rewrite existing
  state or log payloads.
- `cargo clippy --locked --offline --all-targets -- -D warnings`: passed.
- Four usage-skill Markdown files: zero findings. The bundled Python quick
  validator could not import PyYAML; Ruby standard-library YAML plus equivalent
  name/description/frontmatter/TODO checks passed without installing a tool.

Canonical command `RUST_TEST_THREADS=1 python3 -u scripts/verify_non_ui.py`
exited **1: seven of eight checks passed**. Its root Cargo step exceeded the
unchanged 420-second deadline after 82 completed result groups reported
**1,702 passed / zero failed / 18 ignored**; it had reached the final
`usability_input_pty` binary. All 11 ACL regressions and all six workspace-tab
cases passed within that run. No assertion failure was reported before the
cutoff. The decoder passed 39/39; Python ran 234 tests with one skip; docs
checked 471 files with zero findings. Component fingerprints, formatting,
strict all-target Clippy and locked build passed. This is **not** a canonical
8/8 pass. The prior checkout handoff already records the same 420-second
Cargo limitation. Log: `/private/tmp/panelink-acl-verification-20261105.log`.

The separate warm full Rust run completed with **exit 0: 1,707 passed,
zero failed, 20 ignored, across 88 result groups**, including doc-tests.
Command: `RUST_TEST_THREADS=1` with `cargo test --locked --offline`, wrapped by
the existing `scripts.verify_non_ui.run_process` helper with a 600-second
bound. Its output ends with `bounded_root_cargo_result=passed`. This does not
change or replace the canonical timeout. Log:
`/private/tmp/panelink-acl-rust-20261105.log`.

All 13 new ACL cases passed: 11 native integration cases plus the state/log
preservation and diagnostic-disable tests. Final environment: macOS 26.5.2,
arm64, Rust 1.99.1 (`58cf8f9c3`, 2026-10-13). No dependency/lockfile changes.
The local HEAD remained the baseline above; the tracking ref advanced during
verification and the branch is now two commits behind `origin/main`. No fetch,
merge, rebase or push was performed by this task.

No install, user-server restart, live pane input, commit or push. This does
not close broader host/provider or physical-reboot acceptance gates. Rebuilt
source does not update an already-running installed process.

## Branch publication — 2026-11-07

The user requested a new branch and push. This checkpoint packages the verified
ACL changes and affected documentation on `fix/private-storage-acl`, based on
`2958393c4c043e6cba008966d960ae7f76b1fa1b`, with explicit push authorization.
The November 5 no-commit/no-push record above describes that earlier turn.
Application and test content is unchanged; no merge/rebase or new test result,
installation, user-server restart or broader acceptance is claimed here.
