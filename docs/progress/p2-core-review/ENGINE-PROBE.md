# Disposable vt100 0.16.2 feasibility probe

## Result

The patched `vt100` 0.16.2 source is a viable bounded core-engine candidate for the Panelink phase-2 experiment under the experiment's existing dimension and event limits, with the normal-history cap reduced from 2,000 to 1,024 rows. The candidate is **not approved by this probe**: this does not complete dependency/license/advisory/build review, PTY backend review, application adapter review, or compatibility testing. It establishes a source-backed path to those checks without introducing a second parser.

The corrected storage model budgets 1,024 cell slots per row. This is higher than the 956-cell capacity observed for the adversarial resize/insert operation on the current Rust toolchain. The 64 MiB estimate is conditional on the wrapper enforcing the documented dimensions/history/input/event/snapshot caps, using at most two 10 MiB snapshot allocations during replacement, and rechecking standard-container capacity behavior when the Rust toolchain changes. `Vec` and `VecDeque` do not promise a portable maximum excess capacity. This probe did not read vt100's private row capacities directly or measure RSS.

## Source provenance and patch

Source reviewed: `/private/tmp/panelink-p2-cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/vt100-0.16.2`. Code discovery used the indexed vt100 0.16.2 and vte 0.15.0 graph projects before source inspection. The crate archive SHA-256 is `054ff75fb8fa83e609e685106df4faeffdf3a735d3c74ebce97ec557d5d36fd9`; `.cargo_vcs_info.json` records upstream commit `eb66ffaf7d771c13303ef73b29f6f2a56fdacecf`. The copied crate retains its MIT `LICENSE` and Jesse Luehrs copyright notice.

The test-only patched copy is `/private/tmp/panelink-vt100-patched/vt100-0.16.2`; the probe harness is `/private/tmp/panelink-vt100-probe`. The source diff is limited to four crate files:

1. `Cargo.toml:54-56`: disable VTE default features. This removes `vte/std`, selecting its `ArrayVec<u8, 1024>` OSC backing rather than its growable `Vec<u8>`.
2. `src/cell.rs:4-5, 56-73`: cap each cell at eight extra Unicode scalars after its base. A leading combining scalar creates a virtual blank base and still receives no more than eight extras. The original fixed 22-byte content array remains the additional byte limit; `Cell` remains 32 bytes.
3. `src/grid.rs:501-524`: clamp CSI insert-cell count to the remaining columns before the insertion loop, preventing transient row length/capacity growth far beyond the configured width. `src/grid.rs:545-588`: also clamp insert-lines to remaining screen rows and scroll-down to active scroll-region height. These keep row-vector insertion loops within their affected bounds; ordinary bounded sequences retain their state.
4. `src/perform.rs:87-96`: return immediately for VTE's ignored/malformed CSI flag, before screen dispatch or `unhandled_csi` callbacks.

No parser was added. No application source, manifest, lockfile, roadmap, or progress document was edited, and no commit was made.

## Source review and bounded-state reasoning

