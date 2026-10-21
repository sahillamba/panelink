#!/usr/bin/env python3
"""Validate bounded live-history observations; this is not a performance gate."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

MAX_REPORT_BYTES = 8 * 1024 * 1024
CASES = (("paced_80", 80, 768), ("burst_240", 240, 20_000), ("shutdown_240", 240, 100_000))
HISTORY_REASONS = frozenset({
    "admission_contended", "admission_count_full", "admission_bytes_full",
    "final_payload_dropped_contended", "final_payload_dropped_count",
    "final_payload_dropped_bytes", "final_payload_invalid", "storage_error",
    "retry_exhausted", "receipt_failed", "worker_failure", "shutdown_timed_out",
    "work_canceled", "pending_receipts_at_deadline", "final_metadata_unsubmitted",
    "capture_setup_failed", "capture_invalid", "capture_registration_full", "unknown_end_time",
    "canceled_end_metadata_finalized", "canceled_end_metadata_finalize_failed",
})
DRAIN_OUTCOMES = ("not_requested", "drained", "failed", "timed_out")


class InvalidReport(ValueError):
    """A structural or internally inconsistent measurement report."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InvalidReport(message)


def fields(value: Any, expected: set[str], where: str) -> None:
    require(type(value) is dict and set(value) == expected, f"{where}: unexpected or missing fields")


def integer(value: Any, where: str, *, minimum: int = 0, maximum: int = (1 << 64) - 1) -> None:
    require(type(value) is int and minimum <= value <= maximum, f"{where}: invalid integer")


def boolean(value: Any, where: str) -> None:
    require(type(value) is bool, f"{where}: invalid boolean")


def validate_report(report: Any, *, require_release: bool = True) -> None:
    fields(report, {"schema_version", "synthetic", "actual_history_store", "queue_occupancy_instrumented", "build_profile", "cases"}, "report")
    require(type(report["schema_version"]) is int and report["schema_version"] in (1, 2), "report: unsupported schema")
    require(report["synthetic"] is True and report["actual_history_store"] is True, "report: wrong workload/store")
    require(report["queue_occupancy_instrumented"] is False, "report: unsupported queue instrumentation claim")
    require(report["build_profile"] in ("debug", "release"), "report: invalid build profile")
    require(not require_release or report["build_profile"] == "release", "report: release measurements required")
    cases = report["cases"]
    require(type(cases) is list and len(cases) == len(CASES), "report: expected three fixed cases")
    for case, (name, columns, planned) in zip(cases, CASES):
        case_fields = {"name", "columns", "rows_per_pane", "panes", "planned_rows_per_pane", "elapsed_us", "owner_tick_us", "probes", "driver_drop_us", "post_drop_lock_wait_us", "history_failure", "shutdown_during_output", "targets"}
        if report["schema_version"] == 2:
            case_fields.add("history_diagnostics")
        fields(case, case_fields, "case")
        require(case["name"] == name, "case: unexpected case order/name")
        for key, expected in (("columns", columns), ("rows_per_pane", 24), ("panes", 2), ("planned_rows_per_pane", planned)):
            integer(case[key], key)
            require(case[key] == expected, f"case: wrong {key}")
        integer(case["elapsed_us"], "elapsed_us", minimum=1)
        elapsed = case["elapsed_us"]
        for key in ("driver_drop_us", "post_drop_lock_wait_us"):
            integer(case[key], key, maximum=elapsed)
        for key in ("history_failure", "shutdown_during_output"):
            boolean(case[key], key)
        if report["schema_version"] == 2:
            validate_diagnostics(case["history_diagnostics"], case["history_failure"])
        ticks = case["owner_tick_us"]
        require(type(ticks) is list and 1 <= len(ticks) <= 100_000, "case: invalid owner sample count")
        for tick in ticks:
            integer(tick, "owner tick", maximum=elapsed)
        require(sum(ticks) <= elapsed, "case: owner samples exceed elapsed wall time")
        probes = case["probes"]
        require(type(probes) is list and 16 <= len(probes) <= 4096, "case: invalid probe count")
        per_pane = [0, 0]
        for probe in probes:
            fields(probe, {"pane", "control_rtt_us", "input_reply_us", "child_ack_us", "started_during_output", "ack_before_output_done"}, "probe")
            integer(probe["pane"], "probe pane", maximum=1)
            per_pane[probe["pane"]] += 1
            for key in ("control_rtt_us", "input_reply_us", "child_ack_us"):
                integer(probe[key], key, maximum=elapsed)
            for key in ("started_during_output", "ack_before_output_done"):
                boolean(probe[key], key)
            require(not probe["ack_before_output_done"] or probe["started_during_output"], "probe: contradictory overlap observations")
        require(min(per_pane) >= 8, "case: fewer than eight probes for a pane")
        targets = case["targets"]
        require(type(targets) is list and len(targets) == 2, "case: expected two targets")
        panes = set()
        for target in targets:
            fields(target, {"pane", "retained_rows", "segment_bytes", "first_ordinal", "last_ordinal", "gap", "ended", "normal_checkpoint", "alternate_checkpoint", "producer_completed", "completed_rows"}, "target")
            integer(target["pane"], "target pane", maximum=1)
            panes.add(target["pane"])
            integer(target["retained_rows"], "retained rows", maximum=1024)
            integer(target["segment_bytes"], "row segment bytes", maximum=8 * 1024 * 1024)
            for key in ("gap", "ended", "normal_checkpoint", "alternate_checkpoint", "producer_completed"):
                boolean(target[key], key)
            if target["retained_rows"]:
                integer(target["first_ordinal"], "first ordinal", minimum=1)
                integer(target["last_ordinal"], "last ordinal", minimum=target["first_ordinal"])
                require(target["last_ordinal"] - target["first_ordinal"] + 1 >= target["retained_rows"], "target: impossible retained ordinal span")
                require(target["segment_bytes"] > 0, "target: rows without a segment")
            else:
                require(target["first_ordinal"] is None and target["last_ordinal"] is None, "target: empty rows must have null ordinals")
            if target["producer_completed"]:
                integer(target["completed_rows"], "completed rows")
                require(target["completed_rows"] == planned, "target: completed producer count mismatch")
            else:
                require(target["completed_rows"] is None, "target: interrupted producer count must remain unknown")
            require(case["history_failure"] or target["ended"], "target: missing end without a capture failure")
        require(panes == {0, 1}, "case: duplicate target pane")
        if name != "shutdown_240":
            require(not case["shutdown_during_output"] and all(t["producer_completed"] for t in targets), "case: normal-exit workload did not finish generation")
        elif case["shutdown_during_output"]:
            require(any(not t["producer_completed"] for t in targets), "case: shutdown overlap contradicts completed producers")


