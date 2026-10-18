# Phase 2 signal flag source review

Review date: 2026-10-17
Scope: cached `signal-hook` 0.4.5 with default features disabled and transitive `signal-hook-registry` 1.4.8, for a local macOS probe shutdown request only. This is source/API review, not a security audit or dependency adoption decision.

## Finding

The safe flag API is suitable for turning `SIGTERM`, `SIGHUP`, and `SIGINT` into a latched shutdown request without Panelink signal FFI, an iterator, self-pipe, or thread. It does not perform cleanup or wake a blocked host loop. The host loop must poll the flag between bounded, nonblocking I/O steps.

The API does **not** provide bounded unregister. `low_level::unregister(id)` waits for active signal-handler readers through a writer-side spin barrier, which has no timeout. It also does not restore the previous or default signal disposition after the final action is removed. Therefore the library can support bounded child cleanup, but a strict bound on both unregister and process return is not established. Do not put unregister in a destructor that is expected to meet the child-cleanup deadline. If the probe process exits after its cleanup, process termination ends the registrations; if the host process continues, signal-handler ownership needs an explicit process-wide lifecycle decision.

## Safe flag API and behavior

The exact API is:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::{flag, low_level};

let requested = Arc::new(AtomicBool::new(false));
let term_id = flag::register(SIGTERM, Arc::clone(&requested))?;
let hup_id = flag::register(SIGHUP, Arc::clone(&requested))?;
let int_id = flag::register(SIGINT, Arc::clone(&requested))?;
// Between bounded loop steps:
let shutting_down = requested.load(Ordering::Relaxed);
// Retain each ID. If lifecycle policy explicitly requires unregister:
for id in [int_id, hup_id, term_id] {
    let _removed = low_level::unregister(id);
}
```

`signal_hook::flag::register` returns `Result<SigId, std::io::Error>` and contains the unsafe registry call internally. Its registered action only does `flag.store(true, Ordering::SeqCst)`; relaxed reads are permitted for a simple latched boolean. The three signals share one flag, so their identities are intentionally collapsed, and repeated standard signals may coalesce. `SIGTERM`, `SIGHUP`, and `SIGINT` are available through the crate's Unix signal-constant re-exports and are not in its forbidden-signal list. Registration replaces the default disposition with the library dispatcher; the application must observe the flag and exit through its cleanup path.

The flag module explicitly documents polling semantics: it does not block until a signal arrives. A signal received while the host path is blocked on unbounded input or I/O can therefore be acted on late. Check the flag at each bounded host-loop iteration and before entering raw mode or starting the child. A second termination signal only stores `true` again; it does not force termination or accelerate cleanup.

The closure retains only an `Arc<AtomicBool>`. It does not capture terminal input, environment values, or log data. The registry itself is process-global: on the first registration for a signal it installs a dispatcher and saves the prior disposition. If a prior custom handler exists, the dispatcher calls it before the flag action, so an unrelated prior handler can delay or alter delivery. Signal-hook recommends registering early, before additional threads or concurrent signal-handler changes. A one-shot probe should register before host terminal state changes and must not assume it owns the process's only signal handler.

## Registration errors and teardown

Store each successful `SigId` as it is returned. If a later registration fails, successful earlier IDs can be passed individually to the safe `signal_hook::low_level::unregister(id)` API. However, the registry documentation says an error does not guarantee whether an action was registered, and the failed call returns no ID. Stop startup and exit before raw mode or child creation on a registration error; do not continue in a partially configured process.

Unregister is id-specific and returns `true` when it removed the action, `false` if the action was already absent. The implementation clones and swaps registry state, then waits until old signal-handler readers leave. Its `HalfLock::write_barrier` is an active spin loop; no upper time bound is exposed. A signal handler that is still running, including a chained prior handler, can delay unregister. Removing the last action also leaves the installed dispatcher in place and does not restore the default handler. After unregister, a previous custom handler continues to be chained, while the previous default disposition is not resumed. This is acceptable only when the process is terminating or when the broader process deliberately owns that disposition. It is unsuitable as a bounded cleanup step in a process that must return within a hard deadline.

## Source and API evidence

Codebase-memory was used first: the downloaded crate projects were indexed, then `flag::register`, `SigId`, and registry `unregister` symbols were searched and retrieved before reading the source files.

- `signal-hook-0.4.5/src/flag.rs:148-159` defines the safe `Arc<AtomicBool>` registration and its `SeqCst` store. `src/lib.rs:352-362` shows `flag`, `low_level`, and `consts` are available without the iterator feature; `src/lib.rs:370-375` re-exports the Unix `SIGHUP`, `SIGINT`, and `SIGTERM` constants.
- `signal-hook-registry-1.4.8/src/lib.rs:111-123` describes the global handler registry and its reader/writer design. Lines 341-428 show the process-global handler dispatches the saved prior handler before registered actions. Lines 444-456 list forbidden signals; lines 531-549 document first-registration interaction and the recommendation to register before threads/concurrent registration.
- `signal-hook-registry-1.4.8/src/lib.rs:493-495` says an error has no guarantee whether the action was registered. Lines 644-683 show registration and handler installation. Lines 686-715 define ID-specific unregister and state that the prior/default disposition is not restored. `src/half_lock.rs:73-87,165-192` shows unregister's writer swap and unbounded spin barrier.
- `signal-hook-0.4.5/src/low_level/mod.rs:20` re-exports the safe unregister function. The high-level flag action is the only signal callback needed here; do not use the unsafe low-level register API.
- The cached `signal-hook-0.4.5` source also documents that flag handlers are polled, not waitable, in `src/flag.rs:8-19` and `src/lib.rs:188-207`, and that standard signals can be coalesced in `src/lib.rs:117-120`.

## Features, dependency graph, and build inputs

The proposed manifest entry is `signal-hook = { version = "=0.4.5", default-features = false }`. Its default features are `channel` and `iterator`; both stay off, as does `extended-siginfo`. The flag and constants modules remain available. The feature definitions and optional C build path are in `signal-hook-0.4.5/Cargo.toml:59-71` and `build.rs:1-12`; `cc` and `src/low_level/extract.c` are used only with `extended-siginfo-raw`, so that build path is not selected here. The registry crate has no build script and depends on `errno` and `libc` (`signal-hook-registry-1.4.8/Cargo.toml:34-55`).

Offline Cargo resolution for this API-only probe produced this active graph:

```text
signal-hook 0.4.5
├── libc 0.2.191
└── signal-hook-registry 1.4.8
    ├── libc 0.2.191
    └── errno 0.3.14
        └── libc 0.2.191
