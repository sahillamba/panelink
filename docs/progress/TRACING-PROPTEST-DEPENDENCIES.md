# Tracing/proptest dependency adoption review

Date: 2026-11-02. Research/review evidence only.

Parent follow-up (2026-11-02): the initial tool-availability blocker below is
resolved. Installed the previously used cargo-audit 0.22.2 into the disposable
`/tmp/panelink-audit-tool` prefix, then ran its `audit --json` against the
40-package lockfile: exit 0, zero known vulnerabilities and no warnings.
RustSec database commit `263a4d951a0867d4bddc29dddd6d59427c78f9a7`, 1,402
advisories. [Raw result](p6-raw/tracing-proptest-audit.json) is retained. This
scan does not audit source, vendored patches or firm acceptance. Historical
blocked statements below describe the worker's earlier environment.

Parent also regenerated the inventory with development edges included and
preserved license texts for 21 added crates; the lock-only r-efi notice gap
remains explicit in THIRD_PARTY_NOTICES.md.

Scope: review of the three proposed pins in [`Cargo.toml`](../../Cargo.toml) as
they resolve against the actual Panelink manifest, versus the git HEAD
lockfile. Owner of this file only; no source, manifest, lockfile or other
document was changed.

Baseline for comparison: `5ecc22a0c7674e5d2a9f60959b779fa8111f2317`
(`t3/investigate-two-suggestions`), whose `Cargo.lock` holds 18 packages.

This is a metadata/graph/license/build-input review. It is **not** a build
verification, test run, advisory audit, source audit or firm approval.

## What was reviewed

The working tree already carries the proposed manifest entries (parent-owned
edits, not mine):

```toml
[dependencies]
tracing = { version = "=0.1.44", default-features = false, features = ["std"] }
tracing-subscriber = { version = "=0.3.23", default-features = false, features = ["fmt"] }

[dev-dependencies]
proptest = { version = "=1.12.0", default-features = false, features = ["std"] }
```

These match the proposed entries in
[the adoption plan](../design/TRACING-PROPTEST-PLAN.md) (Candidate
versions/features section).

## Method and commands

Read-only resolution against the real manifest; the lockfile was not modified
by these commands (`git status` identical before and after):

| Command | Result |
| --- | --- |
| `cargo metadata --locked --format-version 1` | exit 0, 40 packages, `resolve` present |
| `cargo tree --locked --target aarch64-apple-darwin -e normal,build` | exit 0 |
| `cargo tree --locked --target aarch64-apple-darwin -e normal,build,dev` | exit 0 |
| `cargo tree --locked --target aarch64-apple-darwin -e dev` | exit 0 |
| `cargo tree --locked -e features -i <crate>` (per candidate) | exit 0 |
| `cargo audit --version` | **failed:** `error: no such command: "audit"` |
| `cargo deny --version` | **failed:** `no such command: "deny"` |

Toolchain observed: `cargo 1.99.1`, `rustc 1.99.1`, pinned by
[`rust-toolchain.toml`](../../rust-toolchain.toml) to channel `1.99.1`.

Not run, by task constraint: any build, any test run, dependency installation,
advisory database fetch, `scripts/dependency_inventory.py` (it writes
`docs/progress/P6-DEPENDENCY-INVENTORY.md` and `p6-raw/dependency-inventory.json`,
which this task does not own).

Note on tooling: the `codebase-memory` MCP knowledge graph was not available in
this session (no registered resources or graph tools), so structural questions
were answered with `grep`/`read` plus Cargo metadata instead.

## Lockfile delta versus git HEAD

| Measure | git HEAD | working tree |
| --- | --- | --- |
| `Cargo.lock` package entries | 18 | 40 |
| Added | – | 22 |
| Removed | – | 0 |
| Existing package version/source changes | – | 0 |

All 22 additions are `registry+https://github.com/rust-lang/crates.io-index`
entries with a `checksum` (SHA-256) line present; none is a git or path source.
No pre-existing package was bumped, removed or re-sourced, so the delta is
purely additive.

The 22 added packages:

