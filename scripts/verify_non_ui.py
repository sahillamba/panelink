#!/usr/bin/env python3
"""Run bounded offline checks for delivered non-UI components."""

from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from typing import Callable, Mapping, Sequence


REPO_ROOT = Path(__file__).resolve().parent.parent
COMMANDS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("cargo_test", ("cargo", "test", "--locked", "--offline")),
    (
        "crossterm_input_decoder_tests",
        (
            "cargo",
            "test",
            "--locked",
            "--offline",
            "--manifest-path",
            "vendor/crossterm-input/Cargo.toml",
        ),
    ),
    ("component_sources", ("python3", "scripts/check_component_sources.py")),
    ("cargo_fmt", ("cargo", "fmt", "--check")),
    ("cargo_clippy", ("cargo", "clippy", "--locked", "--offline", "--all-targets", "--", "-D", "warnings")),
    ("cargo_build", ("cargo", "build", "--locked", "--offline")),
    ("python_unittest", ("python3", "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py")),
    ("docs_check", ("python3", "scripts/check_docs.py")),
)
COMMAND_TIMEOUT_SECONDS = 420


def _cargo_home(environment: Mapping[str, str]) -> str | None:
    """Select the caller's Cargo home or the established writable cache."""
    supplied = environment.get("CARGO_HOME")
    if supplied is not None:
        if not supplied:
            raise ValueError("invalid_cargo_home")
        # Resolve in the invoking process before child cwd changes to the repo.
        path = Path(supplied).expanduser().resolve()
        if not path.is_dir() or not os.access(path, os.W_OK | os.X_OK):
            raise ValueError("invalid_cargo_home")
        return str(path)

    # Prefer the standard rustup/Cargo home when it holds a populated registry;
    # the temporary cache was only needed in a restricted environment and can
    # be recreated empty, which would break offline builds.
    standard = Path.home() / ".cargo"
    if (standard / "registry").is_dir() and os.access(standard, os.W_OK | os.X_OK):
        return str(standard)
    fallback = Path("/private/tmp/panelink-p2-cargo-home")
    if fallback.is_dir() and os.access(fallback, os.W_OK | os.X_OK):
        return str(fallback)
    return None


def _valid_root(root: Path) -> bool:
    return root.is_dir() and (root / "Cargo.toml").is_file() and (root / "scripts/check_docs.py").is_file()


def _stop_process_group(process: subprocess.Popen[bytes], *, reap_timeout: int = 2) -> bool:
    """Kill the isolated group and report whether the direct child was reaped."""
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except OSError:
        try:
            process.kill()
        except OSError:
            pass
    try:
        process.wait(timeout=reap_timeout)
    except (OSError, subprocess.TimeoutExpired):
        return False
    return True


def run_process(
    argv: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    timeout: int = COMMAND_TIMEOUT_SECONDS,
    popen: Callable[..., subprocess.Popen[bytes]] = subprocess.Popen,
) -> str:
    """Run one fixed command, inheriting output and reaping it on every path."""
    try:
        process = popen(
            list(argv),
            cwd=cwd,
            env=dict(env),
            stdin=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError:
        return "error"

    try:
        return "passed" if process.wait(timeout=timeout) == 0 else "failed"
    except subprocess.TimeoutExpired:
        return "timed_out" if _stop_process_group(process) else "timed_out_cleanup_incomplete"
    except (KeyboardInterrupt, SystemExit):
        _stop_process_group(process)
        raise
    except Exception:
        _stop_process_group(process)
        return "error"


def run_checks(
    *,
    repo_root: Path = REPO_ROOT,
    environ: Mapping[str, str] | None = None,
    process_runner: Callable[..., str] = run_process,
) -> tuple[int, dict[str, object]]:
    """Run the fixed check list and return a sanitized summary and exit code."""
    root = Path(repo_root).resolve()
    source_environment = os.environ if environ is None else environ
    try:
        cargo_home = _cargo_home(source_environment)
    except (OSError, ValueError):
        return 2, _summary([], setup_status="invalid")
    if not _valid_root(root):
        return 2, _summary([], setup_status="invalid")

    child_environment = dict(source_environment)
    if cargo_home is not None:
        child_environment["CARGO_HOME"] = cargo_home

    checks: list[dict[str, str]] = []
    for name, argv in COMMANDS:
        try:
            status = process_runner(
                argv,
                cwd=root,
                env=child_environment,
                timeout=COMMAND_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            status = "timed_out"
        except (KeyboardInterrupt, SystemExit):
            raise
        except Exception:
            status = "error"
        if status not in {"passed", "failed", "timed_out", "timed_out_cleanup_incomplete", "error"}:
            status = "error"
        checks.append({"name": name, "status": status})

    code = 0 if all(item["status"] == "passed" for item in checks) else 1
    return code, _summary(checks)


def _summary(checks: list[dict[str, str]], *, setup_status: str = "valid") -> dict[str, object]:
    return {
        "component_verification_only": True,
        "milestones_complete": False,
        "host_provider_gates": "not_certified",
        "setup_status": setup_status,
        "checks": checks,
    }


def main(argv: Sequence[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else list(argv)
    if arguments == ["--help"]:
        print("Usage: python3 scripts/verify_non_ui.py\nRun fixed offline checks for delivered non-UI components.")
        return 0
    if arguments:
        print(json.dumps(_summary([], setup_status="invalid"), sort_keys=True))
        return 2
    try:
        code, summary = run_checks()
    except KeyboardInterrupt:
        print(json.dumps(_summary([], setup_status="interrupted"), sort_keys=True))
        return 130
    print(json.dumps(summary, sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
