# Incremental snapshot renderer result

Implemented `terminal::render::render_diff` in `src/terminal/render.rs` as a pure streaming update from the current sanitized `ScreenSnapshot` and an optional caller-owned previous snapshot. It validates the current viewport and snapshot shape before writing. A first frame or dimension change uses the existing full renderer; same-size updates never erase the screen. An exact snapshot repeat emits no bytes.

For stable dimensions, changed cells are positioned and rendered with the existing scalar, width, style, color, and control sanitization. If wide-cell flags change on a row, or either snapshot has malformed wide-cell topology, the renderer repaints that row to clear both old and new glyph footprints safely. Otherwise it emits only changed logical cells, including a wide lead when its continuation changed. Cursor-only updates emit only the cursor position and visibility controls. Cell updates hide a previously visible cursor, disable autowrap while painting, then restore the final cursor, wrapping, and visibility state. Partial writer errors propagate immediately; the caller remains responsible for host-mode restoration and should retain its previous snapshot only after the whole output frame is accepted by host writes.

The 17 focused tests include parser-derived replay of full-plus-diff output against a separate full render, comparing visible cell text, wide-cell flags, colors, styles, cursor position, and cursor visibility. They cover narrow updates, clearing and style changes, combining and matched wide Unicode, wide-to-narrow and narrow-to-wide topology changes, first frame, resize, no-op and cursor-only output, viewport rejection before writing, synthetic child-control sanitization, writer failure redaction, and maximum-size output through a counting sink. The maximum-size diff stays at or below the 4 MiB frame bound.

Verification used the locked dependency graph and offline cache:

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p2-delta-target cargo test --locked --offline --lib terminal::render
running 17 tests
test result: ok. 17 passed; 0 failed; 0 ignored; 30 filtered out
```

```text
$ cargo fmt --check
exit 0 (no output)
```

```text
$ CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-p2-delta-target cargo clippy --locked --offline --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
exit 0
```

No dependency, manifest, snapshot type, or host integration was changed. These parser tests verify emitted screen state, not Apple Terminal compatibility, host resize timing, or restoration after partial output. The pure renderer does not retain prior frames; the host caller owns snapshot caching and mode cleanup. Ownership of `src/terminal/render.rs` and this result file is released to the parent for integration review.

## Parent integration and verification — 2026-10-17

Parent reviewed the renderer and integrated it in `src/p2_probe.rs`. Two fixed-capacity sanitized snapshot caches are allocated before child spawn along with the existing 3 MiB byte frame. A pending target is copied without allocation; it becomes the displayed baseline only after the complete frame is accepted by nonblocking host writes. No blocking flush/drain is used. While output is pending, subsequent core changes remain dirty for a later diff. Resize/discard invalidates the baseline even if clamped core dimensions remain unchanged, forcing a full trusted repaint after the partially emitted frame. No input or child control bytes are replayed. The renderer resets SGR after cell updates.

A host-frame regression exercises partial writes, delayed baseline commit, invalidation after discarding a prefix, full repaint recovery and identical-frame suppression. A snapshot-cache regression checks maximum-size growth, shrink/regrowth without changing storage address, copying cursor/cell state and diagnostic redaction. Two caches cost at most 2,457,600 extra bytes under the existing tested cell-size ceiling. Core snapshot + two caches + fixed frame + decoder/paste + the existing 8 MiB launch allowance remain below the previously reserved 20 MiB shared allowance (under 15 MiB by those source ceilings). This is source accounting, not an RSS measurement.

The first full integration rerun exposed an existing immediate-exit fixture race in `pending_input_is_discarded_once_child_exit_is_observed`: the slave can close between `try_wait` and a queued write, producing a legitimate `write PTY` IO error before the next exit observation. The test now accepts only that named hard-write branch in addition to the normal report and asserts its input is discarded, child status collected and later input disabled. Other pump errors still fail. Production hard-write handling is unchanged; this does not turn that race branch into successful-input evidence.

Final parent checks with `CARGO_HOME=/private/tmp/panelink-p2-cargo-home`:

| Command | Actual result |
| --- | --- |
| `cargo test --locked --offline -q` | Exit 0: 48 library + 18 binary + 4 host fixture/preflight + 11 real-PTY = **81 passed**, **9 ignored**; 0 doctests. |
| `cargo fmt --check` | Exit 0, no output. |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | Exit 0. |
| `git diff --check` | Exit 0. |

The graph index was refreshed after source changes. No dependency, lockfile or vendored source changes were needed; the unchanged decoder suite was not rerun. No commits/pushes. All agent ownership is released. Parser replay and private-PTY tests support the correction; Apple Terminal behavior after this change remains unobserved. The user's mostly-successful qualitative manual report predates this fix and does not establish its visual quality. Remaining cleanup/restoration-timing checks are deferred at the user's request.
