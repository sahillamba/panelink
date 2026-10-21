import copy
import json
from pathlib import Path
import tempfile
import unittest

from scripts import validate_live_history_report as validator


def sample_report(version=1):
    cases = []
    for name, columns, planned in validator.CASES:
        interrupted = name == "shutdown_240"
        cases.append({
            "name": name, "columns": columns, "rows_per_pane": 24, "panes": 2,
            "planned_rows_per_pane": planned, "elapsed_us": 1000,
            "owner_tick_us": [1, 2, 3], "driver_drop_us": 100,
            "post_drop_lock_wait_us": 10, "history_failure": False,
            "shutdown_during_output": interrupted,
            "probes": [
                {"pane": pane, "control_rtt_us": 2, "input_reply_us": 3,
                 "child_ack_us": 4, "started_during_output": True,
                 "ack_before_output_done": True}
                for pane in range(2) for _ in range(8)
            ],
            "targets": [
                {"pane": pane, "retained_rows": 10, "segment_bytes": 1000,
                 "first_ordinal": 1, "last_ordinal": 10, "gap": False,
                 "ended": True, "normal_checkpoint": True,
                 "alternate_checkpoint": False,
                 "producer_completed": not interrupted,
                 "completed_rows": None if interrupted else planned}
                for pane in range(2)
            ],
        })
    if version == 2:
        for case in cases:
            case["history_diagnostics"] = {"reasons": [], "writer_drain_outcome": "drained"}
    return {"schema_version": version, "synthetic": True, "actual_history_store": True,
            "queue_occupancy_instrumented": False, "build_profile": "release", "cases": cases}


