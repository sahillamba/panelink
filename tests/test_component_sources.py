import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from unittest.mock import patch

from scripts import check_component_sources as checker


class ComponentSourceVerifierTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.vt = self.root / "vendor" / "vt100"
        self.ct = self.root / "vendor" / "crossterm-input"
        self.vt.mkdir(parents=True)
        self.ct.mkdir(parents=True)
        self._write_component(self.vt, "PROVENANCE.json", "files")
        self._write_component(self.ct, "FINGERPRINTS.json", "fingerprints")

    def tearDown(self):
        self.temporary.cleanup()

    @staticmethod
    def _write_component(component_root, manifest_name, shape, path="src/sample.rs", data=b"source"):
        source = component_root / path
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(data)
        entries = {path: hashlib.sha256(data).hexdigest()}
        document = {"upstream_crate": "fixture", "files": entries} if shape == "files" else entries
        (component_root / manifest_name).write_text(json.dumps(document), encoding="utf-8")

    def test_matching_fingerprints_pass(self):
        code, summary = checker.verify_sources(self.root)
        self.assertEqual(code, 0)
        self.assertEqual(summary["result"], "match")
        self.assertEqual(
            summary["components"],
            [
                {"component": "vt100", "result": "match"},
                {"component": "crossterm-input", "result": "match"},
            ],
        )

    def test_mismatch_and_missing_file_are_fixed_redacted_failures(self):
        (self.ct / "src/sample.rs").unlink()
        code, summary = checker.verify_sources(self.root)
        self.assertEqual(code, 1)
        self.assertEqual(summary["result"], "invalid")
        self.assertEqual(summary["components"], [{"component": "crossterm-input", "error": "missing_file"}])

        self._write_component(self.ct, "FINGERPRINTS.json", "fingerprints")
        (self.vt / "src/sample.rs").write_bytes(b"changed")
        code, summary = checker.verify_sources(self.root)
        self.assertEqual(code, 1)
        failures = summary["failures"]
        self.assertEqual(
            failures,
            [{"component": "vt100", "error": "hash_mismatch"}],
        )
        self.assertNotIn("src/sample.rs", json.dumps(summary))

    def test_unsafe_paths_are_rejected_before_any_source_content_read(self):
        for unsafe in ("../escape", "/absolute/escape", "C:/drive/escape"):
            with self.subTest(path=unsafe):
                self._write_component(self.vt, "PROVENANCE.json", "files", path="src/safe.rs")
                manifest = {"files": {unsafe: "0" * 64}}
                (self.vt / "PROVENANCE.json").write_text(json.dumps(manifest), encoding="utf-8")
                with patch.object(checker, "_source_digest") as digest:
                    code, summary = checker.verify_sources(self.root)
                self.assertEqual(code, 1)
                self.assertEqual(summary["components"], [{"component": "vt100", "error": "unsafe_path"}])
                digest.assert_not_called()
                self.assertNotIn(unsafe, json.dumps(summary))

    def test_symlink_file_is_rejected_before_any_source_content_read(self):
        outside = self.root.parent / (self.root.name + "-outside-secret")
        outside.write_bytes(b"must not be read")
        try:
            link = self.vt / "src" / "linked.rs"
            link.symlink_to(outside)
            (self.vt / "PROVENANCE.json").write_text(
                json.dumps({"files": {"src/linked.rs": hashlib.sha256(b"must not be read").hexdigest()}}),
                encoding="utf-8",
            )
            with patch.object(checker, "_source_digest") as digest:
                code, summary = checker.verify_sources(self.root)
            self.assertEqual(code, 1)
            self.assertEqual(summary["components"], [{"component": "vt100", "error": "unsafe_path"}])
            digest.assert_not_called()
        finally:
            outside.unlink(missing_ok=True)

    def test_fixed_manifest_symlink_is_not_followed(self):
        external_manifest = self.root.parent / (self.root.name + "-manifest-secret")
        external_manifest.write_text(json.dumps({"files": {}}), encoding="utf-8")
        manifest = self.vt / "PROVENANCE.json"
        manifest.unlink()
        manifest.symlink_to(external_manifest)
        try:
            with patch.object(checker, "_source_digest") as digest:
                code, summary = checker.verify_sources(self.root)
            self.assertEqual(code, 1)
            self.assertEqual(summary["components"], [{"component": "vt100", "error": "unsafe_path"}])
            digest.assert_not_called()
        finally:
            external_manifest.unlink(missing_ok=True)

    def test_fifo_manifest_and_source_fail_without_blocking(self):
        script = (
            "import json,sys; from pathlib import Path; "
            "from scripts import check_component_sources as c; "
            "print(json.dumps(c.verify_sources(Path(sys.argv[1]))[1]))"
        )
        cases = (
            (self.vt / "PROVENANCE.json", "manifest"),
            (self.vt / "src/sample.rs", "source"),
        )
        for path, label in cases:
            with self.subTest(label=label):
                if path.exists() or path.is_symlink():
                    path.unlink()
                os.mkfifo(path)
                try:
                    result = subprocess.run(
                        [sys.executable, "-c", script, str(self.root)],
                        cwd=Path(__file__).resolve().parent.parent,
                        capture_output=True,
                        text=True,
                        timeout=2,
                        check=False,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    summary = json.loads(result.stdout)
                    self.assertEqual(summary["components"], [{"component": "vt100", "error": "unsafe_path"}])
                finally:
                    path.unlink(missing_ok=True)
                self._write_component(self.vt, "PROVENANCE.json", "files")

    def test_actual_cumulative_bytes_stay_bounded_after_preflight(self):
        source_dir = self.vt / "src"
        source_dir.mkdir(exist_ok=True)
        old = b"A" * 900_000
        grown = b"B" * 1_000_000
        entries = {}
        for index in range(9):
            relative = f"src/{index}.rs"
            (self.vt / relative).write_bytes(old)
            entries[relative] = hashlib.sha256(grown).hexdigest()
        (self.vt / "PROVENANCE.json").write_text(
            json.dumps({"files": entries}), encoding="utf-8"
        )

        original_digest = checker._source_digest
        original_read = checker._read_beneath
        mutated = False
        source_bytes_read = 0

        def grow_sources_then_digest(root, parts, limit, limit_error):
            nonlocal mutated
            if not mutated:
                for index in range(9):
                    (self.vt / "src" / f"{index}.rs").write_bytes(grown)
                mutated = True
            return original_digest(root, parts, limit, limit_error)

        def count_source_reads(root, parts, *args, **kwargs):
            nonlocal source_bytes_read
            content = original_read(root, parts, *args, **kwargs)
            if parts[:2] == ("vendor", "vt100") and parts[-1].endswith(".rs"):
                source_bytes_read += len(content)
            return content

        with patch.object(checker, "_source_digest", side_effect=grow_sources_then_digest):
            with patch.object(checker, "_read_beneath", side_effect=count_source_reads):
                code, summary = checker.verify_sources(self.root)

        self.assertEqual(code, 1)
        self.assertIn(
            {"component": "vt100", "error": "oversize_component"},
            summary["failures"],
        )
        self.assertEqual(source_bytes_read, 8 * 1_000_000)
        self.assertLessEqual(source_bytes_read, checker.MAX_COMPONENT_BYTES)

    def test_malformed_json_hash_and_duplicate_key_fail_before_source_reads(self):
        cases = (
            b"{not json",
            b'{"files":{"src/sample.rs":"BAD"}}',
            b'{"files":{"src/sample.rs":"' + b"0" * 64 + b'","src/sample.rs":"' + b"0" * 64 + b'"}}',
        )
        for content in cases:
            with self.subTest(content_length=len(content)):
                (self.vt / "PROVENANCE.json").write_bytes(content)
                with patch.object(checker, "_source_digest") as digest:
                    code, summary = checker.verify_sources(self.root)
                self.assertEqual(code, 1)
                self.assertEqual(summary["components"], [{"component": "vt100", "error": "malformed_manifest"}])
                digest.assert_not_called()
        self._write_component(self.vt, "PROVENANCE.json", "files")

    def test_oversize_manifest_and_file_are_bounded(self):
        (self.vt / "PROVENANCE.json").write_bytes(b" " * (checker.MAX_MANIFEST_BYTES + 1))
        code, summary = checker.verify_sources(self.root)
        self.assertEqual(code, 1)
        self.assertEqual(summary["components"], [{"component": "vt100", "error": "oversize_manifest"}])

        self._write_component(self.vt, "PROVENANCE.json", "files", data=b"x" * (checker.MAX_FILE_BYTES + 1))
        code, summary = checker.verify_sources(self.root)
        self.assertEqual(code, 1)
        self.assertEqual(summary["components"], [{"component": "vt100", "error": "oversize_file"}])

    def test_cli_rejects_arbitrary_paths_with_fixed_usage(self):
        output = StringIO()
        with patch.object(checker, "verify_sources") as verify:
            with redirect_stderr(output):
                code = checker.main(["--root", "/private/secret"])
        self.assertEqual(code, 2)
        verify.assert_not_called()
        self.assertNotIn("/private/secret", output.getvalue())
        self.assertIn("Usage:", output.getvalue())


if __name__ == "__main__":
    unittest.main()
