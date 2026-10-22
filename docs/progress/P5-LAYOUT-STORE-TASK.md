# P5 saved-layout two-slot store — bounded task — 2026-10-21

Status: assigned (stage 2 of the staged plan in [P5-SAVED-LAYOUT.md](P5-SAVED-LAYOUT.md)).
Parent orchestrator owns lib.rs exports, canonical docs, ROADMAP/SESSION and
final verification. Read AGENTS.md, P5-SAVED-LAYOUT.md (sections "Bounded
version-one record" and "Future store, writer and durability contract") and
the codec task P5-SAVED-LAYOUT-CODEC-TASK.md first. Use `src/history_store.rs`
(`open`, `verify_identity`, `replace_named`, `save`, `load`,
`cleanup_temporary_files`, `TemporaryCandidate`) as the house style for
private-directory, rustix `openat`/`renameat`/`unlinkat`, fullfsync and
fixed sanitized errors. Write original code; do not refactor history_store.

## Exclusive ownership

ONLY: new `src/layout_store.rs`, new `tests/p5_layout_store.rs`, new result
note `docs/progress/P5-LAYOUT-STORE.md`. Do not edit any other file, the
manifest, vendor, lib.rs (parent exports `pub mod layout_store;`), ROADMAP or
SESSION. No dependencies, no commits/pushes, no real user files (temp dirs
only, deterministic cleanup). `src/saved_layout.rs` is owned by another
worker; consume its frozen public API only:
`SAVED_LAYOUT_BYTES`, `SavedLayout` (`metadata()`, `pane_count()`, ...),
`LayoutMetadata` (`generation()`, `workspace()`, `snapshot_sequence()`,
`layout_revision()`, `observed_at_unix_ms()`), `encode`, `decode`,
`LayoutRecordError`.

## Exact API

```rust
pub enum LayoutSlot { Current, Previous }            // Copy/Eq/Debug
pub enum SaveOutcome { Written, AlreadyDurable }      // Copy/Eq/Debug
pub struct LayoutStore { /* held dir File + path */ } // redacted Debug
impl LayoutStore {
    pub fn open(path: &Path) -> io::Result<Self>;            // existing dir only, never creates
    pub fn prepare(server_directory: &Path) -> io::Result<Self>; // mkdir server/layout 0700 if absent, then open
    pub fn path(&self) -> &Path;
    pub fn verify_identity(&self) -> io::Result<()>;
    pub(crate) fn verify_server_directory(&self, server_directory: &Path) -> io::Result<()>; // path == server/layout
    pub fn load(&self, slot: LayoutSlot) -> io::Result<Option<SavedLayout>>;
    pub fn load_bytes(&self, slot: LayoutSlot) -> io::Result<Option<[u8; SAVED_LAYOUT_BYTES]>>;
    pub fn save(&self, bound_generation: u64, bound_workspace: u64, candidate: &SavedLayout)
        -> io::Result<SaveOutcome>;
    pub fn cleanup_temporary_files(&self) -> io::Result<usize>;
}
```

Files: `current.layout`, `previous.layout` inside the layout directory. Each
committed slot: regular, single link, owned by euid, mode 0600, exactly 160
bytes, decodes with `saved_layout::decode`. `open` checks the same
absolute/canonical/private-ancestor/0700/no-symlink rules as HistoryStore::open.
Load never creates/modifies anything; absence is `Ok(None)`; any invalid or
unsafe slot is a sanitized error (per slot: an invalid current must not
prevent loading a valid previous). Errors: fixed messages only
(`invalid saved layout`, `unsafe layout storage`, `layout save refused`), no
IDs/paths/bytes. Caller is responsible for singleton exclusion (documented on
`save`/`cleanup`); the guarded writer is a later slice.

## save() rules (fail closed, preflight before any mutation)

1. `verify_identity`. Reject zero bound values. Candidate metadata generation
   must equal bound_generation and workspace equal bound_workspace.
2. Read both slots. Any present slot must be valid, workspace == bound,
   generation <= bound. Current absent + previous present -> refuse. Previous
   present must have generation < current generation OR be byte-identical to
   current (interrupted rotation); otherwise refuse.
3. Both absent: write current only -> `Written`.
4. current.generation < candidate generation (rotation): if previous is not
   byte-identical to current, atomically write EXACT current bytes to
   previous.layout (temp, fsync+fullfsync, rename, directory sync); if it is
   already identical, directory-sync. Only then atomically write current.
   Archive success alone is never acknowledged.
5. current.generation == candidate generation, compare snapshot_sequence:
   equal -> byte-identical: directory sync, `AlreadyDurable`; else refuse.
   lower -> refuse. higher -> refuse if current is empty (pane_count 0,
   terminal); refuse if candidate layout_revision < current's; if candidate is
   nonempty require layout_revision strictly greater; an empty candidate may
   have equal or greater revision. Then write current only (never touch
   previous). Timestamps never order or validate anything.
6. Atomic write: temp name exactly `.current.layout-PID-COUNTER.tmp` /
   `.previous.layout-PID-COUNTER.tmp` (process id, global AtomicU64 counter),
   `openat` WRONLY|CREATE|EXCL|NOFOLLOW|CLOEXEC 0600, write_all, sync_all,
   fcntl_fullfsync, re-read destination and require it still equals the
   preflight bytes (or still absent), rename, sync directory. Remove temp on
   error. Pre-rename error leaves old destination; post-rename directory-sync
   error returns Err (bytes may be visible; a byte-identical retry then
   succeeds via rule 5 / rotation retry, without re-archiving).
   No internal retry loop (writer slice owns retries). Single attempt only.
7. Add `#[cfg(test)]` fault injection mirroring HistoryStore (points: before
   archive rename, after archive before current write, before current rename,
   directory sync) used by in-module unit tests.

## cleanup_temporary_files()

Under caller exclusion. Scan at most 128 directory entries (more -> error,
nothing deleted); at most 16 recognized temporaries (more -> error). Recognized
only the two exact name patterns above with canonical nonzero decimal PID (u32)
and nonzero u64 counter (no leading zeros/sign). Unknown entries, including
committed slots, are preserved. Each candidate must be a private single-link
regular file owned by euid, mode 0600, length <= 160; any unsafe candidate
fails the whole call before the first unlink. Preflight all, then revalidate
and unlink each via the held directory. Sync the directory even if nothing was
removed. Never promote a temporary or repair a committed slot.

## Tests (actual runs required)

Unit (in-module, using fault injection): crash/failure at every fault point
during first save, same-generation save and rotation, then byte-identical
retry recovery: archive-only state (previous == current old bytes) is valid
and retry completes without double archiving; uncertain current rename
(current already new bytes) acknowledges via sync without touching previous.

Integration `tests/p5_layout_store.rs` (public API, temp dirs under
std::env::temp_dir with unique names, 0700): open refuses missing/0755/symlink
dir; prepare creates 0700; first save; same-gen higher seq/revision; equal
seq identical -> AlreadyDurable, different -> refused; lower seq refused;
nonempty equal revision refused; nonempty->empty equal revision accepted;
empty then any further changed candidate refused; wrong generation/workspace
candidate refused; slot generation > bound refused; rotation preserves exact
old bytes in previous and leaves current new; second rotation replaces
previous; missing current + present previous refused for save but previous
still loadable; newer-previous / same-gen-different-bytes previous refused;
corrupt/short/long/0644/symlink/hardlinked current refused and bytes
preserved unchanged (compare bytes + mode); invalid current does not block
load(Previous); load never creates files (directory listing unchanged);
cleanup removes only recognized safe temps, preserves unknown names and
committed slots, refuses >16 recognized or unsafe candidate without deleting
anything; debug/error redaction.

Run and paste ACTUAL output into P5-LAYOUT-STORE.md:

```text
cargo test --locked --offline --lib layout_store -- --test-threads=1
cargo test --locked --offline --test p5_layout_store -- --test-threads=1
cargo clippy --locked --offline --all-targets -- -D warnings
cargo fmt --check
```

Record exact counts, development failures you corrected, limits (no
writer/guard/runtime/CLI, no checksum, no multi-file atomicity) and ownership
release. Proposed scenarios that did not run are not passes.
