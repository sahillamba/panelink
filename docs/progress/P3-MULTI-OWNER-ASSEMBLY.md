# P3 trusted multi-owner assembly

## Delivered scope

`src/server/multi_session.rs` adds a macOS/aarch64-gated public library core, `MultiSessionOwner<E>`. It assembles one shared `GlobalAuthority` and one `ManagedLaunchCoordinator<TargetState<E>>`; the coordinator retains at most two targets and their sinks. The module is not wired into the socket, CLI, server driver, or startup path.

Construction is available only through `with_ledger`. Its caller must supply the existing ledger with the trusted internal startup lease already opened, authorize launch policy before invoking the trusted launch method, and hold the singleton storage lock for the owner's lifetime. No API exposes lease creation, the mutable ledger, the registry, or a concrete target state. The custom Debug implementation avoids sink payloads and launch request data.

Launch delegates identity and ordering to `ManagedLaunchCoordinator`. A target seed allocates the input owner and event journal before spawn; successful spawn is wrapped and attached before post-spawn journal writes. Preparation and journal-preflight failures do not reserve a slot. Intent and spawn failures retain their spent identity. Exact launch retries use the single ledger and skip callbacks. Status and receipt operate globally by lease and do not select a target.

Connect requires an exact retained target and pins each grant to its full five-field identity. The experimental owner negotiates Read/Input only. Optional Events is omitted and required Events is rejected before a fresh lease is issued because watch and reconciliation routing remain deferred. Existing `StateOwner` connections retain the previous Read/Input/Events negotiation mask.

Control decodes and performs global exact-key preflight before target lookup. Global cached or rejected replies return without lookup or metadata synchronization. New requests select the exact retained target, run the existing borrowed control handler (including the pinned-scope check), then synchronize that selected target once, including on denial. There is no generic session router; Describe, Claim, Release, Handoff, Detach, watch, and reconcile routing remain deferred. Input ownership cannot be acquired through this owner API yet.

`poll` expires the shared ledger/grants once and visits each retained session with effects once using a fixed stack snapshot. Each visit expires that session's input ownership, polls its sink once, synchronizes its metadata, and records child exit while retaining the handle. It continues to the second target after a first-target poll error and returns the first error.

## Verification evidence

On the current macOS/aarch64 host, the final new owner unit filter passed 7 tests, including launch preparation/identity ordering, post-spawn handle retention, exact global launch deduplication, target-pinned resume/control isolation, global status/receipt, per-target poll progress, and Events omission/required-capability rejection without consuming a lease ID. Parent review requested the final cases: cached results/conflicts with an empty fake registry, and continued polling/fresh Read after both Spawned and Applied journal faults. Those cases passed before release.

Regression checks passed: `cargo test --locked --offline --lib server::tests::` (33 passed), `cargo test --locked --offline --test p3_socket_detach` (2 passed), `cargo test --locked --offline --test p3_managed_launch` (2 passed), and the separately owned `cargo test --locked --offline --test p3_multi_owner -- --nocapture` (1 real-PTY integration test passed). The matching test-target Clippy check passed.

Strict `cargo clippy --locked --offline --all-targets -- -D warnings`, `cargo fmt --check`, `git diff --check`, and `python3 scripts/check_docs.py` also passed; the docs checker reported 127 Markdown files and no findings. This evidence covers a trusted in-process core and fake sink/journal behavior, plus a separate real-PTY assembly integration test. It does not certify peer authentication, runtime routing, multi-pane UX, scoped watches, stop semantics, host provider compatibility, or acceptance of any broader milestone.
