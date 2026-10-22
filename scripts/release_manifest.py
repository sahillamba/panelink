#!/usr/bin/env python3
"""Build and describe an unsigned, local Panelink release artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import tempfile
import time
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
RELEASE_DIR = ROOT / "release"
BUILD_COMMAND = ["cargo", "build", "--locked", "--offline", "--release"]


def sha256_file(path: Path) -> str:
    """Return a lowercase SHA-256 digest, streaming large inputs."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest(path: Path) -> dict[str, Any]:
    """Load and validate the required top-level manifest schema."""
    with path.open("r", encoding="utf-8") as stream:
        manifest = json.load(stream)
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 1:
        raise ValueError("unsupported release manifest schema")
    binary = manifest.get("binary")
    inputs = manifest.get("inputs")
    if not isinstance(binary, dict) or not isinstance(inputs, dict):
        raise ValueError("release manifest is missing binary or input metadata")
    for label, value in (
        ("binary SHA-256", binary.get("sha256")),
        ("notices SHA-256", inputs.get("notices_sha256")),
    ):
        if not isinstance(value, str) or len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
            raise ValueError(f"invalid {label}")
    return manifest


def manifest_file_digest(manifest: dict[str, Any], section: str, name: str) -> str:
    """Return a validated digest stored at manifest[section][name]."""
    value = manifest.get(section, {}).get(name) if isinstance(manifest.get(section), dict) else None
    if not isinstance(value, str) or len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
        raise ValueError(f"invalid {section}.{name} SHA-256")
    return value


def vendor_tree_hash(vendor_root: Path) -> dict[str, Any]:
    """Describe a vendor tree by sorted relative-path and content digests."""
    files = []
    if vendor_root.exists():
        for path in sorted(item for item in vendor_root.rglob("*") if item.is_file()):
            files.append({"path": path.relative_to(vendor_root).as_posix(), "sha256": sha256_file(path)})
    tree_digest = hashlib.sha256()
    for item in files:
        tree_digest.update(item["path"].encode("utf-8"))
        tree_digest.update(b"\0")
        tree_digest.update(item["sha256"].encode("ascii"))
        tree_digest.update(b"\n")
    return {"sha256": tree_digest.hexdigest(), "file_count": len(files), "files": files}


