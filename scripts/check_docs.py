#!/usr/bin/env python3
"""Check repository Markdown links and basic document hygiene."""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit


REPO_ROOT = Path(__file__).resolve().parent.parent
FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
LINK_RE = re.compile(r"!?\[[^\]\n]*\]\((<[^>\n]*>|[^()\s]+)(?:\s+[^)\n]*)?\)")
CONFLICT_RE = re.compile(r"^(?:<{7}(?: |$)|={7,}$|>{7}(?: |$)|\|{7}(?: |$))")
SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")
INLINE_CODE_RE = re.compile(r"(?<!`)(`+)(?!`)(.*?)\1(?!`)")


def markdown_files() -> list[Path]:
    """Return root-level and docs Markdown files, including untracked files."""
    paths = set(REPO_ROOT.glob("*.md"))
    docs = REPO_ROOT / "docs"
    if docs.exists():
        paths.update(
            path
            for path in docs.rglob("*.md")
            if not any(part in {".git", "vendor", "third_party"} for part in path.relative_to(REPO_ROOT).parts)
        )
    return sorted((path for path in paths if path.is_file()), key=lambda p: str(p))


def local_link_target(source: Path, raw_destination: str) -> tuple[Path | None, str | None]:
    """Return a local target or a fixed diagnostic; external links have neither."""
    destination = raw_destination[1:-1] if raw_destination.startswith("<") and raw_destination.endswith(">") else raw_destination
    if not destination or destination.startswith("#") or destination.startswith("//"):
        return None, None
    if SCHEME_RE.match(destination):
        return None, None
    try:
        parsed = urlsplit(destination)
        path_text = unquote(parsed.path)
        if not path_text:
            return None, None
        path = Path(path_text)
        target = path if path.is_absolute() else source.parent / path
        target = target.resolve(strict=False)
    except (OSError, ValueError, RuntimeError):
        return None, "local link path is invalid"
    try:
        target.relative_to(REPO_ROOT)
    except ValueError:
        return None, "local link escapes repository root"
    return target, None


def without_inline_code(line: str) -> str:
    """Blank matched single-line code spans while keeping link positions stable."""
    return INLINE_CODE_RE.sub(lambda match: " " * len(match.group(0)), line)


def inspect_document(path: Path) -> list[tuple[int, str]]:
    findings: list[tuple[int, str]] = []
    try:
        with path.open("r", encoding="utf-8") as stream:
            lines = stream.readlines()
    except (OSError, UnicodeError) as exc:
        return [(0, f"cannot read document ({type(exc).__name__})")]

    in_fence: tuple[str, int] | None = None
    for line_number, line in enumerate(lines, start=1):
        content = line.rstrip("\r\n")
        if in_fence is None:
            fence_match = FENCE_RE.match(content)
            if fence_match:
                marker = fence_match.group(1)
                in_fence = (marker[0], len(marker))
            else:
                for match in LINK_RE.finditer(without_inline_code(content)):
                    raw = match.group(1)
                    target, link_error = local_link_target(path, raw)
                    if link_error:
                        findings.append((line_number, link_error))
                    elif target is not None:
                        try:
                            exists = target.exists()
                        except (OSError, ValueError, RuntimeError):
                            findings.append((line_number, "local link path is invalid"))
                        else:
                            if not exists:
                                findings.append((line_number, "local link target does not exist"))
        else:
            fence_match = FENCE_RE.match(content)
            if fence_match:
                marker = fence_match.group(1)
                if marker[0] == in_fence[0] and len(marker) >= in_fence[1] and content[fence_match.end():].strip() == "":
                    in_fence = None

        if content.endswith((" ", "\t")):
            findings.append((line_number, "trailing whitespace"))
        if CONFLICT_RE.match(content):
            findings.append((line_number, "merge conflict marker"))

    if lines and not lines[-1].endswith(("\n", "\r")):
        findings.append((len(lines), "missing final newline"))
    return findings


def selected_files(arguments: list[str]) -> tuple[list[Path], list[str]]:
    if not arguments:
        return markdown_files(), []
    files: list[Path] = []
    errors: list[str] = []
    for argument in arguments:
        path = Path(argument).expanduser()
        if not path.is_absolute():
            path = Path.cwd() / path
        try:
            path = path.resolve(strict=False)
            exists = path.exists()
            is_file = path.is_file() if exists else False
        except (OSError, ValueError, RuntimeError):
            errors.append(f"{argument}:0: input path is invalid")
            continue
        if not exists:
            errors.append(f"{argument}:0: input path does not exist")
        elif not is_file:
            errors.append(f"{argument}:0: input path is not a file")
        elif path.suffix.lower() != ".md":
            errors.append(f"{argument}:0: input path is not a Markdown file")
        else:
            files.append(path)
    return files, errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check Markdown links and basic document hygiene.")
    parser.add_argument("paths", nargs="*", metavar="MARKDOWN_PATH", help="Markdown paths, relative to the current directory")
    args = parser.parse_args(argv)
    files, input_errors = selected_files(args.paths)
    for error in input_errors:
        print(error, file=sys.stderr)
    if input_errors:
        print(f"Checked {len(files)} Markdown file(s); invalid input path(s).")
        return 2
    total_findings = 0
    for path in files:
        display_path = os.path.relpath(path, REPO_ROOT)
        for line_number, message in inspect_document(path):
            print(f"{display_path}:{line_number}: {message}")
            total_findings += 1
    print(f"Checked {len(files)} Markdown file(s); {total_findings} finding(s).")
    return 1 if total_findings else 0


if __name__ == "__main__":
    sys.exit(main())