- VTE 0.15.0 `src/lib.rs:44-69` defines fixed intermediate/OSC parameter arrays and, without `std`, `osc_raw: ArrayVec<u8, OSC_RAW_BUF_SIZE>`; `MAX_OSC_RAW` is 1,024 at lines 44-46. `src/lib.rs:544-558` stops appending when the fixed array fills, then still calls OSC dispatch at string termination. VTE `src/params.rs:5-26` stores at most 32 CSI parameter/subparameter values in fixed arrays.
- The long-OSC test sends more than 20 KiB over five separate 4 KiB feeds. The callback receives exactly two params with a 1,021-byte payload; subsequent `OK` text is parsed. This demonstrates bounded retained OSC input, **not rejection of the oversized command**. The engine dispatches the truncated prefix. A truncated title or clipboard write can therefore still reach a callback. Runtime callbacks must synchronously discard unsupported content and must never persist/queue the raw payload. The probe intentionally records only lengths.
- `src/cell.rs:56-80` has both the original byte cap and the new scalar cap. The flood test exercises 40 combining marks after a real base and after the virtual blank base, then inspects actual screen-cell contents: each retains one base plus exactly eight extras.
- `src/row.rs:4-14, 44-47` stores row cells in `Vec<Cell>` and exposes insert/resize operations. `src/grid.rs:8-15, 66-82` stores active rows in `Vec<Row>` and resizes their cell vectors. `src/screen.rs:53-84` holds normal and alternate grids; the alternate grid is constructed with zero scrollback. `src/grid.rs:562-575` pushes normal rows into a `VecDeque` and pops when over its configured history limit. No other parser-owned variable-length byte/character accumulator was found in this source path; callback/output/snapshot allocations remain the wrapper's responsibility.
- Resizing from width 239 to 240 and then inserting 240 cells at column zero grows a row's vector capacity beyond the simpler one-shot-width case. The supplemental capacity example performs those same `Vec<Cell>` operations and observed capacity 956 for a final length of 240 under Rust 1.99.1. For the budget, each row is rounded up to 1,024 cell slots. The same model observes an 80-row `Vec<RowLike>` grow to 160 slots after a transient insert/remove, and a 1,024-history `VecDeque` grow to 2,048 slots after the push-over-limit/pop sequence.
- `Screen::state_formatted` and `contents_formatted` (`src/screen.rs:224-258`) allocate output `Vec<u8>` values. A generous 256-byte-per-visible-cell output allowance gives 4,915,200 bytes for an 80×240 snapshot; the estimate allows 10 MiB per snapshot to cover vector capacity growth. The total below counts two such allocations so replacement may overlap an old pending frame and a newly built one. The later adapter must enforce a single pending frame and avoid retaining an additional formatted copy at the same time.
- `src/perform.rs:97-105` constructs a temporary CSI-parameter `Vec` for unhandled callbacks; VTE's fixed 32-value parameter array bounds that input. Probe callback vectors intentionally collect test evidence and are not a runtime callback design. The runtime should classify synchronously, drop unsupported events before copying, and only retain events within the experiment's 64-event × 16-KiB cap.
- A resize callback can be driven by child CSI 8 t and `Screen::set_size` accepts dimensions directly. The engine does not enforce Panelink's 5×20 through 80×240 limit itself; the PTY/session wrapper must validate every accepted resize before changing engine dimensions.
- `vt100` maps unhandled query forms to `Callbacks::unhandled_csi`; the tested forms arrive as `(intermediates=None, params=[[0]], final='c')`, `[[5]]/'n'`, `[[6]]/'n'`, and `[[18]]/'t'`. VTE represents omitted primary-device-attributes params as zero, so the callback cannot distinguish raw `CSI c` from `CSI 0 c`; the probe treats this semantic default form as the baseline query. The engine emits no automatic reply for the four forms. The wrapper must synthesize only the experiment's exact child-PTY replies; it must not write to the host.
- The OSC52 probe drops child writes and clears and records no clipboard content. For a strict selector `c` read, it proposes only `ESC ] 52 ; c ; BEL` as an empty response to the child. Other selectors (including `p`) receive no response. No host clipboard API, PTY, host terminal, shell wrapper, or child process was used.
- Screen tests cover normal and alternate grid content, resize, and the selected 1,024 history cap. The source's `TERM`/alternate-character-set behavior was not validated end-to-end: the parent is using exact `TERM=panelink-probe` and a minimal custom terminfo for later full-screen fixtures. This probe proves neither terminfo compatibility nor Apple Terminal behavior.

### Conservative 64 MiB budget

Configuration used for this feasibility variant: maximum 80×240 cells; normal history 1,024 rows; alternate history zero; cell capacity budget 1,024 per row; callback queue, input, and snapshot sizes as in `P2-EXPERIMENT.md:68-75`. The history cap is an explicit experiment reduction from 2,000, not a product-level decision.

| Component | Budget (bytes) | Derivation |
| --- | ---: | --- |
| Normal scrollback cell backing | 33,554,432 | 1,024 rows × 1,024 cell slots × 32 bytes |
| Normal + alternate active cell backing | 5,242,880 | 160 rows × 1,024 slots × 32 bytes |
| Scrollback `VecDeque` row slots | 65,536 | 2,048 observed slots × 32-byte row-slot model |
| Active `Vec<Row>` slots | 10,240 | 2 grids × 160 observed slots × 32-byte row-slot model |
| One transient row during scrolling/resize | 32,768 | 1,024 cell slots × 32 bytes |
| Parser objects, other fixed state, allocator margin | 2,097,152 | 2 MiB allowance; measured `Parser<()>` struct is 1,648 bytes, excluding its separately allocated rows |
| Two overlapping visible snapshots | 20,971,520 | 2 × 10 MiB; 256 bytes/cell serialized-output envelope rounded for capacity growth |
| Optional callback event queue | 1,048,576 | 64 events × 16 KiB |
| Bounded input buffer | 65,536 | 64 KiB |
| **Estimated total** | **63,088,640 (60.17 MiB)** | 3.83 MiB below 64 MiB |