`autocfg 1.5.1`, `cfg-if 1.0.5`, `chacha20 0.10.2`, `core_detect 1.0.0`,
`cpufeatures 0.3.1`, `getrandom 0.4.3`, `lazy_static 1.5.1`, `num-traits 0.2.19`,
`once_cell 1.21.4`, `pin-project-lite 0.2.17`, `proptest 1.12.0`, `r-efi 6.0.0`,
`rand 0.10.3`, `rand_core 0.10.1`, `rand_xorshift 0.5.0`, `regex-syntax 0.8.11`,
`sharded-slab 0.1.7`, `thread_local 1.1.10`, `tracing 0.1.44`,
`tracing-core 0.1.36`, `tracing-subscriber 0.3.23`, `unarray 0.1.4`.

This reconciles with the plan's standalone inventories: 9 packages for the
tracing pair (2 direct + 7 transitive) and 14 net-new from the 16-package
proptest inventory (`bitflags` and `libc` already existed), with `cfg-if`
shared by both sides — 9 + 14 − 1 = 22.

## Runtime (normal/build) graph on `aarch64-apple-darwin`

Non-root packages reachable through normal and build edges grow from **14 → 23**
(+9, ≈ +64%). The SEC-01 evidence in [the security review](../SECURITY-REVIEW.md)
records the 14-package figure for the 18-entry lockfile; it is now stale.

| New package | Version | Pulled by | License (declaration) | `rust-version` | Build script | Proc macro |
| --- | --- | --- | --- | --- | --- | --- |
| `cfg-if` | 1.0.5 | `thread_local` | MIT OR Apache-2.0 | 1.32 | no | no |
| `lazy_static` | 1.5.1 | `sharded-slab` | MIT OR Apache-2.0 | not declared | no | no |
| `once_cell` | 1.21.4 | `tracing-core` (`std` → `once_cell`) | MIT OR Apache-2.0 | 1.65 | no | no |
| `pin-project-lite` | 0.2.17 | `tracing` | Apache-2.0 OR MIT | 1.37 | no | no |
| `sharded-slab` | 0.1.7 | `tracing-subscriber` (`registry`) | MIT | 1.42.0 | no | no |
| `thread_local` | 1.1.10 | `tracing-subscriber` (`registry`) | MIT OR Apache-2.0 | 1.63 | no | no |
| `tracing` | 0.1.44 | `panelink` | MIT | 1.65.0 | no | no |
| `tracing-core` | 0.1.36 | `tracing`, `tracing-subscriber` | MIT | 1.65.0 | no | no |
| `tracing-subscriber` | 0.3.23 | `panelink` | MIT | 1.65.0 | no | no |

Runtime-side conclusion: **no build script, no build-dependency, no procedural
macro, no `links` key and no native/C code** among the nine added runtime
packages. This is a metadata plus inspected-manifest finding, not a compiled
result.

## Dev graph on `aarch64-apple-darwin`

Adding dev edges contributes 11 further packages that build only for tests:
`proptest 1.12.0`, `autocfg 1.5.1`, `chacha20 0.10.2`, `core_detect 1.0.0`,
`getrandom 0.4.3`, `num-traits 0.2.19`, `rand 0.10.3`, `rand_core 0.10.1`,
`rand_xorshift 0.5.0`, `regex-syntax 0.8.11`, `unarray 0.1.4`.
`cargo tree -e dev` shows exactly one dev edge: `panelink --dev--> proptest`;
proptest does not reach the runtime graph.

| New dev package | Version | License (declaration) | `rust-version` | Build script | Proc macro |
| --- | --- | --- | --- | --- | --- |
| `proptest` | 1.12.0 | MIT OR Apache-2.0 | 1.88 | no | no |
| `autocfg` | 1.5.1 (build-dependency of `num-traits`) | Apache-2.0 OR MIT | 1.0 | no | no |
| `chacha20` | 0.10.2 | MIT OR Apache-2.0 | 1.85 | no | no |
| `core_detect` | 1.0.0 | MIT/Apache-2.0 | not declared | no | no |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | 1.85 | **yes** | no |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | 1.60 | **yes** | no |
| `rand` | 0.10.3 | MIT OR Apache-2.0 | 1.85 | no | no |
| `rand_core` | 0.10.1 | MIT OR Apache-2.0 | 1.85 | no | no |
| `rand_xorshift` | 0.5.0 | MIT OR Apache-2.0 | 1.85 | no | no |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | 1.65 | no | no |
| `unarray` | 0.1.4 | MIT OR Apache-2.0 | not declared | no | no |

