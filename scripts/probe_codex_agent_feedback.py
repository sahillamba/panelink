#!/usr/bin/env python3
"""Probe recognized Codex /status in a private Panelink workspace.

Requires macOS ARM64, an explicitly selected Panelink binary and Codex executable.
Uses a private Codex home, dummy credential and a loopback fixture provider.
Sends one local /status command, never a model prompt. Evidence stays in the
printed temporary directory. User workspaces and Codex homes are not opened.
"""

import argparse
import json
import os
import signal
from pathlib import Path
import subprocess
import tempfile
import time


def stop_private_updater(home):
    """Stop only the updater recorded in this fixture's private home.

    Codex daemon stop leaves its updater alive in 0.164. Verify both process
    start time and private executable path immediately before each signal.
    """
    record = home / "app-server-daemon/daemon-updater.pid"
    if not record.exists():
        return True
    saved = json.loads(record.read_text(encoding="utf-8"))
    pid = saved["pid"]
    expected_start = " ".join(saved["processStartTime"].split())

    def owned_process():
        info = subprocess.run(
            ["/bin/ps", "-p", str(pid), "-o", "lstart=", "-o", "comm="],
            capture_output=True, text=True, check=False,
        )
        if info.returncode != 0 or not info.stdout.strip():
            return False
        fields = info.stdout.strip().split(None, 5)
        if len(fields) != 6 or " ".join(fields[:5]) != expected_start:
            return False
        executable = Path(fields[5]).resolve()
        return executable.name == "codex" and executable.is_relative_to(home.resolve() / "packages")

    if not owned_process():
        return True
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline and owned_process():
        time.sleep(0.05)
    if owned_process():
        os.kill(pid, signal.SIGKILL)
    return not owned_process()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--codex", required=True, type=Path)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    codex = str(args.codex.absolute())
    repo = Path(__file__).resolve().parents[1]
    root = Path(tempfile.mkdtemp(prefix="panelink-feedback-runtime-", dir="/private/tmp"))
    root.chmod(0o700)
    state, home, terminfo = (root / name for name in ("state", "home", "terminfo"))
    for directory in (state, home, terminfo):
        directory.mkdir(mode=0o700)
    server = state / "server"
    server.mkdir(mode=0o700)
    (home / "config.toml").write_text(
        'model_provider="fixture"\nmodel="fixture"\n'
        '[model_providers.fixture]\nname="Fixture"\n'
        'base_url="http://127.0.0.1:9/v1"\nenv_key="PANELINK_FIXTURE_KEY"\n'
        'wire_api="responses"\n'
        f'[projects."{root}"]\ntrust_level="trusted"\n',
        encoding="utf-8",
    )
    environment = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "SHELL": "/bin/sh",
        "TERM": "xterm-256color",
        "PANELINK_HOME": str(state),
        "CODEX_HOME": str(home),
        "PANELINK_FIXTURE_KEY": "fixture-not-a-secret",
    }

    def cli(*words):
        result = subprocess.run(
            [binary, *words], env=environment, cwd=root,
            stdin=subprocess.DEVNULL, capture_output=True, timeout=12, check=False,
        )
        return result.returncode, json.loads(result.stdout)

    subprocess.run(
        ["/usr/bin/tic", "-x", "-o", str(terminfo),
         str(repo / "docs/terminfo/panelink-probe.terminfo")], check=True,
    )
    evidence = {}
    with (root / "server.log").open("wb") as log:
        process = subprocess.Popen(
            [binary, "serve", "--directory", str(server), "--terminfo", str(terminfo),
             "--initialize", "--allow-input", "--", codex, "--no-daemon", "--no-alt-screen"],
            env=environment, cwd=root, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        )
        try:
            deadline = time.monotonic() + 20
            observation = None
            stable_revision = None
            stable_since = time.monotonic()
            while time.monotonic() < deadline:
                if server.joinpath("control.sock").exists():
                    code, observation = cli("agent", "list", "--json")
                    agents = observation.get("agents", [])
                    if code == 0 and agents and agents[0]["state"] == "idle":
                        revision = agents[0]["screen_revision"]
                        if revision != stable_revision:
                            stable_revision, stable_since = revision, time.monotonic()
                        elif time.monotonic() - stable_since >= 0.5:
                            break
                    else:
                        stable_revision, stable_since = None, time.monotonic()
                if process.poll() is not None:
                    raise RuntimeError("private server exited before readiness")
                time.sleep(0.1)
            else:
                raise RuntimeError(f"Codex idle not observed: {observation}")
            evidence["agent"] = observation
            code, evidence["server_info"] = cli("server-info", "--json")
            assert code == 0, evidence
            # Repeated observation must reuse its dedicated caller context.
            code, second_identity = cli("server-info", "--json")
            assert code == 0 and second_identity == evidence["server_info"], second_identity
            reference = observation["agents"][0]["agent_ref"]
            assert reference
            code, evidence["submission"] = cli("agent", "prompt", reference, "/status", "--json")
            assert code == 0, evidence
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                code, screen = cli("pane", "read", "1", "--lines", "80", "--json")
                if code == 0 and "Token usage:" in screen["text"]:
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError(f"local status response not observed: {screen}")
            evidence["application_status_observed"] = True
            evidence["zero_tokens"] = "0 total" in screen["text"]
            assert evidence["zero_tokens"], screen
            code, evidence["bottom_read"] = cli(
                "pane", "read", "1", "--bottom", "--lines", "4", "--json",
            )
            assert code == 0, evidence
        finally:
            try:
                subprocess.run(
                    [binary, "stop"], env=environment, cwd=root,
                    stdin=subprocess.DEVNULL, capture_output=True, timeout=8, check=False,
                )
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                # Codex 0.164 can leave its daemon/updater alive after the TUI.
                # Its supported stop command uses only this private CODEX_HOME.
                if (home / "app-server-daemon/daemon.pid").exists():
                    cleanup = subprocess.run(
                        [codex, "app-server", "daemon", "stop"], env=environment, cwd=root,
                        stdin=subprocess.DEVNULL, capture_output=True, timeout=15, check=False,
                    )
                    evidence["private_codex_daemon_cleanup_exit"] = cleanup.returncode
                    evidence["private_codex_updater_stopped"] = stop_private_updater(home)
                else:
                    evidence["private_codex_daemon_started"] = False
    result = json.dumps(evidence, indent=2)
    (root / "result.json").write_text(result, encoding="utf-8")
    print(result)
    print(f"evidence {root}")


if __name__ == "__main__":
    main()
