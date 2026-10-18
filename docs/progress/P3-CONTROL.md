# P3-A2a typed control-payload evidence

Status: implemented and locally verified on 2026-10-17; parent integration
review remains pending. This is a borrowed, standard-library-only payload
codec. It does not start or complete Phase 3 server work.

## Implemented scope

src/control_protocol.rs defines the public TargetRef, ReadSource, Command,
ControlRequest, and redacted ControlError APIs. It encodes and decodes the five
approved operations: bounded screen/live-scrollback reads, literal text, paste,
submit, and interrupt. It uses the existing operation key and operation-class
types.

The schema uses version 1, a fixed 68-byte big-endian identity prefix, opcodes
1 through 5, and the approved fixed and length-prefixed bodies. Read requests
are 75 bytes, interrupt requests are 76 bytes, and text requests are 80 bytes
plus their UTF-8 byte length. Input is capped at 32,768 UTF-8 bytes; reads are
capped at 65,536 bytes and 1,024 lines. Decoding borrows text directly from the
input slice and rejects payloads above 65,536 bytes before parsing.

Both decode and encode validate nonzero identifiers, key/target generation
equality, read limits, owner revisions, UTF-8, text controls, exact body length,
and supported discriminators. Text and submit reject Unicode control scalars;
paste preserves caller-supplied valid UTF-8. Debug output and fixed errors do
not include text. Encoding validates before writing and propagates writer
failures without retrying a partial payload.

## Actual verification

Command:

~~~text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib control_protocol
~~~

Actual result:

~~~text
running 15 tests
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 70 filtered out
~~~

The tests cover independent golden Read and Unicode Text payloads, exact
offsets and sizes, all-operation round trips, request-frame integration,
borrowed text pointers, every truncated prefix, invalid schema/opcode/source,
zero and mismatched identifiers, read boundaries, multibyte text limits,
malformed UTF-8, oversized declared lengths, control-scalar handling, empty
operation distinctions, trailing and oversized payloads, operation-class
mapping, redaction, validation before writing, and an injected writer failure.

Command:

~~~text
cargo fmt --check
~~~

Actual output: no output; exit status 0.

Command:

~~~text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
~~~

Actual output:

~~~text
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished dev profile [unoptimized + debuginfo] target(s) in 0.30s
~~~

Exit status: 0.

The first focused compile found two test-only function-name shadowing errors;
the local names were corrected before the passing run. There were no contract
deviations, dependency or manifest changes, or edits outside the two assigned
files.

## Scope and handoff

The codec proves no caller authentication, capability negotiation, target
existence or access, input ownership, lease validity, deduplication, admission,
or successful side effect. A server must check the outer Request frame kind
and perform those validations before reserving and dispatching work. No socket,
CLI, launch, owner handoff, split/layout, provider, persistence, PTY, or runtime
behavior was added or tested.

Implementation ownership is released for parent review. Parent integration
verification remains outstanding.

## Parent integration

Parent reviewed the complete codec sources and added a cross-component regression in control_protocol.rs: the complete encoded target, owner revision and command kind participate in exact ledger payload equality. A known duplicate cannot dispatch twice; changing any of those fields under the same key conflicts. This is server-side duplicate defense, not authorization for a client to resend uncertain input.

Final parent `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline -q` exited 0: **122 passed / 9 ignored** (86 library + 21 binary + 4 host fixture/preflight + 11 core PTY; 0 doctests). Formatting, strict locked/offline all-target Clippy and diff checks passed. No dependency/manifest/lock changes. The one-shot CLI remains the only interactive executable path; typed schemas and Event framing are pure library foundations. No complete server/CLI/event subscription or authentication claim; no phase-completion commit or push.
