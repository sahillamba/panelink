# Independent Review: Saved-Layout Slice (P5)

**Reviewer:** Independent agent (read-only, Oct 2026)
**Scope:** `src/saved_layout.rs`, `src/layout_store.rs`, `src/layout_writer.rs`, layout-related parts of `src/multi_runtime.rs`, `src/server/multi_session.rs`, `src/server_storage.rs`, `src/p3_cli.rs`, `src/default_open.rs`, `src/main.rs` against `docs/progress/P5-SAVED-LAYOUT.md`.
**Method:** Static source analysis + targeted read-only `cargo test` runs. No file mutations except this review document.
**Test runs verified:**
- `cargo test --locked --offline --lib layout_store -- --test-threads=1` → 25 passed
- `cargo test --locked --offline --lib layout_writer -- --test-threads=1` → 17 passed
- `cargo test --locked --offline --lib server_storage -- --test-threads=1` → 15 passed

---

## Summary

The saved-layout slice is **well-engineered and fail-closed**. The three lower layers (codec, store, writer) have thorough unit tests, redacted output, bounded allocation, and atomic disk replacement. No blocker-level correctness bugs were found.

The **primary gap is test coverage for the owner/runtime wiring** in `multi_runtime.rs`: the `LayoutObserver` lifecycle (split/close/resize/natural-exit triggers, empty-tombstone transition, freeze-before-stop, shared 250 ms deadline) is exercised only indirectly by the existing history-capture integration tests, not by layout-specific tests. This is a **major** test gap because it is the boundary where design-contract violations are most likely to appear in practice.

Minor findings: one CLI error-message granularity issue, and a handful of defensive-check/test stubs. All are documented below with file:line references and concrete fixes.

---

## Verified Findings

### MAJOR-1: Missing `LayoutObserver` lifecycle and trigger tests (`src/multi_runtime.rs`)

**Severity:** major
**File:** `src/multi_runtime.rs` (LayoutObserver struct, `enable_layout`, `observe_layout`, `Drop`)
**Evidence:**
- The `LayoutObserver` is instantiated in `enable_layout` (line 490) and observed in `tick` (line 630) and `Drop` (line 411).
- `Drop` freezes the observer (line 413) before child stop, then shares one `MAX_SHUTDOWN_WAIT` deadline between history and layout (lines 419–458).
- Existing `multi_runtime.rs` tests (lines 2010+) cover history capture blocking and full capture registry, but **none** exercise:
  1. First layout observation after `enable_layout`.
  2. A split producing a new sequence/revision.
  3. A close leaving a survivor producing a new observed layout.
  4. Natural child exit (via `poll` → `collapse_exited_panes`) producing an empty tombstone.
  5. Shutdown freeze preventing an empty candidate from overwriting the last pre-stop nonempty layout.
  6. Equal-revision empty transition (nonempty→empty with the same layout revision).
  7. The shared 250 ms deadline actually bounding both history and layout waits.
- `P5-SAVED-LAYOUT.md` explicitly gates acceptance on "test every trigger above, including last exited retained leaf, retained-unknown launch and same-revision empty transition."

**Fix:** Add a `#[cfg(test)]` module in `src/multi_runtime.rs` with fake `LayoutBackend` and `ServerStorage` fixtures that drive the owner through each trigger and assert on `LayoutStatusHandle` outcomes, or add an end-to-end test in the existing integration harness that validates `LayoutStore::load` after a driver drop.

---

## Minor Findings

### MINOR-1: `layout_show_result` collapses all `LayoutStore::open` errors into one label

**Severity:** minor
**File:** `src/default_open.rs:1137`
**Evidence:**
```rust
let store = LayoutStore::open(&state.server.join("layout")).map_err(|_| "no saved layout")?;
```
- A missing directory (`NotFound`) and an unsafe directory (`PermissionDenied`) both map to the same string.
- The spec says read-only inspection should return "a fixed sanitized error/nonzero status and no partial stdout." Distinguishing absence from permission failure is still a fixed sanitized label, and it helps operators without leaking paths.

**Fix:**
```rust
let store = LayoutStore::open(&state.server.join("layout")).map_err(|e| match e.kind() {
    io::ErrorKind::NotFound => "no saved layout",
    _ => "saved layout unreadable",
})?;
```

