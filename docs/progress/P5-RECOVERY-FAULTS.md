# P5 durability retry fault evidence — 2026-10-21

Later writer-component review added three store regressions and corrections:
retry after incoming byte eviction, retry across sparse retained ordinals and
durable gap recording for duplicate-only rows. See
[P5-HISTORY-WRITER.md](P5-HISTORY-WRITER.md). The earlier results below remain
evidence for their original scope; evicted cells are not compared/recovered.

The saved-history store now has deterministic, `cfg(test)`-only, per-instance
fault injection at the pre-rename and directory-sync boundaries. A pre-rename
failure leaves the prior committed row segment or checkpoint intact. A
directory-sync failure happens after rename: the new bytes are visible, while
the attempted operation still returns an error. The test state is private to
each store behind a mutex; no environment switches, public production hook,
global test state or new dependency was added.

Retries acknowledge rows, checkpoints, existing history starts and existing
end times only after syncing the containing directory. Row tests verify a
recaptured batch with updated capture timestamps retries the persisted rows
without duplicates, while changed row contents remain rejected. Checkpoint
tests verify an exact retry still reports repeated sync failures and only
then succeeds; different bytes at the same checkpoint order remain rejected.
Reopen checks confirm the committed destination after each boundary. For
actual PTY checkpoint capture, `Capture` retains only the metadata for its
latest observation, guarded by buffer, target, output sequence and screen
revision. That lets an unchanged observation retry with identical encoded
bytes and its original timestamp; a changed observation receives fresh
metadata. The test demonstrates visible post-rename checkpoint bytes,
repeated failed retries, and eventual successful capture without new PTY
output.

The lifecycle test uses a real fixture PTY and real temporary history files.
An injected end-time directory-sync failure makes `capture_exit` return false,
leaves `end_pending` set and preserves the first persisted end time. A second
injected failure on the store retry still leaves the end pending. After sync
succeeds, another `capture_exit` clears pending and capture failure state.
Pruning from the first end time expires that target while retaining a live
target and a target ended later.

The injected errors are deterministic software I/O failures at the selected
call boundaries. They establish the return, visibility, retry and reopen
behavior for those code paths; they are not a physical power-loss, filesystem
crash or reboot test. Temporary-file cleanup/layout recovery, full V-HISTORY
reboot acceptance, representative storage throughput, and Phase 2/3/4 host
acceptance remain separate gates. The same-observation checkpoint retry is
covered at the real capture caller; stored terminal effects and provider
conversations are still not replayed. The requested `ms-rust` skill remains
unavailable in this environment, so no skill-compliance claim is made.

## Verification

Commands were run from the repository root on the final source changes:

```text
$ cargo test --locked --offline --lib history_store
test result: ok. 3 passed; 0 failed; 0 ignored; 506 filtered out; finished in 0.11s
exit_code=0

$ cargo test --locked --offline --lib server_pty
test result: ok. 2 passed; 0 failed; 0 ignored; 507 filtered out; finished in 0.11s
exit_code=0

$ cargo test --locked --offline --test p5_history_store
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
exit_code=0

$ cargo fmt --check
exit_code=0

$ cargo clippy --locked --offline --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.47s
exit_code=0
```

The focused tests and strict Clippy/fmt checks passed. The parent owns the
integrated verifier and shared roadmap updates; this note does not claim full
Phase 5 completion.
