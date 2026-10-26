# P3 managed-child process metadata sampler

Implementation slice for the accepted contract in
[`P3-PANE-METADATA-TASK.md`](P3-PANE-METADATA-TASK.md). This implements only
the direct-child native sampler and its bounded worker; owner authorization,
deadline handling, publication, and protocol/CLI integration remain in the
multi-session runtime work.

`src/process_metadata.rs` captures an opaque PID plus kernel birth identity,
queries the managed child's current cwd and executable image, and verifies the
same identity again after both path calls. It never reads argv or environment,
and does not follow descendants. Native calls run on a detached worker with
capacity-one job and completion channels. Submit and poll are nonblocking;
dropped workers are not joined because libproc offers no cancellation.

The implementation uses SDK-derived aligned buffers and exact return-size
checks. The local SDK ABI probe was compiled with
`xcrun clang -Wall -Wextra -Werror /private/tmp/panelink-proc-abi.c` and
reported: `proc_bsdinfo` size 136, PID offset 12, start seconds offset 120,
start microseconds offset 128; `proc_vnodepathinfo` size 2352 with cwd path
offset 152; `proc_pidpath` maximum size 4096. No dependency was added. An
SDK-linked `proc_pidpath` probe returned `bytes=31` for a 31-byte path and left
its NUL at `buffer[31]`; the implementation requires that byte-count /
terminator relationship and parses exactly the returned bytes.

## Verification

The focused command `cargo test --locked --offline process_metadata::tests
--lib` passed all ten native tests: initial cwd and direct exec, changed cwd,
a live descendant while the direct child remains the shell, redacted identity
Debug output, synthetic birth-token mismatch, missing PID, malformed path and
native buffer cases, errno status mapping, worker drop while an injected
sampler is gated, and one-queued-job capacity with rejection of a third job.
The birth-token test changes the expected token; it does not simulate kernel
PID reuse. Child fixtures use separate process groups and are killed/reaped on
scope exit. The module was formatted with `rustfmt --edition 2021
src/process_metadata.rs`; `cargo clippy --locked --offline --lib -- -D warnings`
also passed. These are component checks; the parent's full verification still
covers the shared runtime integration.

This is not Phase 3 acceptance evidence. macOS permission/error branches,
kernel PID-reuse race injection, owner-side 500 ms deadline behavior,
target/capability authorization, and end-to-end CLI privacy remain separate
gates.

## Parent review follow-up

Parent review preserved fixed errno reasons for birth-info query failures,
made the capacity-one queue test drain each completion deterministically, and
waited for the expected post-spawn observation in the descendant fixture.
All10 sampler tests pass in the final serial suite. The parent also verified
the gated owner timeout/busy/late-drain path and actual CLI cd/exec/privacy; see
[P3-PANE-METADATA-RESULT.md](P3-PANE-METADATA-RESULT.md). The earlier component
checks above retain their original scope and do not imply actual PID reuse.