def git_value(*arguments: str, default: str | None = None) -> str | None:
    result = subprocess.run(
        ["git", *arguments], cwd=ROOT, text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        return default
    value = result.stdout.strip()
    return value


def run_capture(command: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        message = result.stderr.strip() or result.stdout.strip() or f"exit {result.returncode}"
        raise RuntimeError(f"{shlex.join(command)} failed: {message}")
    return result.stdout.strip()


def render_markdown(manifest: dict[str, Any]) -> str:
    """Render the machine manifest as a compact human-readable record."""
    vendor = manifest["inputs"]["vendor_tree"]
    linked = manifest["binary"]["linked_libraries"]
    lines = [
        "# Panelink release manifest",
        "",
        f"- Commit: `{manifest['source']['commit']}`",
        f"- Tag: `{manifest['source']['tag'] or 'none'}`",
        f"- Dirty checkout: `{str(manifest['source']['dirty']).lower()}`",
        f"- Rust compiler: `{manifest['toolchain']['rustc'].splitlines()[0]}`",
        f"- Cargo: `{manifest['toolchain']['cargo'].splitlines()[0]}`",
        f"- macOS: `{manifest['host']['macos']}` ({manifest['host']['architecture']})",
        f"- Cargo.lock SHA-256: `{manifest['inputs']['cargo_lock_sha256']}`",
        f"- Vendor tree SHA-256: `{vendor['sha256']}` ({vendor['file_count']} files)",
        f"- Notices SHA-256: `{manifest['inputs']['notices_sha256']}`",
        f"- Binary: `{manifest['binary']['path']}` ({manifest['binary']['size_bytes']} bytes)",
        f"- Binary SHA-256: `{manifest['binary']['sha256']}`",
        f"- Build: `{shlex.join(manifest['build']['command'])}` in {manifest['build']['duration_seconds']:.3f} s",
        f"- Independent build SHA-256 identical: `{str(manifest['reproducibility']['identical_binary_sha256']).lower()}`",
        "",
        "## Linked libraries",
        "",
    ]
    lines.extend(f"- `{library}`" for library in linked)
    lines += ["", "Reproducibility is an observation for these two builds, not a general guarantee.", ""]
    return "\n".join(lines)


def build_once(target_dir: Path) -> tuple[Path, float]:
    target_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["CARGO_NET_OFFLINE"] = "true"
    started = time.perf_counter()
    run_capture(BUILD_COMMAND, env=env)
    elapsed = time.perf_counter() - started
    binary = target_dir / "release" / "panelink"
    if not binary.is_file():
        raise RuntimeError(f"cargo succeeded but did not produce {binary}")
    return binary, elapsed


def generate_manifest(*, allow_dirty: bool = False) -> dict[str, Any]:
    """Build twice in isolated targets and write JSON/Markdown manifests."""
    status = git_value("status", "--porcelain")
    if status is None:
        raise RuntimeError("cannot read git status")
    dirty = bool(status)
    if dirty and not allow_dirty:
        raise RuntimeError("working tree is dirty; rerun with --allow-dirty to record that fact")

    RELEASE_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
    final_output = RELEASE_DIR / "panelink-release"
    if final_output.exists() or final_output.is_symlink():
        manifest_path = RELEASE_DIR / "MANIFEST.json"
        try:
            existing_manifest = load_manifest(manifest_path)
            existing_digest = manifest_file_digest(existing_manifest, "binary", "sha256")
        except (OSError, ValueError, json.JSONDecodeError) as error:
            raise RuntimeError("refusing to overwrite unowned release/panelink-release") from error
        if final_output.is_symlink() or not final_output.is_file() or sha256_file(final_output) != existing_digest:
            raise RuntimeError("refusing to overwrite release/panelink-release that differs from its manifest")
    for name in ("panelink-release.previous", "panelink-release.tmp"):
        if (RELEASE_DIR / name).exists() or (RELEASE_DIR / name).is_symlink():
            raise RuntimeError(f"refusing to overwrite existing release output: {name}")
    target_base = Path(os.environ.get("CARGO_TARGET_DIR", tempfile.gettempdir()))
    target_base.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="panelink-release-build-", dir=target_base) as temporary:
        private_root = Path(temporary)
        os.chmod(private_root, 0o700)
        first_binary, first_duration = build_once(private_root / "target-first")
        first_digest = sha256_file(first_binary)
        # A fresh private target ensures Cargo cannot reuse prior build outputs.
        second_binary, second_duration = build_once(private_root / "target-second")
        second_digest = sha256_file(second_binary)
        final_binary = RELEASE_DIR / "panelink-release"
        shutil.copy2(first_binary, final_binary)
        final_binary.chmod(0o755)

    git_commit = git_value("rev-parse", "HEAD") or "unknown"
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "source": {
            "commit": git_commit,
            "tag": git_value("describe", "--tags", "--exact-match") or None,
            "dirty": dirty,
            "dirty_override_used": bool(dirty and allow_dirty),
        },
        "toolchain": {
            "rustc": run_capture(["rustc", "-vV"]),
            "cargo": run_capture(["cargo", "--version", "--verbose"]),
        },
        "host": {
            "macos": run_capture(["sw_vers", "-productVersion"]),
            "architecture": platform.machine(),
        },
        "inputs": {
            "cargo_lock_sha256": sha256_file(ROOT / "Cargo.lock"),
            "vendor_tree": vendor_tree_hash(ROOT / "vendor"),
            "notices_sha256": sha256_file(ROOT / "THIRD_PARTY_NOTICES.md"),
        },
        "binary": {
            "path": "release/panelink-release",
            "size_bytes": final_binary.stat().st_size,
            "sha256": sha256_file(final_binary),
            "linked_libraries": run_capture(["otool", "-L", str(final_binary)]).splitlines()[1:],
        },
        "build": {
            "command": BUILD_COMMAND,
            "duration_seconds": round(first_duration, 3),
            "independent_duration_seconds": round(second_duration, 3),
            "target_dirs": [str(first_binary.parents[1]), str(second_binary.parents[1])],
            "target_directories_were_private_and_ephemeral": True,
        },
        "reproducibility": {
            "first_binary_sha256": first_digest,
            "independent_binary_sha256": second_digest,
            "identical_binary_sha256": first_digest == second_digest,
            "observation": (
                "The two fresh-target artifacts have identical SHA-256 digests."
                if first_digest == second_digest
                else "The two fresh-target artifacts differ; compare toolchain, archive ordering, timestamps, and build inputs before drawing broader conclusions."
            ),
        },
    }
    (RELEASE_DIR / "MANIFEST.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (RELEASE_DIR / "MANIFEST.md").write_text(render_markdown(manifest), encoding="utf-8")
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-dirty", action="store_true", help="allow and record a dirty checkout")
    args = parser.parse_args(argv)
    try:
        manifest = generate_manifest(allow_dirty=args.allow_dirty)
    except (OSError, RuntimeError) as error:
        parser.exit(1, f"release_manifest: {error}\n")
    print(
        "release_manifest: "
        f"commit={manifest['source']['commit']} dirty={str(manifest['source']['dirty']).lower()} "
        f"binary_bytes={manifest['binary']['size_bytes']} sha256={manifest['binary']['sha256']} "
        f"independent_sha256_identical={str(manifest['reproducibility']['identical_binary_sha256']).lower()}"
    )
    print("release/MANIFEST.json")
    print("release/MANIFEST.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
