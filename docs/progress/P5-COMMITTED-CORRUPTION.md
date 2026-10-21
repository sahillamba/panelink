# P5 committed-record corruption and partial-tail policy — 2026-10-21

Status: bounded policy, parent-reviewed store/fresh-process acceptance and
permitted integrated checks complete. Known blocked/failed host gates remain.
This does not complete Phase5 or establish full reboot/physical-crash durability.

## Decision and behavior

Keep whole-file refusal for committed rows and screen checkpoints. Atomic
replacement writes a complete bounded candidate, syncs it and renames it; this
is not an append journal with independently certified earlier records. A
structurally valid prefix cannot certify that the rest was only an interrupted
last write. The format has no checksum or authenticated content. It cannot
detect every plausible content alteration or safely classify arbitrary damage.

| Observed file | Policy |
| --- | --- |
| Complete structurally valid committed file | Read validated sanitized data within existing limits. |
| Missing committed file | Absence; do not invent rows/end/checkpoints. |
| Malformed/truncated/extra-tail committed file | Reject whole file, preserve bytes; no prefix salvage, repair, rename, delete or overwrite. |
| Valid-looking complete-row prefix with declared rows missing | Reject; a row boundary is not a commit certificate. |
| Unsafe entry or identity mismatch | Refuse; never follow a link to repair it. |
| Recognized uncommitted temporary | Never read/promote; existing locked bounded cleanup may discard it. |
| Format-valid altered text/metadata | May remain readable; no arbitrary corruption/authenticity detection claim. |

Existing load/mutation preflights already enforce this decision. CLI show/search
preflight rows and both existing screen files before output, fail the whole
requested target on any invalid component, and emit status1/fixed diagnostics
without partial stdout. Healthy other targets remain accessible by explicit ID;
individual store buffer reads remain independent. Listing is an observation,
not an integrity certificate. Maintenance is not directory-wide transactional;
earlier permitted effects may precede a later error. Clear is not a corrupt-file
repair override. A separate forensic export/repair or integrity migration is
future work; this slice adds no such command.

No production algorithm, storage-format, quota, writer retry/shutdown, manifest,
dependency or vendor change is intended. No stored terminal side effects or
provider conversation actions are replayed. Scope and ownership are in
[P5-COMMITTED-CORRUPTION-TASK.md](P5-COMMITTED-CORRUPTION-TASK.md).

## Parent fresh-process checks

New tests/p5_history_corruption_cli.rs uses test-owned private stores and fresh
binary processes, with five-second subprocess deadlines and isolated process
groups. Six damage fixtures cover truncated row/normal/alternate bodies,
zero-byte rows and raw OSC52/control suffixes on rows/alternate files. Show and
search must refuse with exactly the fixed diagnostic and no partial content.
Healthy explicit-target show/search must still succeed, with sanitized output.
Recursive path/type/mode/mtime/byte snapshots must stay identical across reads;
configured shell marker and runtime-state creation are therefore detected.
A separate valid-orphan/invalid-destination test proves readers never substitute
or promote the temporary; locked cleanup discards the orphan and leaves the
invalid committed destination unchanged and unreadable.

The first parent test run failed during fixture construction: its screen used
unsupported2x8 dimensions. The test now uses the accepted8x20 viewport; no
production limits were changed or assertions relaxed. The next run passed the
read-only matrix but the orphan fixture initialized server identity after it
had populated history, correctly receiving AlreadyExists. Identity setup now
runs while the dedicated server directory is empty. These are software
corruption/reopen tests, not actual disk corruption, power-loss or reboot tests.

## Store regressions and parent focused verification

Luna xhigh supplied six public-store regressions; parent reviewed and reran them
with the two independent CLI tests. All8 passed, with0 failed/ignored. The store
suite checks every proper byte prefix of a compact three-row segment, ten
malformed variants, mutation/refusal/reopen preservation, six representative
checkpoint cut offsets, healthy target/buffer independence, and a format-valid
x-to-y cell alteration that remains readable. That last result demonstrates the
integrity limitation rather than claiming arbitrary corruption detection.
The owner's actual output and development corrections are retained in
[P5-COMMITTED-CORRUPTION-TESTS.md](P5-COMMITTED-CORRUPTION-TESTS.md).

```text
cargo test --locked --offline --test p5_history_corruption --test p5_history_corruption_cli -- --test-threads=1
store tests:6 passed,0 failed,0 ignored (0.21s)
fresh-process CLI tests:2 passed,0 failed,0 ignored (0.77s)
```

Parent reviewed alternate-buffer fixtures: they change only the format's buffer
tag on a normal snapshot, validate the result with the public decoder, then
store it through the real API. These fixtures establish file/buffer isolation,
not a live alternate-screen transition. The earlier private-PTY restart test
remains the separate transition evidence. No production change was needed.

## Integrated verification

The full non-UI verifier reports7/8, exit1. Root library551passed
with4ignored; binary94passed/1failed. The binary failure remains
endpoint_state_classifies_files_and_sockets, PermissionDenied/code1 at the
pathname bind. Decoder39, Python45, component sources, fmt, strict all-target
Clippy and offline build pass. Documentation checks report171 Markdown files
and0 findings. New test files compiled under strict all-target Clippy.

The default Cargo run stops at the binary failure, so all31 integration targets
were also run separately, serial within each target:86passed/17failed/11ignored,
none incomplete. This includes all8 new tests passing again. Failing targets
remain p3a_attach_host5, p3a_default_open6, p3a_serve_pathname1 and
p3b_close_runtime5. Five failures directly report listener_permission_denied,
eleven exit during server startup before raw mode, and one is the known host
stderr assertion. They match the earlier failure set; no host gate is waived.
Strict library rustdoc and the doctest command pass, with0 doctests. Final
documentation checks report171files/0findings and git diff --check passes.

No release benchmark was rerun because all65 baseline application-source and
manifest files are unchanged; this slice changes tests and policy documentation.
All13 preceding raw/provenance JSON artifacts remain byte-identical. Earlier
live timeout/payload-loss observations remain the last measurements, not new
performance evidence for this slice. Component source fingerprints match;
vendor/manifest inputs are unchanged. Graph refreshed fast/nonpersistent after
test freeze. Requested ms-rust remains absent; no guideline compliance claimed.
No commit/push; all ownership released; parent phases3/5/6 remain open.

Supplementary outputs are local/disposable:
`/private/tmp/panelink-corruption-focused.log`,
`/private/tmp/panelink-corruption-verifier.log`,
`/private/tmp/panelink-corruption-integrations/summary.json`,
`/private/tmp/panelink-corruption-rustdoc.log`,
`/private/tmp/panelink-corruption-doctest.log`. Durable tests, this summary and
the owner note preserve the acceptance meaning when those logs expire.

## Remaining work

Committed partial-tail behavior is now specified and tested as refusal, not
salvage. A checksum migration, forensic export or explicit repair tool remains
separate future work. Full saved layout/settings and machine-reboot acceptance,
representative workload, unknown/unsubmitted end intent, lost final payload,
filesystem stalls and provider/host gates remain open. Next is designing a
bounded saved-layout/restart contract using the current two-pane prototype,
without deciding the pending workspace hierarchy or promising process/provider
resurrection. This task does not authorize replay of any stored effects.