def validate_diagnostics(diagnostics: Any, history_failure: bool) -> None:
    fields(diagnostics, {"reasons", "writer_drain_outcome"}, "diagnostics")
    reasons = diagnostics["reasons"]
    require(type(reasons) is list and len(reasons) <= len(HISTORY_REASONS), "diagnostics: invalid reason count")
    require(all(type(reason) is str and reason in HISTORY_REASONS for reason in reasons), "diagnostics: unsupported reason")
    require(len(reasons) == len(set(reasons)), "diagnostics: duplicate reason")
    outcome = diagnostics["writer_drain_outcome"]
    require(type(outcome) is str and outcome in DRAIN_OUTCOMES, "diagnostics: unsupported writer drain")
    require(outcome != "not_requested", "diagnostics: post-drop benchmark writer did not report drain")
    require(outcome != "timed_out" or "shutdown_timed_out" in reasons, "diagnostics: timeout without its reason")
    require(outcome not in ("failed", "timed_out") or history_failure, "diagnostics: incomplete writer drain without sticky failure")
    require(not history_failure or bool(reasons), "diagnostics: capture failure without a reason")
    loss = any(reason.startswith("final_payload_") for reason in reasons)
    require(not loss or history_failure, "diagnostics: final payload loss without sticky failure")
    finalized = any(reason in reasons for reason in
                    ("canceled_end_metadata_finalized", "canceled_end_metadata_finalize_failed"))
    require(not finalized or
            (history_failure and outcome == "timed_out" and
             "shutdown_timed_out" in reasons and "work_canceled" in reasons),
            "diagnostics: canceled metadata result without timeout/cancellation")


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        require(key not in result, "report: duplicate JSON field")
        result[key] = value
    return result


def load_report(path: Path) -> Any:
    with path.open("rb") as stream:
        content = stream.read(MAX_REPORT_BYTES + 1)
    require(len(content) <= MAX_REPORT_BYTES, "report: file exceeds size bound")
    try:
        return json.loads(content, object_pairs_hook=unique_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise InvalidReport("report: malformed JSON") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", type=Path, nargs="+")
    args = parser.parse_args()
    try:
        for path in args.reports:
            validate_report(load_report(path))
    except (OSError, InvalidReport) as error:
        # No record values or file contents are included in diagnostics.
        print(f"FAIL: {error if isinstance(error, InvalidReport) else 'cannot read report'}")
        return 1
    print(f"PASS: {len(args.reports)} live-history report(s); schema/counts/bounds consistent, no performance budget certified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