```

The target-only lock entries also include `windows-sys 0.61.2` and `windows-link 0.2.1` through `errno`. Panelink's current `Cargo.lock` already contains `errno 0.3.14` and `libc 0.2.191` through the reviewed `rustix`/`rustix-openpty` graph, as well as those Windows support packages. The proposed dependency therefore adds only `signal-hook 0.4.5` and `signal-hook-registry 1.4.8`; it does not introduce a second `errno` or `libc` version. The current Panelink lockfile did not yet contain either signal-hook package at review time.

The offline API probe used Rust 1.99.1 (`aarch64-apple-darwin`) and compiled with `cargo check --offline`. Cargo reported builds/checks for `libc 0.2.191`, `errno 0.3.14`, `signal-hook-registry 1.4.8`, and `signal-hook 0.4.5`; no `cc` package or C build was selected. This verifies the API types/features compile on the current Apple Silicon toolchain only; no signal delivery, restoration, unregister timing, or host terminal behavior was tested.

## Source identity, license, and maintenance notes

The source was read from `/private/tmp/panelink-p2-cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/` and the cached crate archives were hashed. Cargo's generated offline lockfile checksums matched the archive SHA-256 values:

| Crate | Version | SHA-256 |
| --- | --- | --- |
| `signal-hook` | 0.4.5 | `b29ffa27d3d59a049bb328258de100f8012848d1b08d444e08bbc02ac468435e` |
| `signal-hook-registry` | 1.4.8 | `c4db69cba1110affc0e9f7bcd48bbf87b3f4fc7c61fc9155afd4c469eb3d6c1b` |

Both manifests declare `MIT OR Apache-2.0`, and both source packages include `LICENSE-MIT` and `LICENSE-APACHE`. The project can select the MIT option and preserve the corresponding notice; the parent owns the final notice and advisory checks. `signal-hook` declares Rust 1.66 and the registry Rust 1.26, both below Panelink's current Rust 1.99.1. The cached manifests label maintenance as `actively-developed`; the upstream [signal-hook repository](https://github.com/vorner/signal-hook) is public and has visible project history, but this metadata and page activity do not guarantee maintenance of these exact pins. No advisory database or full security audit was run.

## Verification record

- **Completed:** Graph-first symbol discovery; direct source and manifest review; dependency resolution and checksum comparison; API-only `cargo check --offline` on Apple Silicon macOS.
- **Not run:** Unit/integration tests, runtime signal-delivery checks, prior-handler interaction checks, bounded-unregister measurements, terminal restoration, Apple Terminal compatibility, advisory scan.
- **Decision boundary:** This evidence supports the atomic-flag registration API only. Parent chooses whether to adopt it and owns integration, notice/advisory checks, and the process-wide registration lifetime. It does not claim that unregister or process return is bounded.

## Parent adoption decision

Parent approves `signal-hook = { version = "=0.4.5", default-features = false }` for the macOS/aarch64 binary-private, one-shot host probe, resolving `signal-hook-registry1.4.8` in the exact lockfile. This avoids handwritten signal FFI and adds only two registry packages, reusing existing libc/errno. MIT is selected; both offered license texts are preserved under third_party/licenses. Parent inspected the flag implementation and build entrypoint: selected build.rs is an empty function with extended-siginfo-raw disabled. It is still a new build-script target; no cc/C source is enabled. No new allocator/runtime/logging library.

Lifecycle decision: the three flag handlers live until process exit. No unregister in cleanup or Drop. Setup happens before child spawn/raw mode; any registration error terminates startup. The binary-private runner is one-shot and main exits immediately after it returns; this is not a reusable library guard or a policy for the future server. Saved termios/host controls and child lifecycle have separate bounded cleanup. Previous-handler chaining and kill/abort limitations remain as described above. Runtime signal/terminal tests are still required by P2-PROBE.md.

Parent independently confirmed both archive hashes against published crates.io checksums and preserved notices. Today's [exact OSV request](p2-core-review/signals-osv-query.json) returned [zero matching baseline advisories](p2-core-review/signals-osv-results.json). [Version/repository observations](p2-core-review/signals-maintenance.json) report neither repository archived nor guaranteed future support: current push2026-10-04, releases0.4.5 on2026-10-04 and registry1.4.8 on2025-12-25. No cargo-audit/cargo-deny, exhaustive source audit or firm certification. Full selected application graph/build verification follows adoption; the graph must not enable channel, iterator, extended-siginfo, or cc.
