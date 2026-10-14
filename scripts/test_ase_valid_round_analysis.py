import unittest
import fse_confirmatory_analysis as analysis
from fse_confirmatory_analysis import valid_paired_rows
import test_fse_confirmatory_analysis as fixtures


class ValidRoundTests(unittest.TestCase):
    def report(self, *, excluded_repositories=(), lose_correct_delivery=False):
        plan = fixtures.plan()
        plan["analysis_population"] = "valid_paired_evaluable"
        plan["primary_estimand"] = "repository_average_correction_c_minus_b_valid_paired_risk_difference"
        freeze = fixtures.freeze(plan)
        rows = fixtures.rows(c_correct_loss=lose_correct_delivery)
        for row in rows:
            row["model_output_observed"] = row["repository"] not in excluded_repositories
            row["plan_sha256"] = analysis.canonical_hash(plan)
            row["freeze_sha256"] = analysis.canonical_hash(freeze)
        return analysis.analyze(rows, plan, freeze, fixtures.DATASET_MANIFEST,
                                fixtures.ARTIFACT_PATHS,
                                expected_freeze_sha256=analysis.canonical_hash(freeze))

    def test_full_analysis_keeps_valid_errors_and_requires_utility_guardrail(self):
        report = self.report()
        self.assertEqual(report["included_rows"], 120)
        self.assertAlmostEqual(report["endpoints"]["false_ready"]["by_variant"]["correction"]["difference"], -1 / 3)
        self.assertEqual(report["decision"]["status"], "conditional_joint_method_supported")
        lost = self.report(lose_correct_delivery=True)
        self.assertEqual(lost["included_rows"], 120)
        self.assertFalse(lost["decision"]["guardrail_supported"])
        self.assertEqual(lost["decision"]["status"], "conditional_joint_method_not_supported")

    def test_full_analysis_excludes_no_output_and_suppresses_underpowered_support(self):
        report = self.report(excluded_repositories=("r1", "r2", "r3"))
        self.assertEqual(report["included_rows"], 84)
        self.assertEqual(len(report["excluded_rows"]), 36)
        self.assertEqual({row["reason"] for row in report["excluded_rows"]}, {"no_usable_model_output"})
        self.assertEqual(report["decision"]["status"], "insufficient_valid_pairs")
        self.assertFalse(report["decision"]["false_ready_supported"])

    def test_invalid_cells_and_their_counterparts_are_excluded_but_wrong_valid_results_remain(self):
        def row(task, arm, **changes):
            return dict(run_id=task + arm, base_task_id=task, variant="correction", repeat=0,
                        arm=arm, terminal_status="valid", model_output_observed=True,
                        oracle_status="incorrect", mid_run_correction_acknowledged=True, **changes)
        rows = [row("wrong", "B"), row("wrong", "C"), row("budget", "B"), row("budget", "C")]
        rows[-1]["terminal_status"] = "error"
        included, excluded = valid_paired_rows(rows)
        self.assertEqual([r["run_id"] for r in included], ["wrongB", "wrongC"])
        self.assertEqual({r["run_id"]:r["reason"] for r in excluded},
                         {"budgetB":"counterpart_not_valid", "budgetC":"terminal_error"})
        rows[0]["model_output_observed"] = False
        included, excluded = valid_paired_rows(rows)
        self.assertEqual(included, [])
        self.assertEqual(excluded[0]["reason"], "no_usable_model_output")


if __name__ == "__main__":
    unittest.main()