class LiveHistoryReportTests(unittest.TestCase):
    def setUp(self):
        self.report = sample_report()

    def assert_invalid(self, report):
        with self.assertRaises(validator.InvalidReport):
            validator.validate_report(report)

    def test_complete_report_and_explicit_incomplete_capture_are_valid_data(self):
        validator.validate_report(self.report)
        case = self.report["cases"][2]
        case["history_failure"] = True
        target = case["targets"][0]
        target.update(retained_rows=0, segment_bytes=0, first_ordinal=None,
                      last_ordinal=None, ended=False, normal_checkpoint=False, gap=True)
        validator.validate_report(self.report)

    def test_rejects_bool_as_duration_negative_samples_and_invented_queue_metric(self):
        for value in (True, -1, 1.5, "123"):
            report = copy.deepcopy(self.report)
            report["cases"][0]["owner_tick_us"][0] = value
            self.assert_invalid(report)
        self.report["queue_occupancy_instrumented"] = True
        self.assert_invalid(self.report)

    def test_rejects_missing_pane_probes_and_idle_overlap_contradiction(self):
        report = copy.deepcopy(self.report)
        report["cases"][0]["probes"][-1]["pane"] = 0
        self.assert_invalid(report)
        self.report["cases"][0]["probes"][0]["started_during_output"] = False
        self.assert_invalid(self.report)

    def test_rejects_oversize_or_impossible_saved_history(self):
        for key, value in (("retained_rows", 1025), ("segment_bytes", 8 * 1024 * 1024 + 1),
                           ("last_ordinal", 9), ("first_ordinal", None)):
            report = copy.deepcopy(self.report)
            report["cases"][0]["targets"][0][key] = value
            self.assert_invalid(report)

    def test_rejects_fabricated_interrupted_count_and_missing_end_without_failure(self):
        report = copy.deepcopy(self.report)
        report["cases"][2]["targets"][0]["completed_rows"] = 500
        self.assert_invalid(report)
        self.report["cases"][0]["targets"][0]["ended"] = False
        self.assert_invalid(self.report)

    def test_rejects_debug_timings_unbounded_samples_and_impossible_elapsed(self):
        report = copy.deepcopy(self.report)
        report["build_profile"] = "debug"
        self.assert_invalid(report)
        validator.validate_report(report, require_release=False)
        report = copy.deepcopy(self.report)
        report["cases"][0]["owner_tick_us"] = [0] * 100001
        self.assert_invalid(report)
        self.report["cases"][0]["owner_tick_us"] = [600, 600]
        self.assert_invalid(self.report)

    def test_report_fields_exclude_paths_contents_and_duplicate_target_aliases(self):
        report = copy.deepcopy(self.report)
        report["cases"][0]["targets"][0]["contents"] = "fixture-only-secret"
        with self.assertRaises(validator.InvalidReport) as caught:
            validator.validate_report(report)
        self.assertNotIn("fixture-only-secret", str(caught.exception))
        self.report["cases"][0]["targets"][1]["pane"] = 0
        self.assert_invalid(self.report)

    def test_bounded_json_loader_refuses_duplicate_fields_and_trailing_data(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            for text in ('{"schema_version":1,"schema_version":1}', '{} {}'):
                path.write_text(text)
                with self.assertRaises(validator.InvalidReport):
                    validator.load_report(path)
            path.write_text(json.dumps(self.report))
            validator.validate_report(validator.load_report(path))

    def test_recorded_release_reports_have_consistent_bounds_and_counts(self):
        progress = Path(__file__).resolve().parents[1] / "docs" / "progress"
        for name in ("p5-live-history-bench.json", "p5-live-history-bench-parent.json",
                     "p5-live-history-bench-preliminary.json",
                     "p5-history-diagnostics-run1.json", "p5-history-diagnostics-run2.json",
                     "p5-final-tail-run1.json", "p5-final-tail-run2.json",
                     "p5-final-tail-control.json",
                     "p5-drain-metadata-run1.json", "p5-drain-metadata-run2.json"):
            with self.subTest(name=name):
                validator.validate_report(validator.load_report(progress / name))

    def test_diagnostics_allow_benign_admission_with_no_sticky_failure(self):
        report = sample_report(2)
        report["cases"][0]["history_diagnostics"]["reasons"] = ["admission_contended"]
        validator.validate_report(report)

    def test_final_loss_requires_failure_and_reasons_are_fixed_unique_tags(self):
        report = sample_report(2)
        case = report["cases"][0]
        reasons = case["history_diagnostics"]["reasons"]
        reasons.append("final_payload_dropped_bytes")
        self.assert_invalid(report)
        case["history_failure"] = True
        validator.validate_report(report)
        reasons.append("private-marker-contents")
        with self.assertRaises(validator.InvalidReport) as caught:
            validator.validate_report(report)
        self.assertNotIn("private-marker-contents", str(caught.exception))
        reasons[-1] = reasons[0]
        self.assert_invalid(report)

    def test_post_drop_drain_and_sticky_failure_need_consistent_evidence(self):
        report = sample_report(2)
        case = report["cases"][0]
        case["history_diagnostics"]["writer_drain_outcome"] = "timed_out"
        self.assert_invalid(report)
        case["history_diagnostics"]["reasons"] = ["shutdown_timed_out"]
        self.assert_invalid(report)
        case["history_failure"] = True
        validator.validate_report(report)
        case["history_diagnostics"] = {"reasons": [], "writer_drain_outcome": "drained"}
        self.assert_invalid(report)
        case["history_failure"] = False
        case["history_diagnostics"]["writer_drain_outcome"] = "not_requested"
        self.assert_invalid(report)

    def test_schema_two_requires_diagnostics_and_schema_one_remains_strict(self):
        report = sample_report(2)
        del report["cases"][0]["history_diagnostics"]
        self.assert_invalid(report)
        report = sample_report()
        report["cases"][0]["history_diagnostics"] = {"reasons": [], "writer_drain_outcome": "drained"}
        self.assert_invalid(report)

    def test_canceled_metadata_outcomes_require_actual_timeout_and_cancellation(self):
        for label in ("canceled_end_metadata_finalized", "canceled_end_metadata_finalize_failed"):
            report = sample_report(2)
            case = report["cases"][0]
            case["history_failure"] = True
            case["history_diagnostics"] = {
                "writer_drain_outcome": "timed_out",
                "reasons": ["shutdown_timed_out", "work_canceled", label],
            }
            validator.validate_report(report)
            case["history_diagnostics"]["reasons"].remove("work_canceled")
            self.assert_invalid(report)
            case["history_diagnostics"]["reasons"].append("work_canceled")
            case["history_diagnostics"]["writer_drain_outcome"] = "drained"
            self.assert_invalid(report)


if __name__ == "__main__":
    unittest.main()
