#!/usr/bin/env python3
"""Run two private, offline release-build/resource observations."""

from __future__ import annotations

import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time
import math
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
RAW_DIR = ROOT / "docs" / "progress" / "p6-raw"
RUNS = 2


def percentile(samples: list[float], quantile: float) -> float | None:
    """Nearest-rank percentile, in milliseconds."""
    if not samples:
        return None
    ordered = sorted(samples)
    rank = max(1, math.ceil(quantile * len(ordered)))
    return ordered[min(rank - 1, len(ordered) - 1)]


def aggregate_runs(runs: list[dict[str, Any]]) -> dict[str, Any]:
    """Summarize completed run fields without inventing values for failures."""
    def values(key: str) -> list[float]:
        return [float(run[key]) for run in runs if isinstance(run.get(key), (int, float))]

    clean = values("clean_release_build_ms")
    incremental = values("incremental_rebuild_ms")
    binary = values("release_binary_bytes")
    stripped = values("stripped_binary_bytes")
    return {
        "run_count": len(runs),
        "clean_release_build_ms": {"p50": percentile(clean, 0.50), "p95": percentile(clean, 0.95), "samples": clean},
        "incremental_rebuild_ms": {"p50": percentile(incremental, 0.50), "p95": percentile(incremental, 0.95), "samples": incremental},
        "release_binary_bytes": {"min": min(binary) if binary else None, "max": max(binary) if binary else None, "samples": binary},
        "stripped_binary_bytes": {"min": min(stripped) if stripped else None, "max": max(stripped) if stripped else None, "samples": stripped},
    }


def command_result(command: list[str], env: dict[str, str], cwd: Path) -> dict[str, Any]:
    started = time.perf_counter()
    result = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, check=False)
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    return {
        "command": command,
        "exit_code": result.returncode,
        "elapsed_ms": round(elapsed_ms, 3),
        "stdout": result.stdout,
        "stderr": result.stderr,
    }


def measure_one(run_index: int, base: Path) -> dict[str, Any]:
    target = base / f"target-{run_index}"
    target.mkdir(mode=0o700)
    source_copy = base / f"source-{run_index}"
    shutil.copytree(
        ROOT,
        source_copy,
        ignore=shutil.ignore_patterns(".git", ".codegraph", "target", "__pycache__"),
    )
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target)
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_INCREMENTAL"] = "1"
    clean = command_result(["cargo", "build", "--locked", "--offline", "--release"], env, source_copy)
    run: dict[str, Any] = {
        "run": run_index,
        "private_source_copy": str(source_copy),
        "private_cargo_target_dir": str(target),
        "registry_cache_origin": str(Path.home() / ".cargo" / "registry"),
        "clean_release_build_ms": clean["elapsed_ms"] if clean["exit_code"] == 0 else None,
        "commands": [clean],
    }
    if clean["exit_code"] != 0:
        run["status"] = "blocked"
        return run

    binary = target / "release" / "panelink"
    if not binary.is_file():
        run["status"] = "blocked"
        run["artifact_error"] = "release binary was not created"
        return run
    run["release_binary_bytes"] = binary.stat().st_size

    strip_path = shutil.which("strip")
    if strip_path:
        copy_path = base / f"panelink-{run_index}-stripped-copy"
        strip_result = command_result([strip_path, "-S", "-o", str(copy_path), str(binary)], env, source_copy)
        run["commands"].append(strip_result)
        run["stripped_binary_bytes"] = copy_path.stat().st_size if strip_result["exit_code"] == 0 else None
        run["strip_available"] = True
        run["strip_path"] = strip_path
        copy_path.unlink(missing_ok=True)
    else:
        run["stripped_binary_bytes"] = None
        run["strip_available"] = False

    library = source_copy / "src" / "lib.rs"
    original = library.read_bytes()
    library.write_bytes(original + b"\n")
    try:
        incremental = command_result(
            ["cargo", "build", "--locked", "--offline", "--release"], env, source_copy
        )
    finally:
        library.write_bytes(original)
    run["commands"].append(incremental)
    run["incremental_rebuild_ms"] = incremental["elapsed_ms"] if incremental["exit_code"] == 0 else None
    run["status"] = "passed" if incremental["exit_code"] == 0 else "blocked"
    return run


def main() -> int:
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    host = {
        "system": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "python": platform.python_version(),
    }
    hardware = subprocess.run(
        ["/usr/sbin/sysctl", "-n", "hw.model", "hw.ncpu", "hw.memsize"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    rustc = subprocess.run(["rustc", "--version", "--verbose"], cwd=ROOT, text=True, capture_output=True, check=False)
    cargo = subprocess.run(["cargo", "--version", "--verbose"], cwd=ROOT, text=True, capture_output=True, check=False)
    versions = {
        "rustc": {"exit_code": rustc.returncode, "stdout": rustc.stdout, "stderr": rustc.stderr},
        "cargo": {"exit_code": cargo.returncode, "stdout": cargo.stdout, "stderr": cargo.stderr},
    }
    runs: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="panelink-p6-build-") as temporary:
        base = Path(temporary)
        os.chmod(base, 0o700)
        for index in range(1, RUNS + 1):
            runs.append(measure_one(index, base))
    raw = {
        "schema_version": 1,
        "synthetic": False,
        "host": host,
        "hardware_query": {
            "command": ["/usr/sbin/sysctl", "-n", "hw.model", "hw.ncpu", "hw.memsize"],
            "exit_code": hardware.returncode,
            "stdout": hardware.stdout,
            "stderr": hardware.stderr,
        },
        "toolchain": versions,
        "runs": runs,
        "aggregate": aggregate_runs(runs),
        "method": "Two independent fresh private CARGO_TARGET_DIR directories; each clean release build followed by a rebuild after touching src/lib.rs. Release artifact copied to private temp before optional strip; repository binary/source bytes are never modified.",
    }
    output = RAW_DIR / "build-measurements.json"
    output.write_text(json.dumps(raw, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"measure_build: runs={len(runs)} statuses={','.join(run['status'] for run in runs)}")
    for run in runs:
        print(
            f"run{run['run']}: clean_ms={run.get('clean_release_build_ms')} "
            f"incremental_ms={run.get('incremental_rebuild_ms')} "
            f"binary_bytes={run.get('release_binary_bytes')} "
            f"stripped_bytes={run.get('stripped_binary_bytes')} status={run['status']}"
        )
    print(f"raw JSON: {output.relative_to(ROOT)}")
    return 0 if all(run["status"] == "passed" for run in runs) else 1


if __name__ == "__main__":
    raise SystemExit(main())
