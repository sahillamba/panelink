# Bounded host decoder implementation task

Scope: a disposable parser-only extraction of the selected MIT Crossterm 0.29.0 component, not a handwritten Panelink escape parser. Review this extraction before application adoption. Do not bring in upstream event IO, global readers, signal handlers, size lookup, terminal commands or async features. Panelink will own host IO/restoration separately through reviewed OS wrappers. This prototype decision is not a product keyboard protocol commitment.

One owner may write only `/private/tmp/panelink-host-decoder/**` and `docs/progress/P2-DECODER-RESULT.md`. Parent owns all application, manifests, roadmap and vendor adoption. No commits/pushes. Match existing Rust style. Read ROADMAP.md, P2-HOST-INPUT.md and this contract first. Use graph-first source discovery. No ms-rust compliance claim.

## Extraction

Create a distinct private crate `crossterm-input`, version `0.29.0-panelink.1`, edition 2021, with only `bitflags = "=2.13.2"` (already reviewed in the application graph). Preserve MIT LICENSE (Copyright 2019 Timon), upstream original parse.rs/event.rs, archive checksum and VCS revision, an extraction recipe/source ranges and a complete patch/fingerprints. Do not execute upstream code generators. Disable doctests that refer to the full upstream facade. Keep applicable upstream parser tests.

Reuse upstream Unix parse.rs and event/key/mouse types plus InternalEvent and KeyboardEnhancementFlags. Remove unused serde/derive-more cfg attributes and command/IO implementations from the extracted type module. Rewrite only module paths needed by the extraction; fix the raw-mode query to a documented constant true, because this decoder is valid only for raw host input. Invalid paste UTF-8 must produce an error before String allocation (replace lossy conversion with checked UTF-8). No newly invented escape grammar. No upstream IO source/readers are compiled or exposed.

## Stream interface

`Decoder::new()` allocates one fixed boxed buffer of 65,548 bytes; `advance(byte: u8, input_available: bool) -> Result<Option<Event>, DecodeError>` feeds one byte into the existing parser and returns at most one event. There is no retained event queue. Ordinary incomplete sequences have a 16,384-byte ceiling. A buffer beginning with the existing bracketed-paste opener has the 65,548-byte ceiling (65,536 payload plus 12 framing bytes). Check the appropriate limit before storing a byte; validate paste length/UTF-8 before allocating its returned String. Reuse upstream parsing for completion/semantics.

Any malformed/over-limit/invalid-UTF8 input permanently poisons this instance. All subsequent calls return the same redacted failure class without interpreting a suffix as key events; no recovery/reset API. A caller must stop the probe and restore the terminal on error. Ignore/drop recognized InternalEvent responses other than ordinary Event, without retaining them or triggering host queries. Debug for Decoder and errors must not expose bytes; make returned Event Debug redact key/paste payload as well.

`finish_escape()` resolves only a pending single ESC through the existing parser after the caller's timeout. It must not flush or reinterpret an incomplete CSI or paste. Report `pending_bytes()` and `is_failed()` without content. A future IO adapter reads at most 1,024 bytes per turn, checks monotonic time between byte calls and owns the single returned event immediately; it must not collect paste events in a queue. One parser call can scan only the bounded input. Measure/adversarially inspect parser work before claiming a host scheduling deadline.

## Verification

Run `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --manifest-path /private/tmp/panelink-host-decoder/Cargo.toml`; `cargo fmt --check --manifest-path ...`; same environment `cargo clippy --locked --offline --manifest-path ... --all-targets -- -D warnings` (record any inherited upstream lint separately rather than hiding it globally). Generate lockfile offline first. Paste actual outputs in result, with source changes/deviations/gotchas. This does not test host IO or terminal restoration.

Tests: ordinary Unicode and arrows across byte boundaries; lone ESC resolution without flushing incomplete CSI; bracketed UTF-8 paste exactly once at opener/closer split boundaries; exact 65,536-byte payload admitted; next byte beyond maximum rejected before storage; ordinary 16 KiB ceiling; invalid UTF-8 paste rejected without lossy expansion; malformed controls poison; rejected paste followed by harmless command-shaped suffix never emits an event; no event queue; redacted Debug; internal query responses discarded. Retain applicable upstream parser tests and report counts. Do not claim root adoption or host compatibility.

Completed checkpoint: candidate39 tests passed, parent reviewed source/provenance and independently reran tests, then adopted the exact component in vendor/crossterm-input. See P2-INPUT-REVIEW.md. The disposable path above is task history; the vendored source is the durable resume copy. Future host IO/restoration remains outside this component.
