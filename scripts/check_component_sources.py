#!/usr/bin/env python3
"""Verify checked-in component source fingerprints without changing files."""

from __future__ import annotations

import errno
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
from typing import Any, Sequence


REPO_ROOT = Path(__file__).resolve().parent.parent
MAX_MANIFEST_BYTES = 64 * 1024
MAX_COMPONENT_FILES = 64
MAX_COMPONENT_BYTES = 8 * 1024 * 1024
MAX_FILE_BYTES = 1024 * 1024
HASH_PATTERN = re.compile(r"[0-9a-f]{64}\Z")

# Each component has a fixed manifest path and an explicit manifest shape.
COMPONENTS = (
    ("vt100", ("vendor", "vt100", "PROVENANCE.json"), ("vendor", "vt100"), "provenance_files"),
    (
        "crossterm-input",
        ("vendor", "crossterm-input", "FINGERPRINTS.json"),
        ("vendor", "crossterm-input"),
        "fingerprints",
    ),
)


class VerificationError(Exception):
    """An intentionally small, non-sensitive verifier failure class."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


def _open_beneath(root: Path, parts: tuple[str, ...]) -> int:
    """Open a regular file beneath root without following any symlink."""
    if not all(hasattr(os, flag) for flag in ("O_NOFOLLOW", "O_DIRECTORY", "O_NONBLOCK")):
        raise VerificationError("safe_open_unavailable")

    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    file_flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    directory_fd: int | None = None
    try:
        directory_fd = os.open(root, directory_flags)
        for part in parts[:-1]:
            next_fd = os.open(part, directory_flags, dir_fd=directory_fd)
            os.close(directory_fd)
            directory_fd = next_fd
        return os.open(parts[-1], file_flags, dir_fd=directory_fd)
    except VerificationError:
        raise
    except FileNotFoundError as error:
        raise VerificationError("missing_file") from error
    except OSError as error:
        if error.errno == errno.ELOOP or error.errno == errno.ENOTDIR:
            raise VerificationError("unsafe_path") from error
        raise VerificationError("unreadable_file") from error
    finally:
        if directory_fd is not None:
            os.close(directory_fd)


def _file_metadata(
    root: Path,
    parts: tuple[str, ...],
    limit: int = MAX_FILE_BYTES,
    limit_error: str = "oversize_file",
) -> tuple[int, int]:
    file_fd = _open_beneath(root, parts)
    try:
        metadata = os.fstat(file_fd)
        if not stat.S_ISREG(metadata.st_mode):
            raise VerificationError("unsafe_path")
        if metadata.st_size > limit:
            raise VerificationError(limit_error)
        return file_fd, metadata.st_size
    except BaseException:
        os.close(file_fd)
        raise


def _read_beneath(
    root: Path,
    parts: tuple[str, ...],
    limit: int = MAX_FILE_BYTES,
    limit_error: str = "oversize_file",
) -> bytes:
    file_fd, _size = _file_metadata(root, parts, limit, limit_error)
    try:
        chunks: list[bytes] = []
        remaining = limit + 1
        while remaining:
            chunk = os.read(file_fd, min(64 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        content = b"".join(chunks)
        if len(content) > limit:
            raise VerificationError(limit_error)
        return content
    finally:
        os.close(file_fd)


def _safe_parts(relative: str) -> tuple[str, ...]:
    """Validate a manifest path before opening any declared source file."""
    if not isinstance(relative, str) or not relative or "\x00" in relative or "\\" in relative:
        raise VerificationError("unsafe_path")
    if relative.startswith("/") or re.match(r"^[A-Za-z]:", relative):
        raise VerificationError("unsafe_path")
    parts = tuple(relative.split("/"))
    if any(part in {"", ".", ".."} for part in parts):
        raise VerificationError("unsafe_path")
    return parts


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise VerificationError("malformed_manifest")
        value[key] = item
    return value


def _manifest_entries(raw: bytes, shape: str) -> list[tuple[str, str]]:
    try:
        document = json.loads(raw.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
    except VerificationError:
        raise
    except (ValueError, RecursionError):
        raise VerificationError("malformed_manifest") from None

    if not isinstance(document, dict):
        raise VerificationError("malformed_manifest")
    if shape == "provenance_files":
        entries = document.get("files")
    else:
        entries = document
    if not isinstance(entries, dict) or not entries or len(entries) > MAX_COMPONENT_FILES:
        raise VerificationError("malformed_manifest")

    validated: list[tuple[str, str]] = []
    for path, digest in entries.items():
        parts = _safe_parts(path)
        if not isinstance(digest, str) or HASH_PATTERN.fullmatch(digest) is None:
            raise VerificationError("malformed_manifest")
        validated.append(("/".join(parts), digest))
    return validated


def _source_digest(
    root: Path, parts: tuple[str, ...], limit: int, limit_error: str
) -> tuple[str, int]:
    content = _read_beneath(root, parts, limit, limit_error)
    return hashlib.sha256(content).hexdigest(), len(content)


def verify_sources(repo_root: Path = REPO_ROOT) -> tuple[int, dict[str, object]]:
    """Validate every manifest and path before reading any declared source."""
    root = Path(repo_root).resolve()
    plans: list[tuple[str, tuple[str, ...], list[tuple[str, str]]]] = []
    failures: list[dict[str, str]] = []

    for name, manifest_parts, component_parts, shape in COMPONENTS:
        try:
            raw = _read_beneath(root, manifest_parts, MAX_MANIFEST_BYTES)
            entries = _manifest_entries(raw, shape)
            plans.append((name, component_parts, entries))
        except VerificationError as error:
            code = "oversize_manifest" if error.code == "oversize_file" else error.code
            failures.append({"component": name, "error": code})

    # Do not read any declared sources until both manifests and every path/hash
    # have passed structural validation.
    if failures:
        return 1, {"result": "invalid", "components": failures}

    # Resolve every declared source as a regular, non-symlink file before
    # reading any source bytes. This catches missing paths and caps total
    # component input independently from the per-file bound.
    for name, component_parts, entries in plans:
        total_size = 0
        try:
            for relative, _expected in entries:
                parts = _safe_parts(relative)
                file_fd, size = _file_metadata(root, component_parts + parts)
                os.close(file_fd)
                total_size += size
                if total_size > MAX_COMPONENT_BYTES:
                    raise VerificationError("oversize_component")
        except VerificationError as error:
            failures.append({"component": name, "error": error.code})
    if failures:
        return 1, {"result": "invalid", "components": failures}

    results: list[dict[str, str]] = []
    for name, component_parts, entries in plans:
        total_read = 0
        try:
            for relative, expected in entries:
                parts = _safe_parts(relative)
                remaining = MAX_COMPONENT_BYTES - total_read
                limit = min(MAX_FILE_BYTES, remaining)
                actual, content_size = _source_digest(
                    root,
                    component_parts + parts,
                    limit,
                    "oversize_component" if remaining < MAX_FILE_BYTES else "oversize_file",
                )
                total_read += content_size
                if actual != expected:
                    raise VerificationError("hash_mismatch")
            results.append({"component": name, "result": "match"})
        except VerificationError as error:
            failures.append({"component": name, "error": error.code})
            results.append({"component": name, "result": "failed"})

    if failures:
        return 1, {"result": "failed", "components": results, "failures": failures}
    return 0, {"result": "match", "components": results}


def main(argv: Sequence[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else list(argv)
    usage = "Usage: python3 scripts/check_component_sources.py [--help]"
    if arguments == ["--help"]:
        print(usage)
        print("Verify fixed checked-in component source fingerprints.")
        return 0
    if arguments:
        print(usage, file=sys.stderr)
        return 2
    try:
        code, summary = verify_sources()
    except (OSError, RuntimeError):
        code, summary = 1, {
            "result": "failed",
            "components": [
                {"component": name, "error": "verification_error"}
                for name, _manifest, _root, _shape in COMPONENTS
            ],
        }
    print(json.dumps(summary, sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
