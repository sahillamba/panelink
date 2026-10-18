# Documentation checker

`python3 scripts/check_docs.py` scans Markdown files directly in the repository
root and all `docs/**/*.md` files, including untracked files. Optional paths are
Markdown files resolved from the caller's current directory:

```sh
python3 scripts/check_docs.py README.md docs/SPEC.md
```

The checker uses only Python's standard library. It checks supported inline
Markdown links of the form `[label](path)` and `[label](<path with spaces>)`,
including image links. Paths are percent-decoded; fragments and query strings
are ignored for file existence. HTTP, HTTPS, mailto, other URI schemes,
protocol-relative links, and same-document anchors are skipped. Local targets
must exist and remain inside the repository. It does not validate fragments or
parse reference links, nested link labels, or the full CommonMark grammar.
Examples inside backtick or tilde fenced code blocks and matched single-line
backtick code spans are ignored for link checks. Whitespace, conflict-marker,
and final-newline checks apply to every document line, including code fences.

For scanned documents it also reports trailing spaces or tabs, Git merge
conflict markers (including diff3 base markers), and a missing final newline.
All trailing whitespace is reported, including the two spaces sometimes used
for Markdown hard breaks. Diagnostics contain paths, line numbers, and finding
types only; the checker does not print document contents, fetch URLs, execute
shell commands, or modify files. Exit status is 0 for a clean scan, 1 for
document findings, and 2 for invalid command-line usage or input paths.

## Verification evidence — 2026-10-18

`python3 scripts/check_docs.py` checked 91 Markdown files with zero findings,
both from the repository and when invoked from `/tmp`. An isolated temporary
fixture verified relative and percent-encoded angle-bracket links, external and
same-document links, ignored fenced and inline link examples, missing and
escaping targets, trailing whitespace, conflict and diff3 markers, missing
final newline, caller-relative input paths, malformed external URI and
percent-encoded NUL handling, and exit statuses 0, 1, and 2. It also verified
that an indented closing fence restores link checking. Missing and non-Markdown
input paths return 2. The temporary fixture was removed automatically.
`python3 -m py_compile` passed using a temporary bytecode cache, and
`git diff --check` passed. The checker itself has no runtime or third-party
package dependencies.
