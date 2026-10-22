"""Unit tests for Phase 6 evidence parsing and aggregation helpers."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import dependency_inventory
import measure_build


class DependencyInventoryTests(unittest.TestCase):
    def test_GIVEN_locked_metadata_WHEN_aggregated_THEN_reports_direct_transitive_and_review_flags(self) -> None:
        metadata = {
            "resolve": {
                "root": "panelink 0.1.0 (path+file:///fixture)",
                "nodes": [
                    {
                        "id": "panelink 0.1.0 (path+file:///fixture)",
                        "deps": [
                            {"pkg": "root-crate 1.0.0 (registry+https://example.invalid)", "dep_kinds": [{"kind": None}]}
                        ],
                    },
                    {
                        "id": "root-crate 1.0.0 (registry+https://example.invalid)",
                        "deps": [
                            {"pkg": "child-crate 2.0.0 (registry+https://example.invalid)", "dep_kinds": [{"kind": "build"}]}
                        ],
                    },
                    {"id": "child-crate 2.0.0 (registry+https://example.invalid)", "deps": []},
                ],
            },
            "packages": [
                {
                    "id": "panelink 0.1.0 (path+file:///fixture)",
                    "name": "panelink",
                    "version": "0.1.0",
                    "manifest_path": "/fixture/Cargo.toml",
                    "license": "MIT",
                    "targets": [{"kind": ["lib"]}],
                },
                {
                    "id": "root-crate 1.0.0 (registry+https://example.invalid)",
                    "name": "root-crate",
                    "version": "1.0.0",
                    "manifest_path": "/registry/root/Cargo.toml",
                    "license": "MIT OR Apache-2.0",
                    "targets": [{"kind": ["lib"]}],
                },
                {
                    "id": "child-crate 2.0.0 (registry+https://example.invalid)",
                    "name": "child-crate",
                    "version": "2.0.0",
                    "manifest_path": "/registry/child/Cargo.toml",
                    "license": None,
                    "targets": [{"kind": ["custom-build"]}, {"kind": ["proc-macro"]}],
                },
            ],
        }
        inventory = dependency_inventory.build_inventory(metadata, "ignored tree", Path("/fixture"))
        self.assertEqual(2, inventory["package_count"])
        by_name = {package["name"]: package for package in inventory["packages"]}
        self.assertEqual("direct", by_name["root-crate"]["role"])
        self.assertEqual(["root-crate"], by_name["child-crate"]["pulled_by"])
        self.assertIn("unknown-license", by_name["child-crate"]["review_flags"])
        self.assertTrue(by_name["child-crate"]["build_script"])
        self.assertTrue(by_name["child-crate"]["proc_macro"])

    def test_GIVEN_unknown_or_unreviewed_license_WHEN_flagged_THEN_names_the_condition(self) -> None:
        self.assertEqual(["unknown-license"], dependency_inventory.license_flags(None))
        self.assertEqual(
            ["non-permissive-or-unreviewed-license:GPL-3.0-only"],
            dependency_inventory.license_flags("GPL-3.0-only"),
        )

    def test_GIVEN_lock_fallback_WHEN_registry_manifest_is_missing_THEN_unknowns_are_explicit(self) -> None:
        inventory = dependency_inventory.build_lock_fallback(
            'version = 4\n\n[[package]]\nname = "sample"\nversion = "1.2.3"\nsource = "registry+https://example.invalid"\n',
            '[package]\nname = "panelink"\nversion = "0.1.0"\n[dependencies]\nsample = "=1.2.3"\n',
            Path("/empty"),
        )
        self.assertEqual(1, inventory["package_count"])
        self.assertEqual("registry", inventory["packages"][0]["source"])
        self.assertIn("unknown-license", inventory["packages"][0]["review_flags"])


class BuildMeasurementTests(unittest.TestCase):
    def test_GIVEN_no_samples_WHEN_percentile_computed_THEN_returns_none(self) -> None:
        self.assertIsNone(measure_build.percentile([], 0.95))

    def test_GIVEN_timing_samples_WHEN_aggregated_THEN_uses_nearest_rank(self) -> None:
        self.assertEqual(2.0, measure_build.percentile([3.0, 1.0, 2.0], 0.50))
        aggregate = measure_build.aggregate_runs(
            [
                {
                    "clean_release_build_ms": 10,
                    "incremental_rebuild_ms": 4,
                    "release_binary_bytes": 100,
                    "stripped_binary_bytes": 80,
                },
                {
                    "clean_release_build_ms": 12,
                    "incremental_rebuild_ms": 6,
                    "release_binary_bytes": 105,
                    "stripped_binary_bytes": 85,
                },
            ]
        )
        self.assertEqual([10.0, 12.0], aggregate["clean_release_build_ms"]["samples"])
        self.assertEqual(12.0, aggregate["clean_release_build_ms"]["p95"])
        self.assertEqual({"min": 100.0, "max": 105.0, "samples": [100.0, 105.0]}, aggregate["release_binary_bytes"])


if __name__ == "__main__":
    unittest.main()
