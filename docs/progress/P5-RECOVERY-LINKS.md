# P5 bounded recovery links — implementation slice

Implemented the frozen API in `P5-RELAUNCH-LINKS-API.md` as a bounded,
metadata-only store at `server/links/links.bin`. The store retains up to 64
fixed-size links, prunes links older than 30 days relative to the supplied
clock, and never creates storage during `open` or `load`. `prepare` creates only
the private `links/` directory when absent.

Each link binds the saved source generation/sequence and old target to an
owner-issued target in a different generation but the same workspace. Zero
identity/time fields and invalid scope are refused. An identical link retry is
idempotent; a different link for the same old target and new generation is
refused. Links are historical associations only and do not authorize target
dispatch or provider recovery.

The binary file contains only the `PLLNK001` magic, a count, fixed numeric
identity fields, an outcome tag, and a timestamp. Reads refuse malformed,
truncated, trailing, oversized, symlink, hard-link, FIFO, and unsafe-mode
storage without rewriting committed bytes. Writes use a private temporary,
file sync plus macOS fullfsync, atomic rename and directory sync. Target and
path-bearing Debug output is redacted.

## Evidence

`tests/p5_recovery_links.rs` exercises synthetic private-directory scenarios
for round trips, one/two links, idempotency/conflict, generation/workspace
checks, 30-day pruning, the 64-link cap, corruption preservation, unsafe
filesystem objects, non-creating reads, encoded-content bounds, and Debug
redaction. The focused integration test run passed 13 tests with no failures.
The library filter passed its one atomic pre-rename fault test with no failures.
This slice does not establish full recovery, relaunch integration, provider
resume, or physical reboot behavior.

## Verification results

Worker commands run on Apple Silicon macOS with Rust/Cargo 1.99.1 and
`CARGO_TARGET_DIR=/var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T/opencode/target-l7`:

- `cargo test --locked --offline --lib recovery_links -- --test-threads=1` —
  passed: 1 passed, 0 failed, 640 filtered out. Actual output ended with
  `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 640 filtered out`.
- `cargo test --locked --offline --test p5_recovery_links -- --test-threads=1` —
  passed: 13 passed, 0 failed. Actual output ended with
  `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- Initial `cargo fmt --check` — failed on formatting differences; source/tests
  were formatted. Final `cargo fmt --check` exited 0 with no output.
- `cargo clippy --locked --offline --all-targets -- -D warnings` — final run
  exited 0: `Finished dev profile ... target(s) in 1.51s`.
- `python3 scripts/check_docs.py` — exited 0:
  `Checked 196 Markdown file(s); 0 finding(s).`

The parent performs integrated review and any additional verification before
changing phase status.