---

### MINOR-2: `LayoutWriter::begin_shutdown` does not re-verify deferred candidate scope

**Severity:** minor (defensive)
**File:** `src/layout_writer.rs:298–323`
**Evidence:**
- `begin_shutdown` takes `self.deferred` and immediately calls `try_send` without re-checking `bound_generation`/`bound_workspace`.
- The deferred slot is only ever populated by `observe()`, which already performs this check and sets `unavailable` on mismatch. An invariant comment would make this explicit.
- A future refactor could bypass `observe()` and write into `deferred` directly.

**Fix:** Add a defensive assertion or document the invariant:
```rust
// Invariant: self.deferred is only ever set by observe(), which refuses
// out-of-scope layouts and sets unavailable=true.
```
Alternatively, add a cheap re-check inside `begin_shutdown`.

---

### MINOR-3: No CLI parser tests for `layout show` / `layout show --previous`

**Severity:** minor
**File:** `src/main.rs` (lines 146–158), `src/main.rs` tests
**Evidence:**
- `main.rs` tests verify `stop`, `probe`, `--help`, `--version`, `doctor`, and malformed arguments, but there is no test for the `layout` branch.
- A regression that accepted `layout show extra` or rejected `--previous` would not be caught by existing unit tests.

**Fix:** Add tests in `main.rs`'s `#[cfg(test)]` module:
```rust
assert_eq!(run(arguments(&["layout", "show"])), 0);
assert_eq!(run(arguments(&["layout", "show", "--previous"])), 0);
assert_eq!(run(arguments(&["layout", "show", "extra"])), 2);
```

---

### MINOR-4: `LayoutObserver` disabled check on sequence overflow is terminal for the generation

**Severity:** minor (suspicion / design-contract note)
**File:** `src/multi_runtime.rs:364–367`
**Evidence:**
```rust
let Some(sequence) = self.sequence.checked_add(1) else {
    self.disabled = true;
    return;
};
```
- If `sequence` wraps `u64::MAX`, the observer is permanently disabled for this generation. No new layout snapshot is ever observed.
- The spec says "Checked exhaustion or missing valid clock disables saving with a fixed diagnostic." It does not say the observer must recover.
- Given realistic server lifetimes, `u64` sequence exhaustion is impossible. This is therefore a theoretical edge case, not a practical bug, but it should be documented as an explicit terminal state rather than a silent disable.

**Fix:** Add a one-line comment in `observe` documenting that sequence exhaustion disables observation for the remaining generation lifetime.

---

### MINOR-5: `LayoutCandidateKey::Live` equality depends on full `LayoutSnapshot` equality

**Severity:** minor (suspicion / contract alignment)
**File:** `src/multi_runtime.rs:320–324`
**Evidence:**
```rust
#[derive(Clone, Copy, Eq, PartialEq)]
enum LayoutCandidateKey {
    Live(LayoutSnapshot),
    Empty,
}
```
- `LayoutSnapshot` equality includes `revision`, `rows`, `columns`, `panes` (target identities and rects), and `divider`.
- The spec says observation emits "only on a changed canonical topology/geometry or transition to empty, not … same-size pending-resize retries."
- If a pane target identity changes but its rect stays the same (e.g., a session was retired and a new split created at the same position), the snapshot is different and a new candidate is emitted. This is correct.
- However, if two distinct `LayoutSnapshot` values happen to compare equal by value but represent different owner states (e.g., different internal `PaneLayout` state not reflected in the snapshot), the observer would coalesce them. This is consistent with the design: the snapshot is the canonical desired geometry.
- **No bug found**, but the contract should be explicit that `LayoutSnapshot` equality is the definition of "canonical topology/geometry" for observation.

**Fix:** Add a doc comment on `LayoutCandidateKey` linking it to the spec's coalescing rule.

---

## Verified Correct Behaviors (negative findings)

The following areas were specifically inspected and found compliant with the design contract:

1. **Codec fixed-size enforcement:** `encode` produces exactly 160 bytes; `decode` rejects any other length, bad magic, nonzero reserved regions, and all malformed target fields. (Lines verified: `src/saved_layout.rs:304–442`.)