The only build scripts anywhere in the 22-package delta are dev-side:

- `getrandom-0.4.3/build.rs` (11 lines): reads `CARGO_CFG_SANITIZE` and emits
  `cargo:rustc-cfg=getrandom_msan`. No `cc`, no linker flags, no external tool.
- `num-traits-0.2.19/build.rs` (7 lines): uses its `autocfg` build-dependency
  to probe `1f64.total_cmp(&2f64)` and re-run paths. Compiler probe only.

No package in the delta declares `links`, so no native library is advertised to
the linker. `cpufeatures` (which would normally be the native-adjacent crate)
is not even compiled on this target — see below.

## Packages present in the lockfile but not built on Apple Silicon

Two of the 22 do not appear in the target-filtered tree for
`aarch64-apple-darwin`; they are resolved for other targets only:

| Package | Version | Gating edge | License (declaration) | `rust-version` |
| --- | --- | --- | --- | --- |
| `cpufeatures` | 0.3.1 | `chacha20` under `cfg(any(target_arch = "x86_64", target_arch = "x86"))` | MIT OR Apache-2.0 | 1.85 |
| `r-efi` | 6.0.0 | `getrandom` under `cfg(all(target_os = "uefi", getrandom_backend = "efi_rng"))` | MIT OR Apache-2.0 OR LGPL-2.1-or-later | 1.68 |

9 (runtime) + 11 (dev) + 2 (other-target) = 22. `r-efi` is the one package in
the delta whose license expression contains an LGPL alternative and whose
packaged directory ships **no license file at all** (only `AUTHORS`, `README`,
`NEWS.md`, `src/`); this is recorded under Findings.

## Exact activated features

Activated sets come from `cargo metadata` `resolve.nodes[].features` and are
confirmed per-target with `cargo tree -e features`:

- `tracing 0.1.44` → `std` only. `attributes` (which would pull the
  `tracing-attributes` procedural macro), `log`, `valuable`, `async-await` and
  all `max_level_*` filters are **not** activated. `tracing-attributes` is not
  in the lockfile at all.
