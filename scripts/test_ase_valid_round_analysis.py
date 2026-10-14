import unittest
from fse_confirmatory_analysis import valid_paired_rows


class ValidRoundTests(unittest.TestCase):
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
