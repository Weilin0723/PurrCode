import json
import contextlib
import io
import tempfile
import unittest
from pathlib import Path

from scripts.research_metrics import ReportError, analyze_report, main


def outcome(
    task_id="task-1",
    *,
    kind="simple",
    expected=1,
    satisfied=1,
    violated=False,
    reported_done=True,
    delivery_state="ready",
):
    run = {
        "reported_done": reported_done,
        "delivery_state": delivery_state,
        "hard_requirements_total": expected,
        "hard_requirements_verified": satisfied,
        "hard_requirements_violated": 0,
        "hard_requirements_unknown": 0,
        "blocking_findings_at_completion": 0,
        "scope_escapes": 0,
        "corrections_issued": 0,
        "corrections_reflected": 0,
        "findings_repaired": 0,
        "findings_raised": 0,
        "correction_cycles": 0,
        "review_tokens": 1,
        "implementation_tokens": 1,
        "elapsed_seconds": 1,
    }
    return {
        "schema_version": 1,
        "task_id": task_id,
        "kind": kind,
        "run": run,
        "satisfied_requirements": satisfied,
        "expected_requirements": expected,
        "violated_forbidden": violated,
    }


def report(*outcomes):
    return {"schema_version": 1, "outcomes": list(outcomes)}


class ResearchMetricsTests(unittest.TestCase):
    def test_ready_does_not_make_external_failure_correct(self):
        result = analyze_report(
            report(outcome(expected=2, satisfied=1, delivery_state="ready"))
        )
        metrics = result["overall"]
        self.assertEqual(metrics["external_correct_tasks"], 0)
        self.assertEqual(metrics["false_completion"]["count"], 1)
        self.assertEqual(metrics["false_completion"]["rate_all_scored"]["rate"], 1.0)
        self.assertEqual(result["gate_consistency"]["ready_tasks"], 1)

    def test_unclaimed_and_zero_expectation_are_not_passes(self):
        result = analyze_report(
            report(
                outcome(task_id="zero", expected=0, satisfied=0, reported_done=True),
                outcome(
                    task_id="unclaimed",
                    expected=1,
                    satisfied=1,
                    reported_done=False,
                    delivery_state=None,
                ),
            )
        )
        metrics = result["overall"]
        self.assertEqual(metrics["scored_tasks"], 1)
        self.assertEqual(metrics["external_correct_tasks"], 1)
        self.assertEqual(metrics["unscored_claims"], 1)
        self.assertIsNone(metrics["false_completion"]["rate_claimed"]["rate"])
        self.assertEqual(metrics["correct_unclaimed_tasks"], 1)
        self.assertEqual(metrics["completion_coverage"]["rate"], 0.0)

    def test_schema_and_count_errors_are_rejected(self):
        bad = report(outcome())
        bad["schema_version"] = 2
        with self.assertRaises(ReportError):
            analyze_report(bad)

        bad = report(outcome(expected=-1, satisfied=0))
        with self.assertRaises(ReportError):
            analyze_report(bad)

        bad = report(outcome())
        del bad["outcomes"][0]["run"]["reported_done"]
        with self.assertRaises(ReportError):
            analyze_report(bad)

    def test_duplicate_task_ids_are_rejected_within_one_report(self):
        with self.assertRaises(ReportError):
            analyze_report(report(outcome(), outcome(task_id="task-1")))

    def test_correct_delivery_is_unconditional_and_kind_is_strict(self):
        result = analyze_report(
            report(
                outcome(task_id="correct", expected=1, satisfied=1, reported_done=True),
                outcome(
                    task_id="incorrect",
                    expected=1,
                    satisfied=0,
                    reported_done=False,
                    delivery_state=None,
                ),
            )
        )
        metrics = result["overall"]
        self.assertEqual(metrics["correct_delivery"]["rate"], 0.5)
        self.assertEqual(metrics["completion_coverage"]["rate"], 1.0)
        interval = metrics["correct_delivery"]["wilson95"]
        self.assertGreaterEqual(interval["lower"], 0.0)
        self.assertLessEqual(interval["upper"], 1.0)

        with self.assertRaises(ReportError):
            analyze_report(report(outcome(kind="unknown")))

    def test_cli_out_json_emits_per_report_without_live_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "alignment.json"
            path.write_text(json.dumps(report(outcome())), encoding="utf-8")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(main([str(path), "--out", "json"]), 0)
            self.assertEqual(len(json.loads(output.getvalue())["reports"]), 1)


if __name__ == "__main__":
    unittest.main()
