# Checked-in component source verifier

The new stdlib-only `scripts/check_component_sources.py` verifies the
currently declared file SHA-256 fingerprints for the two selected local
components. It is read-only and has fixed repository-relative inputs:

- `vendor/vt100/PROVENANCE.json`, reading its `files` object;
- `vendor/crossterm-input/FINGERPRINTS.json`, whose root object maps paths to
  hashes. The crossterm extraction does not contain a `PROVENANCE.json`; the
  checked-in `FINGERPRINTS.json` is the actual source-fingerprint record.

Limits are explicit: 64 KiB per manifest, 64 declared files per component,
1 MiB per source file, and 8 MiB total source bytes per component. The current
records have 16 and 13 entries, respectively; largest source file is 62,267
bytes. The verifier parses both manifests, validates all declared hashes and
lexical paths, then safely opens every declared path without following
symlinks before it reads any component source contents. Final-file opens use
`O_NONBLOCK` so FIFOs cannot hang before the regular-file check; non-regular
files, including devices, fail as `unsafe_path`. The verifier checks the
8 MiB aggregate both during metadata preflight and against actual bytes read
while hashing, so file growth after preflight cannot lift the content budget.
Each bounded read may consume at most one extra sentinel byte beyond its
limit to detect growth; it never reads unbounded input. It hashes bytes as
stored and does not regenerate or alter manifests.

Output contains only the fixed component label and a fixed result/error
class. CLI behavior is `0` when both components match, `1` for mismatch or
invalid/missing/unsafe/oversize input, `2` for arguments other than `--help`,
and `0` for help. It accepts no arbitrary path or command-line root. Unit
tests inject a temporary repo root through the Python API.

## Verification evidence

On this checkout, the live run reported both `vt100` and `crossterm-input`
as `match`. Focused Python tests cover matching and changed hashes, missing
files, traversal and absolute paths, source and manifest symlinks, FIFO
manifest and source rejection under a two-second subprocess timeout,
malformed JSON/hashes, duplicate JSON keys, oversize manifests/files, actual
aggregate byte limits after post-preflight file growth, redacted errors, fixed
CLI arguments, and verifier-runner continuation.

Commands run:

```text
python3 -m unittest tests.test_component_sources tests.test_verify_non_ui
Ran 22 tests; OK.

python3 -m unittest discover -s tests -p 'test_*.py'
Ran 29 tests; OK.

python3 scripts/check_component_sources.py
exit 0; both components match.

python3 scripts/check_component_sources.py --help
exit 0; fixed usage shown.

python3 scripts/check_component_sources.py /tmp/ignored-path
exit 2; fixed usage shown, argument not echoed.
```

The source check compares files against the checked-in fingerprint records.
It does not establish upstream authenticity, dependency safety, licensing
approval, or a security audit. Parent review and the full non-UI verifier
remain separate.