2. **Fail-closed store:** `LayoutStore::save` refuses:
   - stale/future generation or mismatched workspace,
   - missing current with present previous,
   - newer previous generation or same-generation/different-bytes previous,
   - equal/lower sequence,
   - nonempty equal/lower revision,
   - repeated empty after first empty,
   - corrupt current or previous (preserves both and returns error).
   (Lines verified: `src/layout_store.rs:319–430`.)

3. **Atomic rotation:** Archive current to previous with directory sync, then write new current; interrupted rotation is recovered by byte-identical retry without double-archive. (Lines verified: `src/layout_store.rs:392–401`, tests `byte_identical_rotation_retry_skips_double_archive` and `crash_after_archive_leaves_valid_previous_and_retry_completes`.)

4. **Temp cleanup boundedness:** `cleanup_temporary_files` scans at most 128 directory entries and 16 recognized temporaries; each is stat-validated twice before unlink. (Lines verified: `src/layout_store.rs:501–592`.)

5. **Writer coalescing and bounded queue:** `sync_channel(1)` + owner-deferred slot yields at most 3 candidates in flight. `observe` replaces the deferred slot; `flush` attempts submission without blocking. (Lines verified: `src/layout_writer.rs:173–255`.)

6. **Shared 250 ms deadline:** `MultiServerDriver::Drop` computes one `deadline = Instant::now() + MAX_SHUTDOWN_WAIT` and passes it to both history and layout shutdown paths. (Lines verified: `src/multi_runtime.rs:419–458`.)

7. **Freeze-before-stop:** `Drop` calls `observe_layout()` while `frozen == false`, then sets `frozen = true` before any child-stop effect. (Lines verified: `src/multi_runtime.rs:411–413`.)

8. **Empty tombstone conditions:** `layout_terminal_empty` returns `true` only when a layout exists and every pane's recorded state is positively `Exited`. (Lines verified: `src/server/multi_session.rs:400–412`.)

9. **Read-only CLI isolation:** `layout_show` opens the store read-only, does not create directories, and dispatches before any server-start path. (Lines verified: `src/default_open.rs:1120–1148`, `src/main.rs:146–158`.)

10. **No ID/path leakage in errors:** All error paths in the read-only CLI and the store/codec use fixed labels; `Debug` impls are redacted. (Lines verified throughout `saved_layout.rs`, `layout_store.rs`, `default_open.rs`.)

11. **Singleton lock retention:** `claim_layout_writer` duplicates the held `flock` into `LayoutWriterGuard`; the worker thread drops it only on exit. Timeout does not release it. (Lines verified: `src/server_storage.rs:354–376`, `src/layout_writer.rs:477–516`.)

12. **Scope mismatch is sticky:** `LayoutWriter::observe` sets `unavailable = true` on generation/workspace mismatch and refuses all later observations. (Lines verified: `src/layout_writer.rs:186–189`.)

---

## Test Inventory

| Component | Tests | Status |
|---|---|---|
| `saved_layout.rs` codec | ~30 unit tests (roundtrip, validation, rejection, extremes, redaction) | Passing |
| `layout_store.rs` store + rotation | 25 unit tests (fault injection, rotation, cleanup, refusal) | Passing |
| `layout_writer.rs` writer | 17 unit tests (coalescing, retry, timeout, panic, scope, deferred) | Passing |
| `server_storage.rs` claim | 2 unit tests (exclusive claim, failed dup release) | Passing |
| `multi_runtime.rs` layout observer | **0 dedicated tests** | **Gap** |
| `default_open.rs` `layout_show` | **0 dedicated tests** | **Gap** |
| `main.rs` `layout` dispatch | **0 dedicated tests** | **Gap** |

---

## Recommendation

Do not modify production code for the minor findings until the **major test gap (MAJOR-1)** is addressed. The minor items are polish; the missing observer tests are the remaining acceptance gate for claiming this slice complete.

---

FINAL: DONE

The saved-layout slice is architecturally sound and fail-closed at every documented boundary: the 160-byte codec is strict, the two-slot store preserves corrupt data rather than overwriting it, the writer coalesces and bounds queue/memory without blocking the owner, and the runtime freeze-before-stop contract is correctly wired. The one significant remaining risk is the absence of dedicated tests for the `LayoutObserver` trigger matrix and the shared-shutdown deadline, which should be added before this phase is marked complete.