The capacity example is an operation-model check using the same standard-library `Vec<Cell>` and `VecDeque` operations; it is not private-engine introspection, allocator instrumentation, or RSS evidence. The 1,024-slot per-row ceiling and 2× container-slot budgets are conservative for the tested Rust 1.99.1 behavior. Because standard `Vec` capacity excess is not a cross-toolchain contract, this remains a toolchain-conditional source/capacity estimate, not a universal Rust allocation theorem. The wrapper must keep dimensions/history/queues/snapshots within the stated limits, and the cap probe should be repeated on a toolchain update.

## Probe tests and actual verification output

The harness tests engine outputs through `vt100::Parser` and `Screen` APIs rather than reimplementing parser behavior. It covers: >20 KiB OSC across chunks, bounded combining marks, exact callback query forms/no auto-replies, ignored malformed CSI, dropped clipboard writes/clears and strict empty-read response, normal/alternate grid plus resize, history cap 1,024, and oversized insert-cell/insert-lines/scroll-down sequences against their bounded equivalents.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check --manifest-path /private/tmp/panelink-vt100-probe/Cargo.toml
```

Actual output: exit 0, no output.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-vt100-probe/target cargo test --locked --offline --manifest-path /private/tmp/panelink-vt100-probe/Cargo.toml
```

Actual output:

```text
   Compiling vt100 v0.16.2 (/private/tmp/panelink-vt100-patched/vt100-0.16.2)
   Compiling panelink-vt100-probe v0.1.0 (/private/tmp/panelink-vt100-probe)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.46s
     Running unittests src/lib.rs (/private/tmp/panelink-vt100-probe/target/debug/deps/panelink_vt100_probe-d0bc7f321a4578c3)

running 9 tests
test tests::combining_flood_is_limited_with_and_without_a_real_base ... ok
test tests::clipboard_callbacks_drop_writes_and_return_only_an_empty_child_read ... ok
test tests::oversized_insert_is_clamped_to_the_remaining_row_width ... ok
test tests::normal_and_alternate_grids_survive_resize_and_history_stays_capped ... ok
test tests::only_the_four_exact_safe_query_forms_are_classified_without_auto_replies ... ok
test tests::huge_line_insert_and_scroll_down_counts_match_their_row_bounds ... ok
test tests::long_osc_is_bounded_across_separate_feeds ... ok
test tests::maximum_width_insert_after_resize_leaves_a_bounded_blank_row ... ok
test tests::experiment_history_cap_is_enforced_at_1024_rows ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

   Doc-tests panelink_vt100_probe

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The formatter and test commands exited 0. The probe resolved vt100 0.16.2, vte 0.15.0 without the `std` feature, arrayvec 0.7.8, memchr 2.8.3, itoa 1.0.18, and unicode-width 0.2.2. Rust/Cargo were `1.99.1 (58cf8f9c3 2026-10-13)` / `1.99.1 (5f94df478 2026-08-27)`.

Capacity command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home CARGO_TARGET_DIR=/private/tmp/panelink-vt100-probe/target cargo run --locked --offline --manifest-path /private/tmp/panelink-vt100-probe/Cargo.toml --example capacity
```

Actual output:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.00s
     Running `/private/tmp/panelink-vt100-probe/target/debug/examples/capacity`
cell_size=32 resized_and_inserted_row_len=240 resized_and_inserted_row_capacity=956 row_bytes=30592
parser_struct_bytes=1648 row_slot_bytes=32 active_cell_capacity_sum=76480 active_cell_bytes=2447360
active_rows_before_transient_insert_capacity=80 active_rows_after_transient_insert_capacity=160 active_slots_bytes=5120
history_cap=1024 history_len=1024 history_capacity=2048 history_cell_capacity_sum=978944 history_cells_bytes=31326208 history_row_slots_bytes=65536
```

## Remaining gates / gotchas

- The no-std VTE path truncates oversized OSC rather than flagging or fully rejecting it. A truncated sequence can still dispatch a callback; only callback-side drop/no-copy behavior makes that safe for the experiment.
- This probe does not establish a hard portable maximum for `Vec`/`VecDeque` overcapacity. The 60.17 MiB estimate is an explicit toolchain/cap-policy budget with 3.83 MiB headroom. Re-run the adversarial capacity example when rustc changes; do not turn the observed result into an API guarantee.
- The budget assumes the application validates every resize before calling `Screen::set_size`, keeps the configured 1,024 history bound, caps all intermediate queues, and avoids extra simultaneous screen copies beyond the two snapshot allowance. None of those wrapper behaviors are implemented here.
- No complete dependency graph policy/advisory/source/build/licensing review was performed. The MIT notice is present, but this report is not a full third-party-component approval.
- No PTY spawn/cleanup/input/resize path, child fixture, host terminal rendering, clipboard API, terminfo, Apple Terminal, full-screen application, or OpenCode behavior was run. Existing phase-2 PTY and application integration gates remain separate.