- `tracing-subscriber 0.3.23` → `fmt`, `registry`, `std`, `alloc`,
  `sharded-slab`, `thread_local`. **Not** activated: `ansi`, `env-filter`,
  `json`, `chrono`/`time`/`local-time`, `regex`/`matchers`, `serde`,
  `tracing-log`, `valuable`, `parking_lot`, `smallvec`.
  Upstream manifest (0.3.23) shows `fmt = ["registry", "std"]` and
  `registry = ["sharded-slab", "thread_local", "std"]`, so with `fmt`-only the
  slab/thread-local registry pair is unavoidable; there is no "fmt without
  registry" feature in this release. `default = ["smallvec", "fmt", "ansi",
  "tracing-log", "std"]` is disabled, which is what keeps ANSI escapes and
  `tracing-log` bridging out of the graph.
- `proptest 1.12.0` → `std`, `regex-syntax`. **Not** activated: `default`
  (`fork`, `timeout`, `bit-set`), `attr-macro`/`proptest-macro`, `tempfile`,
  `rusty-fork`, `handle-panics`, `unstable`, `f16`, `no_std`.
  Upstream manifest shows `std = ["rand/std", "rand/sys_rng", "regex-syntax",
  "num-traits/std"]`, so `std` necessarily enables `regex-syntax` (the plan
  already noted this), and `regex-syntax` resolves with its own `default`
  feature set (`std`, `unicode` and all `unicode-*` tables) — a dev-only
  compile-cost item that has not been measured.

## MSRV and edition (metadata only)

Highest declared `rust-version` in the delta is **1.88** (`proptest`), then
**1.85** for the edition-2024 crates (`chacha20`, `cpufeatures`, `getrandom`,
`rand`, `rand_core`, `rand_xorshift`). All declared values are below the pinned
toolchain 1.99.1. `core_detect`, `lazy_static` and `unarray` declare **no**
`rust-version` (and `autocfg`/`lazy_static` declare no `edition`, i.e. 2015
default), so metadata compatibility is not established for those three.

This confirms the plan's "tracing pair declares Rust 1.65 and MIT;
proptest declares Rust 1.88 and MIT OR Apache-2.0" statement against the
actually resolved manifests. It remains **metadata compatibility, not build
verification** — no candidate was compiled in this review.

## License files on disk

Checked in the extracted registry sources (`~/.cargo/registry/src/
index.crates.io-1949cf8c6b5b557f/`):

- Single `LICENSE` (MIT text, "Copyright (c) 2019 Tokio Contributors"):
  `tracing`, `tracing-core`, `tracing-subscriber`.
- Single `LICENSE` (MIT text, "Copyright (c) 2019 Eliza Weisman"):
  `sharded-slab`.
- `LICENSE-APACHE` + `LICENSE-MIT` present: `autocfg`, `cfg-if`, `chacha20`,
  `core_detect`, `cpufeatures`, `getrandom`, `lazy_static`, `num-traits`,
  `once_cell`, `pin-project-lite`, `proptest`, `regex-syntax`, `thread_local`,
  `unarray`; plus `COPYRIGHT`: `rand`, `rand_core`, `rand_xorshift`.
- **No license file in the packaged source:** `r-efi 6.0.0`.

Every `license_file` field in Cargo metadata is absent (these crates use
`license` SPDX expressions, not `license-file`), so the file listing above is
the direct on-disk check rather than a metadata field.

Declaration-versus-file consistency was spot-checked for the MIT-only tracing
crates and `proptest`; a line-by-line license text audit of all 22 packages was
**not** performed.

## Advisory scan: blocked, not run

`docs/RELEASE.md` ("Rerun audit") names the approved procedure: install/use the
approved `cargo-audit` in the review environment and run `cargo audit --json`,
recording tool/database versions, date, counts and dispositions, and never
reporting stale or unavailable results as a clean audit.

Neither `cargo-audit` nor `cargo-deny` is installed here, and installing tools
was excluded from this task. The existing `scripts/dependency_inventory.py`
records exactly the same state (`advisory_scan.status = "blocked"`, with a
`shutil.which` probe and the note that no advisory database was used).

Consequently: **no advisory result exists for these 22 packages.** The dated
scan cited by SEC-01 (cargo-audit 0.22.2, 2026-10-21, RustSec database commit
`4b587f00996ebe5c05a73eaef105ef241e876414`, 0 vulnerabilities across 1,348
advisories, against the 18-entry lockfile) predates this change and does not
cover it. A fresh dated scan is a pending gate, not a passed check.

## Build cost: not measured

No clean or incremental compilation, release build, binary-size or test-duration
measurement was performed (builds and tests were out of scope). Therefore there
is **no evidence** yet for compile-time, artifact-size or CI-duration impact of
the +9 runtime packages or the +11 dev packages. The plan's Packet 1 step 3
asks for exactly this and also cautions that the handoff's near-420-second Rust
checks and pre-existing Python bridge errors must be re-established rather than
assumed. CI's cold path (`cargo fetch --locked`, then
`cargo test --locked --offline --no-run` in
[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)) and
`scripts/measure_build.py` are the natural measurement entry points; running
them is parent work.

## Findings

1. **Advisory gate is open.** No `cargo audit` result exists for the new
   packages; the tool is absent and installation was out of scope. SEC-01
   evidence therefore cannot be refreshed by this task.
2. **`r-efi 6.0.0` license handling.** Its expression
   `MIT OR Apache-2.0 OR LGPL-2.1-or-later` contains `LGPL-2.1-or-later`, which
   is not in `scripts/dependency_inventory.py`'s `ALLOWED_LICENSES` set, so the
   inventory's `license_flags()` would emit
   `non-permissive-or-unreviewed-license:LGPL-2.1-or-later`; its packaged source
   also ships no license file. The crate is not compiled on Apple Silicon
   (UEFI-only edge) but it is in the all-target lockfile that the inventory and
   notices review see. Needs an explicit disposition, not silent acceptance.
3. **The inventory script does not classify dev-dependencies.**
   `_dependency_edges()` and `direct_dependency_roots()` keep only
   `None`/`normal`/`build` kinds, so if the inventory were regenerated,
   `proptest` would appear with `role = transitive` and an empty `pulled_by`
   rather than as a direct dev-dependency, and its advisory section is a
   hardcoded "blocked" note. Regenerating that report is outside this task's
   file ownership.
4. **Notices and SEC-01 evidence are stale.** `THIRD_PARTY_NOTICES.md` still
   describes 17 components and none of the 22 new packages; the security
   review's 18-entry-lock audit paragraph and "14 non-root packages" figure no
   longer match. Both are parent-owned updates under plan Packet 1 step 4.
5. **The runtime graph is currently unused.** No `tracing` or `proptest`
   reference exists under `src/` or `vendor/*/src`, so every release build
   would carry 9 extra packages before any instrumentation lands. Splitting the
   two adoptions (plan Packet 1 exit) keeps this revertible.
6. **`fmt`-only still costs a global registry.** `sharded-slab` + `thread_local`
   come with `fmt` by upstream feature design; that is in-process registry
   storage, not file or network I/O, but it should be understood before Packet 3
   rather than treated as an accidental transitive.
7. **Test-time filesystem and compiler-probe side effects (dev only).** proptest
   defaults to `FileFailurePersistence::SourceParallel("proptest-regressions")`
   (upstream `config.rs`), writing regression seed files next to the source on
   failure — consistent with the plan's instruction to commit
   `proptest-regressions`. `getrandom` and `num-traits` build scripts run only
   when the dev graph is compiled; `getrandom` on macOS selects the
   `getentropy(2)` backend (a libc call), not a network or file source.
8. **No native/proc-macro surprise otherwise.** No `links`, no `cc`, no
   procedural macro and no build-dependency on the runtime side; the two build
   scripts are plain Rust cfg/expression probes.

## Status of gates

| Gate | State |
| --- | --- |
| Resolved normal/dev/target graph vs HEAD lock | **Reviewed** (this document) |
| License declarations + license-file presence | **Reviewed** for the 22; full text/obligation audit **not** performed |
| Build scripts / proc macros / native inputs | **Reviewed** from manifests and packaged sources; not executed |
| MSRV metadata vs pinned 1.99.1 | **Reviewed**; 3 packages declare no `rust-version` |
| Advisory scan (`cargo audit --json`) | **Blocked / not run** — tool not installed |
| Build/test cost measurement | **Not measured** — builds and tests out of scope |
| Notices + dependency inventory update | **Pending**, parent-owned |
| Firm security criteria and acceptance | **Pending**, unresolved as stated in the plan and SECURITY-REVIEW |

Nothing in this document constitutes a security audit, a compliance stamp, a
build result or approval to adopt the dependencies. The graph, license, MSRV
and build-input observations above are scoped evidence for the review the plan
requires; SEC-01/SEC-08 remain partial.

## Sources

- Proposed versions/features and open questions:
  [`docs/design/TRACING-PROPTEST-PLAN.md`](../design/TRACING-PROPTEST-PLAN.md)
- Requirement context: [`docs/SECURITY-REVIEW.md`](../SECURITY-REVIEW.md)
  (SEC-01 complete-graph/build-input/advisory requirement; SEC-08 dated-scan
  limitation), [`docs/SPEC.md`](../SPEC.md) (proposed security review
  requirements)
- Notice inventory: [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md)
- Approved audit procedure: [`docs/RELEASE.md`](../RELEASE.md)
- Inventory tooling and license allowlist:
  [`scripts/dependency_inventory.py`](../../scripts/dependency_inventory.py),
  doc hygiene checker [`scripts/check_docs.py`](../../scripts/check_docs.py),
  CI cold path [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)
- Resolved graph: [`Cargo.toml`](../../Cargo.toml), [`Cargo.lock`](../../Cargo.lock),
  `cargo metadata --locked`, `cargo tree --locked --target aarch64-apple-darwin`
- Upstream manifests (primary):
  [tracing 0.1.44](https://docs.rs/crate/tracing/0.1.44/source/Cargo.toml),
  [tracing-subscriber 0.3.23](https://docs.rs/crate/tracing-subscriber/0.3.23/source/Cargo.toml),
  [proptest 1.12.0](https://raw.githubusercontent.com/proptest-rs/proptest/v1.12.0/proptest/Cargo.toml),
  inspected locally in
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{tracing-0.1.44,tracing-core-0.1.36,tracing-subscriber-0.3.23,proptest-1.12.0,getrandom-0.4.3,num-traits-0.2.19,once_cell-1.21.4}/`
