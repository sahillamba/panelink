"""Tests for release manifest helpers and the local installer lifecycle."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts import release_manifest


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class ManifestHelperTests(unittest.TestCase):
    def test_GIVEN_successful_empty_git_status_WHEN_read_THEN_is_clean_not_unavailable(self) -> None:
        completed = subprocess.CompletedProcess(["git"], 0, stdout="", stderr="")
        with patch.object(release_manifest.subprocess, "run", return_value=completed):
            self.assertEqual("", release_manifest.git_value("status", "--porcelain"))

    def test_GIVEN_dirty_checkout_without_override_WHEN_manifest_requested_THEN_build_is_refused(self) -> None:
        with patch.object(release_manifest, "git_value", return_value=" M src/main.rs"):
            with self.assertRaisesRegex(RuntimeError, "working tree is dirty"):
                release_manifest.generate_manifest()

    def test_GIVEN_file_and_sorted_vendor_tree_WHEN_hashed_THEN_stable_content_digests_are_returned(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            one = root / "one"
            vendor = root / "vendor"
            one.write_bytes(b"artifact")
            (vendor / "z").mkdir(parents=True)
            (vendor / "a").mkdir()
            (vendor / "z" / "last.txt").write_bytes(b"last")
            (vendor / "a" / "first.txt").write_bytes(b"first")

            self.assertEqual(digest(b"artifact"), release_manifest.sha256_file(one))
            result = release_manifest.vendor_tree_hash(vendor)
            self.assertEqual(["a/first.txt", "z/last.txt"], [item["path"] for item in result["files"]])
            self.assertEqual(2, result["file_count"])
            self.assertEqual(result, release_manifest.vendor_tree_hash(vendor))

    def test_GIVEN_manifest_json_WHEN_loaded_THEN_binary_and_notice_hashes_are_validated(self) -> None:
        manifest = {
            "schema_version": 1,
            "binary": {"sha256": digest(b"binary")},
            "inputs": {"notices_sha256": digest(b"notices")},
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            loaded = release_manifest.load_manifest(path)
        self.assertEqual(manifest, loaded)
        self.assertEqual(digest(b"binary"), release_manifest.manifest_file_digest(loaded, "binary", "sha256"))

    def test_GIVEN_malformed_or_wrong_schema_manifest_WHEN_loaded_THEN_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            for content in ("not json", '{"schema_version":2}', '{"schema_version":1,"binary":{},"inputs":{}}'):
                with self.subTest(content=content):
                    path.write_text(content, encoding="utf-8")
                    with self.assertRaises((json.JSONDecodeError, ValueError)):
                        release_manifest.load_manifest(path)


class InstallerLifecycleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="panelink-install-test-")
        self.root = Path(self.temporary.name)
        self.fixture = self.root / "checkout"
        self.script_dir = self.fixture / "scripts"
        self.release_dir = self.fixture / "release"
        self.prefix = self.root / "prefix"
        self.home = self.root / "home"
        self.state = self.home / "isolated-panelink-state"
        self.script_dir.mkdir(parents=True)
        self.release_dir.mkdir()
        (self.fixture / "THIRD_PARTY_NOTICES.md").write_bytes((ROOT / "THIRD_PARTY_NOTICES.md").read_bytes())
        shutil.copy2(ROOT / "scripts" / "install.sh", self.script_dir / "install.sh")
        shutil.copy2(ROOT / "scripts" / "release_manifest.py", self.script_dir / "release_manifest.py")
        (self.script_dir / "install.sh").chmod(0o755)
        self.home.mkdir()
        self.state.mkdir(parents=True)
        self.sentinel = self.state / "saved-history-sentinel"
        self.sentinel.write_bytes(b"preserve state")
        self.env = os.environ.copy()
        self.env.update(
            {
                "HOME": str(self.home),
                "PREFIX": str(self.prefix),
                "PANELINK_HOME": str(self.state),
            }
        )
        self.write_release("release-v1")
        self.dev_target = self.root / "checkout" / "bin" / "panelink"
        self.dev_target.parent.mkdir()
        self.dev_target.write_text("development launcher placeholder\n", encoding="utf-8")
        self.dev_link = self.prefix / "bin" / "panelink"
        self.dev_link.parent.mkdir(parents=True)
        self.dev_link.symlink_to(self.dev_target)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def dummy_binary(self, version: str) -> bytes:
        panes_case = "  panes) printf '%s\\n' 'panelink: no workspace is running' >&2; exit 1 ;;\n"
        return (
            "#!/bin/sh\n"
            'case ${1-} in\n'
            f'  --version) printf "%s\\n" "{version}" ;;\n'
            + panes_case
            +
            "  *) exit 0 ;;\n"
            "esac\n"
        ).encode()

    def write_release(self, version: str) -> None:
        binary = self.release_dir / "panelink-release"
        binary.write_bytes(self.dummy_binary(version))
        binary.chmod(0o755)
        notices = self.fixture / "THIRD_PARTY_NOTICES.md"
        manifest = {
            "schema_version": 1,
            "source": {"commit": "test-fixture", "tag": None, "dirty": True},
            "inputs": {"notices_sha256": release_manifest.sha256_file(notices)},
            "binary": {
                "path": "release/panelink-release",
                "sha256": release_manifest.sha256_file(binary),
                "size_bytes": binary.stat().st_size,
            },
        }
        (self.release_dir / "MANIFEST.json").write_text(json.dumps(manifest), encoding="utf-8")

    def run_installer(self, *arguments: str, expected: int = 0) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            ["/bin/sh", str(self.script_dir / "install.sh"), *arguments],
            env=self.env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(expected, result.returncode, msg=f"stdout={result.stdout}\nstderr={result.stderr}")
        return result

    def version(self) -> str:
        result = subprocess.run(
            [str(self.prefix / "bin" / "panelink-release"), "--version"],
            env=self.env,
            text=True,
            capture_output=True,
            check=True,
        )
        return result.stdout.strip()

    def test_GIVEN_release_fixture_WHEN_installed_upgraded_rolled_back_and_uninstalled_THEN_only_tracked_files_change(self) -> None:
        install = self.run_installer("install")
        self.assertIn("installed", install.stdout)
        self.assertEqual("release-v1", self.version())
        self.assertTrue(self.dev_link.is_symlink())
        self.assertTrue(os.path.samefile(self.dev_target, self.dev_link))

        self.write_release("release-v2")
        upgrade = self.run_installer("upgrade")
        self.assertIn("previous binary saved", upgrade.stdout)
        self.assertEqual("release-v2", self.version())
        self.assertEqual("release-v1", subprocess.run(
            [str(self.prefix / "bin" / "panelink-release.previous"), "--version"],
            env=self.env, text=True, capture_output=True, check=True,
        ).stdout.strip())

        rollback = self.run_installer("rollback")
        self.assertIn("rolled back", rollback.stdout)
        self.assertEqual("release-v1", self.version())

        uninstall = self.run_installer("uninstall")
        self.assertIn("state was preserved", uninstall.stdout)
        self.assertFalse((self.prefix / "bin" / "panelink-release").exists())
        self.assertFalse((self.prefix / "bin" / "panelink-release.previous").exists())
        self.assertFalse((self.prefix / "share" / "panelink" / "THIRD_PARTY_NOTICES.md").exists())
        self.assertFalse((self.prefix / "share" / "panelink" / "INSTALLED").exists())
        self.assertEqual("development launcher placeholder\n", self.dev_target.read_text(encoding="utf-8"))
        self.assertTrue(self.dev_link.is_symlink())
        self.assertEqual(b"preserve state", self.sentinel.read_bytes())

    def test_GIVEN_modified_artifact_WHEN_install_requested_THEN_manifest_mismatch_prevents_copy(self) -> None:
        (self.release_dir / "panelink-release").write_bytes(self.dummy_binary("tampered"))
        result = self.run_installer("install", expected=1)
        self.assertIn("SHA-256 does not match manifest", result.stderr)
        self.assertFalse((self.prefix / "bin" / "panelink-release").exists())
        self.assertTrue(self.sentinel.exists())

    def test_GIVEN_purge_without_confirmation_WHEN_uninstall_requested_THEN_fails_without_state_change(self) -> None:
        self.run_installer("install")
        result = self.run_installer("uninstall", "--purge-state", expected=1)
        self.assertIn("requires both", result.stderr)
        self.assertTrue(self.sentinel.exists())

    def test_GIVEN_workspace_running_WHEN_upgrade_requested_THEN_refuses_with_stop_hint(self) -> None:
        self.run_installer("install")
        binary = self.prefix / "bin" / "panelink-release"
        binary.write_text(
            "#!/bin/sh\ncase ${1-} in\n"
            "  --version) printf '%s\\n' 'old-release' ;;\n"
            "  panes) printf '%s\\n' '1 pane' ;;\n"
            "esac\n",
            encoding="utf-8",
        )
        binary.chmod(0o755)
        result = self.run_installer("upgrade", expected=1)
        self.assertIn("workspace is running", result.stderr)
        self.assertIn("panelink stop", result.stderr)
        self.assertEqual("old-release", self.version())

    def test_GIVEN_replace_dev_option_WHEN_installed_then_uninstalled_THEN_original_symlink_is_restored(self) -> None:
        self.run_installer("install", "--replace-dev")
        self.assertEqual("panelink-release", os.readlink(self.dev_link))
        self.assertTrue(os.path.samefile(self.dev_target, self.prefix / "bin" / "panelink.previous"))
        self.run_installer("uninstall")
        self.assertTrue(os.path.samefile(self.dev_target, self.dev_link))
        self.assertFalse((self.prefix / "bin" / "panelink.previous").exists())

if __name__ == "__main__":
    unittest.main()
