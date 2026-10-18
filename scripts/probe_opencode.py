#!/usr/bin/env python3
"""Bounded, read-only OpenCode CLI availability preflight (not an identity test)."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable


TIMEOUT_SECONDS = 5.0
MAX_CAPTURE_BYTES = 4096
VERSION = re.compile(
    r"(?im)^\s*(?:opencode\s+)?v?(\d+\.\d+\.\d+(?:-[0-9a-z][0-9a-z.-]*)?(?:\+[0-9a-z.-]+)?)\s*$"
)


@dataclass(frozen=True)
class CaptureResult:
    returncode: int
    stdout: bytes
    stderr: bytes
    reason: str | None = None


def _kill_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        try:
            process.kill()
        except ProcessLookupError:
            pass


def _run_bounded(
    argv: list[str], *, cwd: str, env: dict[str, str], stdin: int,
    stdout: int, stderr: int, timeout: float, check: bool, shell: bool,
) -> CaptureResult:
    """Capture at most MAX_CAPTURE_BYTES and kill the isolated process group."""
    del check, shell  # The caller fixes these to False; keep a subprocess-like test seam.
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        env=env,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        shell=False,
        start_new_session=True,
        bufsize=0,
    )
    streams = {process.stdout: bytearray(), process.stderr: bytearray()}
    selector = selectors.DefaultSelector()
    reason: str | None = None
    total = 0
    deadline = time.monotonic() + timeout
    try:
        for stream in streams:
            assert stream is not None
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ)

        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                reason = "timed_out"
                break
            for key, _ in selector.select(min(remaining, 0.1)):
                stream = key.fileobj
                allowance = MAX_CAPTURE_BYTES - total
                try:
                    chunk = os.read(stream.fileno(), min(4096, allowance + 1))
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(stream)
                    stream.close()
                    continue
                if len(chunk) > allowance:
                    streams[stream].extend(chunk[:allowance])
                    total += allowance
                    reason = "output_limit"
                    break
                streams[stream].extend(chunk)
                total += len(chunk)
            if reason:
                break

        if reason:
            _kill_group(process)
        try:
            returncode = process.wait(timeout=1.0 if reason else max(0.0, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            reason = reason or "timed_out"
            _kill_group(process)
            returncode = process.wait(timeout=1.0)
        return CaptureResult(returncode, bytes(streams[process.stdout]), bytes(streams[process.stderr]), reason)
    except BaseException:
        _kill_group(process)
        try:
            process.wait(timeout=1.0)
        except subprocess.TimeoutExpired:
            pass
        raise
    finally:
        selector.close()
        for stream in streams:
            if stream is not None and not stream.closed:
                stream.close()


def _blocked(reason: str) -> dict[str, Any]:
    return {
        "schema": 1,
        "check": "opencode_cli_preflight",
        "status": "blocked",
        "reason": reason,
        "version": None,
        "diagnostic": f"blocked:{reason}",
        "live_identity_tested": False,
    }


def probe(
    executable: str | None = None,
    *,
    which: Callable[[str], str | None] = shutil.which,
    runner: Callable[..., CaptureResult] = _run_bounded,
    timeout: float = TIMEOUT_SECONDS,
) -> dict[str, Any]:
    """Check only that an executable returns a recognizable version string."""
    found_by = "explicit" if executable else "path"
    candidate = executable if executable else which("opencode")
    if not candidate:
        outcome = _blocked("missing_executable")
        outcome["discovery"] = found_by
        return outcome

    path = Path(candidate).expanduser()
    try:
        path = path.resolve(strict=True)
    except (OSError, RuntimeError):
        outcome = _blocked("invalid_executable")
        outcome["discovery"] = found_by
        return outcome
    if not path.is_file() or not os.access(path, os.X_OK):
        outcome = _blocked("invalid_executable")
        outcome["discovery"] = found_by
        return outcome

    try:
        with tempfile.TemporaryDirectory(prefix="panelink-opencode-probe-") as temp:
            root = Path(temp)
            config, cache, data = (root / name for name in ("config", "cache", "data"))
            for directory in (config, cache, data):
                directory.mkdir()
            child_env = {
                "HOME": temp,
                "XDG_CONFIG_HOME": str(config),
                "XDG_CACHE_HOME": str(cache),
                "XDG_DATA_HOME": str(data),
                "TMPDIR": temp,
                "PATH": os.defpath,
                "LANG": "C",
                "LC_ALL": "C",
                "OPENCODE_DISABLE_MODELS_FETCH": "1",
            }
            result = runner(
                [str(path), "--version"],
                cwd=temp,
                env=child_env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout,
                check=False,
                shell=False,
            )
            if result.reason:
                outcome = _blocked(result.reason)
            elif result.returncode != 0:
                outcome = _blocked("execution_failed")
            else:
                match = VERSION.search(
                    result.stdout.decode("utf-8", errors="replace")
                    + "\n"
                    + result.stderr.decode("utf-8", errors="replace")
                )
                if not match:
                    outcome = _blocked("unrecognized_version")
                else:
                    outcome = {
                        "schema": 1,
                        "check": "opencode_cli_preflight",
                        "status": "available",
                        "reason": None,
                        "version": match.group(1),
                        "diagnostic": "version_only",
                        "live_identity_tested": False,
                    }
            outcome["discovery"] = found_by
            return outcome
    except OSError:
        outcome = _blocked("execution_failed")
        outcome["discovery"] = found_by
        return outcome


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", help="explicit OpenCode executable path (otherwise search PATH)")
    args = parser.parse_args(argv)
    outcome = probe(args.executable)
    print(json.dumps(outcome, sort_keys=True))
    return 0 if outcome["status"] == "available" else 2


if __name__ == "__main__":
    sys.exit(main())
